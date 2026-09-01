use super::*;

#[test]
fn repository_catalog_is_valid_and_unique() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    let catalog = Catalog::load(&root).unwrap();
    assert!(catalog.all().len() >= 8);
    assert!(catalog.get("udp-rx-he20-ceiling").is_ok());
    assert!(catalog.get("tcp-rx-ht40-split").is_ok());
    assert!(catalog.get("icmp-latency-ht40-split").is_ok());
    assert!(catalog.get("station-reconnect-ht40").is_ok());
    assert_eq!(
        catalog
            .get("udp-rx-ht40-performance-ceiling")
            .unwrap()
            .criteria
            .maximum_idle_channel_utilization_255,
        Some(64)
    );
    for id in [
        "udp-rx-ht40-ceiling",
        "udp-rx-ht40-performance-ceiling",
        "udp-tx-ht40-ceiling",
        "udp-tx-ht40-performance-ceiling",
    ] {
        let scenario = catalog.get(id).unwrap();
        let Workload::Udp {
            rx_rate_bps,
            tx_rate_bps,
            ..
        } = scenario.workload
        else {
            panic!("{id} must remain a UDP workload")
        };
        assert_eq!(
            rx_rate_bps.unwrap_or(0) + tx_rate_bps.unwrap_or(0),
            130_000_000,
            "{id}",
        );
    }
    let tx_task_poll = catalog
        .get("udp-tx-ht40-split-task-poll-diagnostic")
        .unwrap();
    assert_eq!(
        tx_task_poll.data_plane,
        WifiDataPlanePlacement::SplitRadioNetwork
    );
    assert_eq!(tx_task_poll.image, ImageClass::DiagnosticTaskPoll);
    assert_eq!(
        tx_task_poll.criteria.maximum_idle_channel_utilization_255,
        Some(64)
    );
    assert_eq!(
        catalog
            .get("udp-bidirectional-ht40-split-baseline")
            .unwrap()
            .data_plane,
        WifiDataPlanePlacement::SplitRadioNetwork
    );
    assert_eq!(
        catalog
            .get("udp-bidirectional-ht40-split-baseline")
            .unwrap()
            .repetitions,
        5
    );
    assert!(catalog.get("access-point-rx").is_ok());
    assert!(catalog.get("access-point-tx").is_ok());
    assert!(catalog.get("access-point-bidirectional").is_ok());
    assert!(
        catalog
            .all()
            .iter()
            .all(|scenario| scenario.data_plane == WifiDataPlanePlacement::SplitRadioNetwork)
    );
    for id in [
        "access-point-load-rx",
        "access-point-load-tx",
        "access-point-load-bidirectional",
    ] {
        let scenario = catalog.get(id).unwrap();
        assert_eq!(scenario.repetitions, 5);
        assert_eq!(scenario.criteria.minimum_concurrent_ap_clients, Some(2));
    }
}

#[test]
fn ieee802154_event_status_diagnostic_is_exclusive_and_bounded() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    let catalog = Catalog::load(&root).unwrap();
    let scenario = catalog
        .get("ieee802154-event-status-selective-ack")
        .unwrap();
    assert_eq!(scenario.image, ImageClass::DiagnosticIeee802154EventStatus);
    assert!(matches!(
        scenario.workload,
        Workload::Ieee802154EventStatus {
            boots: 3,
            poll_limit: 100_000,
            timer_threshold: 1_000,
        }
    ));
    assert!(scenario.link.is_none());

    let mut wrong_image = scenario.clone();
    wrong_image.image = ImageClass::Correctness;
    assert!(wrong_image.validate().is_err());

    let mut wrong_workload = scenario.clone();
    wrong_workload.workload = Workload::Timebase {
        boots: 1,
        intervals: 2,
        period_millis: 1,
    };
    assert!(wrong_workload.validate().is_err());

    for workload in [
        Workload::Ieee802154EventStatus {
            boots: 0,
            poll_limit: 1,
            timer_threshold: 1,
        },
        Workload::Ieee802154EventStatus {
            boots: 1,
            poll_limit: 0,
            timer_threshold: 1,
        },
        Workload::Ieee802154EventStatus {
            boots: 1,
            poll_limit: 1,
            timer_threshold: 0,
        },
        Workload::Ieee802154EventStatus {
            boots: 21,
            poll_limit: 1,
            timer_threshold: 1,
        },
        Workload::Ieee802154EventStatus {
            boots: 1,
            poll_limit: 1_000_001,
            timer_threshold: 1,
        },
        Workload::Ieee802154EventStatus {
            boots: 1,
            poll_limit: 1,
            timer_threshold: 1_001,
        },
    ] {
        let mut out_of_bounds = scenario.clone();
        out_of_bounds.workload = workload;
        assert!(out_of_bounds.validate().is_err());
    }

    let mut with_network_criteria = scenario.clone();
    with_network_criteria.criteria.exact_delivery = true;
    assert!(with_network_criteria.validate().is_err());

    let mut with_external_evidence = scenario.clone();
    with_external_evidence.evidence.openwrt_tx_monitor_rx = true;
    assert!(with_external_evidence.validate().is_err());
}

