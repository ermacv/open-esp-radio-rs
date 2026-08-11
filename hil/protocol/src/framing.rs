use core::fmt;

use crc::{CRC_32_ISCSI, Crc};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::EvidenceRecord;

pub const MAX_POSTCARD_BYTES: usize = 480;
const CHECKSUM_BYTES: usize = size_of::<u32>();
const MAX_RAW_FRAME_BYTES: usize = MAX_POSTCARD_BYTES + CHECKSUM_BYTES;
const MAX_COBS_FRAME_BYTES: usize = cobs::max_encoding_length(MAX_RAW_FRAME_BYTES);
pub const MAX_WIRE_FRAME_BYTES: usize = 2 + MAX_COBS_FRAME_BYTES + 1;

const CRC32C: Crc<u32> = Crc::<u32>::new(&CRC_32_ISCSI);

/// Computes the digest carried by [`crate::Finished`] for an ordered evidence
/// set.
///
/// The digest covers the canonical postcard representation, including the
/// slice length. Both the target and host use this helper so missing, reordered
/// or mismatched evidence cannot satisfy a `Finished` event. Session identity
/// is checked separately from the surrounding [`crate::Envelope`].
pub fn evidence_crc32c(evidence: &[EvidenceRecord]) -> Result<u32, EncodeError> {
    let mut encoded = [0_u8; MAX_POSTCARD_BYTES];
    let payload = postcard::to_slice(evidence, &mut encoded).map_err(|_| EncodeError::Serialize)?;
    Ok(CRC32C.checksum(payload))
}

