use open_esp_radio_hil_protocol::{
    Capabilities, DecodeCounters, Direction, Envelope, Event, FeatureCapabilities, Finished,
    FlowTransportEvidence, RadioEvidence, ResultSummary, RxRadioEvidence, SessionLinkRequirements,
    SessionReady, StackUsage, StackWatermark, StationLifecycleEvent, TransportEvidence,
};

use super::{
    ProtocolHealth, SessionEvidence, beacon_loss_count_in, command_response_matches,
    next_station_lifecycle_event, session_ready_covers, validate_stack_usage,
};

fn hello(boot_id: u64, message_sequence: u32) -> Envelope<Event> {
    Envelope::new(
        boot_id,
        message_sequence,
        0,
        0,
        Event::Hello(Capabilities {
            features: FeatureCapabilities {
                udp: true,
                tcp: true,
                rx: true,
                tx: true,
                bidirectional: true,
                runtime_initialization: true,
                runtime_configuration: true,
                structured_evidence: true,
                udp_multi_flow: false,
                startup_artifact: true,
                station_epoch_control: true,
                wifi_role_control: true,
                wifi_access_point: true,
                simultaneous_station_access_point: true,
                wifi_monitor_capture: true,
                station_lifecycle_events: true,
                driver_observation_evidence: true,
                rx_delivery_evidence: true,
                task_poll_evidence: false,
                tx_architecture_probe: false,
                core0_rx_cycle_evidence: false,
                mac_irq_evidence: false,
                psram_task_stack: false,
                network_scheduler_evidence: false,
                data_plane_placement: true,
                timebase_probe: true,
                ieee802154_event_status_probe: false,
                ieee802154_ed_event_probe: false,
            },
            maximum_payload_bytes: 1,
            maximum_wire_frame_bytes: 1,
        }),
    )
}

#[test]
fn command_response_requires_boot_session_and_request_identity() {
    let response = Envelope::new(7, 13, 17, 19, Event::Accepted);
    assert!(command_response_matches(&response, 7, 17, 19));
    assert!(!command_response_matches(&response, 8, 17, 19));
    assert!(!command_response_matches(&response, 7, 18, 19));
    assert!(!command_response_matches(&response, 7, 17, 20));
}

fn session_with_rx(rx: RxRadioEvidence) -> SessionEvidence {
    let transport = TransportEvidence {
        rx_bytes: 0,
        tx_bytes: 0,
        rx_units: 0,
        tx_units: 0,
        elapsed_micros: 1,
        transport_errors: 0,
    };
    SessionEvidence {
        transport,
        flow_transport: [
            Some(FlowTransportEvidence::from_session_total(0, transport)),
            None,
        ],
        radio: Some(RadioEvidence {
            rx: Some(rx),
            tx: None,
        }),
        tx_timing: None,
        rx_delivery: None,
        network_scheduler: None,
        egress_policy: None,
        stack: StackUsage {
            cpu0: StackWatermark {
                capacity_bytes: 1,
                free_bytes: 1,
                used_bytes: 0,
                minimum_free_bytes: 1,
            },
            cpu1: StackWatermark {
                capacity_bytes: 1,
                free_bytes: 1,
                used_bytes: 0,
                minimum_free_bytes: 1,
            },
        },
        finished: Finished {
            summary: ResultSummary {
                passed: true,
                evidence_records: 4,
            },
            evidence_crc32c: 0,
        },
    }
}

fn healthy_he_rx() -> RxRadioEvidence {
    RxRadioEvidence {
        phy_format: 4,
        sequence_first: Some(0),
        sequence_highest: Some(99),
        not_s_mpdu_datagrams: 100,
        not_s_mpdu_beacons: 1,
        ampdu_datagrams: 100,
        protocol_ampdu_datagrams: 100,
        reorder_tid: 0,
        reorder_window: 64,
        reorder_first_samples: 1,
        reorder_first_tid: 0,
        reorder_first_start: 7,
        reorder_first_sequence: 9,
        reorder_first_distance: 2,
        reorder_maximum_occupied: 8,
        rx_service_calls: 10,
        rx_frontier_histogram_samples: 10,
        mac_irq_entries: 10,
        mac_irq_classified_entries: 10,
        ..RxRadioEvidence::default()
    }
}

#[test]
fn typed_rx_radio_enforces_order_and_provenance_without_text() {
    assert!(
        session_with_rx(healthy_he_rx())
            .require_rx_radio(4, 100)
            .is_ok()
    );

    let mut reordered = healthy_he_rx();
    reordered.sequence_backward = 1;
    assert!(
        session_with_rx(reordered)
            .require_rx_radio_health(4)
            .is_ok(),
        "performance evidence keeps radio-health guarantees without claiming exact delivery",
    );
    assert!(session_with_rx(reordered).require_rx_radio(4, 100).is_err());

    let mut wrong_provenance = healthy_he_rx();
    wrong_provenance.protocol_ampdu_datagrams = 0;
    wrong_provenance.hardware_ampdu_datagrams = 100;
    assert!(
        session_with_rx(wrong_provenance)
            .require_rx_radio_health(4)
            .is_err()
    );
}