#[test]
fn ieee802154_ed_event_diagnostic_is_exclusive_and_bounded() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    let catalog = Catalog::load(&root).unwrap();
    let scenario = catalog.get("ieee802154-ed-event-selective-write").unwrap();
    assert_eq!(scenario.image, ImageClass::DiagnosticIeee802154EdEvent);
    assert!(matches!(
        scenario.workload,
        Workload::Ieee802154EdEvent {
            boots: 3,
            poll_limit: 100_000,
            timer_threshold: 1_000,
        }
    ));
    assert!(scenario.link.is_none());

    let mut wrong_image = scenario.clone();
    wrong_image.image = ImageClass::Correctness;
    assert!(wrong_image.validate().is_err());

    let mut out_of_bounds = scenario.clone();
    out_of_bounds.workload = Workload::Ieee802154EdEvent {
        boots: 1,
        poll_limit: 1_000_001,
        timer_threshold: 1,
    };
    assert!(out_of_bounds.validate().is_err());

    let mut with_network_criteria = scenario.clone();
    with_network_criteria.criteria.exact_delivery = true;
    assert!(with_network_criteria.validate().is_err());
}

#[test]
fn performance_catalog_is_observer_free_and_covers_both_roles() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    let catalog = Catalog::load(&root).unwrap();
    let performance = catalog
        .all()
        .iter()
        .filter(|scenario| scenario.image == ImageClass::Performance)
        .collect::<Vec<_>>();
    assert!(
        performance.iter().any(|scenario| matches!(
            scenario.workload,
            Workload::Udp { .. } | Workload::Tcp { .. } | Workload::Icmp { .. }
        )),
        "performance catalog must exercise the station data plane",
    );
    assert!(
        performance
            .iter()
            .any(|scenario| matches!(scenario.workload, Workload::AccessPoint { .. })),
        "performance catalog must exercise the access-point data plane",
    );
    for scenario in performance {
        assert!(!scenario.criteria.exact_delivery, "{}", scenario.id);
        assert!(!scenario.criteria.require_no_beacon_loss, "{}", scenario.id);
        assert!(
            scenario.link.is_none_or(|link| link.minimum_mcs.is_none()),
            "{}",
            scenario.id
        );
        assert!(!scenario.evidence.openwrt_tx_monitor_rx, "{}", scenario.id);
        assert!(
            !scenario.evidence.independent_laptop_monitor_rx,
            "{}",
            scenario.id
        );
        assert_eq!(
            scenario.fixture_mutation.openwrt_fixed_guard_interval,
            HtGuardIntervalExpectation::Any,
            "{}",
            scenario.id,
        );
        assert_eq!(
            scenario.fixture_mutation.openwrt_client_fixed_ht_mcs, None,
            "{}",
            scenario.id,
        );
        assert_eq!(
            scenario
                .fixture_mutation
                .openwrt_client_fixed_guard_interval,
            HtGuardIntervalExpectation::Any,
            "{}",
            scenario.id,
        );
    }
}

