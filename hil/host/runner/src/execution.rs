//! Dispatch from validated scenario workloads to their concrete host owners.

use crate::evidence::run::{Failure, FailureKind, Outcome};
use crate::*;

pub(crate) mod context;
mod failure;
#[cfg(test)]
mod tests;
pub(crate) use failure::classify;

#[derive(Default)]
pub(crate) struct ExecutionEvidence {
    pub(crate) measurements: Vec<crate::evidence::run::Measurement>,
    pub(crate) failure: Option<Failure>,
    pub(crate) interrupted: bool,
}

impl ExecutionEvidence {
    pub(crate) fn outcome(&self) -> Outcome {
        if self.interrupted {
            return Outcome::Interrupted;
        }
        match self.failure.as_ref().map(|failure| failure.kind) {
            None => Outcome::Passed,
            Some(FailureKind::Infrastructure) => Outcome::Broken,
            Some(_) => Outcome::Failed,
        }
    }
}

pub(crate) fn execute_workload(
    lab: &crate::lab::config::LabConfig,
    selected: &crate::scenario::Scenario,
    output: &Path,
) -> ExecutionEvidence {
    let context = context::Context::new(lab, context::Settings::from(selected), output);
    let result = execute_workload_inner(&context, selected, output);
    let mut evidence = ExecutionEvidence {
        measurements: context.measurements.snapshot(),
        interrupted: result
            .as_ref()
            .err()
            .is_some_and(|error| oer_process::is_cancelled(&**error)),
        failure: result.err().map(|error| classify(&*error)),
    };
    if oer_process::cancellation_requested() {
        evidence.interrupted = true;
        evidence.failure.get_or_insert_with(|| {
            Failure::new(FailureKind::Infrastructure, "run cancelled by signal")
        });
    }
    evidence
}

