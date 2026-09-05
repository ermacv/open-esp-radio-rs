use super::*;
use open_esp_radio_hil_protocol::{
    Finished, RadioEvidence, ResultSummary, RxConsumerLedgerEvidence, RxDeliveryEvidence,
    RxRadioEvidence, RxSequenceStageEvidence, StackUsage, StackWatermark, TransportEvidence,
};

fn evidence(rx_bytes: u64, tx_bytes: u64, rx_units: u64, tx_units: u64) -> SessionEvidence {
    let transport = TransportEvidence {
        rx_bytes,
        tx_bytes,
        rx_units,
        tx_units,
        elapsed_micros: 1,
        transport_errors: 0,
    };
    SessionEvidence {
        link: open_esp_radio_hil_protocol::LinkHealth {
            rx_frames: 1,
            rx_cobs_errors: 0,
            rx_checksum_errors: 0,
            rx_decode_errors: 0,
            rx_overflows: 0,
            tx_frames: 1,
            tx_dropped: 0,
            text_dropped: 0,
            text_truncated: 0,
        },
        finished: Finished {
            summary: ResultSummary {
                passed: true,
                evidence_records: 0,
            },
            evidence_crc32c: 0,
        },
        transport,
        flow_transport: [
            Some(
                open_esp_radio_hil_protocol::FlowTransportEvidence::from_session_total(
                    0, transport,
                ),
            ),
            None,
        ],
        radio: (rx_units != 0).then_some(RadioEvidence {
            rx: Some(RxRadioEvidence {
                sequence_first: Some(0),
                sequence_highest: u32::try_from(rx_units)
                    .ok()
                    .and_then(|units| units.checked_sub(1)),
                ..RxRadioEvidence::default()
            }),
            tx: None,
        }),
        tx_timing: None,
        rx_delivery: None,
        network_scheduler: None,
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
    }
}

#[test]
fn udp_exact_delivery_checks_both_directions() {
    let sent = UdpTransmission {
        source: Ipv4Addr::LOCALHOST,
        bytes: 1_200,
        datagrams: 1,
        elapsed: Duration::from_secs(1),
        maximum_lateness: Duration::ZERO,
        maximum_catch_up_datagrams: 1,
        deadline_resets: 0,
    };
    let received = Burst {
        bytes: 1_200,
        datagrams: 1,
        started_at_zero: true,
        ..Burst::default()
    };
    assert!(
        validate_udp(
            Some(sent),
            Some(&[received]),
            evidence(1_200, 1_200, 1, 1),
            UdpEvidencePolicy {
                exact_delivery: true,
                driver_observation: true,
                rx_delivery: false,
            },
        )
        .is_ok()
    );
}

#[test]
fn udp_ordering_defect_fails_closed() {
    let received = Burst {
        bytes: 1_200,
        datagrams: 1,
        missing: 1,
        started_at_zero: true,
        ..Burst::default()
    };
    assert!(
        validate_udp(
            None,
            Some(&[received]),
            evidence(0, 1_200, 0, 1),
            UdpEvidencePolicy {
                exact_delivery: true,
                driver_observation: true,
                rx_delivery: false,
            },
        )
        .is_err()
    );
}

#[test]
fn udp_target_rx_reordering_fails_closed_even_when_counts_match() {
    let sent = UdpTransmission {
        source: Ipv4Addr::LOCALHOST,
        bytes: 2_400,
        datagrams: 2,
        elapsed: Duration::from_secs(1),
        maximum_lateness: Duration::ZERO,
        maximum_catch_up_datagrams: 1,
        deadline_resets: 0,
    };
    let mut reordered = evidence(2_400, 0, 2, 0);
    reordered
        .radio
        .as_mut()
        .and_then(|radio| radio.rx.as_mut())
        .expect("RX evidence")
        .sequence_backward = 1;

    assert!(
        validate_udp(
            Some(sent),
            None,
            reordered,
            UdpEvidencePolicy {
                exact_delivery: true,
                driver_observation: true,
                rx_delivery: false,
            },
        )
        .is_err()
    );
}

