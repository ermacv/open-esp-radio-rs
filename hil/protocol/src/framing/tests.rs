use super::*;
use crate::{
    Command, Completion, Direction, Envelope, Event, FlowConfig, Ieee802154EdEventProbeEvidence,
    Ieee802154EdEventProbeRequest, Ieee802154EdEventProbeStop, Ieee802154EventStatusProbeEvidence,
    Ieee802154EventStatusProbeRequest, Ieee802154EventStatusProbeStop,
    Ieee802154ObservedEventState, Ieee802154PolledEdOutcome, Ieee802154RxAbortObservation,
    Ieee802154ValidationEdDurationState, Ieee802154ValidationEventEnableState,
    Ieee802154ValidationRxAbortEnableState, Ipv4Endpoint, SessionConfig, SessionLinkRequirements,
    StackUsage, StackWatermark, StartupArtifactChunk, StationAttemptFailureReason,
    StationDisconnectReason, StationFailureStage, StationLifecycleEvent, Transport, WifiRole,
    WifiRoleTransitionEvidence, WireKind,
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
fn memory_benchmark_bounds_and_worst_case_evidence_fit_the_wire() {
    use crate::{
        MemoryBenchmarkEvidence, MemoryBenchmarkMode, MemoryBenchmarkRequest,
        MemoryBenchmarkSource, MemoryBenchmarkStop,
    };
    let request = MemoryBenchmarkRequest {
        mode: MemoryBenchmarkMode::GdmaAsync,
        source: MemoryBenchmarkSource::Psram,
        bytes: 1536,
        frames: 32,
        iterations: 64,
    };
    assert!(request.validate());
    for frames in [0, 33, u8::MAX] {
        assert!(!MemoryBenchmarkRequest { frames, ..request }.validate());
    }
    for (bytes, frames) in [(4096, 1), (4096, 12), (1514, 32)] {
        assert!(
            MemoryBenchmarkRequest {
                bytes,
                frames,
                ..request
            }
            .validate()
        );
    }
    for (bytes, frames) in [(4096, 13), (1537, 32)] {
        assert!(
            !MemoryBenchmarkRequest {
                bytes,
                frames,
                ..request
            }
            .validate()
        );
    }
    for bytes in [0, 4097, u16::MAX] {
        assert!(!MemoryBenchmarkRequest { bytes, ..request }.validate());
    }
    for iterations in [0, 65, u16::MAX] {
        assert!(
            !MemoryBenchmarkRequest {
                iterations,
                ..request
            }
            .validate()
        );
    }
    assert!(
        MemoryBenchmarkRequest {
            bytes: 1,
            iterations: 1,
            ..request
        }
        .validate()
    );
    let command = Envelope::new(7, 3, 0, 2, Command::ProbeMemoryBenchmark(request));
    let mut encoder = FrameEncoder::new();
    let mut decoder = FrameDecoder::new();
    let mut observed = None;
    decoder.feed(encoder.encode(&command).unwrap(), |result| {
        observed = Some(result.unwrap())
    });
    assert_eq!(observed, Some(command));
    let event = Envelope::new(
        u64::MAX,
        u32::MAX,
        u64::MAX,
        u32::MAX,
        Event::MemoryBenchmarkCompleted(MemoryBenchmarkEvidence {
            request,
            completed_iterations: u16::MAX,
            elapsed_micros: u64::MAX,
            elapsed_cycles: u64::MAX,
            elapsed_instructions: u64::MAX,
            foreground_cycles: u64::MAX,
            foreground_instructions: u64::MAX,
            polls: u32::MAX,
            stop: MemoryBenchmarkStop::GuardCorrupted,
        }),
    );
    let frame = encoder.encode(&event).unwrap();
    assert!(frame.len() <= MAX_WIRE_FRAME_BYTES);
    let mut decoder = FrameDecoder::new();
    let mut observed = None;
    decoder.feed(frame, |result| observed = Some(result.unwrap()));
    assert_eq!(observed, Some(event));
}

#[test]
fn command_envelope_remains_small_enough_for_embedded_queues() {
    let size = core::mem::size_of::<Envelope<Command>>();
    // The largest command owns two independent WPA2 credential sets for
    // one atomic STA+AP request. Keep the complete decoded queue element
    // within an explicit embedded budget instead of splitting that
    // ownership across hidden compatibility state.
    assert!(size <= 288, "command envelope occupies {size} bytes");
}

#[test]
fn ieee802154_event_status_probe_validation_accepts_only_contract_bounds() {
    const MINIMUM: Ieee802154EventStatusProbeRequest = Ieee802154EventStatusProbeRequest {
        poll_limit: 1,
        timer_threshold: 1,
    };
    const MAXIMUM: Ieee802154EventStatusProbeRequest = Ieee802154EventStatusProbeRequest {
        poll_limit: 1_000_000,
        timer_threshold: 1_000,
    };
    const {
        assert!(MINIMUM.validate());
        assert!(MAXIMUM.validate());
        assert!(
            !Ieee802154EventStatusProbeRequest {
                poll_limit: 0,
                timer_threshold: 1,
            }
            .validate()
        );
        assert!(
            !Ieee802154EventStatusProbeRequest {
                poll_limit: 1_000_001,
                timer_threshold: 1,
            }
            .validate()
        );
        assert!(
            !Ieee802154EventStatusProbeRequest {
                poll_limit: 1,
                timer_threshold: 0,
            }
            .validate()
        );
        assert!(
            !Ieee802154EventStatusProbeRequest {
                poll_limit: 1,
                timer_threshold: 1_001,
            }
            .validate()
        );
    }
}

#[test]
fn ieee802154_event_status_probe_command_fits_and_round_trips() {
    let expected = Envelope::new(
        7,
        3,
        9,
        2,
        Command::ProbeIeee802154EventStatus(Ieee802154EventStatusProbeRequest {
            poll_limit: 1_000_000,
            timer_threshold: 1_000,
        }),
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
fn ieee802154_event_status_probe_evidence_fits_and_round_trips() {
    let expected = Envelope::new(
        7,
        3,
        9,
        2,
        Event::Ieee802154EventStatusProbeCompleted(Ieee802154EventStatusProbeEvidence {
            stop: Ieee802154EventStatusProbeStop::Complete,
            event_enable_before: Ieee802154ValidationEventEnableState::Unexpected,
            event_enable_active: Ieee802154ValidationEventEnableState::TimerPairOnly,
            event_enable_after: Ieee802154ValidationEventEnableState::AllMasked,
            post_enable_events: Ieee802154ObservedEventState::Unclassified,
            timer0_value_before_start: u32::MAX,
            timer1_value_before_start: u32::MAX,
            timer0_value_min: u32::MAX,
            timer0_value_max: u32::MAX,
            timer1_value_min: u32::MAX,
            timer1_value_max: u32::MAX,
            timer0_value_after_stop: u32::MAX,
            timer1_value_after_stop: u32::MAX,
            reset_events: Ieee802154ObservedEventState::Clear,
            dual_observed_events: Ieee802154ObservedEventState::Timer0AndTimer1,
            dual_latched_events: Ieee802154ObservedEventState::Timer0AndTimer1,
            after_timer0_ack_events: Ieee802154ObservedEventState::Timer1Only,
            after_timer1_ack_events: Ieee802154ObservedEventState::Clear,
            distinct_snapshot_events: Ieee802154ObservedEventState::Timer0Only,
            distinct_before_ack_events: Ieee802154ObservedEventState::Timer0AndTimer1,
            distinct_after_ack_events: Ieee802154ObservedEventState::Timer1Only,
            cleanup_pending_events: Ieee802154ObservedEventState::UnexpectedNamed,
            final_events: Ieee802154ObservedEventState::Unclassified,
        }),
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
fn ieee802154_ed_event_probe_validation_accepts_only_contract_bounds() {
    for request in [
        Ieee802154EdEventProbeRequest {
            poll_limit: 1,
            timer_threshold: 1,
        },
        Ieee802154EdEventProbeRequest {
            poll_limit: 1_000_000,
            timer_threshold: 1_000,
        },
    ] {
        assert!(request.validate());
    }
    for request in [
        Ieee802154EdEventProbeRequest {
            poll_limit: 0,
            timer_threshold: 1,
        },
        Ieee802154EdEventProbeRequest {
            poll_limit: 1_000_001,
            timer_threshold: 1,
        },
        Ieee802154EdEventProbeRequest {
            poll_limit: 1,
            timer_threshold: 0,
        },
        Ieee802154EdEventProbeRequest {
            poll_limit: 1,
            timer_threshold: 1_001,
        },
    ] {
        assert!(!request.validate());
    }
}

#[test]
fn ieee802154_ed_event_probe_command_and_evidence_fit_and_round_trip() {
    let command = Envelope::new(
        7,
        3,
        9,
        2,
        Command::ProbeIeee802154EdEvent(Ieee802154EdEventProbeRequest {
            poll_limit: 1_000_000,
            timer_threshold: 1_000,
        }),
    );
    let evidence = Envelope::new(
        7,
        3,
        9,
        2,
        Event::Ieee802154EdEventProbeCompleted(Ieee802154EdEventProbeEvidence {
            stop: Ieee802154EdEventProbeStop::Complete,
            production_ed_first: Ieee802154PolledEdOutcome::Complete {
                rss_code: i8::MIN,
                polls: u32::MAX,
            },
            production_ed_second: Some(Ieee802154PolledEdOutcome::Complete {
                rss_code: i8::MAX,
                polls: u32::MAX,
            }),
            event_enable_before: Ieee802154ValidationEventEnableState::AllMasked,
            event_enable_active: Ieee802154ValidationEventEnableState::EdDoneTimer0RxAbortOnly,
            event_enable_after: Ieee802154ValidationEventEnableState::Unexpected,
            rx_abort_enable_before: Ieee802154ValidationRxAbortEnableState::AllMasked,
            rx_abort_enable_active: Ieee802154ValidationRxAbortEnableState::EdOperationReasonsOnly,
            rx_abort_enable_after: Ieee802154ValidationRxAbortEnableState::Unexpected,
            ed_duration_before: Ieee802154ValidationEdDurationState::Other,
            ed_duration_active: Ieee802154ValidationEdDurationState::ValidationEight,
            ed_duration_after: Ieee802154ValidationEdDurationState::Other,
            timer0_value_before_start: u32::MAX,
            timer0_value_min: u32::MAX,
            timer0_value_max: u32::MAX,
            timer0_value_after_stop: u32::MAX,
            reset_events: Ieee802154ObservedEventState::Clear,
            post_enable_events: Ieee802154ObservedEventState::Unclassified,
            observed_events: Ieee802154ObservedEventState::EdDoneAndTimer0,
            terminal_events: Ieee802154ObservedEventState::RxAbortOnly,
            after_ed_done_write_events: Ieee802154ObservedEventState::Timer0Only,
            after_timer0_write_events: Ieee802154ObservedEventState::Clear,
            cleanup_pending_events: Ieee802154ObservedEventState::UnexpectedNamed,
            final_events: Ieee802154ObservedEventState::Unclassified,
            rx_abort_reason: Some(Ieee802154RxAbortObservation::Unclassified),
            stop_command_issued: true,
            cleanup_clear: true,
        }),
    );

    {
        let mut encoder = FrameEncoder::new();
        let frame = encoder.encode(&command).unwrap();
        assert!(frame.len() <= MAX_WIRE_FRAME_BYTES);
        let mut decoder = FrameDecoder::new();
        let mut observed = None;
        decoder.feed(frame, |result| observed = Some(result.unwrap()));
        assert_eq!(observed, Some(command));
    }
    {
        let mut encoder = FrameEncoder::new();
        let frame = encoder.encode(&evidence).unwrap();
        assert!(frame.len() <= MAX_WIRE_FRAME_BYTES);
        let mut decoder = FrameDecoder::new();
        let mut observed = None;
        decoder.feed(frame, |result| observed = Some(result.unwrap()));
        assert_eq!(observed, Some(evidence));
    }
}

#[test]
fn access_point_retry_evidence_fits_and_round_trips() {
    use crate::{WifiAccessPointEvidence, WifiMacRxHardwareEvidence};

    let evidence = WifiAccessPointEvidence {
        rx_hardware: WifiMacRxHardwareEvidence {
            mpdu_count: u16::MAX,
            data_success: u16::MAX,
            fcs_error: u16::MAX,
            abort: u16::MAX,
            abort_fcs_pass: u16::MAX,
            power_drop_error: u16::MAX,
            he_sig_b_error: u16::MAX,
            same_bm_error: u16::MAX,
            signal_field: u16::MAX,
            end: u16::MAX,
            other_unicast: u16::MAX,
            buffer_full: u16::MAX,
            fifo_overflow: u16::MAX,
            tkip_error: u16::MAX,
            bluetooth_block_error: u16::MAX,
            frequency_hop_error: u16::MAX,
            last_unmatched_error: u16::MAX,
            ack_interrupt: u16::MAX,
            rts_interrupt: u16::MAX,
            brx_agc_error: u16::MAX,
            brx_error: u16::MAX,
            nrx_error: u16::MAX,
            nrx_abort: u16::MAX,
            nrx_agc_exit: u16::MAX,
            nrx_baseband_off: u16::MAX,
            nrx_fdm_watchdog: u16::MAX,
            nrx_restart: u16::MAX,
            nrx_service: u16::MAX,
            nrx_tx_over: u16::MAX,
            nrx_unsupported: u16::MAX,
            nrx_he_format: u16::MAX,
            nrx_ht_sig: u16::MAX,
            nrx_he_unsupported: u16::MAX,
            nrx_he_sig_a_crc: u16::MAX,
            rx_hang: u8::MAX,
            tx_hang: u8::MAX,
            rx_tx_hang: u32::MAX,
            rx_tx_panic: u32::MAX,
        },
        data_tx_attempts: u32::MAX,
        data_tx_retried_frames: u32::MAX,
        data_tx_maximum_attempts: u8::MAX,
        data_tx_minimum_final_rate_kbps: u32::MAX,
        data_tx_ack_snr_samples: u32::MAX,
        data_tx_minimum_ack_snr_db: i8::MIN,
        data_tx_maximum_ack_snr_db: i8::MAX,
        tx_ack_timeout_retries: u32::MAX,
        tx_cts_timeout_retries: u32::MAX,
        tx_collision_retries: u32::MAX,
        ..WifiAccessPointEvidence::default()
    };
    let expected = Envelope::new(7, 3, 9, 2, Event::WifiAccessPointStopped(evidence));
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    assert!(frame.len() <= MAX_WIRE_FRAME_BYTES);
    let mut decoder = FrameDecoder::new();
    let mut observed = None;
    decoder.feed(frame, |result| observed = Some(result.unwrap()));
    assert_eq!(observed, Some(expected));
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
fn wire_header_is_fixed_and_precedes_the_postcard_body() {
    let expected = command(7);
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    let mut raw = [0_u8; MAX_RAW_FRAME_BYTES];
    let decoded = cobs::decode(&frame[2..frame.len() - 1], &mut raw).unwrap();

    assert_eq!(&raw[..4], &WIRE_MAGIC);
    assert_eq!(raw[4], FRAMING_VERSION);
    assert_eq!(raw[5], WireKind::Command as u8);
    assert_eq!(u16::from_le_bytes([raw[6], raw[7]]), PROTOCOL_VERSION);
    assert_eq!(
        u64::from_le_bytes(raw[8..16].try_into().unwrap()),
        expected.boot_id
    );
    assert_eq!(
        u32::from_le_bytes(raw[16..20].try_into().unwrap()),
        expected.message_sequence
    );
    let payload_length = usize::from(u16::from_le_bytes([raw[32], raw[33]]));
    assert_eq!(decoded, WIRE_HEADER_BYTES + payload_length + CHECKSUM_BYTES);
    assert_eq!(
        postcard::from_bytes::<Command>(
            &raw[WIRE_HEADER_BYTES..WIRE_HEADER_BYTES + payload_length]
        )
        .unwrap(),
        expected.body
    );
}

#[test]
fn rejects_protocol_version_before_deserializing_the_body() {
    let mut expected = command(7);
    expected.protocol_version = PROTOCOL_VERSION - 1;
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let mut observed = None;
    decoder.feed::<Command>(frame, |result| observed = Some(result));
    assert_eq!(observed, Some(Err(DecodeError::ProtocolVersion)));
    assert_eq!(decoder.counters().protocol_version_errors, 1);
    assert_eq!(decoder.counters().deserialize_errors, 0);
}

#[test]
fn rejects_an_event_on_the_command_endpoint() {
    let expected = Envelope::new(7, 1, 0, 1, Event::Accepted);
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let mut observed = None;
    decoder.feed::<Command>(frame, |result| observed = Some(result));
    assert_eq!(observed, Some(Err(DecodeError::MessageKind)));
    assert_eq!(decoder.counters().message_kind_errors, 1);
    assert_eq!(decoder.counters().deserialize_errors, 0);
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
    decoder.feed::<Command>(&[0], |_| {});
    decoder.feed::<Command>(&[0x55; MAX_COBS_FRAME_BYTES + 4], |_| {});
    decoder.feed::<Command>(&[0], |_| {});

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

    use crate::NetworkCredentials;

    let credentials = NetworkCredentials::try_new(b"test-network", b"private-password").unwrap();
    assert_eq!(credentials.ssid(), b"test-network");
    assert_eq!(credentials.passphrase(), b"private-password");
    let debug = std::format!("{credentials:?}");
    assert!(!debug.contains("private-password"));

    let expected = Envelope::new(7, 1, 0, 1, Command::StartStation(credentials));
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

    use crate::{
        NetworkCredentials, NetworkIpv4Configuration, WifiAccessPointRequest,
        WifiAccessPointSecurity, WifiChannelWidth,
    };

    let request = WifiAccessPointRequest {
        credentials: NetworkCredentials::try_new(b"open-radio-ap", b"private-password").unwrap(),
        security: WifiAccessPointSecurity::Wpa2Personal,
        channel: 6,
        channel_width: WifiChannelWidth::Mhz40Above,
        client_limit: 4,
        ipv4: NetworkIpv4Configuration::Static {
            address: [10, 43, 0, 1],
            prefix_length: 24,
            gateway: None,
        },
    };
    assert_eq!(request.validate(), Ok(()));
    let mut invalid_geometry = request.clone();
    invalid_geometry.channel = 13;
    assert_eq!(
        invalid_geometry.validate(),
        Err(crate::WifiAccessPointRequestError::Channel)
    );
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
fn station_access_point_request_round_trips_as_one_owned_command() {
    use crate::{
        NetworkCredentials, NetworkIpv4Configuration, WifiAccessPointRequest,
        WifiAccessPointSecurity, WifiChannelWidth, WifiStationAccessPointRequest,
    };

    let request = WifiStationAccessPointRequest {
        station_credentials: NetworkCredentials::try_new(b"upstream-ap", b"upstream-password")
            .unwrap(),
        access_point: WifiAccessPointRequest {
            credentials: NetworkCredentials::try_new(b"open-radio-ap", b"downstream-password")
                .unwrap(),
            security: WifiAccessPointSecurity::Wpa2Personal,
            channel: 6,
            channel_width: WifiChannelWidth::Mhz40Above,
            client_limit: 1,
            ipv4: NetworkIpv4Configuration::Static {
                address: [192, 168, 4, 1],
                prefix_length: 24,
                gateway: None,
            },
        },
    };
    assert_eq!(request.validate(), Ok(()));
    let expected = Envelope::new(7, 1, 0, 3, Command::StartStationAccessPoint(request));
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
            network_interface: crate::WifiNetworkInterface::Station,
            transport: Transport::Udp,
            direction: Direction::Bidirectional,
            completion: Completion::DurationMillis(12_000),
            flows: [
                Some(crate::SessionFlowConfig {
                    flow_id: 7,
                    peer: Some(Ipv4Endpoint {
                        address: [192, 0, 2, 10],
                        port: 9_002,
                    }),
                    target_rx: Some(FlowConfig {
                        payload_bytes: 1_200,
                        offered_rate_bps: Some(10_000_000),
                        pacing_group_datagrams: None,
                    }),
                    target_tx: Some(FlowConfig {
                        payload_bytes: 1_472,
                        offered_rate_bps: None,
                        pacing_group_datagrams: None,
                    }),
                }),
                None,
            ],
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
fn two_peer_udp_session_round_trips_without_erasing_flow_identity() {
    let flow = |flow_id, address| {
        Some(crate::SessionFlowConfig {
            flow_id,
            peer: Some(Ipv4Endpoint {
                address,
                port: 9_002 + u16::from(flow_id),
            }),
            target_rx: Some(FlowConfig {
                payload_bytes: 1_472,
                offered_rate_bps: Some(60_000_000),
                pacing_group_datagrams: None,
            }),
            target_tx: Some(FlowConfig {
                payload_bytes: 1_472,
                offered_rate_bps: Some(60_000_000),
                pacing_group_datagrams: None,
            }),
        })
    };
    let expected = Envelope::new(
        7,
        2,
        11,
        3,
        Command::Configure(SessionConfig {
            network_interface: crate::WifiNetworkInterface::AccessPoint,
            transport: Transport::Udp,
            direction: Direction::Bidirectional,
            completion: Completion::DurationMillis(12_000),
            flows: [flow(3, [192, 168, 4, 2]), flow(9, [192, 168, 4, 3])],
            link_requirements: SessionLinkRequirements::NONE,
        }),
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
            cpu0: StackWatermark {
                capacity_bytes: 100,
                free_bytes: 50,
                used_bytes: 50,
                minimum_free_bytes: 25,
            },
            cpu1: StackWatermark {
                capacity_bytes: 80,
                free_bytes: 40,
                used_bytes: 40,
                minimum_free_bytes: 20,
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
        Event, WIFI_MONITOR_FRAME_CHUNK_MAX_LEN, WifiMonitorEvidenceSource, WifiMonitorFrameChunk,
        WifiMonitorObserved,
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
fn control_mailbox_overflow_disconnect_round_trips_on_current_protocol() {
    let expected = Envelope::new(
        7,
        5,
        0,
        43,
        Event::StationLifecycle(crate::StationLifecycleEvent::Disconnected {
            generation: 11,
            reason: StationDisconnectReason::ControlMailboxOverflow,
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
        network_pool_exhausted: u32::MAX,
        network_link_down: u32::MAX,
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
fn maximum_network_scheduler_evidence_fits_and_round_trips() {
    use crate::{EvidenceRecord, NetworkSchedulerEvidence};

    let expected = Envelope::new(
        7,
        3,
        9,
        2,
        Event::Evidence(EvidenceRecord::NetworkScheduler(NetworkSchedulerEvidence {
            polls: u32::MAX,
            ingress_calls: u32::MAX,
            ingress_packets: u32::MAX,
            egress_passes: u32::MAX,
            egress_tx_tokens: u32::MAX,
            egress_blocked: u32::MAX,
            ingress_budget_exhausted: u32::MAX,
            egress_budget_exhausted: u32::MAX,
            started_with_ingress: u32::MAX,
            started_with_egress: u32::MAX,
            exit_drained: u32::MAX,
            exit_work_budget: u32::MAX,
            exit_egress_credit: u32::MAX,
        })),
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
fn maximum_radio_evidence_fits_and_round_trips() {
    use crate::{EvidenceRecord, RadioEvidence, RxRadioEvidence, TxRadioEvidence};

    let expected = Envelope::new(
        7,
        3,
        9,
        2,
        Event::Evidence(EvidenceRecord::Radio(RadioEvidence {
            rx: Some(RxRadioEvidence {
                phy_format: u8::MAX,
                ht40_long_gi_frames: u32::MAX,
                ht40_short_gi_frames: u32::MAX,
                ht40_below_mcs7_frames: u32::MAX,
                ht_invalid_frames: u32::MAX,
                dma_buffer_full: u32::MAX,
                dma_fifo_overflow: u32::MAX,
                network_dropped: u32::MAX,
                irq_drain_saturated: u32::MAX,
                unhandled_irq_entries: u32::MAX,
                sequence_first: Some(u32::MAX),
                sequence_highest: Some(u32::MAX),
                sequence_gap_events: u32::MAX,
                sequence_forward_missing: u32::MAX,
                sequence_backward: u32::MAX,
                sequence_duplicates: u32::MAX,
                sequence_unsequenced: u32::MAX,
                s_mpdu_datagrams: u32::MAX,
                not_s_mpdu_datagrams: u32::MAX,
                s_mpdu_unavailable_datagrams: u32::MAX,
                s_mpdu_beacons: u32::MAX,
                not_s_mpdu_beacons: u32::MAX,
                s_mpdu_unavailable_beacons: u32::MAX,
                ampdu_datagrams: u32::MAX,
                not_ampdu_datagrams: u32::MAX,
                hardware_ampdu_datagrams: u32::MAX,
                hardware_not_ampdu_datagrams: u32::MAX,
                protocol_ampdu_datagrams: u32::MAX,
                protocol_not_ampdu_datagrams: u32::MAX,
                ampdu_unavailable_datagrams: u32::MAX,
                reorder_tid: u8::MAX,
                reorder_window: u16::MAX,
                reorder_first_samples: u32::MAX,
                reorder_first_tid: u8::MAX,
                reorder_first_start: u16::MAX,
                reorder_first_sequence: u16::MAX,
                reorder_first_distance: u16::MAX,
                reorder_current_occupied: u32::MAX,
                reorder_maximum_occupied: u32::MAX,
                rx_service_calls: u32::MAX,
                rx_frontier_histogram_samples: u32::MAX,
                mac_irq_entries: u32::MAX,
                mac_irq_classified_entries: u32::MAX,
            }),
            tx: Some(TxRadioEvidence {
                bandwidth_mhz: u16::MAX,
                aggregate_rate_kbps: u32::MAX,
                aggregates_prepared: u32::MAX,
                prepared_histogram: [u32::MAX; 8],
                ..TxRadioEvidence::default()
            }),
        })),
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
fn maximum_tx_aggregate_timing_evidence_fits_and_round_trips() {
    use crate::{EvidenceRecord, TxAggregateTimingEvidence};

    let expected = Envelope::new(
        7,
        3,
        9,
        2,
        Event::Evidence(EvidenceRecord::TxAggregateTiming(
            TxAggregateTimingEvidence {
                preparation_micros: u32::MAX,
                preparation_max_micros: u32::MAX,
                publication_micros: u32::MAX,
                publication_max_micros: u32::MAX,
                exchange_micros: u32::MAX,
                exchange_max_micros: u32::MAX,
                first_exchanges: u32::MAX,
                first_exchange_micros: u32::MAX,
                first_exchange_max_micros: u32::MAX,
                retried_exchanges: u32::MAX,
                retry_publications: u32::MAX,
                retry_exchange_micros: u32::MAX,
                retry_exchange_max_micros: u32::MAX,
                tx_irq_epochs: u32::MAX,
                tx_irq_service_samples: u32::MAX,
                tx_irq_clock_skew_samples: u32::MAX,
                tx_irq_service_micros: u32::MAX,
                tx_irq_service_max_micros: u32::MAX,
                tx_publication_to_irq_samples: u32::MAX,
                tx_publication_to_irq_micros: u32::MAX,
                tx_publication_to_irq_max_micros: u32::MAX,
                standby_prepared: u32::MAX,
                standby_published: u32::MAX,
                standby_cancelled: u32::MAX,
            },
        )),
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
fn maximum_flow_transport_evidence_fits_and_round_trips() {
    use crate::{EvidenceRecord, FlowTransportEvidence};

    let expected = Envelope::new(
        7,
        3,
        9,
        2,
        Event::Evidence(EvidenceRecord::FlowTransport(FlowTransportEvidence {
            flow_id: u8::MAX,
            rx_bytes: u64::MAX,
            tx_bytes: u64::MAX,
            rx_units: u64::MAX,
            tx_units: u64::MAX,
            elapsed_micros: u64::MAX,
            transport_errors: u32::MAX,
        })),
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
            EvidenceRecord::FlowTransport(_)
            | EvidenceRecord::Radio(_)
            | EvidenceRecord::TxAggregateTiming(_)
            | EvidenceRecord::RxDelivery(_)
            | EvidenceRecord::NetworkScheduler(_)
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
