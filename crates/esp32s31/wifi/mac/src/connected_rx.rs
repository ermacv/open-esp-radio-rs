//! Connected-station receive dispatch without an executor or platform policy.
//!
//! This module owns the frame-classification and protocol-routing state that
//! used to be duplicated by the ordinary and TX-interleaved HIL receive loops.
//! It deliberately does not log, access unrelated peripherals, enqueue into a
//! network stack or await an executor primitive. Those effects are published
//! through [`ConnectedRxSink`] and belong to the integration runner.

use open_esp_radio_ieee80211::{
    data::{
        DataDecapError, DataInterfaceRole, EthernetFrameParts, amsdu_subframes,
        plan_data_decapsulation,
    },
    ndpa::{HeNdpa, HeNdpaError},
    station::StaRxDuplicateFilter,
    station_beacon::{StaBeaconError, StaBeaconObservation, parse_sta_beacon},
    trigger::{TriggerCommonInfo, TriggerParseError, parse_trigger_frame},
};

use crate::{
    rx::{
        PUBLIC_HEADER_SIZE, RxError, RxIngressConfig, RxSegment, extract_control,
        extract_management, view_ccmp_data,
    },
    tx::{HeTriggerScheduledRate, HeTriggerScheduledRateError},
    tx_ampdu::{BlockAckAction, parse_block_ack_action},
};

const TRIGGER_FRAME_CONTROL: u16 = 0x0024;
const NDPA_FRAME_CONTROL: u16 = 0x0054;
const ACTION_FRAME_CONTROL: u16 = 0x00d0;
const BEACON_FRAME_CONTROL: u16 = 0x0080;
const DATA_TYPE_MASK: u16 = 0x000c;
const DATA_TYPE: u16 = 0x0008;
const PROTECTED: u16 = 0x4000;
const RETRY: u16 = 0x0800;
const QOS_SUBTYPE: u16 = 0x0080;

/// Immutable identity and descriptor-ingress policy for one connected STA.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectedRxConfig {
    pub station_address: [u8; 6],
    pub bssid: [u8; 6],
    pub association_id: u16,
    pub ingress: RxIngressConfig,
}

/// Destination class observed before protected-data extraction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedRxProtection {
    Pairwise,
    Group,
    Other,
}

/// One semantic event emitted by the connected frame dispatcher.
///
/// Borrowed frame slices remain valid only for the duration of
/// [`ConnectedRxSink::publish`]. A network adapter must copy or otherwise
/// transfer them into storage that it owns before returning.
#[derive(Clone, Copy, Debug)]
pub enum ConnectedRxEvent<'frame> {
    Beacon(StaBeaconObservation),
    ProtectedFrame(ConnectedRxProtection),
    Trigger {
        common: TriggerCommonInfo,
        schedule: Result<HeTriggerScheduledRate, HeTriggerScheduledRateError>,
        first_user: Option<[u8; 5]>,
    },
    Ndpa {
        dialog_token: u8,
        addressed_to_station: bool,
    },
    BlockAck {
        action: BlockAckAction,
        body: &'frame [u8],
    },
    Ethernet {
        frame: EthernetFrameParts<'frame>,
        raw: &'frame [u8],
        amsdu: bool,
    },
}

/// Owned connected-station event that may cross the lifetime of one staged
/// RX frame.
///
/// Ethernet payload is intentionally absent: it must be copied or transferred
/// to the network queue while [`ConnectedRxEvent::Ethernet`] is borrowed.
/// BlockAck carries its complete parsed fixed fields, so no management-frame
/// body or C-style context pointer needs to survive dispatch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedRxControlEvent {
    Beacon(StaBeaconObservation),
    ProtectedFrame(ConnectedRxProtection),
    Trigger {
        common: TriggerCommonInfo,
        schedule: Result<HeTriggerScheduledRate, HeTriggerScheduledRateError>,
        first_user: Option<[u8; 5]>,
    },
    Ndpa {
        dialog_token: u8,
        addressed_to_station: bool,
    },
    BlockAck(BlockAckAction),
}