#[test]
fn link_guard_interval_observation_is_separate_from_fixed_gi_mutation() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    let catalog = Catalog::load(&root).unwrap();
    let auto = catalog
        .get("udp-rx-ht40-rate-mask-auto-air-diagnostic")
        .unwrap();
    let lgi = catalog
        .get("udp-rx-ht40-rate-mask-lgi-air-diagnostic")
        .unwrap();
    assert_eq!(
        auto.fixture_mutation.openwrt_fixed_guard_interval,
        HtGuardIntervalExpectation::Any,
    );
    assert_eq!(
        lgi.link.unwrap().guard_interval,
        HtGuardIntervalExpectation::Long,
    );
    assert_eq!(
        lgi.fixture_mutation.openwrt_fixed_guard_interval,
        HtGuardIntervalExpectation::Long,
    );

    let mut mismatch = lgi.clone();
    mismatch.fixture_mutation.openwrt_fixed_guard_interval = HtGuardIntervalExpectation::Short;
    assert!(mismatch.validate().is_err());

    let mut unobserved = lgi.clone();
    unobserved.evidence.independent_laptop_monitor_rx = false;
    assert!(unobserved.validate().is_err());
}

#[test]
fn per_frame_phy_requirements_cannot_be_silently_accepted_by_residence_image() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    let catalog = Catalog::load(&root).unwrap();
    let mut residence = catalog
        .get("udp-rx-ht40-task-poll-diagnostic")
        .unwrap()
        .clone();
    residence.link.as_mut().unwrap().minimum_mcs = Some(7);
    assert!(
        residence
            .validate()
            .unwrap_err()
            .to_string()
            .contains("per-frame MCS/GI")
    );

    let mut observed = residence;
    observed.image = ImageClass::DiagnosticCore0RxCycles;
    let Workload::Udp {
        duration_seconds, ..
    } = &mut observed.workload
    else {
        panic!("residence scenario must remain UDP")
    };
    *duration_seconds = CORE0_RX_CYCLE_MAX_DURATION_SECONDS;
    observed.validate().unwrap();
}

#[test]
fn core0_cycle_image_cannot_overrun_its_u32_accumulators() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    let catalog = Catalog::load(&root).unwrap();
    let mut scenario = catalog
        .get("udp-rx-ht40-core0-rx-cycles-diagnostic")
        .unwrap()
        .clone();
    let Workload::Udp {
        duration_seconds, ..
    } = &mut scenario.workload
    else {
        panic!("Core0 cycle scenario must remain UDP")
    };
    *duration_seconds = CORE0_RX_CYCLE_MAX_DURATION_SECONDS + 1;
    assert!(
        scenario
            .validate()
            .unwrap_err()
            .to_string()
            .contains("u32-safe")
    );
}

#[test]
fn deferred_ready_admission_is_a_same_image_core0_control() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    let catalog = Catalog::load(&root).unwrap();
    let deferred = catalog
        .get("udp-rx-ht40-core0-rx-deferred-ready-diagnostic")
        .unwrap();
    let synchronous = catalog
        .get("udp-rx-ht40-core0-rx-cycles-diagnostic")
        .unwrap();
    assert_eq!(deferred.image, synchronous.image);
    assert_eq!(deferred.workload, synchronous.workload);
    assert_eq!(deferred.link, synchronous.link);
    assert_eq!(deferred.evidence, synchronous.evidence);
    assert_eq!(
        deferred.rx_admission,
        WifiRxAdmissionPolicy::DeferredReadyDiagnostic
    );
    assert_eq!(
        synchronous.rx_admission,
        WifiRxAdmissionPolicy::SynchronousShared
    );

    let mut wrong_image = deferred.clone();
    wrong_image.image = ImageClass::DiagnosticTaskPoll;
    assert!(wrong_image.validate().is_err());
}

