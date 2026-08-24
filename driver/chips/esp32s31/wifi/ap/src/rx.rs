//! Protected AP data receive frontier.
//!
//! DMA storage remains borrowed only for this synchronous dispatch. A sink
//! must copy or transfer every Ethernet view before returning, after which the
//! runtime may recycle the descriptor.

use open_esp_radio_esp32s31_wifi::protected_data_rx::{
    ProtectedDataDecapsulation, ProtectedDataRxView, UnprotectedDataFragmentRxError,
    UnprotectedDataRxView, view_protected_data, view_unprotected_data,
    view_unprotected_data_fragment,
};
use open_esp_radio_esp32s31_wifi_mac::{
    rx::{RxError, RxIngressConfig, RxPhyInfo, RxSegment, view_normalized_rx_frame},
    rx_ampdu::{RxBlockAckMpduKey, rx_block_ack_mpdu_key},
};
use open_esp_radio_ieee80211::ccmp::{CcmpHeader, CcmpKeyId, CcmpReplayError, CcmpReplayLane};
use open_esp_radio_ieee80211::data::{
    DataDecapError, DataInterfaceRole, EthernetFrameParts, RxDuplicateFilter,
};
use open_esp_radio_ieee80211::fragmentation::{
    OPEN_DATA_FRAGMENT_TIMEOUT_MICROS, OPEN_DATA_REASSEMBLY_CAPACITY, OpenDataDefragmentation,
    OpenDataDefragmenter, OpenDataFragmentError, OpenDataUnfragmentedAdmission,
    parse_open_data_identity,
};
use open_esp_radio_ieee80211::security::WifiSecurityMode;
use open_esp_radio_wifi_ap::AP_MAX_CLIENTS;
use open_esp_radio_wifi_softmac::MacRxMetadata;

const OPEN_FRAGMENT_CONTEXTS: usize = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ApRxConfig {
    pub access_point: [u8; 6],
    pub ingress: RxIngressConfig,
    pub security: WifiSecurityMode,
}

#[derive(Clone, Copy, Debug)]
pub struct Esp32s31ApRxEvent<'frame> {
    pub frame: EthernetFrameParts<'frame>,
    pub raw: &'frame [u8],
    pub amsdu: bool,
    pub metadata: MacRxMetadata<RxPhyInfo>,
}

pub trait Esp32s31ApRxSink {
    fn publish(&mut self, event: Esp32s31ApRxEvent<'_>);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ApRxError {
    Radio(RxError),
    Data(DataDecapError),
    SecurityModeMismatch,
    PeerQosMismatch,
    PairwiseKeyId(u8),
    Replay(CcmpReplayError),
    KeyGenerationMismatch,
    Fragment(OpenDataFragmentError),
    /// The public header advertises both Protected and fragmentation. This is
    /// rejected before CCMP extraction, so the outcome makes no integrity or
    /// PN-admission claim; protected reassembly needs a fragment-aware replay
    /// transaction that can safely commit the complete PN series.
    ProtectedFragmentationUnsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ApRxDispatch {
    Data { ethernet_frames: u8, amsdu: bool },
    FragmentBuffered { expired: u8, evicted: bool },
    Duplicate,
    ForeignPeer,
    Unauthorized,
    Rejected(Esp32s31ApRxError),
}

/// Value-only request handed from the ordered AP RX dispatcher to the AP key
/// owner. A protected request is created only after S31 hardware has reported
/// successful CCMP integrity verification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ApRxAdmissionRequest {
    peer: [u8; 6],
    lane: CcmpReplayLane,
    ccmp_header: Option<CcmpHeader>,
}

impl Esp32s31ApRxAdmissionRequest {
    pub(crate) const fn new(
        peer: [u8; 6],
        lane: CcmpReplayLane,
        ccmp_header: Option<CcmpHeader>,
    ) -> Self {
        Self {
            peer,
            lane,
            ccmp_header,
        }
    }

    pub const fn peer(self) -> [u8; 6] {
        self.peer
    }

    pub const fn lane(self) -> CcmpReplayLane {
        self.lane
    }

    pub const fn ccmp_header(self) -> Option<CcmpHeader> {
        self.ccmp_header
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Esp32s31ApRxAdmissionOutcome {
    Authorized(Esp32s31ApRxDuplicateOwner),
    Unauthorized,
    Rejected(Esp32s31ApRxError),
}

/// Exact bounded duplicate-filter ownership for one AP association.
///
/// AIDs are allocated from `1..=AP_MAX_CLIENTS`, so the owner resolves to a
/// pre-existing slot before CCMP replay admission can commit. The epoch makes
/// reuse of the same AID (including same-address reassociation) reset history
/// instead of inheriting retry fingerprints from its predecessor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ApRxDuplicateOwner {
    slot: u8,
    association_epoch: u32,
}

impl Esp32s31ApRxDuplicateOwner {
    pub(crate) fn new(association_id: u16, association_epoch: u32) -> Option<Self> {
        let slot = association_id.checked_sub(1)?;
        if usize::from(slot) >= AP_MAX_CLIENTS {
            return None;
        }
        Some(Self {
            slot: u8::try_from(slot).ok()?,
            association_epoch,
        })
    }

    const fn slot(self) -> usize {
        self.slot as usize
    }