impl ConnectedRxEvent<'_> {
    /// Copy only the protocol/control information that is independent of the
    /// staged RX allocation.
    pub const fn control(self) -> Option<ConnectedRxControlEvent> {
        match self {
            Self::Beacon(observation) => Some(ConnectedRxControlEvent::Beacon(observation)),
            Self::ProtectedFrame(protection) => {
                Some(ConnectedRxControlEvent::ProtectedFrame(protection))
            }
            Self::Trigger {
                common,
                schedule,
                first_user,
            } => Some(ConnectedRxControlEvent::Trigger {
                common,
                schedule,
                first_user,
            }),
            Self::Ndpa {
                dialog_token,
                addressed_to_station,
            } => Some(ConnectedRxControlEvent::Ndpa {
                dialog_token,
                addressed_to_station,
            }),
            Self::BlockAck { action, .. } => Some(ConnectedRxControlEvent::BlockAck(action)),
            Self::Ethernet { .. } => None,
        }
    }
}

/// Integration boundary for network delivery, diagnostics and PAC effects.
pub trait ConnectedRxSink {
    fn publish(&mut self, event: ConnectedRxEvent<'_>);
}

/// Closed reason for a frame that reached the connected dispatcher but could
/// not produce a semantic event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedRxError {
    PublicHeader,
    Rx(RxError),
    Trigger(TriggerParseError),
    Ndpa(HeNdpaError),
    Beacon(StaBeaconError),
    Data(DataDecapError),
}

/// Result of consuming one independently owned staged RX frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedRxDispatch {
    Beacon,
    Trigger,
    Ndpa,
    BlockAck,
    Data {
        ethernet_frames: u8,
        amsdu: bool,
    },
    Duplicate,
    Ignored,
    Rejected {
        protection: ConnectedRxProtection,
        error: ConnectedRxError,
    },
}

/// Unique protocol-routing state for one connected station.
pub struct ConnectedRxDispatcher {
    config: ConnectedRxConfig,
    duplicate_filter: StaRxDuplicateFilter,
}

impl ConnectedRxDispatcher {
    pub const fn new(config: ConnectedRxConfig) -> Self {
        Self {
            config,
            duplicate_filter: StaRxDuplicateFilter::new(),
        }
    }

    pub const fn config(&self) -> ConnectedRxConfig {
        self.config
    }