#[test]
fn l1_cache_counter_control_is_a_same_image_core0_control() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    let catalog = Catalog::load(&root).unwrap();
    let enabled = catalog
        .get("udp-rx-ht40-core0-rx-cycles-diagnostic")
        .unwrap();
    let disabled = catalog
        .get("udp-rx-ht40-core0-rx-cache-off-diagnostic")
        .unwrap();

    assert!(enabled.l1_cache_counters);
    assert!(!disabled.l1_cache_counters);
    assert_eq!(enabled.image, disabled.image);
    assert_eq!(enabled.isolation, disabled.isolation);
    assert_eq!(enabled.data_plane, disabled.data_plane);
    assert_eq!(enabled.rx_checksum, disabled.rx_checksum);
    assert_eq!(enabled.rx_admission, disabled.rx_admission);
    assert_eq!(enabled.workload, disabled.workload);
    assert_eq!(enabled.link, disabled.link);
    assert_eq!(enabled.criteria, disabled.criteria);
    assert_eq!(enabled.evidence, disabled.evidence);

    let mut wrong_image = enabled.clone();
    wrong_image.image = ImageClass::DiagnosticTaskPoll;
    assert!(wrong_image.validate().is_err());
}

#[test]
fn ap_egress_control_l1_probe_is_a_same_image_runtime_control() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    let catalog = Catalog::load(&root).unwrap();
    let enabled = catalog
        .get("diagnostic-ap-single-client-tx-egress-control-l1-cache-core0")
        .unwrap();
    let disabled = catalog
        .get("diagnostic-ap-single-client-tx-egress-control-disabled-l1-cache-core0")
        .unwrap();

    assert_eq!(enabled.image, ImageClass::DiagnosticCore0RxCoarse);
    assert_eq!(enabled.image, disabled.image);
    assert!(enabled.l1_cache_counters);
    assert_eq!(enabled.l1_cache_counters, disabled.l1_cache_counters);
    assert_eq!(enabled.workload, disabled.workload);
    assert_eq!(enabled.link, disabled.link);
    assert_eq!(enabled.criteria, disabled.criteria);
    assert_eq!(enabled.evidence, disabled.evidence);
    assert_eq!(enabled.tx_buffer, WifiTxBufferPolicy::DirectDma);
    assert_eq!(
        disabled.tx_buffer,
        WifiTxBufferPolicy::DirectDmaEgressControlDisabledDiagnostic
    );
}

#[test]
fn ap_egress_control_task_poll_probe_omits_intrusive_control_counters() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    let catalog = Catalog::load(&root).unwrap();
    let enabled = catalog
        .get("access-point-single-client-ceiling-tx-task-poll")
        .unwrap();
    let disabled = catalog
        .get("diagnostic-ap-single-client-tx-egress-control-disabled-task-poll")
        .unwrap();

    assert_eq!(enabled.image, ImageClass::DiagnosticTaskPoll);
    assert_eq!(enabled.image, disabled.image);
    assert_eq!(enabled.workload, disabled.workload);
    assert_eq!(enabled.link, disabled.link);
    assert_eq!(enabled.criteria, disabled.criteria);
    assert_eq!(enabled.evidence, disabled.evidence);
    assert_eq!(enabled.tx_buffer, WifiTxBufferPolicy::DirectDma);
    assert_eq!(
        disabled.tx_buffer,
        WifiTxBufferPolicy::DirectDmaEgressControlDisabledDiagnostic
    );

    let enabled = catalog
        .get("access-point-single-client-ceiling-tx-task-residence")
        .unwrap();
    let disabled = catalog
        .get("diagnostic-ap-single-client-tx-egress-control-disabled-task-residence")
        .unwrap();

    assert_eq!(enabled.image, ImageClass::DiagnosticTaskResidence);
    assert_eq!(enabled.image, disabled.image);
    assert_eq!(enabled.workload, disabled.workload);
    assert_eq!(enabled.link, disabled.link);
    assert_eq!(enabled.criteria, disabled.criteria);
    assert_eq!(enabled.evidence, disabled.evidence);
    assert_eq!(enabled.tx_buffer, WifiTxBufferPolicy::DirectDma);
    assert_eq!(
        disabled.tx_buffer,
        WifiTxBufferPolicy::DirectDmaEgressControlDisabledDiagnostic
    );
}