fn execute_workload_inner(
    context: &context::Context<'_>,
    selected: &crate::scenario::Scenario,
    output: &Path,
) -> Result<()> {
    use crate::scenario::{Direction, Workload};
    use crate::workload::{ieee80211, traffic};
    use std::time::Duration;

    let lab = context.lab;

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
        crate::fixture::controlled_ap::ControlledAp::start(
            &lab.station,
            &lab.station_fixture,
            selected
                .link
                .expect("validated station workload has a link expectation")
                .phy,
        )
    })
    .transpose()?;

    match &selected.workload {
        Workload::BootSmoke => boot_smoke(output, context),
        Workload::MemoryBenchmark {
            boots,
            iterations,
            sizes,
            batch_sizes,
        } => crate::workload::system::memory_benchmark::run(
            crate::workload::system::memory_benchmark::Config {
                boots: *boots,
                iterations: *iterations,
                sizes,
                batch_sizes,
            },
            output,
            context,
        ),
        Workload::Timebase {
            boots,
            intervals,
            period_millis,
        } => crate::workload::system::timebase::run(
            crate::workload::system::timebase::Config {
                boots: *boots,
                intervals: *intervals,
                period_millis: *period_millis,
            },
            output,
            context,
        ),
        Workload::Ieee802154EventStatus {
            boots,
            poll_limit,
            timer_threshold,
        } => crate::workload::ieee802154::event_status::run(
            crate::workload::ieee802154::event_status::Config {
                boots: *boots,
                poll_limit: *poll_limit,
                timer_threshold: *timer_threshold,
            },
            output,
            context,
        ),
        Workload::Ieee802154EdEvent {
            boots,
            poll_limit,
            timer_threshold,
        } => crate::workload::ieee802154::ed_event::run(
            crate::workload::ieee802154::ed_event::Config {
                boots: *boots,
                poll_limit: *poll_limit,
                timer_threshold: *timer_threshold,
            },
            output,
            context,
        ),
        Workload::Udp {
            direction,
            duration_seconds,
            rx_rate_bps,
            tx_rate_bps,
            payload_bytes,
        } => {
            let duration = Duration::from_secs(u64::from(*duration_seconds));
            let payload = usize::from(*payload_bytes);
            let phy = selected.link.expect("validated station link").phy;
            match direction {
                Direction::Rx => {
                    let link = selected
                        .link
                        .expect("validated station workload has a link expectation");
                    let config = traffic::rx_traffic::Config {
                        duration,
                        payload,
                        phy,
                        expected_rx_format: match phy {
                            crate::scenario::PhyExpectation::He20 => 4,
                            crate::scenario::PhyExpectation::Ht20
                            | crate::scenario::PhyExpectation::Ht40 => 2,
                        },
                        rate_bps: rx_rate_bps.expect("validated RX rate"),
                        minimum_rate_bps: selected.criteria.minimum_rx_bps,
                        maximum_idle_channel_utilization_255: selected
                            .criteria
                            .maximum_idle_channel_utilization_255,
                        ..Default::default()
                    };
                    crate::workload::traffic::rx_traffic::run(
                        config,
                        output,
                        context,
                        crate::workload::traffic::rx_traffic::EvidencePolicy {
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
                    let (bandwidth_mhz, minimum_rate_kbps) = match phy {
                        crate::scenario::PhyExpectation::He20 => (20, 114_700),
                        crate::scenario::PhyExpectation::Ht40 => (40, 135_000),
                        crate::scenario::PhyExpectation::Ht20 => {
                            return Err("UDP TX requires HE20 or HT40".into());
                        }
                    };
                    let config = traffic::tx_traffic::Config {
                        duration,
                        payload,
                        bandwidth_mhz,
                        minimum_rate_kbps,
                        offered_rate_bps: *tx_rate_bps,
                        throughput_floor_bps: selected.criteria.minimum_tx_bps,
                        maximum_idle_channel_utilization_255: selected
                            .criteria
                            .maximum_idle_channel_utilization_255,
                        ..Default::default()
                    };
                    crate::workload::traffic::tx_traffic::run(
                        config,
                        output,
                        context,
                        selected.criteria.exact_delivery,
                        selected.criteria.require_no_beacon_loss,
                        selected.image.requires_driver_observation(),
                    )
                }
                Direction::Bidirectional => {
                    let link = selected
                        .link
                        .expect("validated station workload has a link expectation");
                    let phy = match phy {
                        crate::scenario::PhyExpectation::He20 => traffic::bidirectional::Phy::He20,
                        crate::scenario::PhyExpectation::Ht40 => traffic::bidirectional::Phy::Ht40,
                        crate::scenario::PhyExpectation::Ht20 => {
                            return Err("bidirectional UDP requires HE20 or HT40".into());
                        }
                    };
                    let config = traffic::bidirectional::Config {
                        duration,
                        payload,
                        phy,
                        rate_bps: rx_rate_bps.expect("validated RX rate"),
                        tx_rate_bps: *tx_rate_bps,
                        rx_floor_bps: selected.criteria.minimum_rx_bps,
                        tx_floor_bps: selected.criteria.minimum_tx_bps,
                        combined_floor_bps: selected.criteria.minimum_combined_bps,
                        ..Default::default()
                    };
                    crate::workload::traffic::bidirectional::run(
                        config,
                        output,
                        context,
                        crate::workload::traffic::bidirectional::RunPolicy {
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
            let direction = match direction {
                Direction::Rx => open_esp_radio_hil_protocol::Direction::Rx,
                Direction::Tx => open_esp_radio_hil_protocol::Direction::Tx,
                Direction::Bidirectional => open_esp_radio_hil_protocol::Direction::Bidirectional,
            };
            let defaults = traffic::tcp_traffic::Config::for_direction(direction);
            let config = traffic::tcp_traffic::Config {
                duration: Duration::from_secs(u64::from(*duration_seconds)),
                chunk_bytes: usize::from(*chunk_bytes),
                rx_rate_bps: rx_rate_bps.or(defaults.rx_rate_bps),
                tx_rate_bps: tx_rate_bps.or(defaults.tx_rate_bps),
                rx_floor_bps: selected.criteria.minimum_rx_bps.or(defaults.rx_floor_bps),
                tx_floor_bps: selected.criteria.minimum_tx_bps.or(defaults.tx_floor_bps),
                ..defaults
            };
            traffic::tcp_traffic::run(
                config,
                output,
                context,
                selected.criteria.require_no_beacon_loss,
            )
        }
        Workload::Icmp {
            count,
            interval_ms,
            timeout_ms,
            payload_bytes,
        } => {
            let config = traffic::icmp_latency::Config {
                count: *count,
                interval: Duration::from_millis(u64::from(*interval_ms)),
                timeout: Duration::from_millis(u64::from(*timeout_ms)),
                payload_bytes: usize::from(*payload_bytes),
                maximum_lost: selected.criteria.maximum_lost.unwrap_or(0).try_into()?,
                maximum_p95: selected
                    .criteria
                    .maximum_p95_ms
                    .map(|ms| Duration::from_millis(u64::from(ms))),
                ..Default::default()
            };
            traffic::icmp_latency::run(
                config,
                output,
                context,
                selected.criteria.require_no_beacon_loss,
            )
        }
        Workload::StationReconnect {
            cycles,
            boots,
            timeout_seconds,
        } => {
            let config = ieee80211::station_lifecycle::Config {
                cycles: *cycles,
                boots: *boots,
                timeout: Duration::from_secs(u64::from(*timeout_seconds)),
                ..Default::default()
            };
            crate::workload::ieee80211::station_lifecycle::run(
                config,
                output,
                context,
                selected.criteria.require_no_beacon_loss,
            )
        }
        Workload::StationApLoss { timeout_seconds } => {
            let config = ieee80211::station_ap_loss::Config {
                timeout: Duration::from_secs(u64::from(*timeout_seconds)),
            };
            crate::workload::ieee80211::station_ap_loss::run(
                config,
                output,
                context,
                selected
                    .link
                    .expect("validated AP-loss workload has a link expectation")
                    .phy,
            )
        }
        Workload::StationApAbsence { timeout_seconds } => {
            let config = ieee80211::station_ap_absence::Config {
                timeout: Duration::from_secs(u64::from(*timeout_seconds)),
            };
            crate::workload::ieee80211::station_ap_absence::run(
                config,
                output,
                context,
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
            let config = ieee80211::control::Config {
                timeout: Duration::from_secs(u64::from(*timeout_seconds)),
                monitor_channel: *channel,
                monitor_duration: Duration::from_secs(u64::from(dwell_seconds.unwrap_or(3))),
                snapshot_length: snapshot_length.unwrap_or(256),
            };
            crate::workload::ieee80211::control::run(
                *operation,
                config,
                output,
                context,
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
            let config = ieee80211::capture::Config {
                timeout: Duration::from_secs(u64::from(*timeout_seconds)),
                duration: Duration::from_secs(u64::from(*duration_seconds)),
                output: output.join("capture.pcapng"),
                channel: *channel,
                snapshot_length: *snapshot_length,
            };
            crate::workload::ieee80211::capture::run(
                config,
                output,
                context,
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
        } => crate::workload::ieee80211::access_point::run(
            crate::workload::ieee80211::access_point::Config {
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
                    == crate::image::ImageClass::DiagnosticRxDelivery,
                capture_independent_laptop_air_monitor: selected
                    .evidence
                    .independent_laptop_air_monitor,
                openwrt_client_fixed_ht_mcs: selected.fixture_mutation.openwrt_client_fixed_ht_mcs,
                openwrt_client_fixed_guard_interval: selected
                    .fixture_mutation
                    .openwrt_client_fixed_guard_interval,
            },
            output,
            context,
        ),
        Workload::StationAccessPoint {
            timeout_seconds,
            duration_seconds,
            direction,
            rate_bps_per_flow,
            minimum_bps_per_flow,
            maximum_fairness_skew_percent,
            payload_bytes,
        } => crate::workload::ieee80211::station_access_point::run(
            crate::workload::ieee80211::station_access_point::Config {
                timeout: std::time::Duration::from_secs(u64::from(*timeout_seconds)),
                duration: std::time::Duration::from_secs(u64::from(*duration_seconds)),
                direction: *direction,
                rate_bps_per_flow: *rate_bps_per_flow,
                minimum_bps_per_flow: *minimum_bps_per_flow,
                maximum_fairness_skew_percent: *maximum_fairness_skew_percent,
                payload_bytes: usize::from(*payload_bytes),
                require_driver_observation: selected.image.requires_driver_observation(),
                capture_independent_laptop_air_monitor: selected
                    .evidence
                    .independent_laptop_air_monitor,
            },
            output,
            context,
        ),
        Workload::StationAccessPointReconnect { timeout_seconds } => {
            crate::workload::ieee80211::station_access_point_reconnect::run(
                std::time::Duration::from_secs(u64::from(*timeout_seconds)),
                output,
                context,
                selected
                    .link
                    .expect("validated paired reconnect has a link expectation")
                    .phy,
            )
        }
    }
}

fn boot_smoke(output: &Path, context: &context::Context<'_>) -> Result<()> {
    context.with_capture(output, |capture| {
        capture.wait_for_boot_smoke(std::time::Duration::from_secs(10))
    })
}