#[test]
fn udp_characterization_reports_receiver_delivery_and_allows_loss() {
    let sent = UdpTransmission {
        source: Ipv4Addr::LOCALHOST,
        bytes: 2_400,
        datagrams: 2,
        elapsed: Duration::from_secs(1),
        maximum_lateness: Duration::ZERO,
        maximum_catch_up_datagrams: 1,
        deadline_resets: 0,
    };
    let received = Burst {
        bytes: 1_200,
        datagrams: 1,
        missing: 1,
        started_at_zero: true,
        ..Burst::default()
    };
    let delivered = validate_udp(
        Some(sent),
        Some(&[received]),
        evidence(1_200, 2_400, 1, 2),
        UdpEvidencePolicy {
            exact_delivery: false,
            driver_observation: true,
            rx_delivery: false,
        },
    )
    .unwrap();

    assert_eq!(delivered, Some(received));
}

#[test]
fn observer_free_udp_rx_accepts_transport_without_driver_evidence() {
    let sent = UdpTransmission {
        source: Ipv4Addr::LOCALHOST,
        bytes: 1_200,
        datagrams: 1,
        elapsed: Duration::from_secs(1),
        maximum_lateness: Duration::ZERO,
        maximum_catch_up_datagrams: 1,
        deadline_resets: 0,
    };
    let mut observed = evidence(1_200, 0, 1, 0);
    observed.radio = None;

    assert!(
        validate_udp(
            Some(sent),
            None,
            observed,
            UdpEvidencePolicy {
                exact_delivery: false,
                driver_observation: false,
                rx_delivery: false,
            },
        )
        .is_ok()
    );
}

#[test]
fn rx_delivery_diagnostic_fails_closed_without_publication_evidence() {
    let sent = UdpTransmission {
        source: Ipv4Addr::LOCALHOST,
        bytes: 1_200,
        datagrams: 1,
        elapsed: Duration::from_secs(1),
        maximum_lateness: Duration::ZERO,
        maximum_catch_up_datagrams: 1,
        deadline_resets: 0,
    };

    let error = validate_udp(
        Some(sent),
        None,
        evidence(1_200, 0, 1, 0),
        UdpEvidencePolicy {
            exact_delivery: true,
            driver_observation: true,
            rx_delivery: true,
        },
    )
    .expect_err("diagnostic evidence must be mandatory");

    assert!(error.to_string().contains("typed RX delivery evidence"));
}

#[test]
fn rx_delivery_diagnostic_rejects_evidence_from_the_wrong_publication_edge() {
    let sent = UdpTransmission {
        source: Ipv4Addr::LOCALHOST,
        bytes: 1_200,
        datagrams: 1,
        elapsed: Duration::from_secs(1),
        maximum_lateness: Duration::ZERO,
        maximum_catch_up_datagrams: 1,
        deadline_resets: 0,
    };
    let mut observed = evidence(1_200, 0, 1, 0);
    observed.rx_delivery = Some(RxDeliveryEvidence {
        udp_consumer: RxSequenceStageEvidence {
            data_units: 1,
            first: Some(0),
            highest: Some(0),
            ..Default::default()
        },
        consumer_ledger: RxConsumerLedgerEvidence {
            unexpected_consumer: 1,
            first_observed: Some(0),
            ..Default::default()
        },
        ..Default::default()
    });

    let error = validate_udp(
        Some(sent),
        None,
        observed,
        UdpEvidencePolicy {
            exact_delivery: true,
            driver_observation: true,
            rx_delivery: true,
        },
    )
    .expect_err("misplaced diagnostic evidence must fail closed");

    assert!(error.to_string().contains("typed AP RX delivery frontier"));
}

#[test]
fn tcp_exact_delivery_checks_both_directions() {
    let sent = TcpTransmission {
        bytes: 8_192,
        writes: 1,
        elapsed: Duration::from_secs(1),
        maximum_lateness: Duration::ZERO,
        maximum_catch_up_writes: 1,
        deadline_resets: 0,
    };
    let received = TcpReception {
        bytes: 8_192,
        reads: 1,
        elapsed: Duration::from_secs(1),
        pattern_errors: 0,
        eof: true,
    };
    assert!(validate_tcp(Some(sent), Some(received), evidence(8_192, 8_192, 1, 1)).is_ok());
}