#[test]
fn checksum_control_changes_only_the_runtime_checksum_policy() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    let catalog = Catalog::load(&root).unwrap();
    let software = catalog
        .get("udp-rx-ht40-task-poll-software-checksum-diagnostic")
        .unwrap();
    let assume_valid = catalog
        .get("udp-rx-ht40-task-poll-no-rx-checksum-diagnostic")
        .unwrap();

    assert_eq!(software.rx_checksum, WifiRxChecksumPolicy::Software);
    assert_eq!(
        assume_valid.rx_checksum,
        WifiRxChecksumPolicy::AssumeValidDiagnostic
    );
    assert_eq!(software.image, assume_valid.image);
    assert_eq!(software.isolation, assume_valid.isolation);
    assert_eq!(software.data_plane, assume_valid.data_plane);
    assert_eq!(software.rx_admission, assume_valid.rx_admission);
    assert_eq!(software.rx_dispatch, assume_valid.rx_dispatch);
    assert_eq!(software.rx_continuation, assume_valid.rx_continuation);
    assert_eq!(software.l1_cache_counters, assume_valid.l1_cache_counters);
    assert_eq!(software.workload, assume_valid.workload);
    assert_eq!(software.link, assume_valid.link);
    assert_eq!(software.criteria, assume_valid.criteria);
    assert_eq!(software.evidence, assume_valid.evidence);
    assert_eq!(software.fixture_mutation, assume_valid.fixture_mutation);
}

#[test]
fn tx_checksum_control_changes_only_the_runtime_checksum_policy() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    let catalog = Catalog::load(&root).unwrap();
    let software = catalog.get("udp-tx-ht40-core0-coarse-diagnostic").unwrap();
    let omit = catalog
        .get("udp-tx-ht40-no-udp-checksum-diagnostic")
        .unwrap();

    assert_eq!(software.tx_udp_checksum, WifiTxUdpChecksumPolicy::Software);
    assert_eq!(
        omit.tx_udp_checksum,
        WifiTxUdpChecksumPolicy::OmitIpv4Diagnostic
    );
    assert_eq!(software.image, omit.image);
    assert_eq!(software.isolation, omit.isolation);
    assert_eq!(software.data_plane, omit.data_plane);
    assert_eq!(software.rx_checksum, omit.rx_checksum);
    assert_eq!(software.rx_admission, omit.rx_admission);
    assert_eq!(software.rx_dispatch, omit.rx_dispatch);
    assert_eq!(software.rx_continuation, omit.rx_continuation);
    assert_eq!(software.l1_cache_counters, omit.l1_cache_counters);
    assert_eq!(software.workload, omit.workload);
    assert_eq!(software.link, omit.link);
    assert_eq!(software.criteria, omit.criteria);
    assert_eq!(software.evidence, omit.evidence);
    assert_eq!(software.fixture_mutation, omit.fixture_mutation);

    let mut wrong_direction = omit.clone();
    let Workload::Udp { direction, .. } = &mut wrong_direction.workload else {
        panic!("TX checksum control must remain UDP")
    };
    *direction = Direction::Rx;
    assert!(wrong_direction.validate().is_err());

    let mut wrong_image = omit.clone();
    wrong_image.image = ImageClass::Performance;
    assert!(wrong_image.validate().is_err());
}

