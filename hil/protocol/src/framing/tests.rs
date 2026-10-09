use super::*;
use crate::network::evidence_crc32c;
use crate::phy::startup_artifact_crc32c;

#[test]
fn gatt_observation_round_trips_with_epoch_and_stack_measurement() {
    let expected = Envelope::new(
        77,
        9,
        0,
        8,
        crate::bluetooth::GattState(crate::bluetooth::BluetoothGattEvidence {
            address: Some([1, 2, 3, 4, 5, 6]),
            connected: true,
            connections: 3,
            disconnections: 2,
            reads: 6,
            writes: 3,
            value: 0x73,
            cpu0_stack: Some(crate::system::StackWatermark {
                capacity_bytes: 65536,
                free_bytes: 32768,
                used_bytes: 32768,
                minimum_free_bytes: 4096,
            }),
            ..Default::default()
        }),
    );
    let mut encoder = FrameEncoder::new();
    let bytes = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, bytes).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}
mod airtime;
mod secure_gatt;

/// Feeds `bytes` and returns the last complete frame as message `M`.
fn receive<M: crate::Message>(
    decoder: &mut FrameDecoder,
    bytes: &[u8],
) -> Option<Result<Envelope<M>, DecodeError>> {
    let mut last = None;
    decoder.feed(M::WIRE_KIND, bytes, |frame| {
        last = Some(frame.and_then(|frame| {
            frame
                .decode::<M>()
                .expect("the frame carries the expected type")
        }));
    });
    last
}
use crate::system::{HangTarget, InjectHang};
use crate::{
    Envelope, WireKind, ieee802154::Ieee802154EdEventProbeEvidence,
    ieee802154::Ieee802154EdEventProbeRequest, ieee802154::Ieee802154EdEventProbeStop,
    ieee802154::Ieee802154EventStatusProbeEvidence, ieee802154::Ieee802154EventStatusProbeRequest,
    ieee802154::Ieee802154EventStatusProbeStop, ieee802154::Ieee802154ObservedEventState,
    ieee802154::Ieee802154PolledEdOutcome, ieee802154::Ieee802154RxAbortObservation,
    ieee802154::Ieee802154ValidationEdDurationState,
    ieee802154::Ieee802154ValidationEventEnableState,
    ieee802154::Ieee802154ValidationRxAbortEnableState, network::Completion, network::Direction,
    network::FlowConfig, network::Ipv4Endpoint, network::SessionConfig,
    network::SessionLinkRequirements, network::Transport, phy::StartupArtifactChunk,
    system::StackUsage, system::StackWatermark, wifi::StationAttemptFailureReason,
    wifi::StationDisconnectReason, wifi::StationFailureStage, wifi::StationLifecycleEvent,
    wifi::WifiRadioRestartEvidence, wifi::WifiRadioRestartRf, wifi::WifiRole,
    wifi::WifiRoleTransitionEvidence,
};

fn command(sequence: u32) -> Envelope<InjectHang> {
    Envelope::new(
        0x1234_5678_9abc_def0,
        sequence,
        42,
        sequence,
        InjectHang(HangTarget::Console),
    )
}

#[test]
fn initialization_preserves_each_explicit_ap_scheduler_policy() {
    for ap_scheduler in [
        crate::wifi::WifiApScheduler::Disabled,
        crate::wifi::WifiApScheduler::RrHtResponse24,
        crate::wifi::WifiApScheduler::DeficitHtResponse24,
    ] {
        let expected = Envelope::new(
            1,
            1,
            0,
            1,
            crate::wifi::Initialize(crate::wifi::InitializationConfiguration {
                ap_scheduler,
                ipv4: crate::wifi::NetworkIpv4Configuration::Dhcp,
                data_plane: Default::default(),
                rx_checksum: Default::default(),
                tx_udp_checksum: Default::default(),
                tx_buffer: Default::default(),
                rx_continuation: Default::default(),
                l1_cache_counters: false,
            }),
        );
        let mut encoder = FrameEncoder::new();
        let mut decoder = FrameDecoder::new();
        let observed =
            receive(&mut decoder, encoder.encode(&expected).unwrap()).map(Result::unwrap);
        assert_eq!(observed, Some(expected));
    }
}