    /// Consume one staged S31 frame and publish its connected-station effects.
    ///
    /// The supplied segment is already detached from DMA ownership. This call
    /// is finite and allocation-free; it performs no MMIO and never waits.
    pub fn dispatch<S: ConnectedRxSink>(
        &mut self,
        segment: RxSegment<'_>,
        mpdu: &mut [u8],
        _ethernet: &mut [u8],
        sink: &mut S,
    ) -> ConnectedRxDispatch {
        let raw = segment.buffer;
        let Some(frame_control) = public_frame_control(raw) else {
            return rejected(ConnectedRxProtection::Other, ConnectedRxError::PublicHeader);
        };
        let protection = public_protection(raw, frame_control, self.config.station_address)
            .unwrap_or(ConnectedRxProtection::Other);

        match frame_control & 0x00fc {
            BEACON_FRAME_CONTROL => {
                let management = match extract_management(
                    core::slice::from_ref(&segment),
                    self.config.ingress,
                    mpdu,
                ) {
                    Ok(management) => management,
                    Err(error) => return rejected(protection, ConnectedRxError::Rx(error)),
                };
                let observation = match parse_sta_beacon(
                    &mpdu[..management.length],
                    self.config.bssid,
                    self.config.association_id,
                ) {
                    Ok(observation) => observation,
                    Err(error) => return rejected(protection, ConnectedRxError::Beacon(error)),
                };
                sink.publish(ConnectedRxEvent::Beacon(observation));
                ConnectedRxDispatch::Beacon
            }
            TRIGGER_FRAME_CONTROL => {
                let control = match extract_control(
                    core::slice::from_ref(&segment),
                    self.config.ingress,
                    mpdu,
                ) {
                    Ok(control) => control,
                    Err(error) => return rejected(protection, ConnectedRxError::Rx(error)),
                };
                let trigger = match parse_trigger_frame(&mpdu[..control.length]) {
                    Ok(trigger) => trigger,
                    Err(error) => return rejected(protection, ConnectedRxError::Trigger(error)),
                };
                let first_user = trigger.user_info_and_padding.get(..5).map(|bytes| {
                    let mut first_user = [0_u8; 5];
                    first_user.copy_from_slice(bytes);
                    first_user
                });
                sink.publish(ConnectedRxEvent::Trigger {
                    common: trigger.common,
                    schedule: HeTriggerScheduledRate::from_trigger_frame(
                        &trigger,
                        self.config.association_id,
                    ),
                    first_user,
                });
                ConnectedRxDispatch::Trigger
            }
            NDPA_FRAME_CONTROL => {
                let control = match extract_control(
                    core::slice::from_ref(&segment),
                    self.config.ingress,
                    mpdu,
                ) {
                    Ok(control) => control,
                    Err(error) => return rejected(protection, ConnectedRxError::Rx(error)),
                };
                let ndpa = match HeNdpa::parse(&mpdu[..control.length]) {
                    Ok(ndpa) => ndpa,
                    Err(error) => return rejected(protection, ConnectedRxError::Ndpa(error)),
                };
                sink.publish(ConnectedRxEvent::Ndpa {
                    dialog_token: ndpa.dialog_token(),
                    addressed_to_station: ndpa.contains_association_id(self.config.association_id),
                });
                ConnectedRxDispatch::Ndpa
            }
            ACTION_FRAME_CONTROL => {
                let management = match extract_management(
                    core::slice::from_ref(&segment),
                    self.config.ingress,
                    mpdu,
                ) {
                    Ok(management) => management,
                    Err(error) => return rejected(protection, ConnectedRxError::Rx(error)),
                };
                if management.length < 24
                    || mpdu[4..10] != self.config.station_address
                    || mpdu[10..16] != self.config.bssid
                    || mpdu[16..22] != self.config.bssid
                {
                    return ConnectedRxDispatch::Ignored;
                }
                let body = &mpdu[24..management.length];
                let Some(action) = parse_block_ack_action(body) else {
                    return ConnectedRxDispatch::Ignored;
                };
                sink.publish(ConnectedRxEvent::BlockAck { action, body });
                ConnectedRxDispatch::BlockAck
            }
            _ => self.dispatch_data(segment, raw, frame_control, protection, sink),
        }
    }

