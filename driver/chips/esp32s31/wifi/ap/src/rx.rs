//! Protected AP data receive frontier.
//!
//! DMA storage remains borrowed only for this synchronous dispatch. A sink
//! must copy or transfer every Ethernet view before returning, after which the
//! runtime may recycle the descriptor.

use open_esp_radio_esp32s31_wifi::protected_data_rx::{
    ProtectedDataDecapsulation, ProtectedDataFragmentRxError, ProtectedDataRxView,
    UnprotectedDataFragmentRxError, UnprotectedDataRxView, view_protected_data,
    view_protected_data_fragment, view_unprotected_data, view_unprotected_data_fragment,
};
use open_esp_radio_esp32s31_wifi_mac::{
    rx::{
        PUBLIC_HEADER_SIZE, RxError, RxIngressConfig, RxPhyInfo, RxSegment,
        view_normalized_rx_frame,
    },
    rx_ampdu::{RxBlockAckMpduKey, rx_block_ack_mpdu_key},
};
use open_esp_radio_ieee80211::ccmp::{CcmpHeader, CcmpKeyId, CcmpReplayError, CcmpReplayLane};
use open_esp_radio_ieee80211::data::{
    DataDecapError, DataInterfaceRole, EthernetFrameParts, RxDuplicateFilter,
};
use open_esp_radio_ieee80211::fragmentation::{
    OPEN_DATA_FRAGMENT_TIMEOUT_MICROS, OPEN_DATA_REASSEMBLY_CAPACITY, OpenDataDefragmentation,
    OpenDataDefragmenter, OpenDataFragmentError, OpenDataFragmentPreflight,
    OpenDataUnfragmentedAdmission, parse_ccmp_data_identity, parse_open_data_identity,
};
use open_esp_radio_ieee80211::security::WifiSecurityMode;
use open_esp_radio_wifi_ap::AP_MAX_CLIENTS;
use open_esp_radio_wifi_softmac::MacRxMetadata;

use crate::security::Esp32s31ApPairwiseRxCandidate;

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
    operation: Esp32s31ApRxAdmissionOperation,
}

/// Minimal request for the complete WPA2 pairwise ordinary-data leaf.
///
/// Fragment preparation/commit state cannot be represented by this type, so
/// the saturated path does not carry or branch on the general operation enum.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ApOrdinaryPairwiseRxRequest {
    peer: [u8; 6],
    lane: CcmpReplayLane,
    ccmp_header: CcmpHeader,
}