    const fn fragmentation_epoch(self) -> u64 {
        (self.association_epoch as u64) << 8 | self.slot as u64
    }
}

/// Unforgeable result of consulting the live AP controlled-port and key
/// owner. Its constructors stay within the chip AP crate so an integration
/// cannot accidentally authorize WPA2 RX with a Boolean or generation zero.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ApRxAdmission {
    outcome: Esp32s31ApRxAdmissionOutcome,
}

impl Esp32s31ApRxAdmission {
    pub(crate) const fn authorized(owner: Esp32s31ApRxDuplicateOwner) -> Self {
        Self {
            outcome: Esp32s31ApRxAdmissionOutcome::Authorized(owner),
        }
    }

    pub(crate) const fn unauthorized() -> Self {
        Self {
            outcome: Esp32s31ApRxAdmissionOutcome::Unauthorized,
        }
    }

    pub(crate) const fn rejected(error: Esp32s31ApRxError) -> Self {
        Self {
            outcome: Esp32s31ApRxAdmissionOutcome::Rejected(error),
        }
    }
}

struct ApPeerDuplicateState {
    address: [u8; 6],
    owner: Esp32s31ApRxDuplicateOwner,
    filter: RxDuplicateFilter,
}

enum ApDataRxView<'frame> {
    Open(UnprotectedDataRxView<'frame>),
    Wpa2Personal(ProtectedDataRxView<'frame>),
}

impl<'frame> ApDataRxView<'frame> {
    fn mpdu(&self) -> &'frame [u8] {
        match self {
            Self::Open(data) => data.mpdu,
            Self::Wpa2Personal(data) => data.mpdu,
        }
    }

    const fn ordering(&self) -> (bool, u16, Option<u8>) {
        match self {
            Self::Open(data) => (data.retry, data.sequence_control, data.tid),
            Self::Wpa2Personal(data) => (data.retry, data.sequence_control, data.tid),
        }
    }

    const fn ccmp_header(&self) -> Option<CcmpHeader> {
        match self {
            Self::Open(_) => None,
            Self::Wpa2Personal(data) => Some(data.ccmp_header),
        }
    }

    fn decapsulate(self) -> Result<ProtectedDataDecapsulation<'frame>, DataDecapError> {
        match self {
            Self::Open(data) => data.decapsulate(DataInterfaceRole::AccessPoint),
            Self::Wpa2Personal(data) => data.decapsulate(DataInterfaceRole::AccessPoint),
        }
    }
}

/// Independent duplicate history for every admitted AP peer.
pub struct Esp32s31ApRxDispatcher {
    config: Esp32s31ApRxConfig,
    duplicates: [Option<ApPeerDuplicateState>; AP_MAX_CLIENTS],
    fragments: OpenDataDefragmenter<OPEN_FRAGMENT_CONTEXTS, OPEN_DATA_REASSEMBLY_CAPACITY>,
}

impl Esp32s31ApRxDispatcher {
    pub const fn new(config: Esp32s31ApRxConfig) -> Self {
        Self {
            config,
            duplicates: [const { None }; AP_MAX_CLIENTS],
            fragments: OpenDataDefragmenter::new(OPEN_DATA_FRAGMENT_TIMEOUT_MICROS),
        }
    }

    /// Begin a new AP epoch without moving the per-peer duplicate table
    /// through an executor stack.
    pub fn reset(&mut self, config: Esp32s31ApRxConfig) {
        self.config = config;
        self.duplicates.fill_with(|| None);
        self.fragments.clear();
    }

    /// Release duplicate ownership when the AP peer close transaction reaches
    /// its terminal edge. A later AID reuse is safe even if this explicit
    /// cleanup is skipped because [`Esp32s31ApRxDuplicateOwner`] replaces the
    /// exact slot on epoch mismatch.
    pub fn forget_peer(&mut self, peer: [u8; 6]) -> bool {
        let fragmented = self.fragments.forget_transmitter(peer) != 0;
        let Some(index) = self
            .duplicates
            .iter()
            .position(|entry| entry.as_ref().is_some_and(|state| state.address == peer))
        else {
            return fragmented;
        };
        self.duplicates[index] = None;
        true
    }

    /// Revoke every incomplete Open MSDU at an AP stop/reset edge.
    pub fn clear_open_fragmentation(&mut self) -> usize {
        self.fragments.clear()
    }

    /// Return whether an ordinary dispatch can borrow its Ethernet payload
    /// directly from the current staging frame. Reassembled payloads live in
    /// the fragment owner and must take the adapter's copying slow path.
    pub fn may_publish_in_place(&self, segment: RxSegment<'_>) -> bool {
        let Ok(normalized) = view_normalized_rx_frame(&segment, self.config.ingress) else {
            return false;
        };
        normalized
            .mpdu
            .get(..24)
            .is_some_and(|mpdu| !fragmented_mpdu(mpdu))
    }

    /// Extract the same public ordering key used by connected-station RX.
    /// Peer authorization and active-agreement lookup remain with the AP
    /// owner because multiple stations can use the same TID concurrently.
    #[inline(always)]
    pub fn reorder_key(&self, segment: RxSegment<'_>) -> Option<RxBlockAckMpduKey> {
        if self.config.security == WifiSecurityMode::Open {
            return None;
        }
        rx_block_ack_mpdu_key(segment.buffer, self.config.access_point, None)
    }

    pub fn dispatch<S, A>(
        &mut self,
        segment: RxSegment<'_>,
        admit: A,
        sink: &mut S,
    ) -> Esp32s31ApRxDispatch
    where
        S: Esp32s31ApRxSink,
        A: FnMut(Esp32s31ApRxAdmissionRequest) -> Esp32s31ApRxAdmission,
    {
        self.dispatch_inner(segment, None, admit, sink)
    }