    fn dispatch_data<S: ConnectedRxSink>(
        &mut self,
        segment: RxSegment<'_>,
        raw: &[u8],
        public_frame_control: u16,
        protection: ConnectedRxProtection,
        sink: &mut S,
    ) -> ConnectedRxDispatch {
        if public_frame_control & (DATA_TYPE_MASK | PROTECTED) != DATA_TYPE | PROTECTED {
            return ConnectedRxDispatch::Ignored;
        }
        sink.publish(ConnectedRxEvent::ProtectedFrame(protection));
        let data = match view_ccmp_data(&segment, self.config.ingress) {
            Ok(data) => data,
            Err(error) => return rejected(protection, ConnectedRxError::Rx(error)),
        };
        let mpdu = data.mpdu;
        if data.frame.mpdu.length < 24 || mpdu[10..16] != self.config.bssid {
            return ConnectedRxDispatch::Ignored;
        }
        let frame_control = u16::from_le_bytes([mpdu[0], mpdu[1]]);
        let sequence_control = u16::from_le_bytes([mpdu[22], mpdu[23]]);
        let tid = if frame_control & QOS_SUBTYPE != 0 && data.frame.mpdu.length >= 26 {
            Some(mpdu[24] & 0x0f)
        } else {
            None
        };
        if self
            .duplicate_filter
            .is_duplicate(frame_control & RETRY != 0, sequence_control, tid)
        {
            return ConnectedRxDispatch::Duplicate;
        }

        match plan_data_decapsulation(
            DataInterfaceRole::Station,
            &mpdu[..data.frame.mpdu.length],
            data.frame.payload_offset,
            data.frame.payload_length,
        ) {
            Ok(plan) => {
                let Some(payload_end) = plan.payload_offset.checked_add(plan.payload_length) else {
                    return rejected(
                        protection,
                        ConnectedRxError::Data(DataDecapError::Truncated),
                    );
                };
                let Some(payload) = mpdu.get(plan.payload_offset..payload_end) else {
                    return rejected(
                        protection,
                        ConnectedRxError::Data(DataDecapError::Truncated),
                    );
                };
                sink.publish(ConnectedRxEvent::Ethernet {
                    frame: EthernetFrameParts {
                        destination: plan.destination,
                        source: plan.source,
                        ether_type: plan.ether_type,
                        payload,
                    },
                    raw,
                    amsdu: false,
                });
                ConnectedRxDispatch::Data {
                    ethernet_frames: 1,
                    amsdu: false,
                }
            }
            Err(DataDecapError::AmsduUnsupported) => {
                let subframes = match amsdu_subframes(
                    DataInterfaceRole::Station,
                    &mpdu[..data.frame.mpdu.length],
                    data.frame.payload_offset,
                    data.frame.payload_length,
                ) {
                    Ok(subframes) => subframes,
                    Err(error) => return rejected(protection, ConnectedRxError::Data(error)),
                };
                let mut count = 0_u8;
                for subframe in subframes {
                    let subframe = match subframe {
                        Ok(subframe) => subframe,
                        Err(error) => return rejected(protection, ConnectedRxError::Data(error)),
                    };
                    sink.publish(ConnectedRxEvent::Ethernet {
                        frame: EthernetFrameParts {
                            destination: subframe.destination,
                            source: subframe.source,
                            ether_type: subframe.ether_type,
                            payload: subframe.payload,
                        },
                        raw,
                        amsdu: true,
                    });
                    count = count.saturating_add(1);
                }
                ConnectedRxDispatch::Data {
                    ethernet_frames: count,
                    amsdu: true,
                }
            }
            Err(error) => rejected(protection, ConnectedRxError::Data(error)),
        }
    }
}

impl Default for ConnectedRxDispatcher {
    fn default() -> Self {
        Self::new(ConnectedRxConfig {
            station_address: [0; 6],
            bssid: [0; 6],
            association_id: 0,
            ingress: RxIngressConfig {
                ring_entry_limit: 1,
                csi_config: 0,
                flags: 0,
            },
        })
    }
}

fn rejected(protection: ConnectedRxProtection, error: ConnectedRxError) -> ConnectedRxDispatch {
    ConnectedRxDispatch::Rejected { protection, error }
}

fn public_frame_control(raw: &[u8]) -> Option<u16> {
    Some(u16::from_le_bytes([
        *raw.get(PUBLIC_HEADER_SIZE)?,
        *raw.get(PUBLIC_HEADER_SIZE + 1)?,
    ]))
}

