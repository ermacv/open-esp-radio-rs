//! Protected AP data receive frontier.
//!
//! DMA storage remains borrowed only for this synchronous dispatch. A sink
//! must copy or transfer every Ethernet view before returning, after which the
//! runtime may recycle the descriptor.

use open_esp_radio_esp32s31_wifi::protected_data_rx::{
    ProtectedDataDecapsulation, ProtectedDataRxView, UnprotectedDataRxView, view_protected_data,
    view_unprotected_data,
};
use open_esp_radio_esp32s31_wifi_mac::{
    rx::{RxError, RxIngressConfig, RxPhyInfo, RxSegment, view_normalized_rx_frame},
    rx_ampdu::{RxBlockAckMpduKey, rx_block_ack_mpdu_key},
};
use open_esp_radio_ieee80211::ccmp::{CcmpHeader, CcmpKeyId, CcmpReplayError, CcmpReplayLane};
use open_esp_radio_ieee80211::data::{
    DataDecapError, DataInterfaceRole, EthernetFrameParts, RxDuplicateFilter,
};
use open_esp_radio_ieee80211::security::WifiSecurityMode;
use open_esp_radio_wifi_ap::AP_MAX_CLIENTS;
use open_esp_radio_wifi_softmac::MacRxMetadata;

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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ApRxDispatch {
    Data { ethernet_frames: u8, amsdu: bool },
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
    Authorized,
    Unauthorized,
    Rejected(Esp32s31ApRxError),
}

/// Unforgeable result of consulting the live AP controlled-port and key
/// owner. Its constructors stay within the chip AP crate so an integration
/// cannot accidentally authorize WPA2 RX with a Boolean or generation zero.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ApRxAdmission {
    outcome: Esp32s31ApRxAdmissionOutcome,
}

impl Esp32s31ApRxAdmission {
    pub(crate) const fn authorized() -> Self {
        Self {
            outcome: Esp32s31ApRxAdmissionOutcome::Authorized,
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
}

impl Esp32s31ApRxDispatcher {
    pub const fn new(config: Esp32s31ApRxConfig) -> Self {
        Self {
            config,
            duplicates: [const { None }; AP_MAX_CLIENTS],
        }
    }

    /// Begin a new AP epoch without moving the per-peer duplicate table
    /// through an executor stack.
    pub fn reset(&mut self, config: Esp32s31ApRxConfig) {
        self.config = config;
        self.duplicates.fill_with(|| None);
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
        if protected != (self.config.security == WifiSecurityMode::Wpa2Personal) {
            return Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::SecurityModeMismatch);
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
        match admit(Esp32s31ApRxAdmissionRequest::new(peer, lane, ccmp_header)).outcome {
            Esp32s31ApRxAdmissionOutcome::Authorized => {}
            Esp32s31ApRxAdmissionOutcome::Unauthorized => {
                return Esp32s31ApRxDispatch::Unauthorized;
            }
            Esp32s31ApRxAdmissionOutcome::Rejected(error) => {
                return Esp32s31ApRxDispatch::Rejected(error);
            }
        }
        let Some(duplicates) = self.duplicate_filter(peer) else {
            return Esp32s31ApRxDispatch::Unauthorized;
        };
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
        A: FnMut([u8; 6]) -> bool,
    {
        let mut is_authorized = is_authorized;
        self.dispatch(
            segment,
            |request| {
                if is_authorized(request.peer()) {
                    Esp32s31ApRxAdmission::authorized()
                } else {
                    Esp32s31ApRxAdmission::unauthorized()
                }
            },
            sink,
        )
    }

    #[inline(always)]
    fn duplicate_filter(&mut self, peer: [u8; 6]) -> Option<&mut RxDuplicateFilter> {
        if let Some(index) = self
            .duplicates
            .iter()
            .position(|entry| entry.as_ref().is_some_and(|state| state.address == peer))
        {
            return self.duplicates[index]
                .as_mut()
                .map(|state| &mut state.filter);
        }
        let index = self.duplicates.iter().position(Option::is_none)?;
        self.duplicates[index] = Some(ApPeerDuplicateState {
            address: peer,
            filter: RxDuplicateFilter::new(),
        });
        self.duplicates[index]
            .as_mut()
            .map(|state| &mut state.filter)
    }
}

fn rejected_data(error: DataDecapError) -> Esp32s31ApRxDispatch {
    Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Data(error))
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

    fn segment(storage: &[u8; 192], descriptor_word0: u32) -> RxSegment<'_> {
        RxSegment {
            descriptor_address: 0x2f00_4000,
            descriptor_word0,
            buffer: storage,
            next_descriptor_address: 0,
        }
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
            dispatcher.dispatch_protected(
                segment(&storage, descriptor_word0),
                |_| false,
                &mut sink,
            ),
            Esp32s31ApRxDispatch::Unauthorized
        );
        assert_eq!(
            dispatcher.dispatch_protected(
                segment(&storage, descriptor_word0),
                |candidate| candidate == PEER,
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
                |candidate| candidate == PEER,
                &mut sink,
            ),
            Esp32s31ApRxDispatch::Duplicate
        );

        storage[PUBLIC_HEADER_SIZE + 10..PUBLIC_HEADER_SIZE + 16].copy_from_slice(&OTHER_PEER);
        assert_eq!(
            dispatcher.dispatch_protected(
                segment(&storage, descriptor_word0),
                |candidate| candidate == PEER || candidate == OTHER_PEER,
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
                    Ok(()) => Esp32s31ApRxAdmission::authorized(),
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
}