    /// Dispatch with the runtime timestamp used for bounded fragment expiry.
    pub fn dispatch_at<S, A>(
        &mut self,
        segment: RxSegment<'_>,
        now_micros: u64,
        admit: A,
        sink: &mut S,
    ) -> Esp32s31ApRxDispatch
    where
        S: Esp32s31ApRxSink,
        A: FnMut(Esp32s31ApRxAdmissionRequest) -> Esp32s31ApRxAdmission,
    {
        self.dispatch_inner(segment, Some(now_micros), admit, sink)
    }

    fn dispatch_inner<S, A>(
        &mut self,
        segment: RxSegment<'_>,
        now_micros: Option<u64>,
        mut admit: A,
        sink: &mut S,
    ) -> Esp32s31ApRxDispatch
    where
        S: Esp32s31ApRxSink,
        A: FnMut(Esp32s31ApRxAdmissionRequest) -> Esp32s31ApRxAdmission,
    {
        let normalized = match view_normalized_rx_frame(&segment, self.config.ingress) {
            Ok(frame) => frame,
            Err(error) => {
                return Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Radio(error));
            }
        };
        let Some(frame_control) = normalized
            .mpdu
            .get(..2)
            .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
        else {
            return Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Radio(RxError::Bounds));
        };
        let protected = frame_control & 0x4000 != 0;
        let fragmented = fragmented_mpdu(normalized.mpdu);
        if fragmented && protected {
            return Esp32s31ApRxDispatch::Rejected(
                Esp32s31ApRxError::ProtectedFragmentationUnsupported,
            );
        }
        if protected != (self.config.security == WifiSecurityMode::Wpa2Personal) {
            return Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::SecurityModeMismatch);
        }
        if fragmented {
            return self.dispatch_open_fragment(segment, now_micros, &mut admit, sink);
        }
        let data = match self.config.security {
            WifiSecurityMode::Open => {
                view_unprotected_data(segment, self.config.ingress).map(ApDataRxView::Open)
            }
            WifiSecurityMode::Wpa2Personal => {
                view_protected_data(segment, self.config.ingress).map(ApDataRxView::Wpa2Personal)
            }
        };
        let data = match data {
            Ok(data) => data,
            Err(error) => {
                return Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Radio(error));
            }
        };
        let mpdu = data.mpdu();
        if mpdu.len() < 24 || mpdu[4..10] != self.config.access_point {
            return Esp32s31ApRxDispatch::ForeignPeer;
        }
        let peer: [u8; 6] = mpdu[10..16]
            .try_into()
            .expect("validated 802.11 address width");
        let (retry, sequence_control, tid) = data.ordering();
        let ccmp_header = data.ccmp_header();
        if let Some(header) = ccmp_header
            && header.key_id() != CcmpKeyId::PAIRWISE
        {
            return Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::PairwiseKeyId(
                header.key_id().value(),
            ));
        }
        let data = match data.decapsulate() {
            Ok(data) => data,
            Err(error) => return rejected_data(error),
        };
        let lane = tid.map_or(CcmpReplayLane::NonQos, CcmpReplayLane::Tid);
        let duplicate_owner =
            match admit(Esp32s31ApRxAdmissionRequest::new(peer, lane, ccmp_header)).outcome {
                Esp32s31ApRxAdmissionOutcome::Authorized(owner) => owner,
                Esp32s31ApRxAdmissionOutcome::Unauthorized => {
                    return Esp32s31ApRxDispatch::Unauthorized;
                }
                Esp32s31ApRxAdmissionOutcome::Rejected(error) => {
                    return Esp32s31ApRxDispatch::Rejected(error);
                }
            };
        self.bind_duplicate_owner(peer, duplicate_owner);
        if ccmp_header.is_none() {
            let identity = match parse_open_data_identity(DataInterfaceRole::AccessPoint, mpdu) {
                Ok(identity) => identity,
                Err(error) => {
                    return Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Fragment(error));
                }
            };
            match self.fragments.admit_unfragmented_in_epoch(
                identity,
                duplicate_owner.fragmentation_epoch(),
                retry,
                now_micros,
            ) {
                Ok(OpenDataUnfragmentedAdmission::Admitted { .. }) => {}
                Ok(OpenDataUnfragmentedAdmission::Duplicate { .. }) => {
                    return Esp32s31ApRxDispatch::Duplicate;
                }
                Err(error) => {
                    return Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Fragment(error));
                }
            }
        }
        let duplicates = self.duplicate_filter(peer, duplicate_owner);
        if duplicates.is_duplicate(retry, sequence_control, tid) {
            return Esp32s31ApRxDispatch::Duplicate;
        }
        let mut frames = data.frames;
        let amsdu = data.amsdu;
        let mut count = 0_u8;
        for frame in &mut frames {
            let frame = match frame {
                Ok(frame) => frame,
                Err(error) => return rejected_data(error),
            };
            sink.publish(Esp32s31ApRxEvent {
                frame,
                raw: data.raw,
                amsdu,
                metadata: data.metadata,
            });
            count = count.saturating_add(1);
        }
        Esp32s31ApRxDispatch::Data {
            ethernet_frames: count,
            amsdu,
        }
    }

    fn dispatch_open_fragment<S, A>(
        &mut self,
        segment: RxSegment<'_>,
        now_micros: Option<u64>,
        admit: &mut A,
        sink: &mut S,
    ) -> Esp32s31ApRxDispatch
    where
        S: Esp32s31ApRxSink,
        A: FnMut(Esp32s31ApRxAdmissionRequest) -> Esp32s31ApRxAdmission,
    {
        let Some(now_micros) = now_micros else {
            return Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Fragment(
                OpenDataFragmentError::ClockUnavailable,
            ));
        };
        let view = match view_unprotected_data_fragment(
            segment,
            self.config.ingress,
            DataInterfaceRole::AccessPoint,
        ) {
            Ok(view) => view,
            Err(UnprotectedDataFragmentRxError::Radio(error)) => {
                return Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Radio(error));
            }
            Err(UnprotectedDataFragmentRxError::Fragment(error)) => {
                return Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Fragment(error));
            }
        };
        let identity = view.fragment.identity();
        if identity.receiver_address() != self.config.access_point {
            return Esp32s31ApRxDispatch::ForeignPeer;
        }
        let peer = identity.transmitter_address();
        let lane = identity
            .tid()
            .map_or(CcmpReplayLane::NonQos, CcmpReplayLane::Tid);
        let duplicate_owner =
            match admit(Esp32s31ApRxAdmissionRequest::new(peer, lane, None)).outcome {
                Esp32s31ApRxAdmissionOutcome::Authorized(owner) => owner,
                Esp32s31ApRxAdmissionOutcome::Unauthorized => {
                    return Esp32s31ApRxDispatch::Unauthorized;
                }
                Esp32s31ApRxAdmissionOutcome::Rejected(error) => {
                    return Esp32s31ApRxDispatch::Rejected(error);
                }
            };
        self.bind_duplicate_owner(peer, duplicate_owner);
        if view.fragment.fragment_number() == 0
            && self
                .duplicate_filter(peer, duplicate_owner)
                .is_known_duplicate(
                    view.fragment.retry(),
                    view.fragment.sequence_control(),
                    identity.tid(),
                )
        {
            // Fragment zero shares the ordinary MPDU's Sequence Control
            // value. Consult the exact association duplicate owner only at
            // this edge so Retry cannot manufacture a new fragment train by
            // toggling More Fragments on an already accepted ordinary MPDU.
            // Later fragments remain with the reassembler's own history, and
            // no fragment mutates ordinary history.
            return Esp32s31ApRxDispatch::Duplicate;
        }
        let raw = view.raw;
        let metadata = view.metadata;
        match self.fragments.ingest_in_epoch(
            view.fragment,
            duplicate_owner.fragmentation_epoch(),
            now_micros,
            |data| {
                sink.publish(Esp32s31ApRxEvent {
                    frame: data.ethernet_frame(),
                    raw,
                    amsdu: false,
                    metadata,
                });
            },
        ) {
            Ok(OpenDataDefragmentation::Buffered { expired, evicted }) => {
                Esp32s31ApRxDispatch::FragmentBuffered {
                    expired,
                    evicted: evicted.is_some(),
                }
            }
            Ok(OpenDataDefragmentation::Duplicate { .. }) => Esp32s31ApRxDispatch::Duplicate,
            Ok(OpenDataDefragmentation::Complete { .. }) => Esp32s31ApRxDispatch::Data {
                ethernet_frames: 1,
                amsdu: false,
            },
            Err(error) => Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Fragment(error)),
        }
    }

    /// Test-only admission shim. Production callers cannot bypass the live
    /// controlled-port and key-generation owner with a Boolean.
    #[cfg(test)]
    pub fn dispatch_protected<S, A>(
        &mut self,
        segment: RxSegment<'_>,
        is_authorized: A,
        sink: &mut S,
    ) -> Esp32s31ApRxDispatch
    where
        S: Esp32s31ApRxSink,
        A: FnMut([u8; 6]) -> Option<Esp32s31ApRxDuplicateOwner>,
    {
        let mut is_authorized = is_authorized;
        self.dispatch(
            segment,
            |request| {
                if let Some(owner) = is_authorized(request.peer()) {
                    Esp32s31ApRxAdmission::authorized(owner)
                } else {
                    Esp32s31ApRxAdmission::unauthorized()
                }
            },
            sink,
        )
    }

    #[inline(always)]
    fn bind_duplicate_owner(&mut self, peer: [u8; 6], owner: Esp32s31ApRxDuplicateOwner) {
        let index = owner.slot();
        let current_matches = self.duplicates[index]
            .as_ref()
            .is_some_and(|state| state.address == peer && state.owner == owner);
        if current_matches {
            return;
        }

        // An AID slot or same-address association epoch has changed. Revoke
        // both sides of a possible slot reuse before installing the new
        // duplicate owner, so no retained Open bytes survive a controlled-
        // port/association generation edge even if explicit peer-close
        // cleanup raced or was skipped.
        if let Some(stale) = self.duplicates[index].take() {
            self.fragments.forget_transmitter(stale.address);
        }
        self.fragments.forget_transmitter(peer);
        self.duplicates[index] = Some(ApPeerDuplicateState {
            address: peer,
            owner,
            filter: RxDuplicateFilter::new(),
        });
    }

    #[inline(always)]
    fn duplicate_filter(
        &mut self,
        peer: [u8; 6],
        owner: Esp32s31ApRxDuplicateOwner,
    ) -> &mut RxDuplicateFilter {
        self.bind_duplicate_owner(peer, owner);
        let index = owner.slot();
        self.duplicates[index]
            .as_mut()
            .map(|state| &mut state.filter)
            .expect("exact duplicate owner always materializes its bounded slot")
    }
}