#[test]
fn ap_ht40_ceiling_separates_performance_from_observed_correctness() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    let catalog = Catalog::load(&root).unwrap();
    for id in [
        "access-point-single-client-ceiling-rx",
        "access-point-single-client-ceiling-tx",
        "access-point-single-client-ceiling-bidirectional",
    ] {
        let scenario = catalog.get(id).unwrap();
        assert_eq!(scenario.repetitions, 3, "{id}");
        assert_eq!(scenario.link.unwrap().phy, PhyExpectation::Ht40, "{id}");
        assert_eq!(scenario.image, ImageClass::Performance, "{id}");
        assert!(!scenario.criteria.require_no_beacon_loss, "{id}");
        assert_eq!(scenario.link.unwrap().minimum_mcs, None, "{id}");
        assert!(
            matches!(
                scenario.workload,
                Workload::AccessPoint {
                    client: AccessPointClient::OpenWrt,
                    ..
                }
            ),
            "{id} must use OpenWrt as the controlled RF peer",
        );
        assert!(
            scenario.tags.iter().any(|tag| tag == "qualification"),
            "{id}"
        );
    }

    let bidirectional = catalog
        .get("access-point-single-client-ceiling-bidirectional")
        .unwrap();
    let offered = |scenario: &Scenario| match scenario.workload {
        Workload::AccessPoint {
            traffic:
                AccessPointTraffic::Udp {
                    rx_rate_bps,
                    tx_rate_bps,
                    ..
                },
            ..
        } => rx_rate_bps.unwrap_or(0) + tx_rate_bps.unwrap_or(0),
        _ => panic!("AP ceiling must remain a UDP workload"),
    };
    for id in [
        "access-point-single-client-ceiling-rx",
        "access-point-single-client-ceiling-tx",
        "access-point-single-client-ceiling-bidirectional",
        "access-point-single-client-ceiling-tx-diagnostic",
        "access-point-single-client-ceiling-tx-task-poll",
        "access-point-single-client-ceiling-bidirectional-task-poll",
    ] {
        assert_eq!(offered(catalog.get(id).unwrap()), 130_000_000, "{id}");
    }
    assert_eq!(bidirectional.criteria.minimum_rx_bps, Some(40_000_000));
    assert_eq!(bidirectional.criteria.minimum_tx_bps, Some(40_000_000));
    assert_eq!(
        bidirectional.criteria.minimum_combined_bps,
        Some(80_000_000)
    );

    let icmp = catalog.get("access-point-icmp").unwrap();
    assert_eq!(icmp.image, ImageClass::Correctness);
    assert_eq!(icmp.repetitions, 3);
    assert!(icmp.criteria.require_no_beacon_loss);
    assert_eq!(icmp.link.unwrap().minimum_mcs, None);
    assert!(matches!(
        icmp.workload,
        Workload::AccessPoint {
            traffic: AccessPointTraffic::Icmp { count: 100, .. },
            ..
        }
    ));
    assert_eq!(icmp.criteria.maximum_lost, Some(0));
    assert_eq!(icmp.criteria.maximum_p95_ms, Some(20));
}

#[test]
fn ap_rx_core0_control_uses_the_production_dispatch_and_continuation_policy() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    let catalog = Catalog::load(&root).unwrap();
    let scenario = catalog
        .get("access-point-single-client-ceiling-rx-core0-coarse")
        .unwrap();

    assert_eq!(scenario.image, ImageClass::DiagnosticCore0RxCoarse);
    assert_eq!(
        scenario.rx_dispatch,
        WifiRxDispatchPolicy::DirectImmediateDiagnostic
    );
    assert_eq!(
        scenario.rx_continuation,
        WifiRxContinuationPolicy::AdaptiveProbeDiagnostic
    );
    assert!(is_rx_only_udp_workload(&scenario.workload));

    let mut bidirectional = scenario.clone();
    let Workload::AccessPoint {
        traffic: AccessPointTraffic::Udp { direction, .. },
        ..
    } = &mut bidirectional.workload
    else {
        panic!("AP Core0 control must remain a UDP workload")
    };
    *direction = Direction::Bidirectional;
    assert!(bidirectional.validate().is_err());

    let cycles = catalog
        .get("access-point-single-client-ceiling-rx-core0-cycles")
        .unwrap();
    assert_eq!(cycles.image, ImageClass::DiagnosticCore0RxCycles);
    assert_eq!(cycles.rx_dispatch, scenario.rx_dispatch);
    assert_eq!(cycles.rx_continuation, scenario.rx_continuation);
    assert_eq!(cycles.workload, scenario.workload);
}

