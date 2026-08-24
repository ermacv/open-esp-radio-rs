//! Connected-station receive dispatch without an executor or platform policy.
//!
//! This module owns the frame-classification and protocol-routing state that
//! used to be duplicated by the ordinary and TX-interleaved HIL receive loops.
//! It deliberately does not log, access unrelated peripherals, enqueue into a
//! network stack or await an executor primitive. Those effects are published
//! through [`ConnectedRxSink`] and belong to the integration runner.

use open_esp_radio_esp32s31_wifi::protected_data_rx::view_protected_data;
use open_esp_radio_esp32s31_wifi_dma::rx_ring::RxSegment;
use open_esp_radio_ieee80211::{
    data::{DataDecapError, DataInterfaceRole, EthernetFrameParts, RxDuplicateFilter},
    esp_now::{ESP_NOW_ACTION_CATEGORY, ESP_NOW_ORGANIZATION_IDENTIFIER},
    ndpa::{HeNdpa, HeNdpaError},
    station::{StaDisconnect, parse_sta_disconnect},
    station_beacon::{StaBeaconError, StaBeaconObservation, parse_sta_beacon},
    trigger::{TriggerCommonInfo, TriggerParseError, parse_trigger_frame},
};
use open_esp_radio_wifi_softmac::{
    EspNowPeerId, EspNowReceiveError, EspNowReceivedV1, EspNowRxEpoch, EspNowRxOutcome,
    MacRxMetadata,
};
#[cfg(test)]
use open_esp_radio_wifi_softmac::{MacRxCryptoStatus, MacRxEvidence};

use open_esp_radio_esp32s31_wifi_mac::{
    rx::{
        PUBLIC_HEADER_SIZE, RxError, RxIngressConfig, RxPhyInfo, decode_normalized_rx_metadata,
        extract_control, extract_management,
    },
    rx_ampdu::{RxBlockAckMpduKey, rx_block_ack_mpdu_key},
    tx::{HeTriggerScheduledRate, HeTriggerScheduledRateError},
    tx_ampdu::{BlockAckAction, parse_block_ack_action},
};

const TRIGGER_FRAME_CONTROL: u16 = 0x0024;
const NDPA_FRAME_CONTROL: u16 = 0x0054;
const ACTION_FRAME_CONTROL: u16 = 0x00d0;
const PROBE_RESPONSE_FRAME_CONTROL: u16 = 0x0050;
const BEACON_FRAME_CONTROL: u16 = 0x0080;
const DISASSOCIATION_FRAME_CONTROL: u16 = 0x00a0;
const DEAUTHENTICATION_FRAME_CONTROL: u16 = 0x00c0;
const DATA_TYPE_MASK: u16 = 0x000c;
const DATA_TYPE: u16 = 0x0008;
const PROTECTED: u16 = 0x4000;
const QOS_SUBTYPE: u16 = 0x0080;
const TO_FROM_DS: u16 = 0x0300;
const QOS_AMSDU_PRESENT: u8 = 0x80;

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

/// MAC identity admitted for an associated HE control exchange.
///
/// Construction remains private to [`ConnectedRxDispatcher`]: a published
/// value proves that the transmitter was the associated BSSID and that the
/// receiver was either this station or an IEEE group address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssociatedHeControlIdentity {
    duration: u16,
    receiver_address: [u8; 6],
    transmitter_address: [u8; 6],
}

impl AssociatedHeControlIdentity {
    pub const fn duration(self) -> u16 {
        self.duration
    }

    pub const fn receiver_address(self) -> [u8; 6] {
        self.receiver_address
    }

    pub const fn transmitter_address(self) -> [u8; 6] {
        self.transmitter_address
    }
}