#[test]
fn parses_normal_and_sub_millisecond_ping_samples() {
    assert_eq!(
        ping_sample_micros("64 bytes from 1.2.3.4: time=1.25 ms")
            .unwrap()
            .unwrap(),
        1_250
    );
    assert_eq!(
        ping_sample_micros("64 bytes from 1.2.3.4: time<1 ms")
            .unwrap()
            .unwrap(),
        1_000
    );
}

#[test]
fn ap_icmp_uses_nearest_rank_percentiles() {
    let samples = (1..=100).map(|sample| sample * 10).collect::<Vec<_>>();

    assert_eq!(percentile_micros(&samples, 50), 500);
    assert_eq!(percentile_micros(&samples, 95), 950);
    assert_eq!(percentile_micros(&samples, 99), 990);
}

#[test]
fn ap_rate_gate_checks_combined_bidirectional_throughput() {
    let report = SessionReport {
        direction: Direction::Bidirectional,
        rx_bytes: 2_000_000,
        tx_bytes: 2_000_000,
        rx_units: 0,
        tx_units: 0,
        elapsed_micros: 1_000_000,
    };
    let mut criteria = Criteria {
        minimum_combined_bps: Some(32_000_000),
        ..Criteria::default()
    };
    assert!(validate_rate_criteria(&report, &criteria).is_ok());
    criteria.minimum_combined_bps = Some(32_000_001);
    assert!(validate_rate_criteria(&report, &criteria).is_err());
}

#[test]
fn sparse_secondary_tx_requires_enough_packets_and_bounded_interarrival() {
    let flow = |flow_id, tx_units, maximum_interarrival_us| MultiClientFlowReport {
        flow_id,
        peer: Ipv4Endpoint {
            address: [10, 43, 0, flow_id.saturating_add(2)],
            port: 9_002 + u16::from(flow_id),
        },
        rx_bytes: 0,
        tx_bytes: tx_units * 1_472,
        rx_units: 0,
        tx_units,
        elapsed_micros: 22_000_000,
        rx_bps: 0,
        tx_bps: tx_units * 1_472 * 8_000_000 / 22_000_000,
        host_tx_started_at_zero: Some(true),
        host_tx_missing: Some(0),
        host_tx_reordered: Some(0),
        host_tx_duplicates: Some(0),
        host_tx_maximum_interarrival_us: Some(maximum_interarrival_us),
        host_tx_sequence_after_maximum_interarrival: Some(2),
    };
    let criteria = Criteria {
        minimum_secondary_tx_datagrams: Some(8),
        maximum_secondary_tx_interarrival_ms: Some(5_500),
        ..Criteria::default()
    };

    assert!(
        validate_multi_client_fairness(
            &[flow(0, 100, 100), flow(1, 10, 5_003_241)],
            Direction::Tx,
            &criteria,
        )
        .is_ok()
    );
    assert!(
        validate_multi_client_fairness(
            &[flow(0, 100, 100), flow(1, 7, 5_003_241)],
            Direction::Tx,
            &criteria,
        )
        .unwrap_err()
        .to_string()
        .contains("delivered 7 datagrams")
    );
    assert!(
        validate_multi_client_fairness(
            &[flow(0, 100, 100), flow(1, 10, 5_500_001)],
            Direction::Tx,
            &criteria,
        )
        .unwrap_err()
        .to_string()
        .contains("inter-arrival")
    );
}

#[test]
fn ap_ht40_mcs7_gate_is_directional_and_fails_closed() {
    let link = Some(LinkExpectation {
        phy: PhyExpectation::Ht40,
        minimum_mcs: Some(7),
        guard_interval: crate::scenario::HtGuardIntervalExpectation::Any,
    });
    let rx = AccessPointTraffic::Udp {
        direction: Direction::Rx,
        duration_seconds: 1,
        rx_rate_bps: Some(1),
        tx_rate_bps: None,
        payload_bytes: 1,
    };
    let tx = AccessPointTraffic::Udp {
        direction: Direction::Tx,
        duration_seconds: 1,
        rx_rate_bps: None,
        tx_rate_bps: Some(1),
        payload_bytes: 1,
    };
    let mut observed = open_esp_radio_hil_protocol::WifiAccessPointEvidence::default();
    assert!(validate_mcs_evidence(&rx, link, &observed).is_err());
    observed.rx_ht_data_frames = 1;
    observed.rx_ht40_mcs_frames[7] = 1;
    assert!(validate_mcs_evidence(&rx, link, &observed).is_ok());
    assert!(validate_mcs_evidence(&tx, link, &observed).is_err());
    observed.tx_ht_aggregates = 1;
    observed.tx_ht40_mcs7_aggregates = 1;
    assert!(validate_mcs_evidence(&tx, link, &observed).is_ok());
}