impl Esp32s31ApOrdinaryPairwiseRxRequest {
    pub(crate) const fn new(peer: [u8; 6], lane: CcmpReplayLane, ccmp_header: CcmpHeader) -> Self {
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

    pub const fn ccmp_header(self) -> CcmpHeader {
        self.ccmp_header
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Esp32s31ApRxAdmissionOperation {
    Ordinary,
    AuthorizeFragment,
    PrepareFragment,
    CommitFragment(Esp32s31ApRxPreparedReplay),
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
            operation: Esp32s31ApRxAdmissionOperation::Ordinary,
        }
    }

    fn authorize_fragment(peer: [u8; 6], lane: CcmpReplayLane, header: CcmpHeader) -> Self {
        Self {
            peer,
            lane,
            ccmp_header: Some(header),
            operation: Esp32s31ApRxAdmissionOperation::AuthorizeFragment,
        }
    }

    fn prepare_fragment(peer: [u8; 6], lane: CcmpReplayLane, header: CcmpHeader) -> Self {
        Self {
            peer,
            lane,
            ccmp_header: Some(header),
            operation: Esp32s31ApRxAdmissionOperation::PrepareFragment,
        }
    }

    fn commit_fragment(prepared: Esp32s31ApRxPreparedReplay) -> Self {
        Self {
            peer: prepared.peer,
            lane: prepared.lane,
            ccmp_header: Some(prepared.ccmp_header),
            operation: Esp32s31ApRxAdmissionOperation::CommitFragment(prepared),
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

    pub(crate) const fn operation(self) -> Esp32s31ApRxAdmissionOperation {
        self.operation
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Esp32s31ApRxPreparedReplay {
    pub(crate) peer: [u8; 6],
    pub(crate) lane: CcmpReplayLane,
    pub(crate) ccmp_header: CcmpHeader,
    pub(crate) owner: Esp32s31ApRxDuplicateOwner,
    pub(crate) candidate: Esp32s31ApRxPreparedCandidate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Esp32s31ApRxPreparedCandidate {
    Hardware(Esp32s31ApPairwiseRxCandidate),
    /// Source tests can model the external two-phase callback without forging
    /// a hardware-key binding. Production engine admission always rejects it.
    #[cfg(test)]
    Model,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Esp32s31ApRxAdmissionOutcome {
    Authorized(Esp32s31ApRxDuplicateOwner),
    Prepared(Esp32s31ApRxPreparedReplay),
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
    key_generation: u32,
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
            key_generation: 0,
        })
    }

    pub(crate) const fn with_key_generation(mut self, key_generation: u32) -> Self {
        self.key_generation = key_generation;
        self
    }

    const fn slot(self) -> usize {
        self.slot as usize
    }

    const fn fragmentation_epoch(self) -> u64 {
        (self.association_epoch as u64) << 32 | self.key_generation as u64
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

    pub(crate) const fn prepared(prepared: Esp32s31ApRxPreparedReplay) -> Self {
        Self {
            outcome: Esp32s31ApRxAdmissionOutcome::Prepared(prepared),
        }
    }

    pub(crate) const fn rejected(error: Esp32s31ApRxError) -> Self {
        Self {
            outcome: Esp32s31ApRxAdmissionOutcome::Rejected(error),
        }
    }

    pub(crate) const fn authorized_owner(self) -> Option<Esp32s31ApRxDuplicateOwner> {
        match self.outcome {
            Esp32s31ApRxAdmissionOutcome::Authorized(owner) => Some(owner),
            Esp32s31ApRxAdmissionOutcome::Prepared(_)
            | Esp32s31ApRxAdmissionOutcome::Unauthorized
            | Esp32s31ApRxAdmissionOutcome::Rejected(_) => None,
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
    fragment_admission_active: bool,
}

impl Esp32s31ApRxDispatcher {
    pub const fn new(config: Esp32s31ApRxConfig) -> Self {
        Self {
            config,
            duplicates: [const { None }; AP_MAX_CLIENTS],
            fragments: OpenDataDefragmenter::new(OPEN_DATA_FRAGMENT_TIMEOUT_MICROS),
            fragment_admission_active: false,
        }
    }

    /// Begin a new AP epoch without moving the per-peer duplicate table
    /// through an executor stack.
    pub fn reset(&mut self, config: Esp32s31ApRxConfig) {
        self.config = config;
        self.duplicates.fill_with(|| None);
        self.fragments.clear();
        self.fragment_admission_active = false;
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
        self.fragment_admission_active = false;
        self.fragments.clear()
    }

    /// Return whether an ordinary dispatch can borrow its Ethernet payload
    /// directly from the current staging frame. Reassembled payloads live in
    /// the fragment owner and must take the adapter's copying slow path.
    pub fn may_publish_in_place(&self, segment: RxSegment<'_>) -> bool {
        // This is an immutable path-selection hint, not protocol admission.
        // The dispatcher below still performs the complete normalized view,
        // security and bounds validation. Avoid repeating that full parser on
        // every ordinary in-order MPDU merely to inspect three public header
        // bits.
        segment
            .buffer
            .get(PUBLIC_HEADER_SIZE..PUBLIC_HEADER_SIZE + 24)
            .is_some_and(|mpdu| !fragmented_mpdu(mpdu))
    }

    /// Return whether the immutable public header selects the complete
    /// ordinary WPA2 QoS-data leaf.
    ///
    /// This is only a path-selection hint. The selected leaf still performs
    /// normalized hardware/CCMP validation, AP address validation, controlled
    /// port admission, replay, duplicate filtering and Ethernet decapsulation.
    /// Any fragment epoch, A-MSDU or non-QoS subtype stays on the complete AP
    /// role graph.
    pub fn may_dispatch_ordinary_pairwise(&self, segment: RxSegment<'_>) -> bool {
        if self.config.security != WifiSecurityMode::Wpa2Personal || self.fragment_admission_active
        {
            return false;
        }
        let Some(mpdu) = segment.buffer.get(PUBLIC_HEADER_SIZE..) else {
            return false;
        };
        let Some(frame_control) = mpdu
            .get(..2)
            .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
        else {
            return false;
        };
        // Type=Data, subtype=QoS Data, Protected=1.
        if frame_control & 0x00fc != 0x0088 || frame_control & 0x4000 == 0 {
            return false;
        }
        if fragmented_mpdu(mpdu) {
            return false;
        }
        let qos_control_offset = 24 + usize::from(frame_control & 0x0300 == 0x0300) * 6;
        mpdu.get(qos_control_offset)
            .is_some_and(|control| control & 0x80 == 0)
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

    #[inline(never)]
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
    #[inline(never)]
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

    /// Dispatch the common complete WPA2 pairwise MPDU without entering the
    /// fragment/open/general role graph.
    ///
    /// The adapter selects this only after immutable public-header preflight
    /// has proved one ordinary current-buffer publication. This function still
    /// performs the complete hardware-CCMP, AP-address, controlled-port,
    /// replay-generation, duplicate and Ethernet validation. If a previous
    /// fragment activated the exceptional ownership graph, it falls back
    /// before mutating any state.
    #[inline(never)]
    pub fn try_dispatch_ordinary_pairwise<S, A>(
        &mut self,
        segment: RxSegment<'_>,
        admit: A,
        sink: &mut S,
    ) -> Option<Esp32s31ApRxDispatch>
    where
        S: Esp32s31ApRxSink,
        A: FnMut(Esp32s31ApOrdinaryPairwiseRxRequest) -> Esp32s31ApRxAdmission,
    {
        if self.config.security != WifiSecurityMode::Wpa2Personal || self.fragment_admission_active
        {
            return None;
        }
        let data = match view_protected_data(segment, self.config.ingress) {
            Ok(data) => data,
            Err(error) => {
                return Some(Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Radio(
                    error,
                )));
            }
        };
        Some(self.dispatch_ordinary_pairwise_view(data, admit, sink))
    }

    #[inline(never)]
    fn dispatch_ordinary_pairwise_view<S, A>(
        &mut self,
        data: ProtectedDataRxView<'_>,
        mut admit: A,
        sink: &mut S,
    ) -> Esp32s31ApRxDispatch
    where
        S: Esp32s31ApRxSink,
        A: FnMut(Esp32s31ApOrdinaryPairwiseRxRequest) -> Esp32s31ApRxAdmission,
    {
        if data.mpdu[4..10] != self.config.access_point {
            return Esp32s31ApRxDispatch::ForeignPeer;
        }
        let peer: [u8; 6] = data.mpdu[10..16]
            .try_into()
            .expect("validated 802.11 address width");
        if data.ccmp_header.key_id() != CcmpKeyId::PAIRWISE {
            return Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::PairwiseKeyId(
                data.ccmp_header.key_id().value(),
            ));
        }
        let retry = data.retry;
        let sequence_control = data.sequence_control;
        let tid = data.tid;
        let ccmp_header = data.ccmp_header;
        let data = match data.decapsulate(DataInterfaceRole::AccessPoint) {
            Ok(data) => data,
            Err(error) => return rejected_data(error),
        };
        let lane = tid.map_or(CcmpReplayLane::NonQos, CcmpReplayLane::Tid);
        let duplicate_owner = match admit(Esp32s31ApOrdinaryPairwiseRxRequest::new(
            peer,
            lane,
            ccmp_header,
        ))
        .outcome
        {
            Esp32s31ApRxAdmissionOutcome::Authorized(owner) => owner,
            Esp32s31ApRxAdmissionOutcome::Prepared(_) => {
                return Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::KeyGenerationMismatch);
            }
            Esp32s31ApRxAdmissionOutcome::Unauthorized => {
                return Esp32s31ApRxDispatch::Unauthorized;
            }
            Esp32s31ApRxAdmissionOutcome::Rejected(error) => {
                return Esp32s31ApRxDispatch::Rejected(error);
            }
        };
        self.bind_duplicate_owner(peer, duplicate_owner);
        let duplicates = self.duplicates[duplicate_owner.slot()]
            .as_mut()
            .map(|state| &mut state.filter)
            .expect("bound duplicate owner materializes its exact slot");
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

    // Keep the open/fragment/reassembly graph out of ordinary pairwise
    // callers. Without this boundary LLVM duplicates the complete exceptional
    // graph into every admission/sink monomorph, making the common AP leaf an
    // order of magnitude larger than its actual fast body.
    #[inline(never)]
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
        if protected != (self.config.security == WifiSecurityMode::Wpa2Personal) {
            return Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::SecurityModeMismatch);
        }
        if fragmented {
            self.fragment_admission_active = true;
            return match self.config.security {
                WifiSecurityMode::Open => {
                    self.dispatch_open_fragment(segment, now_micros, &mut admit, sink)
                }
                WifiSecurityMode::Wpa2Personal => {
                    self.dispatch_protected_fragment(segment, now_micros, &mut admit, sink)
                }
            };
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
        let preauthorized = if self.fragment_admission_active
            && let Some(header) = ccmp_header
        {
            match admit(Esp32s31ApRxAdmissionRequest::authorize_fragment(
                peer, lane, header,
            ))
            .outcome
            {
                Esp32s31ApRxAdmissionOutcome::Authorized(owner) => {
                    self.bind_duplicate_owner(peer, owner);
                    let identity = match parse_ccmp_data_identity(
                        DataInterfaceRole::AccessPoint,
                        mpdu,
                        header,
                    ) {
                        Ok(identity) => identity,
                        Err(error) => {
                            return Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Fragment(
                                error,
                            ));
                        }
                    };
                    match self.fragments.admit_unfragmented_in_epoch(
                        identity,
                        owner.fragmentation_epoch(),
                        retry,
                        now_micros,
                    ) {
                        Ok(OpenDataUnfragmentedAdmission::Admitted { .. }) => {}
                        Ok(OpenDataUnfragmentedAdmission::Duplicate { .. }) => {
                            return Esp32s31ApRxDispatch::Duplicate;
                        }
                        Err(error) => {
                            return Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Fragment(
                                error,
                            ));
                        }
                    }
                    Some(owner)
                }
                Esp32s31ApRxAdmissionOutcome::Unauthorized => {
                    return Esp32s31ApRxDispatch::Unauthorized;
                }
                Esp32s31ApRxAdmissionOutcome::Rejected(error) => {
                    return Esp32s31ApRxDispatch::Rejected(error);
                }
                Esp32s31ApRxAdmissionOutcome::Prepared(_) => {
                    return Esp32s31ApRxDispatch::Rejected(
                        Esp32s31ApRxError::KeyGenerationMismatch,
                    );
                }
            }
        } else {
            None
        };
        let duplicate_owner =
            match admit(Esp32s31ApRxAdmissionRequest::new(peer, lane, ccmp_header)).outcome {
                Esp32s31ApRxAdmissionOutcome::Authorized(owner) => owner,
                Esp32s31ApRxAdmissionOutcome::Prepared(_) => {
                    return Esp32s31ApRxDispatch::Rejected(
                        Esp32s31ApRxError::KeyGenerationMismatch,
                    );
                }
                Esp32s31ApRxAdmissionOutcome::Unauthorized => {
                    return Esp32s31ApRxDispatch::Unauthorized;
                }
                Esp32s31ApRxAdmissionOutcome::Rejected(error) => {
                    return Esp32s31ApRxDispatch::Rejected(error);
                }
            };
        if preauthorized.is_some_and(|owner| owner != duplicate_owner) {
            self.fragments.forget_transmitter(peer);
            return Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::KeyGenerationMismatch);
        }
        self.bind_duplicate_owner(peer, duplicate_owner);
        if self.fragment_admission_active && ccmp_header.is_none() {
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
        let duplicate_owner = match admit(Esp32s31ApRxAdmissionRequest::new(peer, lane, None))
            .outcome
        {
            Esp32s31ApRxAdmissionOutcome::Authorized(owner) => owner,
            Esp32s31ApRxAdmissionOutcome::Prepared(_) => {
                return Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::KeyGenerationMismatch);
            }
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

    fn dispatch_protected_fragment<S, A>(
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
        let view = match view_protected_data_fragment(
            segment,
            self.config.ingress,
            DataInterfaceRole::AccessPoint,
        ) {
            Ok(view) => view,
            Err(ProtectedDataFragmentRxError::Radio(error)) => {
                return Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Radio(error));
            }
            Err(ProtectedDataFragmentRxError::Fragment(error)) => {
                return Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Fragment(error));
            }
        };
        let identity = view.fragment.identity();
        if identity.receiver_address() != self.config.access_point {
            return Esp32s31ApRxDispatch::ForeignPeer;
        }
        if view.ccmp_header.key_id() != CcmpKeyId::PAIRWISE {
            return Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::PairwiseKeyId(
                view.ccmp_header.key_id().value(),
            ));
        }
        let peer = identity.transmitter_address();
        let lane = identity
            .tid()
            .map_or(CcmpReplayLane::NonQos, CcmpReplayLane::Tid);
        let owner = match admit(Esp32s31ApRxAdmissionRequest::authorize_fragment(
            peer,
            lane,
            view.ccmp_header,
        ))
        .outcome
        {
            Esp32s31ApRxAdmissionOutcome::Authorized(owner) => owner,
            Esp32s31ApRxAdmissionOutcome::Unauthorized => {
                return Esp32s31ApRxDispatch::Unauthorized;
            }
            Esp32s31ApRxAdmissionOutcome::Rejected(error) => {
                return Esp32s31ApRxDispatch::Rejected(error);
            }
            Esp32s31ApRxAdmissionOutcome::Prepared(_) => {
                return Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::KeyGenerationMismatch);
            }
        };
        if view.fragment.fragment_number() == 0
            && self.is_known_ordinary_duplicate(
                peer,
                owner,
                view.fragment.retry(),
                view.fragment.sequence_control(),
                identity.tid(),
            )
        {
            // Consult only the exact association/PTK generation already
            // authorized above. This must precede duplicate-owner binding,
            // replay preparation and fragment preflight: all three can mutate
            // state, while an ordinary MPDU Retry is a read-only rejection.
            return Esp32s31ApRxDispatch::Duplicate;
        }
        self.bind_duplicate_owner(peer, owner);
        let fragment = view.fragment;
        let more_fragments = fragment.more_fragments();
        let raw = view.raw;
        let metadata = view.metadata;
        let epoch = owner.fragmentation_epoch();
        let admission = match self
            .fragments
            .preflight_in_epoch(fragment, epoch, now_micros)
        {
            Ok(OpenDataFragmentPreflight::Duplicate { .. }) => {
                return Esp32s31ApRxDispatch::Duplicate;
            }
            Ok(OpenDataFragmentPreflight::Admitted(admission)) => admission,
            Err(error) => {
                return Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Fragment(error));
            }
        };
        let prepared = match admit(Esp32s31ApRxAdmissionRequest::prepare_fragment(
            peer,
            lane,
            view.ccmp_header,
        ))
        .outcome
        {
            Esp32s31ApRxAdmissionOutcome::Prepared(prepared) if prepared.owner == owner => prepared,
            Esp32s31ApRxAdmissionOutcome::Prepared(_) => {
                return Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::KeyGenerationMismatch);
            }
            Esp32s31ApRxAdmissionOutcome::Unauthorized => {
                return Esp32s31ApRxDispatch::Unauthorized;
            }
            Esp32s31ApRxAdmissionOutcome::Rejected(error) => {
                return Esp32s31ApRxDispatch::Rejected(error);
            }
            Esp32s31ApRxAdmissionOutcome::Authorized(_) => {
                return Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::KeyGenerationMismatch);
            }
        };

        if more_fragments {
            let outcome = match admission
                .ingest(|_| unreachable!("More Fragments cannot complete one MSDU"))
            {
                Ok(outcome) => outcome,
                Err(error) => {
                    return Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Fragment(error));
                }
            };
            let commit = match admit(Esp32s31ApRxAdmissionRequest::commit_fragment(prepared))
                .outcome
            {
                Esp32s31ApRxAdmissionOutcome::Authorized(committed) if committed == owner => Ok(()),
                Esp32s31ApRxAdmissionOutcome::Rejected(error) => Err(error),
                _ => Err(Esp32s31ApRxError::KeyGenerationMismatch),
            };
            if let Err(error) = commit {
                self.fragments.discard(identity, epoch);
                return Esp32s31ApRxDispatch::Rejected(error);
            }
            return match outcome {
                OpenDataDefragmentation::Buffered { expired, evicted } => {
                    Esp32s31ApRxDispatch::FragmentBuffered {
                        expired,
                        evicted: evicted.is_some(),
                    }
                }
                _ => unreachable!("More Fragments produces only a buffered admission"),
            };
        }

        let outcome = admission.ingest(|data| {
            match admit(Esp32s31ApRxAdmissionRequest::commit_fragment(prepared)).outcome {
                Esp32s31ApRxAdmissionOutcome::Authorized(committed) if committed == owner => {}
                Esp32s31ApRxAdmissionOutcome::Rejected(error) => return Err(error),
                _ => return Err(Esp32s31ApRxError::KeyGenerationMismatch),
            }
            sink.publish(Esp32s31ApRxEvent {
                frame: data.ethernet_frame(),
                raw,
                amsdu: false,
                metadata,
            });
            Ok::<(), Esp32s31ApRxError>(())
        });
        match outcome {
            Ok(OpenDataDefragmentation::Complete { value: Ok(()), .. }) => {
                Esp32s31ApRxDispatch::Data {
                    ethernet_frames: 1,
                    amsdu: false,
                }
            }
            Ok(OpenDataDefragmentation::Complete {
                value: Err(error), ..
            }) => {
                self.fragments.discard(identity, epoch);
                Esp32s31ApRxDispatch::Rejected(error)
            }
            Ok(_) => unreachable!("final fragment produces one completion"),
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

    #[inline(always)]
    fn is_known_ordinary_duplicate(
        &self,
        peer: [u8; 6],
        owner: Esp32s31ApRxDuplicateOwner,
        retry: bool,
        sequence_control: u16,
        tid: Option<u8>,
    ) -> bool {
        self.duplicates
            .get(owner.slot())
            .and_then(Option::as_ref)
            .filter(|state| state.address == peer && state.owner == owner)
            .is_some_and(|state| {
                state
                    .filter
                    .is_known_duplicate(retry, sequence_control, tid)
            })
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

    #[expect(
        clippy::too_many_arguments,
        reason = "the test vector keeps every independently mutated 802.11/CCMP field explicit"
    )]
    fn protected_fragment(
        storage: &mut [u8; 192],
        sequence: u16,
        fragment: u8,
        more_fragments: bool,
        retry: bool,
        packet_number: u64,
        address3: [u8; 6],
        payload: &[u8],
    ) -> u32 {
        let mpdu_length = 24 + 8 + payload.len() + 8;
        let signal_length = mpdu_length + 4;
        storage.fill(0);
        storage[0x1f] = 1;
        storage[TAIL..TAIL + 4].copy_from_slice(
            &(((signal_length + 4) as u32) << 16 | signal_length as u32).to_le_bytes(),
        );
        let frame = &mut storage[PUBLIC_HEADER_SIZE..PUBLIC_HEADER_SIZE + mpdu_length];
        let mut frame_control = 0x4108_u16;
        if more_fragments {
            frame_control |= 0x0400;
        }
        if retry {
            frame_control |= 0x0800;
        }
        frame[..2].copy_from_slice(&frame_control.to_le_bytes());
        frame[4..10].copy_from_slice(&AP);
        frame[10..16].copy_from_slice(&PEER);
        frame[16..22].copy_from_slice(&address3);
        frame[22..24].copy_from_slice(&((sequence << 4) | u16::from(fragment)).to_le_bytes());
        frame[24..32].copy_from_slice(
            &CcmpHeader::new(
                CcmpPacketNumber::new(packet_number).unwrap(),
                CcmpKeyId::PAIRWISE,
            )
            .encode(),
        );
        frame[32..32 + payload.len()].copy_from_slice(payload);
        192 | (((PUBLIC_HEADER_SIZE + signal_length) as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31
    }

    #[test]
    fn ordinary_pairwise_fast_path_matches_general_dispatch_and_duplicate_state() {
        let payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1, 2, 3, 4];
        let mut storage = [0_u8; 192];
        let descriptor =
            protected_fragment(&mut storage, 17, 0, false, false, 3, DESTINATION, &payload);
        let owner = duplicate_owner(1, 9).with_key_generation(4);
        let mut general = Esp32s31ApRxDispatcher::new(config());
        let mut fast = Esp32s31ApRxDispatcher::new(config());
        let mut general_sink = Sink::default();
        let mut fast_sink = Sink::default();

        let general_result = general.dispatch_at(
            segment(&storage, descriptor),
            1,
            |_| Esp32s31ApRxAdmission::authorized(owner),
            &mut general_sink,
        );
        let fast_result = fast
            .try_dispatch_ordinary_pairwise(
                segment(&storage, descriptor),
                |_| Esp32s31ApRxAdmission::authorized(owner),
                &mut fast_sink,
            )
            .expect("ordinary pairwise path is available");

        assert_eq!(fast_result, general_result);
        assert_eq!(fast_sink.ethernet, general_sink.ethernet);
        assert_eq!(
            fast_result,
            Esp32s31ApRxDispatch::Data {
                ethernet_frames: 1,
                amsdu: false,
            }
        );

        storage[PUBLIC_HEADER_SIZE + 1] |= 0x08;
        assert_eq!(
            general.dispatch_at(
                segment(&storage, descriptor),
                2,
                |_| Esp32s31ApRxAdmission::authorized(owner),
                &mut general_sink,
            ),
            Esp32s31ApRxDispatch::Duplicate
        );
        assert_eq!(
            fast.try_dispatch_ordinary_pairwise(
                segment(&storage, descriptor),
                |_| Esp32s31ApRxAdmission::authorized(owner),
                &mut fast_sink,
            ),
            Some(Esp32s31ApRxDispatch::Duplicate)
        );
    }

    #[test]
    fn ordinary_pairwise_fallback_preserves_fragment_clock() {
        let payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1, 2, 3, 4];
        let mut storage = [0_u8; 192];
        let descriptor =
            protected_fragment(&mut storage, 18, 0, false, false, 4, DESTINATION, &payload);
        let owner = duplicate_owner(1, 10).with_key_generation(5);
        let mut general = Esp32s31ApRxDispatcher::new(config());
        let mut fast = Esp32s31ApRxDispatcher::new(config());
        // Model the exceptional state left after any earlier fragment. The
        // typed ordinary leaf must decline without mutation; the caller then
        // enters the complete graph with its fragment clock.
        general.fragment_admission_active = true;
        fast.fragment_admission_active = true;
        let mut general_sink = Sink::default();
        let mut fast_sink = Sink::default();

        let mut admit = |_: Esp32s31ApRxAdmissionRequest| Esp32s31ApRxAdmission::authorized(owner);
        let general_result = general.dispatch_at(
            segment(&storage, descriptor),
            77,
            &mut admit,
            &mut general_sink,
        );
        assert_eq!(
            fast.try_dispatch_ordinary_pairwise(
                segment(&storage, descriptor),
                |_| Esp32s31ApRxAdmission::authorized(owner),
                &mut fast_sink,
            ),
            None
        );
        let fast_result = fast.dispatch_at(
            segment(&storage, descriptor),
            77,
            &mut admit,
            &mut fast_sink,
        );

        assert_eq!(fast_result, general_result);
        assert_eq!(fast_sink.ethernet, general_sink.ethernet);
        assert_eq!(
            fast_result,
            Esp32s31ApRxDispatch::Data {
                ethernet_frames: 1,
                amsdu: false,
            }
        );
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
    fn ap_ccmp_fragments_commit_each_pn_after_exact_bounded_admission() {
        let first_payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1, 2];
        let final_payload = [3, 4, 5];
        let mut first_storage = [0_u8; 192];
        let first_descriptor = protected_fragment(
            &mut first_storage,
            7,
            0,
            true,
            false,
            3,
            DESTINATION,
            &first_payload,
        );
        let mut final_storage = [0_u8; 192];
        let final_descriptor = protected_fragment(
            &mut final_storage,
            7,
            1,
            false,
            false,
            4,
            DESTINATION,
            &final_payload,
        );
        let mut dispatcher = Esp32s31ApRxDispatcher::new(config());
        let mut sink = Sink::default();
        let owner = duplicate_owner(1, 1).with_key_generation(7);
        let replay = core::cell::RefCell::new(CcmpRxReplayState::default());
        let pending = core::cell::RefCell::new(None);
        let mut admit = |request: Esp32s31ApRxAdmissionRequest| match request.operation() {
            Esp32s31ApRxAdmissionOperation::AuthorizeFragment => {
                Esp32s31ApRxAdmission::authorized(owner)
            }
            Esp32s31ApRxAdmissionOperation::PrepareFragment => {
                let header = request.ccmp_header().unwrap();
                match replay
                    .borrow()
                    .prepare(request.lane(), header.packet_number())
                {
                    Ok(candidate) => {
                        pending.replace(Some(candidate));
                        Esp32s31ApRxAdmission::prepared(Esp32s31ApRxPreparedReplay {
                            peer: request.peer(),
                            lane: request.lane(),
                            ccmp_header: header,
                            owner,
                            candidate: Esp32s31ApRxPreparedCandidate::Model,
                        })
                    }
                    Err(error) => Esp32s31ApRxAdmission::rejected(Esp32s31ApRxError::Replay(error)),
                }
            }
            Esp32s31ApRxAdmissionOperation::CommitFragment(_) => {
                let candidate = pending
                    .borrow_mut()
                    .take()
                    .expect("one prepared replay candidate");
                match replay.borrow_mut().commit(candidate) {
                    Ok(()) => Esp32s31ApRxAdmission::authorized(owner),
                    Err(error) => Esp32s31ApRxAdmission::rejected(Esp32s31ApRxError::Replay(error)),
                }
            }
            Esp32s31ApRxAdmissionOperation::Ordinary => {
                panic!("fragment path cannot request ordinary admission")
            }
        };

        assert_eq!(
            dispatcher.dispatch_at(
                segment(&first_storage, first_descriptor),
                1,
                &mut admit,
                &mut sink,
            ),
            Esp32s31ApRxDispatch::FragmentBuffered {
                expired: 0,
                evicted: false,
            }
        );
        first_storage[PUBLIC_HEADER_SIZE + 1] |= 0x08;
        assert_eq!(
            dispatcher.dispatch_at(
                segment(&first_storage, first_descriptor),
                2,
                &mut admit,
                &mut sink,
            ),
            Esp32s31ApRxDispatch::Duplicate
        );
        first_storage[PUBLIC_HEADER_SIZE + 32] ^= 0x01;
        assert_eq!(
            dispatcher.dispatch_at(
                segment(&first_storage, first_descriptor),
                3,
                &mut admit,
                &mut sink,
            ),
            Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Fragment(
                OpenDataFragmentError::RetryPayloadMismatch { fragment_number: 0 }
            ))
        );
        assert_eq!(
            dispatcher.dispatch_at(
                segment(&final_storage, final_descriptor),
                4,
                &mut admit,
                &mut sink,
            ),
            Esp32s31ApRxDispatch::Data {
                ethernet_frames: 1,
                amsdu: false,
            }
        );
        assert_eq!(sink.ethernet.len(), 1);
        assert_eq!(&sink.ethernet[0][14..], &[1, 2, 3, 4, 5]);

        final_storage[PUBLIC_HEADER_SIZE + 1] |= 0x08;
        assert_eq!(
            dispatcher.dispatch_at(
                segment(&final_storage, final_descriptor),
                5,
                &mut admit,
                &mut sink,
            ),
            Esp32s31ApRxDispatch::Duplicate
        );
        assert_eq!(sink.ethernet.len(), 1);
        assert!(pending.borrow().is_none());
        assert_eq!(
            replay.borrow().highest(CcmpReplayLane::NonQos),
            CcmpPacketNumber::new(4)
        );
    }

    #[test]
    fn protected_retry_cannot_turn_an_ordinary_mpdu_into_a_fragment_train() {
        let payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
        let mut ordinary_storage = [0_u8; 192];
        let ordinary_descriptor = protected_fragment(
            &mut ordinary_storage,
            7,
            0,
            false,
            false,
            3,
            DESTINATION,
            &payload,
        );
        let mut retry_first_storage = [0_u8; 192];
        let retry_first_descriptor = protected_fragment(
            &mut retry_first_storage,
            7,
            0,
            true,
            true,
            4,
            DESTINATION,
            &payload,
        );
        let mut colliding_final_storage = [0_u8; 192];
        let colliding_final_descriptor = protected_fragment(
            &mut colliding_final_storage,
            7,
            1,
            false,
            false,
            5,
            DESTINATION,
            &[2],
        );
        let mut new_first_storage = [0_u8; 192];
        let new_first_descriptor = protected_fragment(
            &mut new_first_storage,
            8,
            0,
            true,
            true,
            4,
            DESTINATION,
            &payload,
        );
        let mut new_final_storage = [0_u8; 192];
        let new_final_descriptor = protected_fragment(
            &mut new_final_storage,
            8,
            1,
            false,
            false,
            5,
            DESTINATION,
            &[2],
        );
        let owner = duplicate_owner(1, 1).with_key_generation(7);
        let replay = core::cell::RefCell::new(CcmpRxReplayState::default());
        let pending = core::cell::RefCell::new(None);
        let mut admit = |request: Esp32s31ApRxAdmissionRequest| match request.operation() {
            Esp32s31ApRxAdmissionOperation::Ordinary => {
                let header = request.ccmp_header().expect("protected ordinary request");
                let candidate = {
                    replay
                        .borrow()
                        .prepare(request.lane(), header.packet_number())
                };
                match candidate.and_then(|candidate| replay.borrow_mut().commit(candidate)) {
                    Ok(()) => Esp32s31ApRxAdmission::authorized(owner),
                    Err(error) => Esp32s31ApRxAdmission::rejected(Esp32s31ApRxError::Replay(error)),
                }
            }
            Esp32s31ApRxAdmissionOperation::AuthorizeFragment => {
                Esp32s31ApRxAdmission::authorized(owner)
            }
            Esp32s31ApRxAdmissionOperation::PrepareFragment => {
                let header = request.ccmp_header().expect("protected fragment request");
                match replay
                    .borrow()
                    .prepare(request.lane(), header.packet_number())
                {
                    Ok(candidate) => {
                        pending.replace(Some(candidate));
                        Esp32s31ApRxAdmission::prepared(Esp32s31ApRxPreparedReplay {
                            peer: request.peer(),
                            lane: request.lane(),
                            ccmp_header: header,
                            owner,
                            candidate: Esp32s31ApRxPreparedCandidate::Model,
                        })
                    }
                    Err(error) => Esp32s31ApRxAdmission::rejected(Esp32s31ApRxError::Replay(error)),
                }
            }
            Esp32s31ApRxAdmissionOperation::CommitFragment(_) => {
                let candidate = pending
                    .borrow_mut()
                    .take()
                    .expect("one prepared replay candidate");
                match replay.borrow_mut().commit(candidate) {
                    Ok(()) => Esp32s31ApRxAdmission::authorized(owner),
                    Err(error) => Esp32s31ApRxAdmission::rejected(Esp32s31ApRxError::Replay(error)),
                }
            }
        };
        let mut dispatcher = Esp32s31ApRxDispatcher::new(config());
        let mut sink = Sink::default();

        assert_eq!(
            dispatcher.dispatch_at(
                segment(&ordinary_storage, ordinary_descriptor),
                1,
                &mut admit,
                &mut sink,
            ),
            Esp32s31ApRxDispatch::Data {
                ethernet_frames: 1,
                amsdu: false,
            }
        );
        assert_eq!(sink.ethernet.len(), 1);

        assert_eq!(
            dispatcher.dispatch_at(
                segment(&retry_first_storage, retry_first_descriptor),
                2,
                &mut admit,
                &mut sink,
            ),
            Esp32s31ApRxDispatch::Duplicate
        );
        assert_eq!(dispatcher.fragments.active_contexts(), 0);
        assert!(pending.borrow().is_none());
        assert_eq!(
            replay.borrow().highest(CcmpReplayLane::NonQos),
            CcmpPacketNumber::new(3),
            "duplicate admission must precede replay prepare and commit"
        );

        assert_eq!(
            dispatcher.dispatch_at(
                segment(&colliding_final_storage, colliding_final_descriptor),
                3,
                &mut admit,
                &mut sink,
            ),
            Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Fragment(
                OpenDataFragmentError::Orphan { fragment_number: 1 }
            ))
        );
        assert_eq!(dispatcher.fragments.active_contexts(), 0);
        assert_eq!(sink.ethernet.len(), 1);

        // A retried fragment zero absent from ordinary history remains a
        // legitimate new train and owns its normal per-fragment replay edges.
        assert_eq!(
            dispatcher.dispatch_at(
                segment(&new_first_storage, new_first_descriptor),
                4,
                &mut admit,
                &mut sink,
            ),
            Esp32s31ApRxDispatch::FragmentBuffered {
                expired: 0,
                evicted: false,
            }
        );
        assert_eq!(
            dispatcher.dispatch_at(
                segment(&new_final_storage, new_final_descriptor),
                5,
                &mut admit,
                &mut sink,
            ),
            Esp32s31ApRxDispatch::Data {
                ethernet_frames: 1,
                amsdu: false,
            }
        );
        assert_eq!(sink.ethernet.len(), 2);
        assert_eq!(
            replay.borrow().highest(CcmpReplayLane::NonQos),
            CcmpPacketNumber::new(5)
        );
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
            if matches!(
                request.operation(),
                Esp32s31ApRxAdmissionOperation::AuthorizeFragment
            ) {
                return Esp32s31ApRxAdmission::authorized(duplicate_owner(1, 1));
            }
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