#[test]
fn ap_staged_promotion_phase_probe_uses_the_coarse_core0_image() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    let catalog = Catalog::load(&root).unwrap();
    let scenario = catalog
        .get("diagnostic-ap-two-client-udp-tx-promotion-phases")
        .unwrap();

    assert_eq!(scenario.image, ImageClass::DiagnosticCore0RxCoarse);
    assert_eq!(
        scenario.tx_buffer,
        WifiTxBufferPolicy::PsramStagingCopyDiagnostic
    );
    assert!(matches!(
        scenario.workload,
        Workload::AccessPoint {
            traffic: AccessPointTraffic::UdpMultiClient {
                direction: Direction::Tx,
                ..
            },
            ..
        }
    ));
}

#[test]
fn ap_egress_burst_probe_keeps_direct_dma_in_the_task_residence_image() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    let catalog = Catalog::load(&root).unwrap();
    let scenario = catalog
        .get("diagnostic-ap-two-client-udp-tx-egress-burst")
        .unwrap();

    assert_eq!(scenario.image, ImageClass::DiagnosticTaskResidence);
    assert_eq!(
        scenario.tx_buffer,
        WifiTxBufferPolicy::DirectDmaEgressBurstDiagnostic
    );
    assert!(matches!(
        scenario.workload,
        Workload::AccessPoint {
            traffic: AccessPointTraffic::UdpMultiClient {
                direction: Direction::Tx,
                ..
            },
            ..
        }
    ));
}

#[test]
fn ap_two_client_checksum_probe_is_a_coarse_tx_only_diagnostic() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    let catalog = Catalog::load(&root).unwrap();
    let scenario = catalog
        .get("diagnostic-ap-two-client-udp-tx-no-checksum-core0")
        .unwrap();

    assert_eq!(scenario.image, ImageClass::DiagnosticCore0RxCoarse);
    assert_eq!(
        scenario.tx_udp_checksum,
        WifiTxUdpChecksumPolicy::OmitIpv4Diagnostic
    );
    assert!(matches!(
        scenario.workload,
        Workload::AccessPoint {
            traffic: AccessPointTraffic::UdpMultiClient {
                direction: Direction::Tx,
                ..
            },
            ..
        }
    ));
}

#[test]
fn ap_sparse_peer_probe_keeps_two_packet_pacing_and_cpu_observation() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    let catalog = Catalog::load(&root).unwrap();
    let scenario = catalog
        .get("diagnostic-ap-saturated-plus-sparse-tx-core0")
        .unwrap();

    assert_eq!(scenario.image, ImageClass::DiagnosticCore0RxCoarse);
    assert_eq!(
        scenario.data_plane,
        WifiDataPlanePlacement::SplitRadioNetwork
    );
    assert_eq!(scenario.criteria.minimum_secondary_tx_datagrams, Some(8));
    assert_eq!(
        scenario.criteria.maximum_secondary_tx_interarrival_ms,
        Some(5_500)
    );
    assert!(matches!(
        scenario.workload,
        Workload::AccessPoint {
            traffic: AccessPointTraffic::UdpMultiClient {
                direction: Direction::Tx,
                tx_rate_bps_per_flow: Some(130_000_000),
                secondary_tx_rate_bps: Some(4_710),
                secondary_tx_pacing_group_datagrams: Some(2),
                ..
            },
            ..
        }
    ));

    let residence = catalog
        .get("diagnostic-ap-saturated-plus-sparse-tx-task-residence")
        .unwrap();
    assert_eq!(residence.image, ImageClass::DiagnosticTaskResidence);
    assert_eq!(residence.workload, scenario.workload);
    assert_eq!(residence.criteria, scenario.criteria);

    let performance = catalog
        .get("access-point-saturated-plus-sparse-tx")
        .unwrap();
    assert_eq!(performance.image, ImageClass::Performance);
    assert_eq!(performance.workload, scenario.workload);
    assert!(!performance.criteria.exact_delivery);
    assert_eq!(performance.criteria.minimum_tx_bps, Some(105_000_000));
    assert_eq!(performance.criteria.minimum_bps_per_flow, Some(1));
    assert_eq!(performance.criteria.minimum_secondary_tx_datagrams, Some(8));
    assert_eq!(
        performance.criteria.maximum_secondary_tx_interarrival_ms,
        Some(5_500)
    );
    assert_eq!(performance.repetitions, 3);
}