#[test]
fn memory_benchmark_bounds_and_worst_case_evidence_fit_the_wire() {
    use crate::{
        system::MemoryBenchmarkEvidence, system::MemoryBenchmarkMode,
        system::MemoryBenchmarkRequest, system::MemoryBenchmarkSource, system::MemoryBenchmarkStop,
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
    let command = Envelope::new(7, 3, 0, 2, crate::system::RunMemoryBenchmark(request));
    let mut encoder = FrameEncoder::new();
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, encoder.encode(&command).unwrap()).map(Result::unwrap);
    assert_eq!(observed, Some(command));
    let event = Envelope::new(
        u64::MAX,
        u32::MAX,
        u64::MAX,
        u32::MAX,
        crate::system::MemoryBenchmarkCompleted(MemoryBenchmarkEvidence {
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
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(event));
}

#[test]
fn command_envelope_remains_small_enough_for_embedded_queues() {
    let size = core::mem::size_of::<Envelope<crate::wifi::StartStationAccessPoint>>();
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
        crate::ieee802154::ProbeEventStatus(Ieee802154EventStatusProbeRequest {
            poll_limit: 1_000_000,
            timer_threshold: 1_000,
        }),
    );
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    assert!(frame.len() <= MAX_WIRE_FRAME_BYTES);
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn ieee802154_event_status_probe_evidence_fits_and_round_trips() {
    let expected = Envelope::new(
        7,
        3,
        9,
        2,
        crate::ieee802154::EventStatusProbed(Ieee802154EventStatusProbeEvidence {
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
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
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
        crate::ieee802154::ProbeEdEvent(Ieee802154EdEventProbeRequest {
            poll_limit: 1_000_000,
            timer_threshold: 1_000,
        }),
    );
    let evidence = Envelope::new(
        7,
        3,
        9,
        2,
        crate::ieee802154::EdEventProbed(Ieee802154EdEventProbeEvidence {
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
        let observed = receive(&mut decoder, frame).map(Result::unwrap);
        assert_eq!(observed, Some(command));
    }
    {
        let mut encoder = FrameEncoder::new();
        let frame = encoder.encode(&evidence).unwrap();
        assert!(frame.len() <= MAX_WIRE_FRAME_BYTES);
        let mut decoder = FrameDecoder::new();
        let observed = receive(&mut decoder, frame).map(Result::unwrap);
        assert_eq!(observed, Some(evidence));
    }
}

#[test]
fn access_point_retry_evidence_fits_and_round_trips() {
    use crate::{wifi::WifiAccessPointEvidence, wifi::WifiMacRxHardwareEvidence};

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
    let evidence = crate::wifi::WifiAccessPointEvidence {
        first_rx_protocol_rejection: Some(crate::wifi::WifiRxRejection {
            reason: crate::wifi::WifiRxRejectionReason::FragmentRetryPacketNumberMismatch {
                fragment_number: u8::MAX,
                expected: u64::MAX,
                observed: u64::MAX,
            },
            at_micros: u64::MAX,
            transmitter: Some([u8::MAX; 6]),
            frame_control: Some(u16::MAX),
            sequence_control: Some(u16::MAX),
            tid: Some(u8::MAX),
            key_id: Some(u8::MAX),
            packet_number: Some(u64::MAX),
            mpdu_length: u32::MAX,
        }),
        tx_retention: Some(crate::wifi::WifiTxRetentionEvidence {
            active_queue_full: u32::MAX,
            unicast_power_save_full: u32::MAX,
            group_power_save_full: u32::MAX,
        }),
        ..evidence
    };
    let expected = Envelope::new(7, 3, 9, 2, crate::wifi::AccessPointStopped(evidence));
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    assert!(frame.len() <= MAX_WIRE_FRAME_BYTES);
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
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
        if let Some(message) = receive(&mut decoder, core::slice::from_ref(byte)) {
            observed = Some(message.unwrap());
        }
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
    assert_eq!(
        u64::from_le_bytes(raw[6..14].try_into().unwrap()),
        expected.boot_id
    );
    assert_eq!(
        u32::from_le_bytes(raw[14..18].try_into().unwrap()),
        expected.message_sequence
    );
    assert_eq!(raw[KEY_RANGE], <InjectHang as crate::Message>::KEY.0);
    let payload_length = usize::from(u16::from_le_bytes(raw[LENGTH_RANGE].try_into().unwrap()));
    assert_eq!(decoded, WIRE_HEADER_BYTES + payload_length + CHECKSUM_BYTES);
    assert_eq!(
        postcard::from_bytes::<InjectHang>(
            &raw[WIRE_HEADER_BYTES..WIRE_HEADER_BYTES + payload_length]
        )
        .unwrap(),
        expected.body
    );
}

#[test]
fn a_frame_of_another_type_is_not_decoded_as_this_one() {
    let mut encoder = FrameEncoder::new();
    let frame = encoder
        .encode(&Envelope::new(7, 1, 0, 1, crate::base::GetBootStatus))
        .unwrap();
    let mut decoder = FrameDecoder::new();
    let mut seen = None;
    decoder.feed(WireKind::Command, frame, |result| {
        let frame = result.unwrap();
        assert_eq!(
            frame.key,
            <crate::base::GetBootStatus as crate::Message>::KEY
        );
        seen = Some(frame.decode::<InjectHang>().is_none());
    });
    assert_eq!(seen, Some(true));
    assert_eq!(decoder.counters().frames, 1);
}

#[test]
fn rejects_an_event_on_the_command_endpoint() {
    let expected = Envelope::new(7, 1, 0, 1, crate::base::Accepted);
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let observed = receive::<InjectHang>(&mut decoder, frame);
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
    decoder.feed(
        WireKind::Command,
        &input[..NOISE.len() + frame.len()],
        |result| {
            if let Ok(frame) = result {
                observed = frame.decode::<InjectHang>().map(Result::unwrap);
            }
        },
    );
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
    decoder.feed(WireKind::Command, &damaged[..damaged_length], |result| {
        errors += usize::from(result.is_err());
    });
    let observed = receive(&mut decoder, second_frame).map(Result::unwrap);
    assert_eq!(errors, 1);
    assert_eq!(observed, Some(second));
}

#[test]
fn discards_overfull_noise_until_a_delimiter() {
    let expected = command(3);
    let mut decoder = FrameDecoder::new();
    decoder.feed(WireKind::Command, &[0], |_| {});
    decoder.feed(WireKind::Command, &[0x55; MAX_COBS_FRAME_BYTES + 4], |_| {});
    decoder.feed(WireKind::Command, &[0], |_| {});

    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
    assert_eq!(decoder.counters().overflows, 1);
}

#[test]
fn credentials_round_trip_without_debugging_the_secret() {
    extern crate std;

    use crate::wifi::NetworkCredentials;

    let credentials = NetworkCredentials::try_new(b"test-network", b"private-password").unwrap();
    assert_eq!(credentials.ssid(), b"test-network");
    assert_eq!(credentials.passphrase(), b"private-password");
    let debug = std::format!("{credentials:?}");
    assert!(!debug.contains("private-password"));

    let expected = Envelope::new(
        7,
        1,
        0,
        1,
        crate::wifi::StartStation(crate::wifi::StationStart {
            credentials,
            power_save: crate::wifi::WifiStationPowerSave::MaxModem { listen_interval: 3 },
        }),
    );
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn access_point_request_round_trips_without_debugging_the_secret() {
    extern crate std;

    use crate::{
        wifi::NetworkCredentials, wifi::NetworkIpv4Configuration, wifi::WifiAccessPointRequest,
        wifi::WifiAccessPointSecurity, wifi::WifiChannelWidth,
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
        Err(crate::wifi::WifiAccessPointRequestError::Channel)
    );
    let debug = std::format!("{request:?}");
    assert!(!debug.contains("private-password"));

    let expected = Envelope::new(7, 1, 0, 2, crate::wifi::StartAccessPoint(request));
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn station_access_point_request_round_trips_as_one_owned_command() {
    use crate::{
        wifi::NetworkCredentials, wifi::NetworkIpv4Configuration, wifi::WifiAccessPointRequest,
        wifi::WifiAccessPointSecurity, wifi::WifiChannelWidth, wifi::WifiStationAccessPointRequest,
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
    let expected = Envelope::new(7, 1, 0, 3, crate::wifi::StartStationAccessPoint(request));
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn asymmetric_bidirectional_session_round_trips() {
    let expected = Envelope::new(
        7,
        2,
        11,
        3,
        crate::network::Configure(SessionConfig {
            network_interface: crate::wifi::WifiNetworkInterface::Station,
            transport: Transport::Udp,
            direction: Direction::Bidirectional,
            completion: Completion::DurationMillis(12_000),
            flows: [
                Some(crate::network::SessionFlowConfig {
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
                    payload_identity: None,
                }),
                None,
            ],
            link_requirements: SessionLinkRequirements::tx_block_ack(0),
        }),
    );
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn two_peer_udp_session_round_trips_without_erasing_flow_identity() {
    let flow = |flow_id, address| {
        Some(crate::network::SessionFlowConfig {
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
            payload_identity: None,
        })
    };
    let expected = Envelope::new(
        7,
        2,
        11,
        3,
        crate::network::Configure(SessionConfig {
            network_interface: crate::wifi::WifiNetworkInterface::AccessPoint,
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
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn stack_usage_query_and_correlated_response_round_trip() {
    let command = Envelope::new(7, 2, 0, 9, crate::system::GetStacks);
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&command).unwrap();
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(command));

    let response = Envelope::new(
        7,
        3,
        0,
        9,
        crate::system::Stacks(StackUsage {
            cpu0_irq: None,
            cpu1_irq: None,
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
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(response));
}

#[test]
fn dedicated_irq_stacks_round_trip_with_explicit_inactive_hart() {
    let watermark = StackWatermark {
        capacity_bytes: 32768,
        free_bytes: 30000,
        used_bytes: 2768,
        minimum_free_bytes: 4096,
    };
    for cpu1 in [None, Some(watermark)] {
        let response = Envelope::new(
            7,
            3,
            0,
            9,
            crate::system::InterruptStacks {
                cpu0: Some(watermark),
                cpu1,
            },
        );
        let mut encoder = FrameEncoder::new();
        let mut decoder = FrameDecoder::new();
        let observed =
            receive(&mut decoder, encoder.encode(&response).unwrap()).map(Result::unwrap);
        assert_eq!(observed, Some(response));
    }
}

#[test]
fn station_beacon_loss_generation_round_trips() {
    let expected = Envelope::new(
        7,
        3,
        0,
        0,
        crate::wifi::StationLifecycle(StationLifecycleEvent::Disconnected {
            generation: 4,
            reason: StationDisconnectReason::BeaconLoss,
        }),
    );
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn connected_station_negotiated_link_round_trips_on_current_version() {
    let expected = Envelope::new(
        7,
        4,
        0,
        0,
        crate::wifi::StationLifecycle(StationLifecycleEvent::Connected {
            generation: 5,
            association_bandwidth_mhz: Some(40),
            security: Some(crate::wifi::StationLinkSecurity::Wpa2Personal {
                management_protection: true,
            }),
        }),
    );
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn station_retry_exhaustion_round_trips_without_text_markers() {
    let expected = Envelope::new(
        7,
        4,
        0,
        0,
        crate::wifi::StationLifecycle(StationLifecycleEvent::RetryExhausted {
            generation: 1,
            attempts: 3,
            stage: StationFailureStage::CandidateSelection,
            reason: StationAttemptFailureReason::NoCandidate,
        }),
    );
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn station_fault_round_trips_with_its_portable_reason() {
    let expected = Envelope::new(
        7,
        4,
        0,
        0,
        crate::wifi::StationLifecycle(StationLifecycleEvent::Faulted {
            generation: 2,
            phase: crate::wifi::StationFaultPhase::Teardown,
            cause: crate::wifi::StationFaultCause::Security,
        }),
    );
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn explicit_wifi_role_transition_round_trips_with_request_identity() {
    let expected = Envelope::new(
        7,
        5,
        0,
        42,
        crate::wifi::RoleTransitioned(WifiRoleTransitionEvidence {
            previous: WifiRole::Station,
            current: WifiRole::Idle,
            generation: 9,
        }),
    );
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn radio_restart_round_trips_rf_outcome_with_request_identity() {
    let expected = Envelope::new(
        7,
        6,
        0,
        43,
        crate::wifi::RadioRestarted(WifiRadioRestartEvidence {
            generation: 10,
            rf: WifiRadioRestartRf::ClosedAndWoken,
        }),
    );
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn maximum_monitor_frame_chunk_fits_and_round_trips() {
    use crate::{
        wifi::WIFI_MONITOR_FRAME_CHUNK_MAX_LEN, wifi::WifiMonitorEvidenceSource,
        wifi::WifiMonitorFrameChunk, wifi::WifiMonitorObserved,
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
    let expected = Envelope::new(9, 3, 0, 77, crate::wifi::MonitorFrame(chunk));
    let mut encoder = FrameEncoder::new();
    let wire = encoder.encode(&expected).unwrap();
    assert!(wire.len() <= MAX_WIRE_FRAME_BYTES);
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, wire).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn control_mailbox_overflow_disconnect_round_trips_on_current_protocol() {
    let expected = Envelope::new(
        7,
        5,
        0,
        43,
        crate::wifi::StationLifecycle(crate::wifi::StationLifecycleEvent::Disconnected {
            generation: 11,
            reason: StationDisconnectReason::ControlMailboxOverflow,
        }),
    );
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn maximum_startup_artifact_chunk_fits_and_round_trips() {
    let bytes = [0x5a; crate::phy::STARTUP_ARTIFACT_CHUNK_MAX_LEN];
    let checksum = startup_artifact_crc32c(&bytes);
    let chunk = StartupArtifactChunk::try_new(
        crate::phy::STARTUP_ARTIFACT_CHUNK_MAX_LEN as u16,
        0,
        checksum,
        &bytes,
    )
    .unwrap();
    let expected = Envelope::new(7, 2, 0, 2, crate::phy::UploadStartupArtifact(chunk));
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    assert!(frame.len() <= MAX_WIRE_FRAME_BYTES);

    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn maximum_rx_delivery_evidence_fits_and_round_trips() {
    use crate::{
        network::EvidenceRecord, network::RxConsumerLedgerEvidence, network::RxDeliveryEvidence,
        network::RxMacOrderEvidence, network::RxReorderDeliveryEvidence,
        network::RxSequenceStageEvidence,
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
            first_forward_gap: Some(crate::network::RxForwardGapEvidence {
                previous_udp: u32::MAX,
                current_udp: u32::MAX,
                tid: u8::MAX,
                previous_mac: u16::MAX,
                current_mac: u16::MAX,
            }),
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
        crate::network::Evidence(EvidenceRecord::RxDelivery(delivery)),
    );
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    assert!(frame.len() <= MAX_WIRE_FRAME_BYTES);
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn maximum_network_scheduler_evidence_fits_and_round_trips() {
    use crate::{network::EvidenceRecord, network::NetworkSchedulerEvidence};

    let expected = Envelope::new(
        7,
        3,
        9,
        2,
        crate::network::Evidence(EvidenceRecord::NetworkScheduler(NetworkSchedulerEvidence {
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
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn maximum_zero_copy_evidence_fits_and_round_trips() {
    use crate::{network::EvidenceRecord, network::RxZeroCopyEvidence};

    let expected = Envelope::new(
        7,
        3,
        9,
        2,
        crate::network::Evidence(EvidenceRecord::RxZeroCopy(RxZeroCopyEvidence {
            cap: u16::MAX,
            adopted: u32::MAX,
            copied_over_cap: u32::MAX,
            copied_unfit: u32::MAX,
            dropped: u32::MAX,
            held_at_end: u16::MAX,
            peak_held: u16::MAX,
        })),
    );
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    assert!(frame.len() <= MAX_WIRE_FRAME_BYTES);
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn maximum_radio_evidence_fits_and_round_trips() {
    use crate::{
        network::EvidenceRecord, network::RadioEvidence, network::RxRadioEvidence,
        network::TxRadioEvidence,
    };

    let expected = Envelope::new(
        7,
        3,
        9,
        2,
        crate::network::Evidence(EvidenceRecord::Radio(RadioEvidence {
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
                station_terminal: crate::network::StationTxTerminalEvidence {
                    exchanges: u32::MAX,
                    mpdus: u32::MAX,
                    acknowledged: u32::MAX,
                    unacknowledged: u32::MAX,
                    ordinary_recovered: u32::MAX,
                    ordinary_failed: u32::MAX,
                    invalid_statuses: u32::MAX,
                },
                bandwidth_mhz: u16::MAX,
                aggregate_rate_kbps: u32::MAX,
                aggregates_prepared: u32::MAX,
                publications_pending_start: u32::MAX,
                publications_pending_end: u32::MAX,
                prepared_histogram: [u32::MAX; 8],
                ..TxRadioEvidence::default()
            }),
        })),
    );
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    assert!(frame.len() <= MAX_WIRE_FRAME_BYTES);
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn maximum_tx_aggregate_timing_evidence_fits_and_round_trips() {
    use crate::{network::EvidenceRecord, network::TxAggregateTimingEvidence};

    let expected = Envelope::new(
        7,
        3,
        9,
        2,
        crate::network::Evidence(EvidenceRecord::TxAggregateTiming(
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
                standby_pending_start: u32::MAX,
                standby_pending_end: u32::MAX,
            },
        )),
    );
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    assert!(frame.len() <= MAX_WIRE_FRAME_BYTES);
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn maximum_flow_transport_evidence_fits_and_round_trips() {
    use crate::{network::EvidenceRecord, network::FlowTransportEvidence};

    let expected = Envelope::new(
        7,
        3,
        9,
        2,
        crate::network::Evidence(EvidenceRecord::FlowTransport(FlowTransportEvidence {
            rx_maximum_silence_micros: Some(u64::MAX),
            flow_id: u8::MAX,
            rx_bytes: u64::MAX,
            tx_bytes: u64::MAX,
            rx_units: u64::MAX,
            tx_units: u64::MAX,
            rx_late_bytes: 0,
            rx_late_units: 0,
            elapsed_micros: u64::MAX,
            transport_errors: u32::MAX,
        })),
    );
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    assert!(frame.len() <= MAX_WIRE_FRAME_BYTES);
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
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
    use crate::{network::EvidenceRecord, network::TransportEvidence};

    let first = EvidenceRecord::Transport(TransportEvidence {
        rx_maximum_silence_micros: None,
        rx_bytes: 1_200,
        tx_bytes: 0,
        rx_units: 1,
        tx_units: 0,
        rx_late_bytes: 0,
        rx_late_units: 0,
        elapsed_micros: 100,
        transport_errors: 0,
    });
    let second = EvidenceRecord::Transport(TransportEvidence {
        rx_maximum_silence_micros: None,
        rx_bytes: 2_400,
        ..match first {
            EvidenceRecord::Transport(evidence) => evidence,
            EvidenceRecord::FlowTransport(_)
            | EvidenceRecord::Radio(_)
            | EvidenceRecord::TxAggregateTiming(_)
            | EvidenceRecord::RxDelivery(_)
            | EvidenceRecord::NetworkScheduler(_)
            | EvidenceRecord::Link(_)
            | EvidenceRecord::Stack(_)
            | EvidenceRecord::RxZeroCopy(_) => unreachable!(),
        }
    });

    assert_eq!(evidence_crc32c(&[first]), evidence_crc32c(&[first]));
    assert_ne!(evidence_crc32c(&[first]), evidence_crc32c(&[second]));
    assert_ne!(
        evidence_crc32c(&[first, second]),
        evidence_crc32c(&[second, first])
    );
}

#[test]
fn ieee802154_air_check_validation_accepts_only_contract_bounds() {
    use crate::{
        ieee802154::IEEE802154_AIR_CHECK_MAX_CYCLES, ieee802154::Ieee802154AirCheckRequest,
    };
    let valid = Ieee802154AirCheckRequest {
        channel: 11,
        cycles: 1,
        energy_scan_micros: 128,
        receive_window_millis: 1,
        scheduled_lead_micros: 1_000,
        scheduled_window_micros: 1_000,
    };
    assert!(valid.validate());
    assert!(
        Ieee802154AirCheckRequest {
            channel: 26,
            cycles: IEEE802154_AIR_CHECK_MAX_CYCLES as u8,
            energy_scan_micros: 1_000_000,
            receive_window_millis: 10_000,
            scheduled_lead_micros: 1_000_000,
            scheduled_window_micros: 1_000_000,
        }
        .validate()
    );
    for request in [
        Ieee802154AirCheckRequest {
            channel: 10,
            ..valid
        },
        Ieee802154AirCheckRequest {
            channel: 27,
            ..valid
        },
        Ieee802154AirCheckRequest { cycles: 0, ..valid },
        Ieee802154AirCheckRequest {
            cycles: IEEE802154_AIR_CHECK_MAX_CYCLES as u8 + 1,
            ..valid
        },
        Ieee802154AirCheckRequest {
            energy_scan_micros: 127,
            ..valid
        },
        Ieee802154AirCheckRequest {
            receive_window_millis: 10_001,
            ..valid
        },
        Ieee802154AirCheckRequest {
            scheduled_lead_micros: 999,
            ..valid
        },
        Ieee802154AirCheckRequest {
            scheduled_window_micros: 999,
            ..valid
        },
        Ieee802154AirCheckRequest {
            scheduled_window_micros: 1_000_001,
            ..valid
        },
    ] {
        assert!(!request.validate());
    }
}

#[test]
fn ieee802154_air_check_command_and_evidence_fit_and_round_trip() {
    use crate::{
        ieee802154::IEEE802154_AIR_CHECK_MAX_CYCLES, ieee802154::Ieee802154AirCcaOutcome,
        ieee802154::Ieee802154AirCheckEvidence, ieee802154::Ieee802154AirCheckRequest,
        ieee802154::Ieee802154AirCheckStop, ieee802154::Ieee802154AirCycle,
        ieee802154::Ieee802154AirEnergyOutcome, ieee802154::Ieee802154AirTransmit,
        ieee802154::Ieee802154AirTxOutcome, ieee802154::Ieee802154AirWindow,
    };
    let transmit = Ieee802154AirTransmit {
        outcome: Ieee802154AirTxOutcome::InvalidAcknowledgement,
        requested_at_micros: u64::MAX,
        done_at_micros: u64::MAX,
    };
    let cycle = Ieee802154AirCycle {
        energy: Ieee802154AirEnergyOutcome::Energy(i8::MIN),
        cca: Ieee802154AirCcaOutcome::Busy,
        direct: transmit,
        scheduled: [transmit; 2],
        received_frames: u16::MAX,
        strongest_rssi_dbm: Some(i8::MIN),
        scheduled_window: Ieee802154AirWindow {
            ended: true,
            start_micros: u64::MAX,
            end_micros: u64::MAX,
            done_at_micros: u64::MAX,
            received_frames: u16::MAX,
            first_frame_at_micros: Some(u64::MAX),
        },
    };
    round_trip(Envelope::new(
        7,
        3,
        9,
        2,
        crate::ieee802154::RunAirCheck(Ieee802154AirCheckRequest {
            channel: 26,
            cycles: 4,
            energy_scan_micros: u32::MAX,
            receive_window_millis: u32::MAX,
            scheduled_lead_micros: u32::MAX,
            scheduled_window_micros: u32::MAX,
        }),
    ));
    round_trip(Envelope::new(
        7,
        3,
        9,
        2,
        crate::ieee802154::AirCheckCompleted(Ieee802154AirCheckEvidence {
            stop: Ieee802154AirCheckStop::Complete,
            completed_cycles: u8::MAX,
            cycles: [cycle; IEEE802154_AIR_CHECK_MAX_CYCLES],
        }),
    ));
}

fn round_trip<T>(message: Envelope<T>)
where
    T: crate::Message + core::fmt::Debug + PartialEq,
{
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&message).unwrap();
    assert!(frame.len() <= MAX_WIRE_FRAME_BYTES);
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(message));
}

#[test]
fn ieee802154_session_messages_at_their_bounds_fit_and_round_trip() {
    use crate::{
        ieee802154::IEEE802154_SESSION_FRAME_CAPACITY,
        ieee802154::IEEE802154_SESSION_RECORDED_FRAMES, ieee802154::Ieee802154AirTxOutcome,
        ieee802154::Ieee802154SessionAck, ieee802154::Ieee802154SessionConfig,
        ieee802154::Ieee802154SessionFrame, ieee802154::Ieee802154SessionPendingMode,
        ieee802154::Ieee802154SessionPendingRequest, ieee802154::Ieee802154SessionReceiveEvidence,
        ieee802154::Ieee802154SessionReceivedFrame, ieee802154::Ieee802154SessionResult,
        ieee802154::Ieee802154SessionTransmitEvidence,
        ieee802154::Ieee802154SessionTransmitRequest, ieee802154::Ieee802154SessionTxMode,
    };
    let full = || {
        let mut frame = Ieee802154SessionFrame::new();
        frame
            .extend_from_slice(&[0xa5; IEEE802154_SESSION_FRAME_CAPACITY])
            .unwrap();
        frame
    };
    let config = Ieee802154SessionConfig {
        channel: 26,
        pan_id: u16::MAX,
        short_address: u16::MAX,
        extended_address: [u8::MAX; 8],
        promiscuous: true,
        maintenance_policy: crate::ieee802154::Ieee802154SessionMaintenancePolicy::Quiesced,
        background_maintenance: true,
        enhanced_ack: true,
        wifi_coexistence: true,
    };
    assert!(config.validate());
    assert!(
        !Ieee802154SessionConfig {
            channel: 10,
            ..config
        }
        .validate()
    );
    assert!(
        !Ieee802154SessionTransmitRequest {
            frame: Ieee802154SessionFrame::from_slice(&[1, 2]).unwrap(),
            mode: Ieee802154SessionTxMode::Direct,
            max_frame_retries: 0,
        }
        .validate()
    );
    round_trip(Envelope::new(
        7,
        3,
        9,
        2,
        crate::ieee802154::StartSession(config),
    ));
    round_trip(Envelope::new(
        7,
        3,
        9,
        2,
        crate::ieee802154::TransmitSession(Ieee802154SessionTransmitRequest {
            frame: full(),
            mode: Ieee802154SessionTxMode::CsmaCa { max_backoffs: 4 },
            max_frame_retries: 15,
        }),
    ));
    round_trip(Envelope::new(7, 3, 9, 2, crate::ieee802154::ReceiveSession));
    round_trip(Envelope::new(7, 3, 9, 2, crate::ieee802154::CollectSession));
    round_trip(Envelope::new(
        7,
        3,
        9,
        2,
        crate::ieee802154::SetSessionPending(Ieee802154SessionPendingRequest {
            mode: Ieee802154SessionPendingMode::Zigbee,
            short_address: Some(u16::MAX),
        }),
    ));
    round_trip(Envelope::new(7, 3, 9, 2, crate::ieee802154::StopSession));
    round_trip(Envelope::new(
        7,
        3,
        9,
        2,
        crate::ieee802154::MaintainSessionPhy,
    ));
    round_trip(Envelope::new(
        7,
        3,
        9,
        2,
        crate::ieee802154::AssessSessionChannel(
            crate::ieee802154::Ieee802154SessionAssessRequest {
                channel: 26,
                energy_scan_micros: 1_000_000,
            },
        ),
    ));
    round_trip(Envelope::new(
        7,
        3,
        9,
        2,
        crate::ieee802154::RestartSessionRadio,
    ));
    round_trip(Envelope::new(
        7,
        3,
        9,
        2,
        crate::ieee802154::ReadSessionRecentRssi,
    ));
    let mut frames = heapless::Vec::new();
    for _ in 0..IEEE802154_SESSION_RECORDED_FRAMES {
        frames
            .push(Ieee802154SessionReceivedFrame {
                length: u8::MAX,
                crc32c: u32::MAX,
                rssi_dbm: i8::MIN,
                lqi: u8::MAX,
            })
            .unwrap();
    }
    round_trip(Envelope::new(
        7,
        3,
        9,
        2,
        crate::ieee802154::SessionStarted(Ieee802154SessionResult::StartFailed),
    ));
    round_trip(Envelope::new(
        7,
        3,
        9,
        2,
        crate::ieee802154::SessionTransmitted(Ieee802154SessionTransmitEvidence {
            result: Ieee802154SessionResult::Done,
            outcome: Ieee802154AirTxOutcome::Success,
            acknowledgement: Some(Ieee802154SessionAck {
                frame: full(),
                rssi_dbm: i8::MIN,
                lqi: u8::MAX,
            }),
        }),
    ));
    round_trip(Envelope::new(
        7,
        3,
        9,
        2,
        crate::ieee802154::SessionReceived(Ieee802154SessionReceiveEvidence {
            result: Ieee802154SessionResult::Done,
            total: u16::MAX,
            frames,
        }),
    ));
    round_trip(Envelope::new(
        7,
        3,
        9,
        2,
        crate::ieee802154::SessionStopped(crate::ieee802154::Ieee802154SessionStopEvidence {
            result: Ieee802154SessionResult::Done,
            maintenance: crate::ieee802154::Ieee802154SessionMaintenanceCounts {
                not_due: u16::MAX,
                tracked: u16::MAX,
                awaiting_other_clients: u16::MAX,
                busy: u16::MAX,
                failed: true,
            },
            coexistence: crate::ieee802154::Ieee802154SessionCoexistence {
                enabled: true,
                disable_failed: true,
            },
        }),
    ));
    round_trip(Envelope::new(
        7,
        3,
        9,
        2,
        crate::ieee802154::SessionPhyMaintained(
            crate::ieee802154::Ieee802154SessionPhyMaintenance::Tracked,
        ),
    ));
    round_trip(Envelope::new(
        7,
        3,
        9,
        2,
        crate::ieee802154::SessionRadioRestarted(
            crate::ieee802154::Ieee802154SessionRestartEvidence {
                result: crate::ieee802154::Ieee802154SessionResult::StopFailed,
                rf_closed: true,
            },
        ),
    ));
    round_trip(Envelope::new(
        7,
        3,
        9,
        2,
        crate::ieee802154::SessionRecentRssi(crate::ieee802154::Ieee802154SessionRecentRssi {
            result: crate::ieee802154::Ieee802154SessionResult::Done,
            rssi_dbm: i8::MIN,
        }),
    ));
    round_trip(Envelope::new(
        7,
        3,
        9,
        2,
        crate::ieee802154::SessionAssessed(crate::ieee802154::Ieee802154SessionAssessment {
            result: crate::ieee802154::Ieee802154SessionResult::Done,
            energy: crate::ieee802154::Ieee802154AirEnergyOutcome::Energy(i8::MIN),
            cca: crate::ieee802154::Ieee802154AirCcaOutcome::Busy,
        }),
    ));
}

#[test]
fn ieee802154_thread_messages_at_their_bounds_fit_and_round_trip() {
    use crate::{
        ieee802154::IEEE802154_THREAD_DATASET_CAPACITY,
        ieee802154::IEEE802154_THREAD_PAYLOAD_CAPACITY,
        ieee802154::IEEE802154_THREAD_RECORDED_DATAGRAMS, ieee802154::Ieee802154SessionResult,
        ieee802154::Ieee802154ThreadDatagram, ieee802154::Ieee802154ThreadDataset,
        ieee802154::Ieee802154ThreadPayload, ieee802154::Ieee802154ThreadReceiveEvidence,
        ieee802154::Ieee802154ThreadRole, ieee802154::Ieee802154ThreadSendRequest,
        ieee802154::Ieee802154ThreadStartRequest, ieee802154::Ieee802154ThreadState,
    };
    let payload = || {
        let mut payload = Ieee802154ThreadPayload::new();
        payload
            .extend_from_slice(&[0xa5; IEEE802154_THREAD_PAYLOAD_CAPACITY])
            .unwrap();
        payload
    };
    let mut dataset = Ieee802154ThreadDataset::new();
    dataset
        .extend_from_slice(&[0x5a; IEEE802154_THREAD_DATASET_CAPACITY])
        .unwrap();
    round_trip(Envelope::new(
        7,
        3,
        9,
        2,
        crate::ieee802154::StartThread(Ieee802154ThreadStartRequest {
            dataset,
            rx_on_when_idle: true,
            udp_port: u16::MAX,
        }),
    ));
    round_trip(Envelope::new(7, 3, 9, 2, crate::ieee802154::GetThread));
    round_trip(Envelope::new(
        7,
        3,
        9,
        2,
        crate::ieee802154::SendThread(Ieee802154ThreadSendRequest {
            destination: [u8::MAX; 16],
            port: u16::MAX,
            payload: payload(),
        }),
    ));
    round_trip(Envelope::new(7, 3, 9, 2, crate::ieee802154::CollectThread));
    round_trip(Envelope::new(7, 3, 9, 2, crate::ieee802154::StopThread));
    let mut datagrams = heapless::Vec::new();
    for _ in 0..IEEE802154_THREAD_RECORDED_DATAGRAMS {
        datagrams
            .push(Ieee802154ThreadDatagram {
                source: [u8::MAX; 16],
                port: u16::MAX,
                payload: payload(),
            })
            .unwrap();
    }
    round_trip(Envelope::new(
        7,
        3,
        9,
        2,
        crate::ieee802154::ThreadStarted(Ieee802154SessionResult::StartFailed),
    ));
    round_trip(Envelope::new(
        7,
        3,
        9,
        2,
        crate::ieee802154::ThreadState(Ieee802154ThreadState {
            result: Ieee802154SessionResult::Done,
            role: Ieee802154ThreadRole::Leader,
            rloc16: u16::MAX,
            mesh_local_eid: Some([u8::MAX; 16]),
        }),
    ));
    round_trip(Envelope::new(
        7,
        3,
        9,
        2,
        crate::ieee802154::ThreadSent(Ieee802154SessionResult::CommandRejected),
    ));
    round_trip(Envelope::new(
        7,
        3,
        9,
        2,
        crate::ieee802154::ThreadReceived(Ieee802154ThreadReceiveEvidence {
            result: Ieee802154SessionResult::EventsLost,
            total: u16::MAX,
            datagrams,
        }),
    ));
    round_trip(Envelope::new(
        7,
        3,
        9,
        2,
        crate::ieee802154::ThreadStopped(Ieee802154SessionResult::StopFailed),
    ));
}

#[test]
fn ieee802154_route_probe_messages_at_their_bounds_fit_and_round_trip() {
    use crate::{
        ieee802154::IEEE802154_ROUTE_PROBE_MAX_ENTRIES, ieee802154::Ieee802154ObservedEventState,
        ieee802154::Ieee802154RouteProbeEntry, ieee802154::Ieee802154RouteProbeEvidence,
        ieee802154::Ieee802154RouteProbeRequest, ieee802154::Ieee802154RouteProbeStop,
        ieee802154::Ieee802154SameBitOutcome,
    };
    let request = Ieee802154RouteProbeRequest {
        threshold_micros: 10_000,
        settle_micros: 100_000,
    };
    assert!(request.validate());
    for invalid in [
        Ieee802154RouteProbeRequest {
            threshold_micros: 0,
            settle_micros: 400,
        },
        Ieee802154RouteProbeRequest {
            threshold_micros: 100,
            settle_micros: 399,
        },
        Ieee802154RouteProbeRequest {
            threshold_micros: 10_001,
            settle_micros: 100_000,
        },
    ] {
        assert!(!invalid.validate(), "{invalid:?}");
    }
    round_trip(Envelope::new(
        7,
        3,
        9,
        2,
        crate::ieee802154::ProbeRoute(request),
    ));
    let entry = Ieee802154RouteProbeEntry {
        snapshot: Ieee802154ObservedEventState::Unclassified,
        before_acknowledgement: Ieee802154ObservedEventState::Timer0AndTimer1,
    };
    let full = || {
        let mut entries = heapless::Vec::new();
        for _ in 0..IEEE802154_ROUTE_PROBE_MAX_ENTRIES {
            entries.push(entry).unwrap();
        }
        entries
    };
    round_trip(Envelope::new(
        7,
        3,
        9,
        2,
        crate::ieee802154::RouteProbed(Ieee802154RouteProbeEvidence {
            stop: Ieee802154RouteProbeStop::RouteFailed,
            polled_snapshot: Ieee802154ObservedEventState::Timer0Only,
            polled_after_acknowledgement: Ieee802154ObservedEventState::Clear,
            polled_control: Ieee802154ObservedEventState::Timer0Only,
            polled_outcome: Ieee802154SameBitOutcome::Retained,
            retrigger_entries: full(),
            same_bit_entries: full(),
            final_events: Ieee802154ObservedEventState::UnexpectedNamed,
        }),
    ));
}

#[test]
fn full_trace_pages_fit_a_frame_and_round_trip() {
    let entry = crate::telemetry::TraceEntry {
        tag: u16::MAX,
        kind: u16::MAX,
        t_us: u32::MAX,
        words: [u32::MAX; 2],
    };
    round_trip(Envelope::new(
        u64::MAX,
        u32::MAX,
        u64::MAX,
        u32::MAX,
        crate::telemetry::TraceEntriesPage(crate::telemetry::TraceEntries {
            first: u16::MAX,
            next: u16::MAX,
            entries: core::iter::repeat_n(entry, crate::telemetry::TRACE_ENTRY_PAGE).collect(),
        }),
    ));
    round_trip(Envelope::new(
        u64::MAX,
        u32::MAX,
        u64::MAX,
        u32::MAX,
        crate::telemetry::TraceSnapshot(Some(crate::telemetry::TraceSnapshotPage {
            slot: u8::MAX,
            point: u16::MAX,
            tag: u16::MAX,
            t_us: u32::MAX,
            len: u16::MAX,
            truncated: true,
            offset: u16::MAX,
            words: core::iter::repeat_n(u32::MAX, crate::telemetry::TRACE_SNAPSHOT_PAGE).collect(),
        })),
    ));
    round_trip(Envelope::new(
        u64::MAX,
        u32::MAX,
        u64::MAX,
        u32::MAX,
        crate::telemetry::TraceState(crate::telemetry::TraceStatus {
            installed: true,
            entries: u16::MAX,
            snapshot_slots: u8::MAX,
            snapshot_words: u16::MAX,
            running: true,
            frozen: true,
            mask: u64::MAX,
            trigger: Some((u16::MAX, u16::MAX)),
            holding_previous: true,
            stored_entries: u16::MAX,
            stored_snapshots: u8::MAX,
        }),
    ));
}

#[test]
fn the_largest_hci_exchange_fits_one_frame_each_way() {
    let mut encoder = FrameEncoder::new();
    let command = Envelope::new(
        u64::MAX,
        u32::MAX,
        u64::MAX,
        u32::MAX,
        crate::bluetooth::ExchangeHci(crate::bluetooth::BluetoothHciRequest::Command {
            opcode: u16::MAX,
            parameters: heapless::Vec::from_slice(
                &[0xa5; crate::bluetooth::BLUETOOTH_HCI_PARAMETER_BYTES],
            )
            .unwrap(),
        }),
    );
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, encoder.encode(&command).unwrap()).map(Result::unwrap);
    assert_eq!(observed, Some(command));
    let event = Envelope::new(
        u64::MAX,
        u32::MAX,
        u64::MAX,
        u32::MAX,
        crate::bluetooth::HciResponse(crate::bluetooth::BluetoothHciResponse::Event {
            packet: heapless::Vec::from_slice(&[0x5a; crate::bluetooth::BLUETOOTH_HCI_EVENT_BYTES])
                .unwrap(),
            dropped: u16::MAX,
        }),
    );
    let frame = encoder.encode(&event).unwrap();
    assert!(frame.len() <= MAX_WIRE_FRAME_BYTES);
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(event));
}

#[test]
fn an_intact_frame_whose_payload_is_not_its_type_is_a_payload_error() {
    // Equal keys mean equal schemas, so only a defective sender produces
    // this: an intact frame whose body is no value of the type it names.
    let expected = command(9);
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    let encoded = &frame[2..frame.len() - 1];
    let mut raw = [0u8; 64];
    raw[..encoded.len()].copy_from_slice(encoded);
    let length = cobs::decode_in_place(&mut raw[..encoded.len()]).unwrap();
    let protected = length - CHECKSUM_BYTES;
    assert_eq!(
        protected,
        WIRE_HEADER_BYTES + 1,
        "a hang target is one variant byte"
    );
    raw[WIRE_HEADER_BYTES] = 0x7f;
    let checksum = CRC32C.checksum(&raw[..protected]).to_le_bytes();
    raw[protected..length].copy_from_slice(&checksum);
    let mut reencoded = [0u8; 80];
    reencoded[1] = 0;
    let written = cobs::encode(&raw[..length], &mut reencoded[2..]);
    let frame = &reencoded[..written + 3];

    let mut decoder = FrameDecoder::new();
    let observed = receive::<InjectHang>(&mut decoder, frame);
    assert_eq!(observed, Some(Err(DecodeError::Payload)));
    assert_eq!(decoder.counters().frames, 1);
}

#[test]
fn a_message_serialized_by_its_producer_frames_like_an_encoded_one() {
    let expected = command(11);
    let mut encoder = FrameEncoder::new();
    let direct = encoder.encode(&expected).unwrap().to_vec();
    let outbound = Outbound::new(
        expected.message_sequence,
        expected.session_id,
        expected.request_id,
        &expected.body,
    )
    .unwrap();
    assert!(outbound.is::<InjectHang>());
    let queued = encoder
        .encode_outbound(expected.boot_id, &outbound)
        .unwrap()
        .to_vec();
    assert_eq!(queued, direct);
}

#[test]
fn a_max_modem_power_save_needs_a_listen_interval() {
    use crate::wifi::WifiStationPowerSave;
    assert!(WifiStationPowerSave::None.is_valid());
    assert!(WifiStationPowerSave::MinModem.is_valid());
    assert!(WifiStationPowerSave::MaxModem { listen_interval: 3 }.is_valid());
    assert!(!WifiStationPowerSave::MaxModem { listen_interval: 0 }.is_valid());
}