fn rejected_data(error: DataDecapError) -> Esp32s31ApRxDispatch {
    Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Data(error))
}

fn fragmented_mpdu(mpdu: &[u8]) -> bool {
    let Some(header) = mpdu.get(..24) else {
        return false;
    };
    let frame_control = u16::from_le_bytes([header[0], header[1]]);
    frame_control & 0x000c == 0x0008 && (frame_control & 0x0400 != 0 || header[22] & 0x0f != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_esp_radio_esp32s31_wifi_mac::rx::{PUBLIC_HEADER_SIZE, RxSegment};
    use open_esp_radio_ieee80211::ccmp::{CcmpPacketNumber, CcmpRxReplayState};

    const AP: [u8; 6] = [2, 0, 0, 0, 0, 1];
    const PEER: [u8; 6] = [2, 0, 0, 0, 0, 2];
    const OTHER_PEER: [u8; 6] = [2, 0, 0, 0, 0, 4];
    const DESTINATION: [u8; 6] = [2, 0, 0, 0, 0, 3];
    const TAIL: usize = 0x38;
    const LENGTH_SHIFT: u32 = 14;
    const BIT_30: u32 = 1 << 30;
    const BIT_31: u32 = 1 << 31;

    #[derive(Default)]
    struct Sink {
        ethernet: std::vec::Vec<std::vec::Vec<u8>>,
    }

    impl Esp32s31ApRxSink for Sink {
        fn publish(&mut self, event: Esp32s31ApRxEvent<'_>) {
            let mut frame = std::vec![0; event.frame.length()];
            event.frame.copy_to(&mut frame).unwrap();
            self.ethernet.push(frame);
        }
    }

    fn config() -> Esp32s31ApRxConfig {
        Esp32s31ApRxConfig {
            access_point: AP,
            ingress: RxIngressConfig {
                ring_entry_limit: 1,
                csi_config: 0,
                flags: 0,
            },
            security: WifiSecurityMode::Wpa2Personal,
        }
    }

    fn open_config() -> Esp32s31ApRxConfig {
        Esp32s31ApRxConfig {
            security: WifiSecurityMode::Open,
            ..config()
        }
    }

    fn duplicate_owner(association_id: u16, epoch: u32) -> Esp32s31ApRxDuplicateOwner {
        Esp32s31ApRxDuplicateOwner::new(association_id, epoch).unwrap()
    }

    fn segment(storage: &[u8; 192], descriptor_word0: u32) -> RxSegment<'_> {
        RxSegment {
            descriptor_address: 0x2f00_4000,
            descriptor_word0,
            buffer: storage,
            next_descriptor_address: 0,
        }
    }

    fn open_fragment(
        storage: &mut [u8; 192],
        sequence: u16,
        fragment: u8,
        more_fragments: bool,
        address3: [u8; 6],
        payload: &[u8],
    ) -> u32 {
        let mpdu_length = 24 + payload.len();
        let signal_length = mpdu_length + 4;
        storage.fill(0);
        storage[0x1f] = 1;
        storage[TAIL..TAIL + 4].copy_from_slice(
            &(((signal_length + 4) as u32) << 16 | signal_length as u32).to_le_bytes(),
        );
        let frame = &mut storage[PUBLIC_HEADER_SIZE..PUBLIC_HEADER_SIZE + mpdu_length];
        let mut frame_control = 0x0108_u16;
        if more_fragments {
            frame_control |= 0x0400;
        }
        frame[..2].copy_from_slice(&frame_control.to_le_bytes());
        frame[4..10].copy_from_slice(&AP);
        frame[10..16].copy_from_slice(&PEER);
        frame[16..22].copy_from_slice(&address3);
        frame[22..24].copy_from_slice(&((sequence << 4) | u16::from(fragment)).to_le_bytes());
        frame[24..].copy_from_slice(payload);
        192 | (((PUBLIC_HEADER_SIZE + signal_length) as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31
    }

    #[test]
    fn open_ap_reassembly_requires_live_peer_admission_and_copying_publication() {
        let first_payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1, 2];
        let final_payload = [3, 4, 5];
        let mut first_storage = [0_u8; 192];
        let first_descriptor = open_fragment(
            &mut first_storage,
            0x123,
            0,
            true,
            DESTINATION,
            &first_payload,
        );
        let mut final_storage = [0_u8; 192];
        let final_descriptor = open_fragment(
            &mut final_storage,
            0x123,
            1,
            false,
            DESTINATION,
            &final_payload,
        );
        let mut dispatcher = Esp32s31ApRxDispatcher::new(open_config());
        let mut sink = Sink::default();

        assert!(!dispatcher.may_publish_in_place(segment(&first_storage, first_descriptor)));
        assert_eq!(
            dispatcher.dispatch_at(
                segment(&first_storage, first_descriptor),
                10,
                |_| Esp32s31ApRxAdmission::unauthorized(),
                &mut sink,
            ),
            Esp32s31ApRxDispatch::Unauthorized
        );
        assert_eq!(dispatcher.clear_open_fragmentation(), 0);

        assert_eq!(
            dispatcher.dispatch_at(
                segment(&first_storage, first_descriptor),
                11,
                |_| Esp32s31ApRxAdmission::authorized(duplicate_owner(1, 1)),
                &mut sink,
            ),
            Esp32s31ApRxDispatch::FragmentBuffered {
                expired: 0,
                evicted: false,
            }
        );
        first_storage[PUBLIC_HEADER_SIZE + 1] &= !0x04;
        first_storage[PUBLIC_HEADER_SIZE + 1] |= 0x08;
        assert_eq!(
            dispatcher.dispatch_at(
                segment(&first_storage, first_descriptor),
                12,
                |_| Esp32s31ApRxAdmission::authorized(duplicate_owner(1, 1)),
                &mut sink,
            ),
            Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Fragment(
                OpenDataFragmentError::MoreFragmentsMismatch
            ))
        );
        first_storage[PUBLIC_HEADER_SIZE + 1] &= !0x08;
        first_storage[PUBLIC_HEADER_SIZE + 1] |= 0x04;
        assert_eq!(
            dispatcher.dispatch_at(
                segment(&final_storage, final_descriptor),
                13,
                |_| Esp32s31ApRxAdmission::authorized(duplicate_owner(1, 2)),
                &mut sink,
            ),
            Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Fragment(
                OpenDataFragmentError::Orphan { fragment_number: 1 }
            ))
        );
        assert_eq!(dispatcher.clear_open_fragmentation(), 0);
        assert!(sink.ethernet.is_empty());
        assert_eq!(
            dispatcher.dispatch_at(
                segment(&first_storage, first_descriptor),
                14,
                |_| Esp32s31ApRxAdmission::authorized(duplicate_owner(1, 2)),
                &mut sink,
            ),
            Esp32s31ApRxDispatch::FragmentBuffered {
                expired: 0,
                evicted: false,
            }
        );
        assert_eq!(
            dispatcher.dispatch_at(
                segment(&final_storage, final_descriptor),
                15,
                |_| Esp32s31ApRxAdmission::authorized(duplicate_owner(1, 2)),
                &mut sink,
            ),
            Esp32s31ApRxDispatch::Data {
                ethernet_frames: 1,
                amsdu: false,
            }
        );
        assert_eq!(sink.ethernet.len(), 1);
        assert_eq!(&sink.ethernet[0][..6], &DESTINATION);
        assert_eq!(&sink.ethernet[0][6..12], &PEER);
        assert_eq!(&sink.ethernet[0][12..14], &0x0800_u16.to_be_bytes());
        assert_eq!(&sink.ethernet[0][14..], &[1, 2, 3, 4, 5]);

        let _ = dispatcher.dispatch_at(
            segment(&first_storage, first_descriptor),
            20,
            |_| Esp32s31ApRxAdmission::authorized(duplicate_owner(1, 2)),
            &mut sink,
        );
        assert!(dispatcher.forget_peer(PEER));
        assert_eq!(
            dispatcher.dispatch_at(
                segment(&final_storage, final_descriptor),
                21,
                |_| Esp32s31ApRxAdmission::authorized(duplicate_owner(1, 2)),
                &mut sink,
            ),
            Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Fragment(
                OpenDataFragmentError::Orphan { fragment_number: 1 }
            ))
        );
    }

    #[test]
    fn ap_protected_fragment_rejects_before_replay_admission() {
        let payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
        let mut storage = [0_u8; 192];
        let descriptor = open_fragment(&mut storage, 7, 0, true, DESTINATION, &payload);
        storage[PUBLIC_HEADER_SIZE + 1] |= 0x40;
        let mut dispatcher = Esp32s31ApRxDispatcher::new(config());
        let mut sink = Sink::default();
        assert_eq!(
            dispatcher.dispatch_at(
                segment(&storage, descriptor),
                1,
                |_| panic!("protected fragment must not reach replay admission"),
                &mut sink,
            ),
            Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::ProtectedFragmentationUnsupported)
        );
        assert!(sink.ethernet.is_empty());
    }

    #[test]
    fn open_retry_cannot_turn_an_ordinary_mpdu_into_a_fragment_train() {
        let payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
        let mut ordinary_storage = [0_u8; 192];
        let ordinary_descriptor =
            open_fragment(&mut ordinary_storage, 7, 0, false, DESTINATION, &payload);
        let owner = duplicate_owner(1, 1);
        let mut dispatcher = Esp32s31ApRxDispatcher::new(open_config());
        let mut sink = Sink::default();

        assert_eq!(
            dispatcher.dispatch_at(
                segment(&ordinary_storage, ordinary_descriptor),
                1,
                |_| Esp32s31ApRxAdmission::authorized(owner),
                &mut sink,
            ),
            Esp32s31ApRxDispatch::Data {
                ethernet_frames: 1,
                amsdu: false,
            }
        );

        ordinary_storage[PUBLIC_HEADER_SIZE + 1] |= 0x04 | 0x08;
        assert_eq!(
            dispatcher.dispatch_at(
                segment(&ordinary_storage, ordinary_descriptor),
                2,
                |_| Esp32s31ApRxAdmission::authorized(owner),
                &mut sink,
            ),
            Esp32s31ApRxDispatch::Duplicate
        );
        assert_eq!(dispatcher.clear_open_fragmentation(), 0);

        let mut final_storage = [0_u8; 192];
        let final_descriptor = open_fragment(&mut final_storage, 7, 1, false, DESTINATION, &[2]);
        assert_eq!(
            dispatcher.dispatch_at(
                segment(&final_storage, final_descriptor),
                3,
                |_| Esp32s31ApRxAdmission::authorized(owner),
                &mut sink,
            ),
            Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Fragment(
                OpenDataFragmentError::Orphan { fragment_number: 1 }
            ))
        );

        let mut invalid_first_storage = [0_u8; 192];
        let invalid_first_descriptor =
            open_fragment(&mut invalid_first_storage, 8, 0, true, DESTINATION, &[0; 9]);
        assert_eq!(
            dispatcher.dispatch_at(
                segment(&invalid_first_storage, invalid_first_descriptor),
                4,
                |_| Esp32s31ApRxAdmission::authorized(owner),
                &mut sink,
            ),
            Esp32s31ApRxDispatch::FragmentBuffered {
                expired: 0,
                evicted: false,
            }
        );
        let mut invalid_final_storage = [0_u8; 192];
        let invalid_final_descriptor =
            open_fragment(&mut invalid_final_storage, 8, 1, false, DESTINATION, &[2]);
        assert_eq!(
            dispatcher.dispatch_at(
                segment(&invalid_final_storage, invalid_final_descriptor),
                5,
                |_| Esp32s31ApRxAdmission::authorized(owner),
                &mut sink,
            ),
            Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Fragment(
                OpenDataFragmentError::InvalidLlcSnap
            ))
        );

        invalid_first_storage[PUBLIC_HEADER_SIZE + 1] |= 0x08;
        assert_eq!(
            dispatcher.dispatch_at(
                segment(&invalid_first_storage, invalid_first_descriptor),
                6,
                |_| Esp32s31ApRxAdmission::authorized(owner),
                &mut sink,
            ),
            Esp32s31ApRxDispatch::FragmentBuffered {
                expired: 0,
                evicted: false,
            },
            "failed fragment trains do not poison ordinary duplicate history"
        );
        assert_eq!(dispatcher.clear_open_fragmentation(), 1);
        assert_eq!(sink.ethernet.len(), 1);
    }

    #[test]
    fn admits_only_authorized_peer_and_suppresses_its_retry() {
        const HEADER: usize = 24;
        const PAYLOAD: [u8; 4] = [1, 2, 3, 4];
        const MPDU: usize = HEADER + 8 + 8 + PAYLOAD.len() + 8;
        const SIGNAL: usize = MPDU + 4;
        let mut storage = [0_u8; 192];
        storage[0x1f] = 1;
        storage[TAIL..TAIL + 4]
            .copy_from_slice(&(((SIGNAL + 4) as u32) << 16 | SIGNAL as u32).to_le_bytes());
        let frame = &mut storage[PUBLIC_HEADER_SIZE..PUBLIC_HEADER_SIZE + MPDU];
        frame[..2].copy_from_slice(&0x4108_u16.to_le_bytes());
        frame[4..10].copy_from_slice(&AP);
        frame[10..16].copy_from_slice(&PEER);
        frame[16..22].copy_from_slice(&DESTINATION);
        frame[22..24].copy_from_slice(&0x1230_u16.to_le_bytes());
        frame[HEADER..HEADER + 8].copy_from_slice(&[3, 0, 0, 0x20, 0, 0, 0, 0]);
        frame[HEADER + 8..HEADER + 16].copy_from_slice(&[0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0]);
        frame[HEADER + 16..HEADER + 20].copy_from_slice(&PAYLOAD);
        let descriptor_word0 =
            192 | (((PUBLIC_HEADER_SIZE + SIGNAL) as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31;
        let mut dispatcher = Esp32s31ApRxDispatcher::new(config());
        assert_eq!(
            dispatcher.reorder_key(segment(&storage, descriptor_word0)),
            None,
            "legacy data does not enter a BlockAck sequence space"
        );
        let mut sink = Sink::default();
        assert_eq!(
            dispatcher
                .dispatch_protected(segment(&storage, descriptor_word0), |_| None, &mut sink,),
            Esp32s31ApRxDispatch::Unauthorized
        );
        assert_eq!(
            dispatcher.dispatch_protected(
                segment(&storage, descriptor_word0),
                |candidate| (candidate == PEER).then_some(duplicate_owner(1, 1)),
                &mut sink,
            ),
            Esp32s31ApRxDispatch::Data {
                ethernet_frames: 1,
                amsdu: false,
            }
        );
        assert_eq!(&sink.ethernet[0][..6], &DESTINATION);
        assert_eq!(&sink.ethernet[0][6..12], &PEER);
        assert_eq!(&sink.ethernet[0][14..], &PAYLOAD);

        storage[PUBLIC_HEADER_SIZE + 1] |= 0x08;
        assert_eq!(
            dispatcher.dispatch_protected(
                segment(&storage, descriptor_word0),
                |candidate| (candidate == PEER).then_some(duplicate_owner(1, 1)),
                &mut sink,
            ),
            Esp32s31ApRxDispatch::Duplicate
        );

        storage[PUBLIC_HEADER_SIZE + 10..PUBLIC_HEADER_SIZE + 16].copy_from_slice(&OTHER_PEER);
        assert_eq!(
            dispatcher.dispatch_protected(
                segment(&storage, descriptor_word0),
                |candidate| match candidate {
                    PEER => Some(duplicate_owner(1, 1)),
                    OTHER_PEER => Some(duplicate_owner(2, 1)),
                    _ => None,
                },
                &mut sink,
            ),
            Esp32s31ApRxDispatch::Data {
                ethernet_frames: 1,
                amsdu: false,
            }
        );
    }

    #[test]
    fn reused_pairwise_pn_is_rejected_before_publication_even_with_a_new_sequence() {
        const HEADER: usize = 24;
        const PAYLOAD: [u8; 4] = [1, 2, 3, 4];
        const MPDU: usize = HEADER + 8 + 8 + PAYLOAD.len() + 8;
        const SIGNAL: usize = MPDU + 4;
        let mut storage = [0_u8; 192];
        storage[0x1f] = 1;
        storage[TAIL..TAIL + 4]
            .copy_from_slice(&(((SIGNAL + 4) as u32) << 16 | SIGNAL as u32).to_le_bytes());
        let frame = &mut storage[PUBLIC_HEADER_SIZE..PUBLIC_HEADER_SIZE + MPDU];
        frame[..2].copy_from_slice(&0x4108_u16.to_le_bytes());
        frame[4..10].copy_from_slice(&AP);
        frame[10..16].copy_from_slice(&PEER);
        frame[16..22].copy_from_slice(&DESTINATION);
        frame[22..24].copy_from_slice(&0x1230_u16.to_le_bytes());
        frame[HEADER..HEADER + 8].copy_from_slice(&[3, 0, 0, 0x20, 0, 0, 0, 0]);
        frame[HEADER + 8..HEADER + 16].copy_from_slice(&[0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0]);
        frame[HEADER + 16..HEADER + 20].copy_from_slice(&PAYLOAD);
        let descriptor_word0 =
            192 | (((PUBLIC_HEADER_SIZE + SIGNAL) as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31;
        let mut dispatcher = Esp32s31ApRxDispatcher::new(config());
        let mut sink = Sink::default();
        let mut replay = CcmpRxReplayState::default();
        let mut admit = |request: Esp32s31ApRxAdmissionRequest| {
            let header = request
                .ccmp_header()
                .expect("WPA2 dispatch carries one parsed CCMP header");
            match replay.prepare(request.lane(), header.packet_number()) {
                Ok(candidate) => match replay.commit(candidate) {
                    Ok(()) => Esp32s31ApRxAdmission::authorized(duplicate_owner(1, 1)),
                    Err(error) => Esp32s31ApRxAdmission::rejected(Esp32s31ApRxError::Replay(error)),
                },
                Err(error) => Esp32s31ApRxAdmission::rejected(Esp32s31ApRxError::Replay(error)),
            }
        };

        assert_eq!(
            dispatcher.dispatch(segment(&storage, descriptor_word0), &mut admit, &mut sink,),
            Esp32s31ApRxDispatch::Data {
                ethernet_frames: 1,
                amsdu: false,
            }
        );
        storage[PUBLIC_HEADER_SIZE + 22..PUBLIC_HEADER_SIZE + 24]
            .copy_from_slice(&0x4560_u16.to_le_bytes());
        let pn3 = CcmpPacketNumber::new(3).unwrap();
        assert_eq!(
            dispatcher.dispatch(segment(&storage, descriptor_word0), &mut admit, &mut sink,),
            Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Replay(CcmpReplayError::Replayed {
                packet_number: pn3,
                highest: pn3,
            })),
        );
        assert_eq!(sink.ethernet.len(), 1);

        storage[PUBLIC_HEADER_SIZE + HEADER + 3] = 0x60;
        assert_eq!(
            dispatcher.dispatch(segment(&storage, descriptor_word0), &mut admit, &mut sink,),
            Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::PairwiseKeyId(1)),
        );
        assert_eq!(sink.ethernet.len(), 1);
    }

    #[test]
    fn duplicate_slots_are_reclaimed_across_close_reassociation_and_stop() {
        let mut dispatcher = Esp32s31ApRxDispatcher::new(config());
        let first = duplicate_owner(1, 1);
        assert!(
            !dispatcher
                .duplicate_filter(PEER, first)
                .is_duplicate(false, 0x1230, None)
        );
        assert!(
            dispatcher
                .duplicate_filter(PEER, first)
                .is_duplicate(true, 0x1230, None)
        );

        assert!(dispatcher.forget_peer(PEER));
        assert!(
            !dispatcher
                .duplicate_filter(PEER, first)
                .is_duplicate(true, 0x1230, None)
        );

        // Same-address reassociation retains its AID but owns a new epoch.
        let reassociated = duplicate_owner(1, 2);
        assert!(
            !dispatcher
                .duplicate_filter(PEER, reassociated)
                .is_duplicate(true, 0x1230, None)
        );

        // Churn through every bounded AID and then reuse one. No stale peer
        // can consume capacity because an AID selects its exact slot.
        for association_id in 1..=AP_MAX_CLIENTS as u16 {
            let mut peer = PEER;
            peer[5] = u8::try_from(association_id).unwrap();
            let owner = duplicate_owner(association_id, 10 + u32::from(association_id));
            assert!(!dispatcher.duplicate_filter(peer, owner).is_duplicate(
                false,
                association_id << 4,
                None
            ));
        }
        assert_eq!(
            dispatcher.duplicates.iter().flatten().count(),
            AP_MAX_CLIENTS
        );
        assert!(
            !dispatcher
                .duplicate_filter(OTHER_PEER, duplicate_owner(1, 99))
                .is_duplicate(true, 0x1230, None)
        );

        dispatcher.reset(config());
        assert!(dispatcher.duplicates.iter().all(Option::is_none));
        assert!(
            !dispatcher
                .duplicate_filter(PEER, duplicate_owner(1, 100))
                .is_duplicate(true, 0x1230, None)
        );
    }
}