#[test]
fn ap_single_client_load_matches_the_station_split_core_baseline() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    let catalog = Catalog::load(&root).unwrap();
    let ap = catalog
        .get("access-point-single-client-load-bidirectional")
        .unwrap();
    let station = catalog
        .get("udp-bidirectional-ht40-split-baseline")
        .unwrap();

    let offered = |scenario: &Scenario| match scenario.workload {
        Workload::AccessPoint {
            traffic:
                AccessPointTraffic::Udp {
                    direction: Direction::Bidirectional,
                    rx_rate_bps,
                    tx_rate_bps,
                    ..
                },
            ..
        } => (rx_rate_bps, tx_rate_bps),
        Workload::Udp {
            direction: Direction::Bidirectional,
            rx_rate_bps: Some(rx_rate_bps),
            tx_rate_bps: Some(tx_rate_bps),
            ..
        } => (Some(rx_rate_bps), Some(tx_rate_bps)),
        _ => panic!("station-level comparison requires bidirectional UDP"),
    };

    assert_eq!(offered(ap), offered(station));
    assert_eq!(ap.criteria.minimum_rx_bps, station.criteria.minimum_rx_bps);
    assert_eq!(ap.criteria.minimum_tx_bps, station.criteria.minimum_tx_bps);
    assert_eq!(
        ap.criteria.minimum_combined_bps,
        station.criteria.minimum_combined_bps
    );
    assert_eq!(ap.image, ImageClass::Performance);
}

#[test]
fn ht40_matrix_covers_seven_balanced_ninety_megabit_points() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    let catalog = Catalog::load(&root).unwrap();
    let mut rates = catalog
        .all()
        .iter()
        .filter(|scenario| scenario.tags.iter().any(|tag| tag == "ht40-matrix-90"))
        .map(|scenario| match scenario.workload {
            Workload::Udp {
                direction: Direction::Bidirectional,
                rx_rate_bps: Some(rx),
                tx_rate_bps: Some(tx),
                ..
            } => (rx, tx),
            _ => panic!("matrix tag belongs to a non-bidirectional UDP scenario"),
        })
        .collect::<Vec<_>>();
    rates.sort_unstable();
    assert!(
        catalog
            .all()
            .iter()
            .filter(|scenario| scenario.tags.iter().any(|tag| tag == "ht40-matrix-90"))
            .all(|scenario| {
                !scenario.criteria.exact_delivery
                    && scenario.data_plane == WifiDataPlanePlacement::SplitRadioNetwork
                    && scenario.repetitions == 3
            })
    );
    assert_eq!(
        rates,
        vec![
            (15_000_000, 75_000_000),
            (25_000_000, 65_000_000),
            (35_000_000, 55_000_000),
            (45_000_000, 45_000_000),
            (55_000_000, 35_000_000),
            (65_000_000, 25_000_000),
            (75_000_000, 15_000_000),
        ]
    );
}

#[test]
fn scenario_files_cannot_contain_lab_secrets() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|extension| extension != "toml") {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap().to_ascii_lowercase();
        for forbidden in ["ssid", "passphrase", "password", "ssh_target", "serial"] {
            assert!(
                !text.contains(forbidden),
                "{} contains {forbidden}",
                path.display()
            );
        }
    }
}

#[test]
fn access_point_direction_requires_matching_rates() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    let catalog = Catalog::load(&root).unwrap();
    let scenario = catalog.get("access-point-rx").unwrap();
    assert!(validate_direction_rates(Direction::Rx, Some(1), None, scenario).is_ok());
    assert!(validate_direction_rates(Direction::Rx, None, None, scenario).is_err());
    assert!(validate_direction_rates(Direction::Tx, None, Some(1), scenario).is_ok());
    assert!(validate_direction_rates(Direction::Tx, Some(1), Some(1), scenario).is_err());
    assert!(validate_direction_rates(Direction::Bidirectional, Some(1), Some(1), scenario).is_ok());
    assert!(validate_direction_rates(Direction::Bidirectional, Some(1), None, scenario).is_err());
}