#[test]
fn runtime_stack_policy_rejects_low_headroom_on_either_core() {
    let watermark = StackWatermark {
        capacity_bytes: 16_000,
        free_bytes: 4_000,
        used_bytes: 12_000,
        minimum_free_bytes: 4_000,
    };
    assert!(
        validate_stack_usage(StackUsage {
            cpu0: watermark,
            cpu1: watermark,
        })
        .is_ok()
    );
    let insufficient = StackWatermark {
        minimum_free_bytes: 4_001,
        ..watermark
    };
    assert!(
        validate_stack_usage(StackUsage {
            cpu0: watermark,
            cpu1: insufficient,
        })
        .is_err()
    );
}

#[test]
fn bidirectional_readiness_covers_both_owned_data_planes() {
    assert!(session_ready_covers(
        Direction::Bidirectional,
        SessionReady {
            direction: Direction::Bidirectional,
            tx_block_ack_tid: Some(0),
        },
        Direction::Rx,
        SessionLinkRequirements::tx_block_ack(0),
    ));
    assert!(session_ready_covers(
        Direction::Bidirectional,
        SessionReady {
            direction: Direction::Bidirectional,
            tx_block_ack_tid: Some(0),
        },
        Direction::Tx,
        SessionLinkRequirements::tx_block_ack(0),
    ));
    assert!(session_ready_covers(
        Direction::Bidirectional,
        SessionReady {
            direction: Direction::Rx,
            tx_block_ack_tid: None,
        },
        Direction::Rx,
        SessionLinkRequirements::tx_block_ack(0),
    ));
    assert!(!session_ready_covers(
        Direction::Rx,
        SessionReady {
            direction: Direction::Bidirectional,
            tx_block_ack_tid: None,
        },
        Direction::Rx,
        SessionLinkRequirements::NONE,
    ));
    assert!(!session_ready_covers(
        Direction::Tx,
        SessionReady {
            direction: Direction::Tx,
            tx_block_ack_tid: None,
        },
        Direction::Tx,
        SessionLinkRequirements::tx_block_ack(0),
    ));
}

#[test]
fn target_sequence_discontinuity_is_a_fatal_protocol_error() {
    let mut health = ProtocolHealth::default();
    health.observe(&hello(7, 0), DecodeCounters::default());
    health.observe(
        &Envelope::new(7, 2, 0, 0, Event::Accepted),
        DecodeCounters::default(),
    );
    assert!(
        health
            .failure
            .as_deref()
            .is_some_and(|failure| { failure.contains("expected 1, observed 2") })
    );
}

#[test]
fn a_new_boot_must_restart_its_target_sequence() {
    let mut health = ProtocolHealth::default();
    health.observe(&hello(7, 0), DecodeCounters::default());
    health.observe(
        &Envelope::new(8, 3, 0, 0, Event::Accepted),
        DecodeCounters::default(),
    );
    assert!(
        health
            .failure
            .as_deref()
            .is_some_and(|failure| { failure.contains("boot 8") })
    );
}

#[test]
fn a_valid_new_boot_clears_previous_boot_failure() {
    let mut health = ProtocolHealth::default();
    health.observe(&hello(7, 0), DecodeCounters::default());
    health.observe(
        &Envelope::new(7, 2, 0, 0, Event::Accepted),
        DecodeCounters::default(),
    );
    assert!(health.failure.is_some());
    health.observe(&hello(8, 0), DecodeCounters::default());
    assert_eq!(health.boot_id, Some(8));
    assert_eq!(health.failure, None);
}

#[test]
fn lifecycle_cursor_ignores_events_from_the_previous_boot() {
    let messages = [
        Envelope::new(
            7,
            1,
            0,
            0,
            Event::StationLifecycle(StationLifecycleEvent::RetryExhausted {
                generation: 1,
                attempts: 3,
                stage: open_esp_radio_hil_protocol::StationFailureStage::CandidateSelection,
                reason: open_esp_radio_hil_protocol::StationAttemptFailureReason::NoCandidate,
            }),
        ),
        Envelope::new(
            8,
            1,
            0,
            0,
            Event::StationLifecycle(StationLifecycleEvent::Connected { generation: 0 }),
        ),
    ];
    let mut cursor = 0;
    assert_eq!(
        next_station_lifecycle_event(&messages, &mut cursor, 8),
        Some(StationLifecycleEvent::Connected { generation: 0 })
    );
    assert_eq!(cursor, 2);
}

#[test]
fn beacon_loss_qualification_ignores_previous_boots() {
    let mut messages = vec![
        hello(7, 0),
        Envelope::new(
            7,
            1,
            0,
            0,
            Event::StationLifecycle(StationLifecycleEvent::Disconnected {
                generation: 0,
                reason: open_esp_radio_hil_protocol::StationDisconnectReason::BeaconLoss,
            }),
        ),
        hello(8, 0),
        Envelope::new(
            8,
            1,
            0,
            0,
            Event::StationLifecycle(StationLifecycleEvent::Connected { generation: 0 }),
        ),
    ];
    assert_eq!(beacon_loss_count_in(&messages), 0);

    messages.push(Envelope::new(
        8,
        2,
        0,
        0,
        Event::StationLifecycle(StationLifecycleEvent::Disconnected {
            generation: 0,
            reason: open_esp_radio_hil_protocol::StationDisconnectReason::BeaconLoss,
        }),
    ));
    assert_eq!(beacon_loss_count_in(&messages), 1);
}