/// One semantic event emitted by the connected frame dispatcher.
///
/// Borrowed frame slices remain valid only for the duration of
/// [`ConnectedRxSink::publish`]. A network adapter must copy or otherwise
/// transfer them into storage that it owns before returning.
#[derive(Clone, Copy, Debug)]
pub enum ConnectedRxEvent<'frame> {
    Beacon {
        observation: StaBeaconObservation,
        metadata: MacRxMetadata<RxPhyInfo>,
    },
    ProbeResponse,
    Trigger {
        identity: AssociatedHeControlIdentity,
        common: TriggerCommonInfo,
        schedule: Result<HeTriggerScheduledRate, HeTriggerScheduledRateError>,
        first_user: Option<[u8; 5]>,
        runtime_received_at_micros: Option<u64>,
    },
    Ndpa {
        identity: AssociatedHeControlIdentity,
        dialog_token: u8,
        addressed_to_station: bool,
        runtime_received_at_micros: Option<u64>,
    },
    BlockAck {
        action: BlockAckAction,
        body: &'frame [u8],
    },
    /// Strictly decoded plaintext ESP-NOW datagram from one configured peer.
    ///
    /// `received` borrows the normalized MPDU and must be copied by a sink
    /// which retains it after [`ConnectedRxSink::publish`] returns.
    EspNow {
        received: EspNowReceivedV1<'frame>,
        metadata: MacRxMetadata<RxPhyInfo>,
    },
    PeerDisconnect(StaDisconnect),
    Ethernet {
        frame: EthernetFrameParts<'frame>,
        raw: &'frame [u8],
        amsdu: bool,
        metadata: MacRxMetadata<RxPhyInfo>,
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
    ProbeResponse,
    Trigger {
        identity: AssociatedHeControlIdentity,
        common: TriggerCommonInfo,
        schedule: Result<HeTriggerScheduledRate, HeTriggerScheduledRateError>,
        first_user: Option<[u8; 5]>,
        runtime_received_at_micros: Option<u64>,
    },
    Ndpa {
        identity: AssociatedHeControlIdentity,
        dialog_token: u8,
        addressed_to_station: bool,
        runtime_received_at_micros: Option<u64>,
    },
    BlockAck(BlockAckAction),
    PeerDisconnect(StaDisconnect),
}

impl ConnectedRxEvent<'_> {
    /// Copy only the protocol/control information that is independent of the
    /// staged RX allocation.
    pub const fn control(self) -> Option<ConnectedRxControlEvent> {
        match self {
            Self::Beacon { observation, .. } => Some(ConnectedRxControlEvent::Beacon(observation)),
            Self::ProbeResponse => Some(ConnectedRxControlEvent::ProbeResponse),
            Self::Trigger {
                identity,
                common,
                schedule,
                first_user,
                runtime_received_at_micros,
            } => Some(ConnectedRxControlEvent::Trigger {
                identity,
                common,
                schedule,
                first_user,
                runtime_received_at_micros,
            }),
            Self::Ndpa {
                identity,
                dialog_token,
                addressed_to_station,
                runtime_received_at_micros,
            } => Some(ConnectedRxControlEvent::Ndpa {
                identity,
                dialog_token,
                addressed_to_station,
                runtime_received_at_micros,
            }),
            Self::BlockAck { action, .. } => Some(ConnectedRxControlEvent::BlockAck(action)),
            Self::EspNow { .. } => None,
            Self::PeerDisconnect(disconnect) => {
                Some(ConnectedRxControlEvent::PeerDisconnect(disconnect))
            }
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
    EspNow(EspNowReceiveError),
    Data(DataDecapError),
}

/// Result of consuming one independently owned staged RX frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedRxDispatch {
    Beacon,
    ProbeResponse,
    Trigger,
    Ndpa,
    BlockAck,
    EspNow {
        peer: EspNowPeerId,
    },
    EspNowDuplicate {
        peer: EspNowPeerId,
    },
    PeerDisconnect,
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
    duplicate_filter: RxDuplicateFilter,
    esp_now: Option<EspNowRxEpoch>,
}

impl ConnectedRxDispatcher {
    pub const fn new(config: ConnectedRxConfig) -> Self {
        Self {
            config,
            duplicate_filter: RxDuplicateFilter::new(),
            esp_now: None,
        }
    }