#[test]
fn ap_guard_interval_gate_tolerates_only_epoch_warmup_frames() {
    let link = Some(LinkExpectation {
        phy: PhyExpectation::Ht40,
        minimum_mcs: Some(7),
        guard_interval: HtGuardIntervalExpectation::Short,
    });
    let rx = AccessPointTraffic::Udp {
        direction: Direction::Rx,
        duration_seconds: 1,
        rx_rate_bps: Some(1),
        tx_rate_bps: None,
        payload_bytes: 1,
    };
    let mut observed = open_esp_radio_hil_protocol::WifiAccessPointEvidence {
        rx_ht_data_frames: 100,
        rx_ht40_short_gi_frames: 99,
        rx_ht40_long_gi_frames: 1,
        ..Default::default()
    };
    observed.rx_ht40_mcs_frames[7] = 100;
    assert!(validate_mcs_evidence(&rx, link, &observed).is_ok());

    observed.rx_ht40_short_gi_frames = 98;
    observed.rx_ht40_long_gi_frames = 2;
    assert!(validate_mcs_evidence(&rx, link, &observed).is_err());
}

#[test]
fn ap_hardware_rx_health_uses_terminal_mac_counters() {
    let mut observed = open_esp_radio_hil_protocol::WifiAccessPointEvidence::default();
    assert!(validate_rx_hardware_health(0, &observed).is_ok());

    observed.rx_hardware.buffer_full = 1;
    let error = validate_rx_hardware_health(2, &observed)
        .unwrap_err()
        .to_string();
    assert!(error.contains("cycle 2"));
    assert!(error.contains("buffer_full=1"));

    observed.rx_hardware.buffer_full = 0;
    observed.rx_hardware.fifo_overflow = 1;
    assert!(validate_rx_hardware_health(3, &observed).is_err());
}

#[test]
fn functional_ap_observation_does_not_require_optional_pipeline_report() {
    let observed = open_esp_radio_hil_protocol::WifiAccessPointEvidence {
        beacons_transmitted: 1,
        authentication_responses: 1,
        association_responses: 1,
        authorized_peers: 1,
        maximum_associated_peers: 1,
        maximum_authorized_peers: 1,
        peer_removals: 1,
        wpa2_response_windows: 2,
        disassociations_prepared: 1,
        disassociations_published: 1,
        disassociations_acknowledged: 1,
        deauthentications_prepared: 1,
        deauthentications_published: 1,
        deauthentications_acknowledged: 1,
        completed_rx_units: 0,
        completed_rx_descriptors: 0,
        recycled_rx_descriptors: 0,
        ..Default::default()
    };

    assert!(
        validate_access_point_observation(0, WifiAccessPointSecurity::Wpa2Personal, 1, &observed,)
            .is_ok()
    );

    let mut open = observed;
    open.wpa2_response_windows = 0;
    assert!(validate_access_point_observation(0, WifiAccessPointSecurity::Open, 1, &open).is_ok());
}

#[test]
fn performance_cycle_omits_unavailable_driver_observation() {
    let report = AccessPointReport {
        schema: ACCESS_POINT_REPORT_SCHEMA,
        fixture_preparation: None,
        boots: vec![BootReport {
            boot: 0,
            cycles: vec![CycleReport {
                cycle: 0,
                traffic: TrafficReport::None,
                secondary_client: None,
                primary_client_link: None,
                secondary_client_link: None,
                independent_air: None,
                access_point: None,
            }],
        }],
    };

    let json = serde_json::to_value(report).unwrap();
    assert_eq!(json["schema"], ACCESS_POINT_REPORT_SCHEMA);
    assert!(json["fixture_preparation"].is_null());
    assert!(json["boots"][0]["cycles"][0]["independent_air"].is_null());
    assert!(
        json["boots"][0]["cycles"][0]
            .get("independent_air_rx")
            .is_none()
    );
    assert!(json["boots"][0]["cycles"][0].get("access_point").is_none());
}
