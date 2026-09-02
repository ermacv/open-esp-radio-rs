//! Dispatch from validated scenario workloads to their concrete host owners.

use crate::*;

#[derive(Default)]
pub(crate) struct ExecutionEvidence {
    pub(crate) measurements: Vec<reporting::run::Measurement>,
    pub(crate) failure: Option<String>,
}

pub(crate) fn execute_workload(
    lab: &transport::lab_config::LabConfig,
    selected: &qualification::scenario::Scenario,
    output: &Path,
) -> ExecutionEvidence {
    execute_workload_inner(lab, selected, output).unwrap_or_else(|error| ExecutionEvidence {
        measurements: Vec::new(),
        failure: Some(error.to_string()),
    })
}

fn execute_workload_inner(
    lab: &transport::lab_config::LabConfig,
    selected: &qualification::scenario::Scenario,
    output: &Path,
) -> Result<ExecutionEvidence> {
    use qualification::scenario::{Direction, Workload};

    lab.set_data_plane(selected.data_plane);
    lab.set_rx_checksum(selected.rx_checksum);
    lab.set_tx_udp_checksum(selected.tx_udp_checksum);
    lab.set_tx_buffer(selected.tx_buffer);
    lab.set_rx_admission(selected.rx_admission);
    lab.set_rx_dispatch(selected.rx_dispatch);
    lab.set_rx_continuation(selected.rx_continuation);
    lab.set_l1_cache_counters(selected.l1_cache_counters);

    // Ordinary station workloads own their AP fixture for the complete run.
    // Loss/absence and Wi-Fi-role workloads manage that lifetime internally;
    // target-AP and timebase workloads must not materialize a station AP.
    let _station_ap = matches!(
        &selected.workload,
        Workload::Udp { .. }
            | Workload::Tcp { .. }
            | Workload::Icmp { .. }
            | Workload::StationReconnect { .. }
            | Workload::StationAccessPoint { .. }
    )
    .then(|| {
        transport::controlled_ap::ControlledAp::start(
            &lab.station,
            &lab.station_fixture,
            selected
                .link
                .expect("validated station workload has a link expectation")
                .phy,
        )
    })
    .transpose()?;

    let result = match &selected.workload {
        Workload::BootSmoke => boot_smoke(output, lab),
        Workload::Timebase {
            boots,
            intervals,
            period_millis,
        } => transport::timebase::run(
            transport::timebase::Config {
                boots: *boots,
                intervals: *intervals,
                period_millis: *period_millis,
            },
            output,
            lab,
        ),
        Workload::Ieee802154EventStatus {
            boots,
            poll_limit,
            timer_threshold,
        } => transport::ieee802154_event_status::run(
            transport::ieee802154_event_status::Config {
                boots: *boots,
                poll_limit: *poll_limit,
                timer_threshold: *timer_threshold,
            },
            output,
            lab,
        ),
        Workload::Ieee802154EdEvent {
            boots,
            poll_limit,
            timer_threshold,
        } => transport::ieee802154_ed_event::run(
            transport::ieee802154_ed_event::Config {
                boots: *boots,
                poll_limit: *poll_limit,
                timer_threshold: *timer_threshold,
            },
            output,
            lab,
        ),
        Workload::Udp {
            direction,
            duration_seconds,
            rx_rate_bps,
            tx_rate_bps,
            payload_bytes,
        } => {
            let mut arguments = Vec::new();
            push_option(&mut arguments, "--seconds", duration_seconds);
            push_option(&mut arguments, "--payload", payload_bytes);
            push_option(
                &mut arguments,
                "--phy",
                selected
                    .link
                    .expect("validated station workload has a link expectation")
                    .phy
                    .id(),
            );
            match direction {
                Direction::Rx => {
                    let link = selected
                        .link
                        .expect("validated station workload has a link expectation");
                    push_option(
                        &mut arguments,
                        "--rate",
                        rx_rate_bps.expect("validated RX rate"),
                    );
                    if let Some(floor) = selected.criteria.minimum_rx_bps {
                        push_option(&mut arguments, "--floor", floor);
                    }
                    if let Some(maximum) = selected.criteria.maximum_idle_channel_utilization_255 {
                        push_option(
                            &mut arguments,
                            "--max-idle-channel-utilization-255",
                            maximum,
                        );
                    }
                    traffic::rx_traffic::run(
                        arguments,
                        output,
                        lab,
                        traffic::rx_traffic::EvidencePolicy {
                            require_exact_delivery: selected.criteria.exact_delivery,
                            require_no_beacon_loss: selected.criteria.require_no_beacon_loss,
                            require_driver_observation: selected
                                .image
                                .requires_driver_observation(),
                            capture_openwrt_tx_monitor: selected.evidence.openwrt_tx_monitor_rx,
                            capture_independent_laptop_monitor: selected
                                .evidence
                                .independent_laptop_air_monitor,
                            minimum_mcs: link.minimum_mcs,
                            guard_interval: link.guard_interval,
                            fixture_guard_interval: selected
                                .fixture_mutation
                                .openwrt_fixed_guard_interval,
                        },
                    )
                }
                Direction::Tx => {
                    if let Some(rate) = tx_rate_bps {
                        push_option(&mut arguments, "--rate", rate);
                    }
                    if let Some(floor) = selected.criteria.minimum_tx_bps {
                        push_option(&mut arguments, "--floor", floor);
                    }
                    if let Some(maximum) = selected.criteria.maximum_idle_channel_utilization_255 {
                        push_option(
                            &mut arguments,
                            "--max-idle-channel-utilization-255",
                            maximum,
                        );
                    }
                    traffic::tx_traffic::run(
                        arguments,
                        output,
                        lab,
                        selected.criteria.exact_delivery,
                        selected.criteria.require_no_beacon_loss,
                        selected.image.requires_driver_observation(),
                    )
                }
                Direction::Bidirectional => {
                    let link = selected
                        .link
                        .expect("validated station workload has a link expectation");
                    push_option(
                        &mut arguments,
                        "--rate",
                        rx_rate_bps.expect("validated RX rate"),
                    );
                    if let Some(rate) = tx_rate_bps {
                        push_option(&mut arguments, "--tx-rate", rate);
                    }
                    if let Some(floor) = selected.criteria.minimum_tx_bps {
                        push_option(&mut arguments, "--tx-floor", floor);
                    }
                    if let Some(floor) = selected.criteria.minimum_rx_bps {
                        push_option(&mut arguments, "--rx-floor", floor);
                    }
                    if let Some(floor) = selected.criteria.minimum_combined_bps {
                        push_option(&mut arguments, "--combined-floor", floor);
                    }
                    traffic::bidirectional::run(
                        arguments,
                        output,
                        lab,
                        traffic::bidirectional::RunPolicy {
                            require_exact_delivery: selected.criteria.exact_delivery,
                            require_no_beacon_loss: selected.criteria.require_no_beacon_loss,
                            capture_openwrt_tx_monitor_rx: selected.evidence.openwrt_tx_monitor_rx,
                            capture_independent_laptop_air_monitor: selected
                                .evidence
                                .independent_laptop_air_monitor,
                            require_driver_observation: selected
                                .image
                                .requires_driver_observation(),
                            minimum_mcs: link.minimum_mcs,
                            guard_interval: link.guard_interval,
                            fixture_guard_interval: selected
                                .fixture_mutation
                                .openwrt_fixed_guard_interval,
                        },
                    )
                }
            }
        }
        Workload::Tcp {
            direction,
            duration_seconds,
            rx_rate_bps,
            tx_rate_bps,
            chunk_bytes,
        } => {
            let mut arguments = Vec::new();
            push_option(&mut arguments, "--seconds", duration_seconds);
            push_option(&mut arguments, "--chunk", chunk_bytes);
            if let Some(rate) = rx_rate_bps {
                push_option(&mut arguments, "--rx-rate", rate);
            }
            if let Some(rate) = tx_rate_bps {
                push_option(&mut arguments, "--tx-rate", rate);
            }
            if let Some(floor) = selected.criteria.minimum_tx_bps {
                push_option(&mut arguments, "--tx-floor", floor);
            }
            if let Some(floor) = selected.criteria.minimum_rx_bps {
                push_option(&mut arguments, "--rx-floor", floor);
            }
            match direction {
                Direction::Rx => traffic::tcp_traffic::run_rx(
                    arguments,
                    output,
                    lab,
                    selected.criteria.require_no_beacon_loss,
                ),
                Direction::Tx => traffic::tcp_traffic::run_tx(
                    arguments,
                    output,
                    lab,
                    selected.criteria.require_no_beacon_loss,
                ),
                Direction::Bidirectional => traffic::tcp_traffic::run_bidirectional(
                    arguments,
                    output,
                    lab,
                    selected.criteria.require_no_beacon_loss,
                ),
            }
        }
        Workload::Icmp {
            count,
            interval_ms,
            timeout_ms,
            payload_bytes,
        } => {
            let mut arguments = Vec::new();
            push_option(&mut arguments, "--count", count);
            push_option(&mut arguments, "--interval-ms", interval_ms);
            push_option(&mut arguments, "--timeout-ms", timeout_ms);
            push_option(&mut arguments, "--payload", payload_bytes);
            if let Some(maximum) = selected.criteria.maximum_lost {
                push_option(&mut arguments, "--max-lost", maximum);
            }
            if let Some(maximum) = selected.criteria.maximum_p95_ms {
                push_option(&mut arguments, "--max-p95-ms", maximum);
            }
            let evidence = traffic::icmp_latency::run(
                arguments,
                output,
                lab,
                selected.criteria.require_no_beacon_loss,
            )?;
            return Ok(ExecutionEvidence {
                measurements: evidence.measurements,
                failure: evidence.acceptance_failure,
            });
        }
        Workload::StationReconnect {
            cycles,
            boots,
            timeout_seconds,
        } => {
            let mut arguments = Vec::new();
            push_option(&mut arguments, "--cycles", cycles);
            push_option(&mut arguments, "--boots", boots);
            push_option(&mut arguments, "--timeout-seconds", timeout_seconds);
            qualification::station_lifecycle::run(
                arguments,
                output,
                lab,
                selected.criteria.require_no_beacon_loss,
            )
        }
        Workload::StationApLoss { timeout_seconds } => {
            let mut arguments = Vec::new();
            push_option(&mut arguments, "--timeout-seconds", timeout_seconds);
            qualification::station_ap_loss::run(
                arguments,
                output,
                lab,
                selected
                    .link
                    .expect("validated AP-loss workload has a link expectation")
                    .phy,
            )
        }
        Workload::StationApAbsence { timeout_seconds } => {
            let mut arguments = Vec::new();
            push_option(&mut arguments, "--timeout-seconds", timeout_seconds);
            qualification::station_ap_absence::run(
                arguments,
                output,
                lab,
                selected
                    .link
                    .expect("validated AP-absence workload has a link expectation")
                    .phy,
            )
        }
        Workload::WifiRole {
            operation,
            timeout_seconds,
            channel,
            dwell_seconds,
            snapshot_length,
        } => {
            let mut arguments = Vec::new();
            push_option(&mut arguments, "--timeout-seconds", timeout_seconds);
            if let Some(channel) = channel {
                push_option(&mut arguments, "--channel", channel);
            }
            if let Some(seconds) = dwell_seconds {
                push_option(&mut arguments, "--monitor-seconds", seconds);
            }
            if let Some(length) = snapshot_length {
                push_option(&mut arguments, "--snapshot-length", length);
            }
            transport::wifi_control::run(
                operation.id(),
                arguments,
                output,
                lab,
                selected
                    .link
                    .expect("validated Wi-Fi role has a link expectation")
                    .phy,
            )
        }
        Workload::MonitorCapture {
            timeout_seconds,
            duration_seconds,
            channel,
            snapshot_length,
        } => {
            let mut arguments = Vec::new();
            push_option(
                &mut arguments,
                "--output",
                output.join("capture.pcapng").display(),
            );
            push_option(&mut arguments, "--timeout-seconds", timeout_seconds);
            push_option(&mut arguments, "--seconds", duration_seconds);
            if let Some(channel) = channel {
                push_option(&mut arguments, "--channel", channel);
            }
            push_option(&mut arguments, "--snapshot-length", snapshot_length);
            transport::wifi_capture::run(
                arguments,
                output,
                lab,
                selected
                    .link
                    .expect("validated monitor capture has a link expectation")
                    .phy,
            )
        }
        Workload::AccessPoint {
            cycles,
            boots,
            timeout_seconds,
            client,
            security,
            traffic,
        } => qualification::access_point::run(
            qualification::access_point::Config {
                cycles: *cycles,
                boots: *boots,
                timeout: std::time::Duration::from_secs(u64::from(*timeout_seconds)),
                client: *client,
                security: *security,
                traffic: traffic.clone(),
                criteria: selected.criteria.clone(),
                expected_link: selected.link,
                require_driver_observation: selected.image.requires_driver_observation(),
                require_rx_delivery_evidence: selected.image
                    == qualification::scenario::ImageClass::DiagnosticRxDelivery,
                capture_independent_laptop_air_monitor: selected
                    .evidence
                    .independent_laptop_air_monitor,
                openwrt_client_fixed_ht_mcs: selected.fixture_mutation.openwrt_client_fixed_ht_mcs,
                openwrt_client_fixed_guard_interval: selected
                    .fixture_mutation
                    .openwrt_client_fixed_guard_interval,
            },
            output,
            lab,
        ),
        Workload::StationAccessPoint {
            timeout_seconds,
            duration_seconds,
            direction,
            rate_bps_per_flow,
            minimum_bps_per_flow,
            maximum_fairness_skew_percent,
            payload_bytes,
        } => qualification::station_access_point::run(
            qualification::station_access_point::Config {
                timeout: std::time::Duration::from_secs(u64::from(*timeout_seconds)),
                duration: std::time::Duration::from_secs(u64::from(*duration_seconds)),
                direction: *direction,
                rate_bps_per_flow: *rate_bps_per_flow,
                minimum_bps_per_flow: *minimum_bps_per_flow,
                maximum_fairness_skew_percent: *maximum_fairness_skew_percent,
                payload_bytes: usize::from(*payload_bytes),
                require_driver_observation: selected.image.requires_driver_observation(),
                require_egress_policy_evidence: selected.criteria.require_egress_policy_evidence,
                maximum_egress_unused_grant_percent: selected
                    .criteria
                    .maximum_egress_unused_grant_percent,
                maximum_egress_progress_without_grant: selected
                    .criteria
                    .maximum_egress_progress_without_grant,
                capture_independent_laptop_air_monitor: selected
                    .evidence
                    .independent_laptop_air_monitor,
            },
            output,
            lab,
        ),
        Workload::StationAccessPointReconnect { timeout_seconds } => {
            qualification::station_access_point_reconnect::run(
                std::time::Duration::from_secs(u64::from(*timeout_seconds)),
                output,
                lab,
                selected
                    .link
                    .expect("validated paired reconnect has a link expectation")
                    .phy,
            )
        }
    };
    result?;
    Ok(ExecutionEvidence::default())
}

fn boot_smoke(output: &Path, lab: &transport::lab_config::LabConfig) -> Result<()> {
    let capture = evidence::traffic_capture::SerialCapture::start_with_reset(&lab.device.serial);
    let result = capture.wait_for_boot_smoke(std::time::Duration::from_secs(10));
    capture.finish_to(output)?;
    result
}

fn push_option(arguments: &mut Vec<String>, name: &str, value: impl ToString) {
    arguments.push(name.to_owned());
    arguments.push(value.to_string());
}