    /// Attach one already station/channel-qualified ESP-NOW receive epoch.
    ///
    /// Hardware receive-policy ownership remains at the connected composition
    /// boundary. This method only installs portable peer and duplicate state.
    pub fn with_esp_now_rx_epoch(mut self, epoch: EspNowRxEpoch) -> Self {
        assert_eq!(
            epoch.config().station().interface.address,
            self.config.station_address,
            "ESP-NOW RX epoch must belong to the connected station"
        );
        self.esp_now = Some(epoch);
        self
    }

    pub const fn esp_now_rx_epoch(&self) -> Option<&EspNowRxEpoch> {
        self.esp_now.as_ref()
    }

    /// Revoke receive authority and clear all duplicate fingerprints before
    /// this connected owner is returned across a stop/restart boundary.
    pub fn stop_esp_now_rx_epoch(&mut self) -> usize {
        let Some(mut epoch) = self.esp_now.take() else {
            return 0;
        };
        epoch.reset_duplicate_history()
    }

    pub const fn config(&self) -> ConnectedRxConfig {
        self.config
    }

    /// Return whether this staged unit can publish an Ethernet frame.
    ///
    /// This deliberately performs only immutable public-header
    /// classification. It does not advance duplicate history or authenticate
    /// payload bytes, so an async integration may acquire network capacity
    /// before the finite dispatch without changing protocol state.
    pub fn may_publish_ethernet(&self, segment: RxSegment<'_>) -> bool {
        public_frame_control(segment.buffer).is_some_and(|frame_control| {
            frame_control & (DATA_TYPE_MASK | PROTECTED) == DATA_TYPE | PROTECTED
        })
    }

    /// Return whether a protected QoS data unit advertises A-MSDU payload.
    ///
    /// This immutable public-header check lets an async adapter select its
    /// deferred multi-frame publication path without parsing/decrypting the
    /// MPDU twice or mutating duplicate history. Malformed input may select
    /// the conservative path and then be rejected by [`Self::dispatch`].
    pub fn may_publish_amsdu(&self, segment: RxSegment<'_>) -> bool {
        let raw = segment.buffer;
        let Some(frame_control) = public_frame_control(raw) else {
            return false;
        };
        if frame_control & (DATA_TYPE_MASK | PROTECTED | QOS_SUBTYPE)
            != DATA_TYPE | PROTECTED | QOS_SUBTYPE
        {
            return false;
        }
        // Four-address QoS moves the control field by one address slot.
        let qos_control_offset =
            PUBLIC_HEADER_SIZE + 24 + usize::from(frame_control & TO_FROM_DS == TO_FROM_DS) * 6;
        raw.get(qos_control_offset)
            .is_some_and(|control| control & QOS_AMSDU_PRESENT != 0)
    }

    /// Classify a frame that belongs to a receive BlockAck sequence space.
    ///
    /// Group, foreign, unprotected, non-QoS and fragmented frames remain on
    /// the direct dispatch path. Agreement state still decides whether the
    /// returned TID is currently reordered.
    pub fn reorder_key(&self, segment: RxSegment<'_>) -> Option<RxBlockAckMpduKey> {
        rx_block_ack_mpdu_key(
            segment.buffer,
            self.config.station_address,
            Some(self.config.bssid),
        )
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
        self.dispatch_with_runtime_received_at(segment, mpdu, _ethernet, None, sink)
    }

