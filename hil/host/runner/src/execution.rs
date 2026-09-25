//! Dispatch from validated scenario workloads to their concrete host owners.

use std::path::Path;

use crate::Result;
use hil_core::{evidence::run::Failure, evidence::run::FailureKind, evidence::run::Outcome};

pub(crate) mod doctor;
pub(crate) mod firmware;
pub(crate) mod fixture_check;
pub(crate) mod orchestration;
pub(crate) mod preflight;
#[cfg(test)]
mod tests;
pub(crate) use hil_core::failure::classify;

#[derive(Default)]
pub(crate) struct ExecutionEvidence {
    pub(crate) measurements: Vec<hil_core::evidence::run::Measurement>,
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
    lab: &hil_core::lab::config::LabConfig,
    selected: &hil_core::scenario::Scenario,
    output: &Path,
    fixture: &hil_wifi::fixture::prepared::Prepared,
) -> ExecutionEvidence {
    let context =
        hil_core::context::Context::new(lab, hil_core::session::Settings::from(selected), output);
    let result = execute_workload_inner(&context, fixture, selected, output);
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
    context: &hil_core::context::Context<'_>,
    fixture: &hil_wifi::fixture::prepared::Prepared,
    selected: &hil_core::scenario::Scenario,
    output: &Path,
) -> Result<()> {
    use hil_core::scenario::{Direction, Workload};
    use hil_wifi::workload::{ieee80211, traffic};
    use std::time::Duration;

    match &selected.workload {
        Workload::BluetoothGatt => hil_bluetooth::workload::bluetooth::gatt::run(output, context),
        Workload::BluetoothSecureGatt => hil_bluetooth::workload::bluetooth::secure_gatt::run(
            output,
            context,
            hil_core::scenario::SecureGattShutdown::BondLoadFailure,
            hil_bluetooth::workload::bluetooth::secure_gatt::IrqSampling::EverySnapshot,
        ),
        Workload::BluetoothSecureGattTiming => {
            hil_bluetooth::workload::bluetooth::secure_gatt::run(
                output,
                context,
                hil_core::scenario::SecureGattShutdown::BondLoadFailure,
                hil_bluetooth::workload::bluetooth::secure_gatt::IrqSampling::BoundaryOnly,
            )
        }
        Workload::BluetoothSecureGattHciReadFailure => {
            hil_bluetooth::workload::bluetooth::secure_gatt::run(
                output,
                context,
                hil_core::scenario::SecureGattShutdown::HciReadFailure,
                hil_bluetooth::workload::bluetooth::secure_gatt::IrqSampling::EverySnapshot,
            )
        }
        Workload::BluetoothDtm {
            boots,
            minimum_packets,
            quiet_cycles,
        } => hil_bluetooth::workload::bluetooth::run(
            *boots,
            *minimum_packets,
            *quiet_cycles,
            output,
            context,
        ),
        Workload::BluetoothSecurityFailure {
            failure,
            read_version_before_disconnect,
        } => hil_bluetooth::workload::bluetooth::security_failure::run(
            *failure,
            *read_version_before_disconnect,
            output,
            context,
        ),
        Workload::BluetoothEncryptedAcl {
            key_refresh,
            active_maintenance,
        } => hil_bluetooth::workload::bluetooth::run_peripheral(
            hil_bluetooth::workload::bluetooth::PeripheralConfig {
                boots: 1,
                connections: 2,
                hold_millis: if *active_maintenance { 1000 } else { 0 },
                termination: oer_hil_protocol::BluetoothPeripheralTermination::PeerReset,
                retire_after: true,
                restart_between_connections: false,
                maintain_between_connections: false,
                calibration_threshold: None,
                encrypted: true,
                key_refresh: *key_refresh,
                encrypted_maintenance: *active_maintenance,
            },
            output,
            context,
        ),
        Workload::BluetoothAclBackpressure { active_maintenance } => {
            hil_bluetooth::workload::bluetooth::backpressure::run(
                output,
                context,
                *active_maintenance,
            )
        }
        Workload::BluetoothMaintenanceDeadline => {
            hil_bluetooth::workload::bluetooth::deadline::run(
                output,
                context,
                oer_hil_protocol::ResetReason::Software,
            )
        }
        Workload::BluetoothWatchdogReset => hil_bluetooth::workload::bluetooth::deadline::run(
            output,
            context,
            oer_hil_protocol::ResetReason::MainWatchdog1,
        ),
        Workload::SystemWatchdog => hil_system::workload::system::watchdog::run(output, context),
        Workload::BluetoothPhyWatchdog => {
            hil_bluetooth::workload::bluetooth::phy_watchdog::run(output, context)
        }
        Workload::WifiPhyWatchdog => hil_wifi::workload::ieee80211::phy_watchdog::run(
            output,
            context,
            selected
                .link
                .expect("validated PHY watchdog has a link expectation")
                .phy,
        ),
        Workload::BluetoothAclCalibration {
            duration_millis,
            minimum_calibrations,
        } => hil_bluetooth::workload::bluetooth::calibration::run(
            *duration_millis,
            *minimum_calibrations,
            output,
            context,
        ),
        Workload::BluetoothPeripheral {
            boots,
            connections,
            hold_millis,
            termination,
            retire_after,
            restart_between_connections,
            maintain_between_connections,
            calibration_threshold,
        } => hil_bluetooth::workload::bluetooth::run_peripheral(
            hil_bluetooth::workload::bluetooth::PeripheralConfig {
                encrypted: false,
                key_refresh: false,
                encrypted_maintenance: false,
                boots: *boots,
                connections: *connections,
                hold_millis: *hold_millis,
                termination: *termination,
                retire_after: *retire_after,
                restart_between_connections: *restart_between_connections,
                maintain_between_connections: *maintain_between_connections,
                calibration_threshold: *calibration_threshold,
            },
            output,
            context,
        ),
        Workload::BootSmoke => boot_smoke(output, context),
        Workload::MemoryBenchmark {
            boots,
            iterations,
            sizes,
            batch_sizes,
        } => hil_system::workload::system::memory_benchmark::run(
            hil_system::workload::system::memory_benchmark::Config {
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
        } => hil_system::workload::system::timebase::run(
            hil_system::workload::system::timebase::Config {
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
        } => hil_ieee802154::workload::ieee802154::event_status::run(
            hil_ieee802154::workload::ieee802154::event_status::Config {
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
        } => hil_ieee802154::workload::ieee802154::ed_event::run(
            hil_ieee802154::workload::ieee802154::ed_event::Config {
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
            station_pause,
            station_pause_after_millis,
            station_pause_attempts,
            station_pause_interval_millis,
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
                        require_post_maintenance_echo: selected
                            .criteria
                            .require_post_maintenance_echo,
                        maximum_rx_silence_ms: selected.criteria.maximum_rx_silence_ms,
                        require_nonzero_rfpll_correction: selected
                            .criteria
                            .require_nonzero_rfpll_correction,
                        station_pause: *station_pause,
                        station_pause_after: station_pause_after_millis
                            .map(|value| Duration::from_millis(u64::from(value)))
                            .unwrap_or_default(),
                        station_pause_attempts: station_pause_attempts.unwrap_or(1),
                        station_pause_interval: station_pause_interval_millis
                            .map(|value| Duration::from_millis(u64::from(value)))
                            .unwrap_or_default(),
                        duration,
                        payload,
                        phy,
                        expected_rx_format: match phy {
                            hil_core::scenario::PhyExpectation::He20 => 4,
                            hil_core::scenario::PhyExpectation::Ht20
                            | hil_core::scenario::PhyExpectation::Ht40 => 2,
                        },
                        rate_bps: rx_rate_bps.expect("validated RX rate"),
                        minimum_rate_bps: selected.criteria.minimum_rx_bps,
                        maximum_idle_channel_utilization_255: selected
                            .criteria
                            .maximum_idle_channel_utilization_255,
                        ..Default::default()
                    };
                    hil_wifi::workload::traffic::rx_traffic::run(
                        config,
                        output,
                        context,
                        hil_wifi::workload::traffic::rx_traffic::EvidencePolicy {
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
                        hil_core::scenario::PhyExpectation::He20 => (20, 114_700),
                        hil_core::scenario::PhyExpectation::Ht40 => (40, 135_000),
                        hil_core::scenario::PhyExpectation::Ht20 => {
                            return Err("UDP TX requires HE20 or HT40".into());
                        }
                    };
                    let config = traffic::tx_traffic::Config {
                        require_nonzero_rfpll_correction: selected
                            .criteria
                            .require_nonzero_rfpll_correction,
                        station_pause: *station_pause,
                        station_pause_after: station_pause_after_millis
                            .map(|value| Duration::from_millis(u64::from(value)))
                            .unwrap_or_default(),
                        station_pause_attempts: station_pause_attempts.unwrap_or(1),
                        station_pause_interval: station_pause_interval_millis
                            .map(|value| Duration::from_millis(u64::from(value)))
                            .unwrap_or_default(),
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
                    hil_wifi::workload::traffic::tx_traffic::run(
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
                        hil_core::scenario::PhyExpectation::He20 => {
                            traffic::bidirectional::Phy::He20
                        }
                        hil_core::scenario::PhyExpectation::Ht40 => {
                            traffic::bidirectional::Phy::Ht40
                        }
                        hil_core::scenario::PhyExpectation::Ht20 => {
                            return Err("bidirectional UDP requires HE20 or HT40".into());
                        }
                    };
                    let config = traffic::bidirectional::Config {
                        maximum_rx_silence_ms: selected.criteria.maximum_rx_silence_ms,
                        require_nonzero_rfpll_correction: selected
                            .criteria
                            .require_nonzero_rfpll_correction,
                        station_pause: *station_pause,
                        station_pause_after: station_pause_after_millis
                            .map(|value| Duration::from_millis(u64::from(value)))
                            .unwrap_or_default(),
                        station_pause_attempts: station_pause_attempts.unwrap_or(1),
                        station_pause_interval: station_pause_interval_millis
                            .map(|value| Duration::from_millis(u64::from(value)))
                            .unwrap_or_default(),
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
                    hil_wifi::workload::traffic::bidirectional::run(
                        config,
                        output,
                        context,
                        hil_wifi::workload::traffic::bidirectional::RunPolicy {
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
                Direction::Rx => oer_hil_protocol::Direction::Rx,
                Direction::Tx => oer_hil_protocol::Direction::Tx,
                Direction::Bidirectional => oer_hil_protocol::Direction::Bidirectional,
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
            hil_wifi::workload::ieee80211::station_lifecycle::run(
                config,
                output,
                context,
                selected.criteria.require_no_beacon_loss,
            )
        }
        Workload::StationApLoss {
            timeout_seconds,
            require_recovery_echo,
        } => {
            let config = ieee80211::station_ap_loss::Config {
                require_recovery_echo: *require_recovery_echo,
                timeout: Duration::from_secs(u64::from(*timeout_seconds)),
            };
            hil_wifi::workload::ieee80211::station_ap_loss::run(
                config,
                output,
                context,
                fixture,
                selected
                    .link
                    .expect("validated AP-loss workload has a link expectation")
                    .phy,
            )
        }
        Workload::StationApAbsence {
            timeout_seconds,
            initially_absent,
        } => {
            let config = ieee80211::station_ap_absence::Config {
                initially_absent: *initially_absent,
                timeout: Duration::from_secs(u64::from(*timeout_seconds)),
            };
            hil_wifi::workload::ieee80211::station_ap_absence::run(
                config,
                output,
                context,
                fixture,
                selected
                    .link
                    .expect("validated AP-absence workload has a link expectation")
                    .phy,
            )
        }
        Workload::WifiRole {
            operation,
            timeout_seconds,
            cycles,
            channel,
            dwell_seconds,
            snapshot_length,
        } => {
            let config = ieee80211::control::Config {
                timeout: Duration::from_secs(u64::from(*timeout_seconds)),
                restart_cycles: cycles.unwrap_or(1),
                monitor_channel: *channel,
                monitor_duration: Duration::from_secs(u64::from(dwell_seconds.unwrap_or(3))),
                snapshot_length: snapshot_length.unwrap_or(256),
            };
            hil_wifi::workload::ieee80211::control::run(
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
            hil_wifi::workload::ieee80211::capture::run(
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
            probe_load,
            cycles,
            boots,
            timeout_seconds,
            client,
            security,
            traffic,
        } => hil_wifi::workload::ieee80211::access_point::run(
            hil_wifi::workload::ieee80211::access_point::Config {
                probe_load: *probe_load,
                cycles: *cycles,
                boots: *boots,
                timeout: std::time::Duration::from_secs(u64::from(*timeout_seconds)),
                client: *client,
                security: *security,
                traffic: traffic.clone(),
                criteria: selected.criteria.clone(),
                expected_link: selected.link,
                require_driver_observation: selected.image.requires_driver_observation(),
                require_rx_delivery_evidence: matches!(
                    selected.image,
                    hil_core::image::ImageClass::DiagnosticRxDelivery
                        | hil_core::image::ImageClass::DiagnosticRxDeliveryPhyHotSram
                ),
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
        } => hil_wifi::workload::ieee80211::station_access_point::run(
            hil_wifi::workload::ieee80211::station_access_point::Config {
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
            hil_wifi::workload::ieee80211::station_access_point_reconnect::run(
                std::time::Duration::from_secs(u64::from(*timeout_seconds)),
                output,
                context,
                fixture,
                selected
                    .link
                    .expect("validated paired reconnect has a link expectation")
                    .phy,
            )
        }
    }
}

fn boot_smoke(output: &Path, context: &hil_core::context::Context<'_>) -> Result<()> {
    context.with_capture(output, |capture| {
        capture.wait_for_boot_smoke(std::time::Duration::from_secs(10))
    })
}