/// Computes the transfer-integrity digest of one opaque startup artifact.
///
/// This checksum is not an identity or qualification hash. It only prevents
/// a receiver from accepting an incomplete or internally inconsistent UART
/// transfer.
pub fn startup_artifact_crc32c(bytes: &[u8]) -> u32 {
    CRC32C.checksum(bytes)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncodeError {
    Serialize,
}

impl fmt::Display for EncodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize => {
                formatter.write_str("HIL message exceeds the wire frame or cannot be serialized")
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeError {
    Cobs,
    TooShort,
    Checksum,
    Deserialize,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cobs => formatter.write_str("invalid COBS frame"),
            Self::TooShort => formatter.write_str("HIL frame does not contain a checksum"),
            Self::Checksum => formatter.write_str("HIL frame checksum mismatch"),
            Self::Deserialize => formatter.write_str("invalid HIL postcard payload"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DecodeCounters {
    pub frames: u32,
    pub cobs_errors: u32,
    pub too_short: u32,
    pub checksum_errors: u32,
    pub deserialize_errors: u32,
    pub overflows: u32,
}

pub struct FrameEncoder {
    raw: [u8; MAX_RAW_FRAME_BYTES],
    wire: [u8; MAX_WIRE_FRAME_BYTES],
}

impl Default for FrameEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameEncoder {
    pub const fn new() -> Self {
        Self {
            raw: [0; MAX_RAW_FRAME_BYTES],
            wire: [0; MAX_WIRE_FRAME_BYTES],
        }
    }

    /// Encodes one independently resynchronizable frame.
    ///
    /// A leading delimiter discards any preceding ROM or text output. The
    /// trailing delimiter terminates this frame for an incremental decoder.
    pub fn encode<T>(&mut self, message: &T) -> Result<&[u8], EncodeError>
    where
        T: Serialize + ?Sized,
    {
        let payload_length = postcard::to_slice(message, &mut self.raw[..MAX_POSTCARD_BYTES])
            .map_err(|_| EncodeError::Serialize)?
            .len();
        let checksum = CRC32C.checksum(&self.raw[..payload_length]);
        self.raw[payload_length..payload_length + CHECKSUM_BYTES]
            .copy_from_slice(&checksum.to_le_bytes());

        // Two leading delimiters recover even when arbitrary pre-protocol
        // binary output contained a zero and made the decoder enter a false
        // frame. In the normal case both simply keep it armed for a body.
        self.wire[0] = 0;
        self.wire[1] = 0;
        let encoded_length = cobs::encode(
            &self.raw[..payload_length + CHECKSUM_BYTES],
            &mut self.wire[2..2 + MAX_COBS_FRAME_BYTES],
        );
        self.wire[2 + encoded_length] = 0;
        Ok(&self.wire[..encoded_length + 3])
    }
}

impl Drop for FrameEncoder {
    fn drop(&mut self) {
        self.raw.zeroize();
        self.wire.zeroize();
    }
}

pub struct FrameDecoder {
    encoded: [u8; MAX_COBS_FRAME_BYTES],
    length: usize,
    inside_frame: bool,
    discard_until_delimiter: bool,
    counters: DecodeCounters,
}

impl Default for FrameDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameDecoder {
    pub const fn new() -> Self {
        Self {
            encoded: [0; MAX_COBS_FRAME_BYTES],
            length: 0,
            inside_frame: false,
            discard_until_delimiter: false,
            counters: DecodeCounters {
                frames: 0,
                cobs_errors: 0,
                too_short: 0,
                checksum_errors: 0,
                deserialize_errors: 0,
                overflows: 0,
            },
        }
    }

    pub const fn counters(&self) -> DecodeCounters {
        self.counters
    }

    /// Feeds arbitrary serial chunks and calls `receive` for every complete
    /// frame. Empty delimiters are ignored, so senders may prefix every frame
    /// with a delimiter to recover from unframed boot output.
    pub fn feed<T>(&mut self, bytes: &[u8], mut receive: impl FnMut(Result<T, DecodeError>))
    where
        T: for<'de> Deserialize<'de>,
    {
        for &byte in bytes {
            if byte == 0 {
                if self.discard_until_delimiter {
                    self.discard_until_delimiter = false;
                    self.inside_frame = false;
                    self.length = 0;
                    continue;
                }
                if !self.inside_frame {
                    self.inside_frame = true;
                    self.length = 0;
                    continue;
                }
                if self.length == 0 {
                    // Repeated leading delimiters deliberately keep the
                    // decoder armed for a body.
                    continue;
                }
                let result = self.decode();
                self.encoded[..self.length].zeroize();
                self.inside_frame = false;
                self.length = 0;
                receive(result);
                continue;
            }

            if self.discard_until_delimiter || !self.inside_frame {
                continue;
            }
            if self.length == self.encoded.len() {
                self.counters.overflows = self.counters.overflows.saturating_add(1);
                self.encoded[..self.length].zeroize();
                self.discard_until_delimiter = true;
                self.length = 0;
                continue;
            }
            self.encoded[self.length] = byte;
            self.length += 1;
        }
    }

    fn decode<T>(&mut self) -> Result<T, DecodeError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let decoded_length =
            cobs::decode_in_place(&mut self.encoded[..self.length]).map_err(|_| {
                self.counters.cobs_errors = self.counters.cobs_errors.saturating_add(1);
                DecodeError::Cobs
            })?;
        if decoded_length < CHECKSUM_BYTES {
            self.counters.too_short = self.counters.too_short.saturating_add(1);
            return Err(DecodeError::TooShort);
        }
        let payload_length = decoded_length - CHECKSUM_BYTES;
        let expected = u32::from_le_bytes(
            self.encoded[payload_length..decoded_length]
                .try_into()
                .expect("checksum length is fixed"),
        );
        if CRC32C.checksum(&self.encoded[..payload_length]) != expected {
            self.counters.checksum_errors = self.counters.checksum_errors.saturating_add(1);
            return Err(DecodeError::Checksum);
        }
        let message = postcard::from_bytes(&self.encoded[..payload_length]).map_err(|_| {
            self.counters.deserialize_errors = self.counters.deserialize_errors.saturating_add(1);
            DecodeError::Deserialize
        })?;
        self.counters.frames = self.counters.frames.saturating_add(1);
        Ok(message)
    }
}

impl Drop for FrameDecoder {
    fn drop(&mut self) {
        self.encoded.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Command, Completion, Direction, Envelope, Event, FlowConfig, Ipv4Endpoint, SessionConfig,
        SessionLinkRequirements, StackUsage, StackWatermark, StartupArtifactChunk,
        StationAttemptFailureReason, StationDisconnectReason, StationFailureStage,
        StationLifecycleEvent, Transport, WifiRole, WifiRoleTransitionEvidence,
    };

    fn command(sequence: u32) -> Envelope<Command> {
        Envelope::new(
            0x1234_5678_9abc_def0,
            sequence,
            42,
            sequence,
            Command::Start,
        )
    }

    #[test]
    fn round_trips_one_byte_at_a_time() {
        let expected = command(7);
        let mut encoder = FrameEncoder::new();
        let frame = encoder.encode(&expected).unwrap();
        let mut decoder = FrameDecoder::new();
        let mut observed = None;
        for byte in frame {
            decoder.feed(core::slice::from_ref(byte), |result| {
                observed = Some(result.unwrap())
            });
        }
        assert_eq!(observed, Some(expected));
        assert_eq!(decoder.counters().frames, 1);
    }

    #[test]
    fn leading_delimiter_recovers_from_text_output() {
        let expected = command(9);
        let mut encoder = FrameEncoder::new();
        let frame = encoder.encode(&expected).unwrap();
        const NOISE: &[u8] = b"rom boot text\n";
        let mut input = [0_u8; MAX_WIRE_FRAME_BYTES + NOISE.len()];
        input[..NOISE.len()].copy_from_slice(NOISE);
        input[NOISE.len()..NOISE.len() + frame.len()].copy_from_slice(frame);

        let mut decoder = FrameDecoder::new();
        let mut observed = None;
        decoder.feed(&input[..NOISE.len() + frame.len()], |result| {
            if let Ok(message) = result {
                observed = Some(message);
            }
        });
        assert_eq!(observed, Some(expected));
    }

    #[test]
    fn rejects_checksum_corruption_and_recovers_for_next_frame() {
        let first = command(1);
        let second = command(2);
        let mut encoder = FrameEncoder::new();
        let mut damaged = [0_u8; MAX_WIRE_FRAME_BYTES];
        let first_frame = encoder.encode(&first).unwrap();
        damaged[..first_frame.len()].copy_from_slice(first_frame);
        let damaged_length = first_frame.len();
        damaged[damaged_length - 3] ^= 0x40;
        let second_frame = encoder.encode(&second).unwrap();

        let mut decoder = FrameDecoder::new();
        let mut errors = 0;
        let mut observed = None;
        decoder.feed(
            &damaged[..damaged_length],
            |result: Result<Envelope<Command>, _>| {
                errors += usize::from(result.is_err());
            },
        );
        decoder.feed(second_frame, |result| observed = Some(result.unwrap()));
        assert_eq!(errors, 1);
        assert_eq!(observed, Some(second));
    }

    #[test]
    fn discards_overfull_noise_until_a_delimiter() {
        let expected = command(3);
        let mut decoder = FrameDecoder::new();
        decoder.feed::<Envelope<Command>>(&[0], |_| {});
        decoder.feed::<Envelope<Command>>(&[0x55; MAX_COBS_FRAME_BYTES + 4], |_| {});
        decoder.feed::<Envelope<Command>>(&[0], |_| {});

        let mut encoder = FrameEncoder::new();
        let frame = encoder.encode(&expected).unwrap();
        let mut observed = None;
        decoder.feed(frame, |result| observed = Some(result.unwrap()));
        assert_eq!(observed, Some(expected));
        assert_eq!(decoder.counters().overflows, 1);
    }

    #[test]
    fn credentials_round_trip_without_debugging_the_secret() {
        extern crate std;

        use crate::{NetworkConfiguration, NetworkCredentials, NetworkIpv4Configuration};

        let credentials =
            NetworkCredentials::try_new(b"test-network", b"private-password").unwrap();
        assert_eq!(credentials.ssid(), b"test-network");
        assert_eq!(credentials.passphrase(), b"private-password");
        let debug = std::format!("{credentials:?}");
        assert!(!debug.contains("private-password"));

        let expected = Envelope::new(
            7,
            1,
            0,
            1,
            Command::ProvisionNetwork(NetworkConfiguration {
                credentials,
                ipv4: NetworkIpv4Configuration::Static {
                    address: [10, 42, 0, 138],
                    prefix_length: 24,
                    gateway: Some([10, 42, 0, 1]),
                },
            }),
        );
        let mut encoder = FrameEncoder::new();
        let frame = encoder.encode(&expected).unwrap();
        let mut decoder = FrameDecoder::new();
        let mut observed = None;
        decoder.feed(frame, |result| observed = Some(result.unwrap()));
        assert_eq!(observed, Some(expected));
    }

    #[test]
    fn access_point_request_round_trips_without_debugging_the_secret() {
        extern crate std;

        use crate::{NetworkCredentials, WifiAccessPointRequest};

        let request = WifiAccessPointRequest {
            credentials: NetworkCredentials::try_new(b"open-radio-ap", b"private-password")
                .unwrap(),
            channel: 6,
        };
        assert_eq!(request.validate(), Ok(()));
        let debug = std::format!("{request:?}");
        assert!(!debug.contains("private-password"));

        let expected = Envelope::new(7, 1, 0, 2, Command::StartAccessPoint(request));
        let mut encoder = FrameEncoder::new();
        let frame = encoder.encode(&expected).unwrap();
        let mut decoder = FrameDecoder::new();
        let mut observed = None;
        decoder.feed(frame, |result| observed = Some(result.unwrap()));
        assert_eq!(observed, Some(expected));
    }

    #[test]
    fn asymmetric_bidirectional_session_round_trips() {
        let expected = Envelope::new(
            7,
            2,
            11,
            3,
            Command::Configure(SessionConfig {
                transport: Transport::Udp,
                direction: Direction::Bidirectional,
                completion: Completion::DurationMillis(12_000),
                peer: Some(Ipv4Endpoint {
                    address: [192, 0, 2, 10],
                    port: 9_002,
                }),
                target_rx: Some(FlowConfig {
                    payload_bytes: 1_200,
                    offered_rate_bps: Some(10_000_000),
                }),
                target_tx: Some(FlowConfig {
                    payload_bytes: 1_472,
                    offered_rate_bps: None,
                }),
                link_requirements: SessionLinkRequirements::tx_block_ack(0),
            }),
        );
        let mut encoder = FrameEncoder::new();
        let frame = encoder.encode(&expected).unwrap();
        let mut decoder = FrameDecoder::new();
        let mut observed = None;
        decoder.feed(frame, |result| observed = Some(result.unwrap()));
        assert_eq!(observed, Some(expected));
    }

    #[test]
    fn stack_usage_query_and_correlated_response_round_trip() {
        let command = Envelope::new(7, 2, 0, 9, Command::QueryStackUsage);
        let mut encoder = FrameEncoder::new();
        let frame = encoder.encode(&command).unwrap();
        let mut decoder = FrameDecoder::new();
        let mut observed = None;
        decoder.feed(frame, |result| observed = Some(result.unwrap()));
        assert_eq!(observed, Some(command));

        let response = Envelope::new(
            7,
            3,
            0,
            9,
            Event::StackUsage(StackUsage {
                minimum_free_percent: 25,
                cpu0: StackWatermark {
                    capacity_bytes: 100,
                    free_bytes: 50,
                    used_bytes: 50,
                },
                cpu1: StackWatermark {
                    capacity_bytes: 80,
                    free_bytes: 40,
                    used_bytes: 40,
                },
            }),
        );
        let frame = encoder.encode(&response).unwrap();
        let mut observed = None;
        decoder.feed(frame, |result| observed = Some(result.unwrap()));
        assert_eq!(observed, Some(response));
    }

    #[test]
    fn station_beacon_loss_generation_round_trips() {
        let expected = Envelope::new(
            7,
            3,
            0,
            0,
            Event::StationLifecycle(StationLifecycleEvent::Disconnected {
                generation: 4,
                reason: StationDisconnectReason::BeaconLoss,
            }),
        );
        let mut encoder = FrameEncoder::new();
        let frame = encoder.encode(&expected).unwrap();
        let mut decoder = FrameDecoder::new();
        let mut observed = None;
        decoder.feed(frame, |result| observed = Some(result.unwrap()));
        assert_eq!(observed, Some(expected));
    }

    #[test]
    fn station_retry_exhaustion_round_trips_without_text_markers() {
        let expected = Envelope::new(
            7,
            4,
            0,
            0,
            Event::StationLifecycle(StationLifecycleEvent::RetryExhausted {
                generation: 1,
                attempts: 3,
                stage: StationFailureStage::CandidateSelection,
                reason: StationAttemptFailureReason::NoCandidate,
            }),
        );
        let mut encoder = FrameEncoder::new();
        let frame = encoder.encode(&expected).unwrap();
        let mut decoder = FrameDecoder::new();
        let mut observed = None;
        decoder.feed(frame, |result| observed = Some(result.unwrap()));
        assert_eq!(observed, Some(expected));
    }

    #[test]
    fn explicit_wifi_role_transition_round_trips_with_request_identity() {
        let expected = Envelope::new(
            7,
            5,
            0,
            42,
            Event::WifiRoleTransitioned(WifiRoleTransitionEvidence {
                previous: WifiRole::Station,
                current: WifiRole::Idle,
                generation: 9,
            }),
        );
        let mut encoder = FrameEncoder::new();
        let frame = encoder.encode(&expected).unwrap();
        let mut decoder = FrameDecoder::new();
        let mut observed = None;
        decoder.feed(frame, |result| observed = Some(result.unwrap()));
        assert_eq!(observed, Some(expected));
    }

    #[test]
    fn maximum_monitor_frame_chunk_fits_and_round_trips() {
        use crate::{
            Event, WIFI_MONITOR_FRAME_CHUNK_MAX_LEN, WifiMonitorEvidenceSource,
            WifiMonitorFrameChunk, WifiMonitorObserved,
        };

        let bytes = [0xa5; WIFI_MONITOR_FRAME_CHUNK_MAX_LEN];
        let chunk = WifiMonitorFrameChunk::try_new(
            7,
            11,
            123_456,
            WIFI_MONITOR_FRAME_CHUNK_MAX_LEN as u16,
            1_024,
            0,
            Some(WifiMonitorObserved {
                source: WifiMonitorEvidenceSource::Hardware,
                value: 6,
            }),
            Some(WifiMonitorObserved {
                source: WifiMonitorEvidenceSource::Hardware,
                value: -42,
            }),
            None,
            &bytes,
        )
        .unwrap();
        let expected = Envelope::new(9, 3, 0, 77, Event::WifiMonitorFrame(chunk));
        let mut encoder = FrameEncoder::new();
        let wire = encoder.encode(&expected).unwrap();
        assert!(wire.len() <= MAX_WIRE_FRAME_BYTES);
        let mut decoder = FrameDecoder::new();
        let mut observed = None;
        decoder.feed(wire, |result| observed = Some(result.unwrap()));
        assert_eq!(observed, Some(expected));
    }

    #[test]
    fn maximum_startup_artifact_chunk_fits_and_round_trips() {
        let bytes = [0x5a; crate::STARTUP_ARTIFACT_CHUNK_MAX_LEN];
        let checksum = startup_artifact_crc32c(&bytes);
        let chunk = StartupArtifactChunk::try_new(
            crate::STARTUP_ARTIFACT_CHUNK_MAX_LEN as u16,
            0,
            checksum,
            &bytes,
        )
        .unwrap();
        let expected = Envelope::new(7, 2, 0, 2, Command::UploadStartupArtifact(chunk));
        let mut encoder = FrameEncoder::new();
        let frame = encoder.encode(&expected).unwrap();
        assert!(frame.len() <= MAX_WIRE_FRAME_BYTES);

        let mut decoder = FrameDecoder::new();
        let mut observed = None;
        decoder.feed(frame, |result| observed = Some(result.unwrap()));
        assert_eq!(observed, Some(expected));
    }

    #[test]
    fn maximum_rx_delivery_evidence_fits_and_round_trips() {
        use crate::{
            EvidenceRecord, RxConsumerLedgerEvidence, RxDeliveryEvidence, RxMacOrderEvidence,
            RxReorderDeliveryEvidence, RxSequenceStageEvidence,
        };

        let stage = RxSequenceStageEvidence {
            data_units: u32::MAX,
            first: Some(u32::MAX),
            highest: Some(u32::MAX),
            gap_events: u32::MAX,
            forward_missing: u32::MAX,
            late_recovered: u32::MAX,
            duplicates: u32::MAX,
            backward_unclassified: u32::MAX,
            first_anomaly: Some(u32::MAX),
            control_markers: u32::MAX,
            data_after_terminal: u32::MAX,
        };
        let delivery = RxDeliveryEvidence {
            post_reorder: stage,
            network_enqueued: stage,
            udp_consumer: stage,
            consumer_ledger: RxConsumerLedgerEvidence {
                matched: u32::MAX,
                enqueued_not_consumed: u32::MAX,
                skipped_before_observed: u32::MAX,
                unexpected_consumer: u32::MAX,
                overflow: u32::MAX,
                first_expected: Some(u32::MAX),
                first_observed: Some(u32::MAX),
            },
            mac_order: RxMacOrderEvidence {
                backward_mac_backward: u32::MAX,
                backward_mac_same: u32::MAX,
                backward_mac_forward: u32::MAX,
                backward_mac_other_tid: u32::MAX,
                backward_mac_unavailable: u32::MAX,
            },
            reorder: RxReorderDeliveryEvidence {
                ingress: u32::MAX,
                ingress_retries: u32::MAX,
                direct: u32::MAX,
                buffered: u32::MAX,
                released: u32::MAX,
                missing: u32::MAX,
                stale: u32::MAX,
                gap_expiries: u32::MAX,
                maximum_occupied: u32::MAX,
                discarded: u32::MAX,
            },
            network_queue_full: u32::MAX,
            network_invalid_length: u32::MAX,
        };
        let expected = Envelope::new(
            7,
            2,
            9,
            2,
            Event::Evidence(EvidenceRecord::RxDelivery(delivery)),
        );
        let mut encoder = FrameEncoder::new();
        let frame = encoder.encode(&expected).unwrap();
        assert!(frame.len() <= MAX_WIRE_FRAME_BYTES);
        let mut decoder = FrameDecoder::new();
        let mut observed = None;
        decoder.feed(frame, |result| observed = Some(result.unwrap()));
        assert_eq!(observed, Some(expected));
    }

    #[test]
    fn startup_artifact_chunk_rejects_empty_and_out_of_range_payloads() {
        assert!(StartupArtifactChunk::try_new(0, 0, 0, &[1]).is_err());
        assert!(StartupArtifactChunk::try_new(1, 0, 0, &[]).is_err());
        assert!(StartupArtifactChunk::try_new(4, 3, 0, &[1, 2]).is_err());
    }

    #[test]
    fn evidence_digest_is_order_and_value_sensitive() {
        use crate::{EvidenceRecord, TransportEvidence};

        let first = EvidenceRecord::Transport(TransportEvidence {
            rx_bytes: 1_200,
            tx_bytes: 0,
            rx_units: 1,
            tx_units: 0,
            elapsed_micros: 100,
            transport_errors: 0,
        });
        let second = EvidenceRecord::Transport(TransportEvidence {
            rx_bytes: 2_400,
            ..match first {
                EvidenceRecord::Transport(evidence) => evidence,
                EvidenceRecord::RxDelivery(_)
                | EvidenceRecord::Link(_)
                | EvidenceRecord::Stack(_) => unreachable!(),
            }
        });

        assert_eq!(evidence_crc32c(&[first]), evidence_crc32c(&[first]));
        assert_ne!(evidence_crc32c(&[first]), evidence_crc32c(&[second]));
        assert_ne!(
            evidence_crc32c(&[first, second]),
            evidence_crc32c(&[second, first])
        );
    }
}
