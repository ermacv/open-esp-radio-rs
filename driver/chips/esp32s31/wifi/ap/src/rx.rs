//! Protected AP data receive frontier.
//!
//! DMA storage remains borrowed only for this synchronous dispatch. A sink
//! must copy or transfer every Ethernet view before returning, after which the
//! runtime may recycle the descriptor.

use open_esp_radio_esp32s31_wifi_mac::rx::{
    RxError, RxIngressConfig, RxPhyInfo, RxSegment, decode_normalized_rx_metadata, view_ccmp_data,
};
use open_esp_radio_ieee80211::data::{
    DataDecapError, DataInterfaceRole, EthernetFrameParts, RxDuplicateFilter, amsdu_subframes,
    plan_data_decapsulation,
};
use open_esp_radio_wifi_softmac::{MacRxCryptoStatus, MacRxEvidence, MacRxMetadata};

const RETRY: u16 = 0x0800;
const QOS_SUBTYPE: u16 = 0x0080;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31ApRxConfig {
    pub access_point: [u8; 6],
    pub ingress: RxIngressConfig,
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ApRxDispatch {
    Data { ethernet_frames: u8, amsdu: bool },
    Duplicate,
    ForeignPeer,
    Unauthorized,
    Rejected(Esp32s31ApRxError),
}

/// Duplicate history for the one peer admitted by the initial AP service.
pub struct Esp32s31ApRxDispatcher {
    config: Esp32s31ApRxConfig,
    duplicates: RxDuplicateFilter,
}

impl Esp32s31ApRxDispatcher {
    pub const fn new(config: Esp32s31ApRxConfig) -> Self {
        Self {
            config,
            duplicates: RxDuplicateFilter::new(),
        }
    }

    pub fn dispatch_protected<S: Esp32s31ApRxSink>(
        &mut self,
        segment: RxSegment<'_>,
        authorized_peer: Option<[u8; 6]>,
        sink: &mut S,
    ) -> Esp32s31ApRxDispatch {
        let Some(peer) = authorized_peer else {
            return Esp32s31ApRxDispatch::Unauthorized;
        };
        let data = match view_ccmp_data(&segment, self.config.ingress) {
            Ok(data) => data,
            Err(error) => {
                return Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Radio(error));
            }
        };
        let mpdu = data.mpdu;
        if mpdu.len() < 24 || mpdu[4..10] != self.config.access_point || mpdu[10..16] != peer {
            return Esp32s31ApRxDispatch::ForeignPeer;
        }
        let frame_control = u16::from_le_bytes([mpdu[0], mpdu[1]]);
        let sequence_control = u16::from_le_bytes([mpdu[22], mpdu[23]]);
        let tid = if frame_control & QOS_SUBTYPE != 0 && mpdu.len() >= 26 {
            Some(mpdu[24] & 0x0f)
        } else {
            None
        };
        if self
            .duplicates
            .is_duplicate(frame_control & RETRY != 0, sequence_control, tid)
        {
            return Esp32s31ApRxDispatch::Duplicate;
        }
        let Some(mut metadata) = decode_normalized_rx_metadata(segment.buffer) else {
            return Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Radio(RxError::Metadata));
        };
        metadata.crypto =
            MacRxEvidence::HardwareObserved(MacRxCryptoStatus::DecryptedAndIntegrityVerified);

        match plan_data_decapsulation(
            DataInterfaceRole::AccessPoint,
            mpdu,
            data.frame.payload_offset,
            data.frame.payload_length,
        ) {
            Ok(plan) => {
                let Some(payload_end) = plan.payload_offset.checked_add(plan.payload_length) else {
                    return rejected_data(DataDecapError::Truncated);
                };
                let Some(payload) = mpdu.get(plan.payload_offset..payload_end) else {
                    return rejected_data(DataDecapError::Truncated);
                };
                metadata.amsdu = MacRxEvidence::ProtocolValidated(false);
                sink.publish(Esp32s31ApRxEvent {
                    frame: EthernetFrameParts {
                        destination: plan.destination,
                        source: plan.source,
                        ether_type: plan.ether_type,
                        payload,
                    },
                    raw: segment.buffer,
                    amsdu: false,
                    metadata,
                });
                Esp32s31ApRxDispatch::Data {
                    ethernet_frames: 1,
                    amsdu: false,
                }
            }
            Err(DataDecapError::AmsduUnsupported) => {
                let subframes = match amsdu_subframes(
                    DataInterfaceRole::AccessPoint,
                    mpdu,
                    data.frame.payload_offset,
                    data.frame.payload_length,
                ) {
                    Ok(subframes) => subframes,
                    Err(error) => return rejected_data(error),
                };
                metadata.amsdu = MacRxEvidence::ProtocolValidated(true);
                let mut count = 0_u8;
                for subframe in subframes {
                    let subframe = match subframe {
                        Ok(subframe) => subframe,
                        Err(error) => return rejected_data(error),
                    };
                    sink.publish(Esp32s31ApRxEvent {
                        frame: EthernetFrameParts {
                            destination: subframe.destination,
                            source: subframe.source,
                            ether_type: subframe.ether_type,
                            payload: subframe.payload,
                        },
                        raw: segment.buffer,
                        amsdu: true,
                        metadata,
                    });
                    count = count.saturating_add(1);
                }
                Esp32s31ApRxDispatch::Data {
                    ethernet_frames: count,
                    amsdu: true,
                }
            }
            Err(error) => rejected_data(error),
        }
    }
}

fn rejected_data(error: DataDecapError) -> Esp32s31ApRxDispatch {
    Esp32s31ApRxDispatch::Rejected(Esp32s31ApRxError::Data(error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_esp_radio_esp32s31_wifi_mac::rx::{PUBLIC_HEADER_SIZE, RxSegment};

    const AP: [u8; 6] = [2, 0, 0, 0, 0, 1];
    const PEER: [u8; 6] = [2, 0, 0, 0, 0, 2];
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
        let mut sink = Sink::default();
        assert_eq!(
            dispatcher.dispatch_protected(segment(&storage, descriptor_word0), None, &mut sink),
            Esp32s31ApRxDispatch::Unauthorized
        );
        assert_eq!(
            dispatcher.dispatch_protected(
                segment(&storage, descriptor_word0),
                Some(PEER),
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
                Some(PEER),
                &mut sink,
            ),
            Esp32s31ApRxDispatch::Duplicate
        );
    }
}