    /// Dispatch with the executor-clock sample attached by the physical RX
    /// producer at its first completed-frame handoff.
    ///
    /// The timestamp is optional because executor-neutral and synthetic users
    /// cannot manufacture it. Runtime Trigger/NDPA response policy rejects a
    /// missing sample rather than starting a fresh window at mailbox dequeue.
    pub fn dispatch_with_runtime_received_at<S: ConnectedRxSink>(
        &mut self,
        segment: RxSegment<'_>,
        mpdu: &mut [u8],
        _ethernet: &mut [u8],
        runtime_received_at_micros: Option<u64>,
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
                let Some(metadata) = decode_normalized_rx_metadata(raw) else {
                    return rejected(protection, ConnectedRxError::Rx(RxError::Metadata));
                };
                sink.publish(ConnectedRxEvent::Beacon {
                    observation,
                    metadata,
                });
                ConnectedRxDispatch::Beacon
            }
            PROBE_RESPONSE_FRAME_CONTROL => {
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
                sink.publish(ConnectedRxEvent::ProbeResponse);
                ConnectedRxDispatch::ProbeResponse
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
                let Some(identity) = self.associated_he_control_identity(
                    trigger.duration,
                    trigger.receiver_address,
                    trigger.transmitter_address,
                ) else {
                    return ConnectedRxDispatch::Ignored;
                };
                let first_user = trigger.user_info_and_padding.get(..5).map(|bytes| {
                    let mut first_user = [0_u8; 5];
                    first_user.copy_from_slice(bytes);
                    first_user
                });
                sink.publish(ConnectedRxEvent::Trigger {
                    identity,
                    common: trigger.common,
                    schedule: HeTriggerScheduledRate::from_trigger_frame(
                        &trigger,
                        self.config.association_id,
                    ),
                    first_user,
                    runtime_received_at_micros,
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
                let mut receiver_address = [0_u8; 6];
                receiver_address.copy_from_slice(ndpa.receiver_address());
                let mut transmitter_address = [0_u8; 6];
                transmitter_address.copy_from_slice(ndpa.transmitter_address());
                let Some(identity) = self.associated_he_control_identity(
                    ndpa.duration(),
                    receiver_address,
                    transmitter_address,
                ) else {
                    return ConnectedRxDispatch::Ignored;
                };
                sink.publish(ConnectedRxEvent::Ndpa {
                    identity,
                    dialog_token: ndpa.dialog_token(),
                    addressed_to_station: ndpa.contains_association_id(self.config.association_id),
                    runtime_received_at_micros,
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
                if management.length < 24 {
                    return ConnectedRxDispatch::Ignored;
                }
                let body = &mpdu[24..management.length];
                let is_associated_peer_action = mpdu[4..10] == self.config.station_address
                    && mpdu[10..16] == self.config.bssid
                    && mpdu[16..22] == self.config.bssid;
                if is_associated_peer_action && let Some(action) = parse_block_ack_action(body) {
                    sink.publish(ConnectedRxEvent::BlockAck { action, body });
                    return ConnectedRxDispatch::BlockAck;
                }

                // Do not turn every vendor Action frame into an ESP-NOW
                // rejection. Once the category/OUI identify ESP-NOW, however,
                // the strict codec owns all remaining bounds, address, BSSID,
                // type and version failures.
                if self.esp_now.is_none() || !is_esp_now_action_candidate(body) {
                    return ConnectedRxDispatch::Ignored;
                }
                let outcome = match self
                    .esp_now
                    .as_mut()
                    .expect("candidate admission checked ESP-NOW epoch presence")
                    .receive_v1(&mpdu[..management.length])
                {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        return rejected(protection, ConnectedRxError::EspNow(error));
                    }
                };
                match outcome {
                    EspNowRxOutcome::Received(received) => {
                        let metadata = decode_normalized_rx_metadata(raw)
                            .unwrap_or_else(MacRxMetadata::unavailable);
                        let peer = received.peer();
                        sink.publish(ConnectedRxEvent::EspNow { received, metadata });
                        ConnectedRxDispatch::EspNow { peer }
                    }
                    EspNowRxOutcome::Duplicate { peer } => {
                        ConnectedRxDispatch::EspNowDuplicate { peer }
                    }
                }
            }
            DISASSOCIATION_FRAME_CONTROL | DEAUTHENTICATION_FRAME_CONTROL => {
                let management = match extract_management(
                    core::slice::from_ref(&segment),
                    self.config.ingress,
                    mpdu,
                ) {
                    Ok(management) => management,
                    Err(error) => return rejected(protection, ConnectedRxError::Rx(error)),
                };
                let Some(disconnect) = parse_sta_disconnect(
                    &mpdu[..management.length],
                    self.config.station_address,
                    self.config.bssid,
                ) else {
                    return ConnectedRxDispatch::Ignored;
                };
                sink.publish(ConnectedRxEvent::PeerDisconnect(disconnect));
                ConnectedRxDispatch::PeerDisconnect
            }
            _ => self.dispatch_data(segment, frame_control, protection, sink),
        }
    }

    fn associated_he_control_identity(
        &self,
        duration: u16,
        receiver_address: [u8; 6],
        transmitter_address: [u8; 6],
    ) -> Option<AssociatedHeControlIdentity> {
        if transmitter_address != self.config.bssid
            || (receiver_address != self.config.station_address && receiver_address[0] & 1 == 0)
        {
            return None;
        }
        Some(AssociatedHeControlIdentity {
            duration,
            receiver_address,
            transmitter_address,
        })
    }

    fn dispatch_data<S: ConnectedRxSink>(
        &mut self,
        segment: RxSegment<'_>,
        public_frame_control: u16,
        protection: ConnectedRxProtection,
        sink: &mut S,
    ) -> ConnectedRxDispatch {
        if public_frame_control & (DATA_TYPE_MASK | PROTECTED) != DATA_TYPE | PROTECTED {
            return ConnectedRxDispatch::Ignored;
        }
        let data = match view_protected_data(segment, self.config.ingress) {
            Ok(data) => data,
            Err(error) => return rejected(protection, ConnectedRxError::Rx(error)),
        };
        let mpdu = data.mpdu;
        if mpdu[10..16] != self.config.bssid {
            return ConnectedRxDispatch::Ignored;
        }
        if self
            .duplicate_filter
            .is_duplicate(data.retry, data.sequence_control, data.tid)
        {
            return ConnectedRxDispatch::Duplicate;
        }

        let data = match data.decapsulate(DataInterfaceRole::Station) {
            Ok(data) => data,
            Err(error) => return rejected(protection, ConnectedRxError::Data(error)),
        };
        let mut frames = data.frames;
        let amsdu = data.amsdu;
        let mut count = 0_u8;
        for frame in &mut frames {
            let frame = match frame {
                Ok(frame) => frame,
                Err(error) => return rejected(protection, ConnectedRxError::Data(error)),
            };
            sink.publish(ConnectedRxEvent::Ethernet {
                frame,
                raw: data.raw,
                amsdu,
                metadata: data.metadata,
            });
            count = count.saturating_add(1);
        }
        ConnectedRxDispatch::Data {
            ethernet_frames: count,
            amsdu,
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

fn is_esp_now_action_candidate(body: &[u8]) -> bool {
    if body.first() != Some(&ESP_NOW_ACTION_CATEGORY) {
        return false;
    }
    body.len() < 4 || body[1..4] == ESP_NOW_ORGANIZATION_IDENTIFIER
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

    use open_esp_radio_esp32s31_wifi_dma::descriptor::{BIT_30, BIT_31, LENGTH_SHIFT};

    use super::*;

    const STATION: [u8; 6] = [0x10, 0x11, 0x12, 0x13, 0x14, 0x15];
    const BSSID: [u8; 6] = [0x20, 0x21, 0x22, 0x23, 0x24, 0x25];
    const SOURCE: [u8; 6] = [0x30, 0x31, 0x32, 0x33, 0x34, 0x35];
    const TAIL_OFFSET: usize = 0x38;
    const FRAME_OFFSET: usize = 0x40;

    #[derive(Default)]
    struct RecordingSink {
        beacons: Vec<StaBeaconObservation>,
        beacon_metadata: Vec<MacRxMetadata<RxPhyInfo>>,
        probe_responses: u32,
        ethernet: Vec<Vec<u8>>,
        ethernet_metadata: Vec<MacRxMetadata<RxPhyInfo>>,
        block_ack: Vec<BlockAckAction>,
        peer_disconnects: Vec<StaDisconnect>,
    }

    impl ConnectedRxSink for RecordingSink {
        fn publish(&mut self, event: ConnectedRxEvent<'_>) {
            match event {
                ConnectedRxEvent::Beacon {
                    observation,
                    metadata,
                } => {
                    self.beacons.push(observation);
                    self.beacon_metadata.push(metadata);
                }
                ConnectedRxEvent::ProbeResponse => {
                    self.probe_responses = self.probe_responses.saturating_add(1);
                }
                ConnectedRxEvent::Ethernet {
                    frame, metadata, ..
                } => {
                    let mut bytes = std::vec![0; frame.length()];
                    frame.copy_to(&mut bytes).unwrap();
                    self.ethernet.push(bytes);
                    self.ethernet_metadata.push(metadata);
                }
                ConnectedRxEvent::BlockAck { action, .. } => self.block_ack.push(action),
                ConnectedRxEvent::PeerDisconnect(disconnect) => {
                    self.peer_disconnects.push(disconnect);
                }
                ConnectedRxEvent::Trigger { .. }
                | ConnectedRxEvent::Ndpa { .. }
                | ConnectedRxEvent::EspNow { .. } => {}
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
        assert!(!dispatcher.may_publish_amsdu(segment(&storage, SIGNAL)));
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
        assert_eq!(
            sink.beacon_metadata[0].s_mpdu,
            MacRxEvidence::HardwareObserved(true)
        );
        assert_eq!(
            sink.beacon_metadata[0].ampdu,
            MacRxEvidence::ProtocolValidated(false)
        );
    }

    #[test]
    fn routes_only_probe_responses_from_the_associated_bssid() {
        const MPDU: usize = 36;
        const SIGNAL: usize = MPDU + 4;
        let mut storage = [0_u8; 192];
        set_tail(&mut storage, SIGNAL);
        let frame = &mut storage[FRAME_OFFSET..FRAME_OFFSET + MPDU];
        frame[..2].copy_from_slice(&PROBE_RESPONSE_FRAME_CONTROL.to_le_bytes());
        frame[4..10].copy_from_slice(&STATION);
        frame[10..16].copy_from_slice(&BSSID);
        frame[16..22].copy_from_slice(&BSSID);

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
            ConnectedRxDispatch::ProbeResponse
        );
        assert_eq!(sink.probe_responses, 1);

        storage[FRAME_OFFSET + 10..FRAME_OFFSET + 16].copy_from_slice(&SOURCE);
        assert_eq!(
            dispatcher.dispatch(
                segment(&storage, SIGNAL),
                &mut mpdu,
                &mut ethernet,
                &mut sink,
            ),
            ConnectedRxDispatch::Ignored
        );
        assert_eq!(sink.probe_responses, 1);
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
        // Synthetic connected frames are standalone MPDUs unless a test
        // explicitly overrides the hardware `cur_single_mpdu` bit.
        storage[0x1f] = 1;
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
        assert!(!dispatcher.may_publish_amsdu(segment(&storage, SIGNAL)));
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
        assert_eq!(sink.ethernet.len(), 1);
        assert_eq!(&sink.ethernet[0][..6], &STATION);
        assert_eq!(&sink.ethernet[0][6..12], &SOURCE);
        assert_eq!(&sink.ethernet[0][12..14], &0x0800_u16.to_be_bytes());
        assert_eq!(&sink.ethernet[0][14..], &PAYLOAD);
        assert_eq!(
            sink.ethernet_metadata[0].crypto,
            MacRxEvidence::HardwareObserved(MacRxCryptoStatus::DecryptedAndIntegrityVerified)
        );
        assert_eq!(
            sink.ethernet_metadata[0].amsdu,
            MacRxEvidence::ProtocolValidated(false)
        );
        assert_eq!(
            sink.ethernet_metadata[0].s_mpdu,
            MacRxEvidence::HardwareObserved(true)
        );
        assert_eq!(
            sink.ethernet_metadata[0].ampdu,
            MacRxEvidence::ProtocolValidated(false)
        );

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
    fn preflight_detects_amsdu_without_mutating_dispatch_state() {
        const HEADER: usize = 26;
        const FIRST_SUBFRAME: usize = 24;
        const SECOND_SUBFRAME: usize = 25;
        const MPDU: usize = HEADER + 8 + FIRST_SUBFRAME + SECOND_SUBFRAME + 8;
        const SIGNAL: usize = MPDU + 4;
        let mut storage = [0_u8; 192];
        set_tail(&mut storage, SIGNAL);
        let frame = &mut storage[FRAME_OFFSET..FRAME_OFFSET + MPDU];
        frame[0] = 0x88;
        frame[1] = 0x42;
        frame[4..10].copy_from_slice(&STATION);
        frame[10..16].copy_from_slice(&BSSID);
        frame[16..22].copy_from_slice(&SOURCE);
        frame[22..24].copy_from_slice(&0x1230_u16.to_le_bytes());
        frame[24] = 0x80;
        frame[HEADER..HEADER + 8].copy_from_slice(&[3, 0, 0, 0x20, 0, 0, 0, 0]);
        let mut offset = HEADER + 8;
        frame[offset..offset + 6].copy_from_slice(&STATION);
        frame[offset + 6..offset + 12].copy_from_slice(&SOURCE);
        frame[offset + 12..offset + 14].copy_from_slice(&10_u16.to_be_bytes());
        frame[offset + 14..offset + 22].copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]);
        frame[offset + 22..offset + 24].copy_from_slice(&[1, 2]);
        offset += FIRST_SUBFRAME;
        frame[offset..offset + 6].copy_from_slice(&[0xff; 6]);
        frame[offset + 6..offset + 12].copy_from_slice(&SOURCE);
        frame[offset + 12..offset + 14].copy_from_slice(&11_u16.to_be_bytes());
        frame[offset + 14..offset + 22].copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x06]);
        frame[offset + 22..offset + 25].copy_from_slice(&[3, 4, 5]);

        let mut dispatcher = ConnectedRxDispatcher::new(config());
        let segment = segment(&storage, SIGNAL);
        assert_eq!(
            dispatcher.reorder_key(segment),
            Some(RxBlockAckMpduKey {
                peer: BSSID,
                tid: 0,
                sequence: 0x123,
                retry: false,
            })
        );
        assert!(dispatcher.may_publish_amsdu(segment));
        // Preflight is repeatable and does not claim duplicate history.
        assert!(dispatcher.may_publish_amsdu(segment));

        let mut sink = RecordingSink::default();
        let mut mpdu = [0_u8; 128];
        let mut ethernet = [0_u8; 128];
        assert_eq!(
            dispatcher.dispatch(segment, &mut mpdu, &mut ethernet, &mut sink),
            ConnectedRxDispatch::Data {
                ethernet_frames: 2,
                amsdu: true,
            }
        );
        assert_eq!(sink.ethernet.len(), 2);
        assert!(sink.ethernet_metadata.iter().all(|metadata| {
            metadata.amsdu == MacRxEvidence::ProtocolValidated(true)
                && metadata.s_mpdu == MacRxEvidence::HardwareObserved(true)
                && metadata.ampdu == MacRxEvidence::ProtocolValidated(false)
                && metadata.crypto
                    == MacRxEvidence::HardwareObserved(
                        MacRxCryptoStatus::DecryptedAndIntegrityVerified,
                    )
        }));
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

    #[test]
    fn routes_only_peer_disconnects_addressed_to_this_station() {
        const MPDU: usize = 26;
        const SIGNAL: usize = MPDU + 4;
        let mut storage = [0_u8; 192];
        set_tail(&mut storage, SIGNAL);
        let frame = &mut storage[FRAME_OFFSET..FRAME_OFFSET + MPDU];
        frame[..2].copy_from_slice(&DEAUTHENTICATION_FRAME_CONTROL.to_le_bytes());
        frame[4..10].copy_from_slice(&STATION);
        frame[10..16].copy_from_slice(&BSSID);
        frame[16..22].copy_from_slice(&BSSID);
        frame[24..26].copy_from_slice(&7_u16.to_le_bytes());

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
            ConnectedRxDispatch::PeerDisconnect
        );
        assert_eq!(
            sink.peer_disconnects,
            [StaDisconnect {
                kind: open_esp_radio_ieee80211::station::StaDisconnectKind::Deauthentication,
                reason_code: 7,
            }]
        );

        storage[FRAME_OFFSET + 4..FRAME_OFFSET + 10].copy_from_slice(&SOURCE);
        assert_eq!(
            dispatcher.dispatch(
                segment(&storage, SIGNAL),
                &mut mpdu,
                &mut ethernet,
                &mut sink,
            ),
            ConnectedRxDispatch::Ignored
        );
        assert_eq!(sink.peer_disconnects.len(), 1);
    }
}