fn public_protection(
    raw: &[u8],
    frame_control: u16,
    station_address: [u8; 6],
) -> Option<ConnectedRxProtection> {
    if frame_control & (DATA_TYPE_MASK | PROTECTED) != DATA_TYPE | PROTECTED {
        return Some(ConnectedRxProtection::Other);
    }
    let destination = raw.get(PUBLIC_HEADER_SIZE + 4..PUBLIC_HEADER_SIZE + 10)?;
    if destination.first().is_some_and(|byte| byte & 1 != 0) {
        Some(ConnectedRxProtection::Group)
    } else if destination == station_address {
        Some(ConnectedRxProtection::Pairwise)
    } else {
        Some(ConnectedRxProtection::Other)
    }
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use crate::descriptor::{BIT_30, BIT_31, LENGTH_SHIFT};

    use super::*;

    const STATION: [u8; 6] = [0x10, 0x11, 0x12, 0x13, 0x14, 0x15];
    const BSSID: [u8; 6] = [0x20, 0x21, 0x22, 0x23, 0x24, 0x25];
    const SOURCE: [u8; 6] = [0x30, 0x31, 0x32, 0x33, 0x34, 0x35];
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = 0x40;

    #[derive(Default)]
    struct RecordingSink {
        beacons: Vec<StaBeaconObservation>,
        protected: Vec<ConnectedRxProtection>,
        ethernet: Vec<Vec<u8>>,
        block_ack: Vec<BlockAckAction>,
    }

    impl ConnectedRxSink for RecordingSink {
        fn publish(&mut self, event: ConnectedRxEvent<'_>) {
            match event {
                ConnectedRxEvent::Beacon(observation) => self.beacons.push(observation),
                ConnectedRxEvent::ProtectedFrame(protection) => self.protected.push(protection),
                ConnectedRxEvent::Ethernet { frame, .. } => {
                    let mut bytes = std::vec![0; frame.length()];
                    frame.copy_to(&mut bytes).unwrap();
                    self.ethernet.push(bytes);
                }
                ConnectedRxEvent::BlockAck { action, .. } => self.block_ack.push(action),
                ConnectedRxEvent::Trigger { .. } | ConnectedRxEvent::Ndpa { .. } => {}
            }
        }
    }

    #[test]
    fn routes_associated_beacon_and_local_tim_as_owned_control_state() {
        const FIXED: usize = 36;
        const TIM: [u8; 6] = [5, 4, 0, 3, 1, 0x80];
        const MPDU: usize = FIXED + TIM.len();
        const SIGNAL: usize = MPDU + 4;
        let mut storage = [0_u8; 192];
        set_tail(&mut storage, SIGNAL);
        let frame = &mut storage[FRAME_OFFSET..FRAME_OFFSET + MPDU];
        frame[..2].copy_from_slice(&BEACON_FRAME_CONTROL.to_le_bytes());
        frame[4..10].fill(0xff);
        frame[10..16].copy_from_slice(&BSSID);
        frame[16..22].copy_from_slice(&BSSID);
        frame[24..32].copy_from_slice(&123_u64.to_le_bytes());
        frame[32..34].copy_from_slice(&100_u16.to_le_bytes());
        frame[34..36].copy_from_slice(&0x0431_u16.to_le_bytes());
        frame[FIXED..].copy_from_slice(&TIM);

        let mut dispatcher = ConnectedRxDispatcher::new(config());
        let mut sink = RecordingSink::default();
        let mut mpdu = [0_u8; 128];
        let mut ethernet = [0_u8; 128];
        assert_eq!(
            dispatcher.dispatch(
                segment(&storage, SIGNAL),
                &mut mpdu,
                &mut ethernet,
                &mut sink,
            ),
            ConnectedRxDispatch::Beacon
        );
        assert_eq!(sink.beacons.len(), 1);
        assert_eq!(sink.beacons[0].timestamp_tsf, 123);
        assert_eq!(sink.beacons[0].interval_tu, 100);
        assert!(sink.beacons[0].tim.unwrap().unicast_buffered);
        assert!(sink.beacons[0].tim.unwrap().group_buffered);
    }

    fn config() -> ConnectedRxConfig {
        ConnectedRxConfig {
            station_address: STATION,
            bssid: BSSID,
            association_id: 7,
            ingress: RxIngressConfig {
                ring_entry_limit: 1,
                csi_config: 0,
                flags: 0,
            },
        }
    }

    fn segment(storage: &[u8; 192], signal_length: usize) -> RxSegment<'_> {
        let received = FRAME_OFFSET + signal_length;
        RxSegment {
            descriptor_address: 0x2f00_4000,
            descriptor_word0: 192 | ((received as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
            buffer: storage,
            next_descriptor_address: 0,
        }
    }

    fn set_tail(storage: &mut [u8; 192], signal_length: usize) {
        storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
            &(((signal_length + 4) as u32) << 16 | signal_length as u32).to_le_bytes(),
        );
    }

    #[test]
    fn dispatches_protected_ethernet_and_owns_duplicate_history() {
        const HEADER: usize = 24;
        const PAYLOAD: [u8; 4] = [1, 2, 3, 4];
        const MPDU: usize = HEADER + 8 + 8 + PAYLOAD.len() + 8;
        const SIGNAL: usize = MPDU + 4;
        let mut storage = [0_u8; 192];
        set_tail(&mut storage, SIGNAL);
        let frame = &mut storage[FRAME_OFFSET..FRAME_OFFSET + MPDU];
        frame[0] = 0x08;
        frame[1] = 0x42;
        frame[4..10].copy_from_slice(&STATION);
        frame[10..16].copy_from_slice(&BSSID);
        frame[16..22].copy_from_slice(&SOURCE);
        frame[22..24].copy_from_slice(&0x1230_u16.to_le_bytes());
        frame[HEADER..HEADER + 8].copy_from_slice(&[3, 0, 0, 0x20, 0, 0, 0, 0]);
        frame[HEADER + 8..HEADER + 16].copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]);
        frame[HEADER + 16..HEADER + 20].copy_from_slice(&PAYLOAD);

        let mut dispatcher = ConnectedRxDispatcher::new(config());
        let mut sink = RecordingSink::default();
        let mut mpdu = [0_u8; 128];
        let mut ethernet = [0_u8; 128];
        assert_eq!(
            dispatcher.dispatch(
                segment(&storage, SIGNAL),
                &mut mpdu,
                &mut ethernet,
                &mut sink,
            ),
            ConnectedRxDispatch::Data {
                ethernet_frames: 1,
                amsdu: false,
            }
        );
        assert_eq!(sink.protected, [ConnectedRxProtection::Pairwise]);
        assert_eq!(sink.ethernet.len(), 1);
        assert_eq!(&sink.ethernet[0][..6], &STATION);
        assert_eq!(&sink.ethernet[0][6..12], &SOURCE);
        assert_eq!(&sink.ethernet[0][12..14], &0x0800_u16.to_be_bytes());
        assert_eq!(&sink.ethernet[0][14..], &PAYLOAD);

        storage[FRAME_OFFSET + 1] |= 0x08;
        assert_eq!(
            dispatcher.dispatch(
                segment(&storage, SIGNAL),
                &mut mpdu,
                &mut ethernet,
                &mut sink,
            ),
            ConnectedRxDispatch::Duplicate
        );
    }

    #[test]
    fn routes_an_addressed_block_ack_action_without_platform_effects() {
        const BODY: [u8; 9] = [3, 0, 9, 0x02, 0x08, 0, 0, 0x30, 0x12];
        const MPDU: usize = 24 + BODY.len();
        const SIGNAL: usize = MPDU + 4;
        let mut storage = [0_u8; 192];
        set_tail(&mut storage, SIGNAL);
        let frame = &mut storage[FRAME_OFFSET..FRAME_OFFSET + MPDU];
        frame[0] = 0xd0;
        frame[4..10].copy_from_slice(&STATION);
        frame[10..16].copy_from_slice(&BSSID);
        frame[16..22].copy_from_slice(&BSSID);
        frame[24..].copy_from_slice(&BODY);

        let mut dispatcher = ConnectedRxDispatcher::new(config());
        let mut sink = RecordingSink::default();
        let mut mpdu = [0_u8; 128];
        let mut ethernet = [0_u8; 128];
        assert_eq!(
            dispatcher.dispatch(
                segment(&storage, SIGNAL),
                &mut mpdu,
                &mut ethernet,
                &mut sink,
            ),
            ConnectedRxDispatch::BlockAck
        );
        assert_eq!(sink.block_ack.len(), 1);
        assert!(matches!(
            sink.block_ack[0],
            BlockAckAction::AddbaRequest {
                dialog_token: 9,
                tid: 0,
                immediate: true,
                window: 32,
                starting_sequence: 0x123,
                ..
            }
        ));
    }
}
