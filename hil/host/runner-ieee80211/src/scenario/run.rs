//! Dispatch from a validated Wi-Fi scenario to its workload owner.

use std::{path::Path, time::Duration};

use hil_core::{context::Context, image::ImageClass, lab::link::PhyExpectation};

use super::{
    Direction, InducedProtection, ProtectionPeer, RoleOperation, StationIcmp, StationReconnect,
    StationTcp, StationUdp, WifiScenario, WifiWorkload, station::PROTECTION_CHECKS,
};
use crate::{
    Result,
    evidence::{air::MacAddress, protection::Expectation},
    fixture::{openwrt::air_monitor::ProtectionCapture, prepared::Prepared},
    workload::{
        ieee80211::{self, control::Operation},
        traffic,
    },
};

pub(super) fn execute(
    scenario: &WifiScenario,
    output: &Path,
    context: &Context<'_>,
    fixture: &Prepared,
) -> Result<()> {
    let image = scenario.image;
    let seconds = |value: u16| Duration::from_secs(u64::from(value));
    match &scenario.workload {
        WifiWorkload::StationUdp(workload) => station_udp(workload, image, output, context),
        WifiWorkload::StationTcp(workload) => station_tcp(workload, output, context),
        WifiWorkload::StationIcmp(workload) => station_icmp(workload, output, context),
        WifiWorkload::StationReconnect(workload) => station_reconnect(workload, output, context),
        WifiWorkload::StationApLoss {
            link,
            timeout_seconds,
            require_recovery_echo,
        } => ieee80211::station_ap_loss::run(
            ieee80211::station_ap_loss::Config {
                require_recovery_echo: *require_recovery_echo,
                timeout: seconds(*timeout_seconds),
            },
            output,
            context,
            fixture,
            link.phy,
        ),
        WifiWorkload::StationApAbsence {
            link,
            timeout_seconds,
            initially_absent,
        } => ieee80211::station_ap_absence::run(
            ieee80211::station_ap_absence::Config {
                initially_absent: *initially_absent,
                timeout: seconds(*timeout_seconds),
            },
            output,
            context,
            fixture,
            link.phy,
        ),
        WifiWorkload::Role {
            link,
            timeout_seconds,
            operation,
        } => {
            let (operation, config) = role(*operation, seconds(*timeout_seconds));
            ieee80211::control::run(operation, config, output, context, link.phy)
        }
        WifiWorkload::PhyWatchdog { link } => {
            ieee80211::phy_watchdog::run(output, context, link.phy)
        }
        WifiWorkload::MonitorCapture {
            link,
            timeout_seconds,
            duration_seconds,
            channel,
            snapshot_length,
        } => ieee80211::capture::run(
            ieee80211::capture::Config {
                timeout: seconds(*timeout_seconds),
                duration: Duration::from_secs(u64::from(*duration_seconds)),
                output: output.join("capture.pcapng"),
                channel: *channel,
                snapshot_length: *snapshot_length,
            },
            output,
            context,
            link.phy,
        ),
        WifiWorkload::AccessPoint(workload) => ieee80211::access_point::run(
            ieee80211::access_point::Config {
                probe_load: workload.probe_load,
                cycles: workload.cycles,
                boots: workload.boots,
                timeout: seconds(workload.timeout_seconds),
                clients: workload.clients,
                security: workload.security,
                traffic: workload.traffic.clone(),
                link: workload.link,
                capture_independent_laptop_air_monitor: workload.independent_air_monitor,
                require_driver_observation: image.requires_driver_observation(),
                require_rx_delivery_evidence: matches!(
                    image,
                    ImageClass::DiagnosticRxDelivery | ImageClass::DiagnosticRxDeliveryPhyHotSram
                ),
            },
            output,
            context,
        ),
        WifiWorkload::StationAccessPoint(workload) => ieee80211::station_access_point::run(
            ieee80211::station_access_point::Config {
                timeout: seconds(workload.timeout_seconds),
                duration: seconds(workload.duration_seconds),
                direction: workload.direction,
                rate_bps_per_flow: workload.rate_bps_per_flow,
                minimum_bps_per_flow: workload.minimum_bps_per_flow,
                maximum_fairness_skew_percent: workload.maximum_fairness_skew_percent,
                payload_bytes: usize::from(workload.payload_bytes),
                require_driver_observation: image.requires_driver_observation(),
                capture_independent_laptop_air_monitor: workload.independent_air_monitor,
            },
            output,
            context,
        ),
        WifiWorkload::StationAccessPointReconnect {
            link,
            timeout_seconds,
        } => ieee80211::station_access_point_reconnect::run(
            seconds(*timeout_seconds),
            output,
            context,
            fixture,
            link.phy,
        ),
    }
}

/// Run station UDP, observing the target's protection when the workload
/// induces it. Both verdicts are reported; the traffic failure wins.
fn station_udp(
    workload: &StationUdp,
    image: ImageClass,
    output: &Path,
    context: &Context<'_>,
) -> Result<()> {
    let Some(induced) = workload.induced_protection else {
        return station_udp_traffic(workload, image, output, context);
    };
    // The bound only stops a capture whose workload never finishes it.
    let bound = Duration::from_secs(u64::from(workload.duration_seconds) + 180);
    let capture = ProtectionCapture::start(context.lab, bound, output)?;
    let traffic = station_udp_traffic(workload, image, output, context);
    let protection = assess_protection(capture, induced, context);
    traffic.and(protection)
}

fn assess_protection(
    capture: ProtectionCapture,
    induced: InducedProtection,
    context: &Context<'_>,
) -> Result<()> {
    use hil_core::evidence::run::{Comparison, Measurement, MeasurementUnit};
    // The non-HT member is the laptop: its own data to the AP is not the
    // target's. An overlapping legacy BSS never joins the AP.
    let peer = match induced.peer {
        ProtectionPeer::NonHtMember => Some(
            std::fs::read_to_string("/sys/class/net/wlan0/address")?
                .trim()
                .parse::<MacAddress>()?,
        ),
        ProtectionPeer::OverlappingLegacyBss => None,
    };
    let evidence = capture.finish(
        peer,
        Expectation {
            erp: induced.peer == ProtectionPeer::OverlappingLegacyBss,
        },
    )?;
    if evidence.data_ppdus < MINIMUM_PROTECTION_PPDUS || evidence.nav_evaluated == 0 {
        return Err(format!(
            "protection observation is insufficient: {} data PPDUs, {} with an evaluable NAV",
            evidence.data_ppdus, evidence.nav_evaluated
        )
        .into());
    }
    let measurements = [
        Measurement::observed(
            PROTECTION_CHECKS[0],
            evidence.protected_basis_points(),
            MeasurementUnit::BasisPoints,
        )
        .evaluated(
            Comparison::AtLeast,
            u64::from(induced.minimum_protected_ppdu_percent) * 100,
        ),
        Measurement::observed(
            PROTECTION_CHECKS[1],
            u64::from(evidence.wrong_control_rate),
            MeasurementUnit::Count,
        )
        .evaluated(Comparison::Exactly, 0),
        Measurement::observed(
            PROTECTION_CHECKS[2],
            u64::from(evidence.nav_short),
            MeasurementUnit::Count,
        )
        .evaluated(Comparison::Exactly, 0),
    ];
    let failed = measurements
        .iter()
        .filter(|measurement| {
            measurement.verdict == Some(hil_core::evidence::run::MeasurementVerdict::Failed)
        })
        .map(|measurement| format!("{}={}", measurement.name, measurement.value))
        .collect::<Vec<_>>();
    context.measurements.record(measurements);
    if !failed.is_empty() {
        return Err(format!(
            "target did not follow the induced BSS protection: {}; {evidence:?}",
            failed.join(", ")
        )
        .into());
    }
    Ok(())
}

/// Fewer target PPDUs cannot establish a protection share.
const MINIMUM_PROTECTION_PPDUS: u32 = 50;

fn station_udp_traffic(
    workload: &StationUdp,
    image: ImageClass,
    output: &Path,
    context: &Context<'_>,
) -> Result<()> {
    let duration = Duration::from_secs(u64::from(workload.duration_seconds));
    let payload = usize::from(workload.payload_bytes);
    let link = workload.link;
    let criteria = workload.criteria;
    let millis = |value: Option<u32>| Duration::from_millis(u64::from(value.unwrap_or(0)));
    let maintenance = workload.maintenance;
    let station_pause = maintenance.map(|maintenance| maintenance.operation);
    let station_pause_after = millis(maintenance.and_then(|maintenance| maintenance.after_millis));
    let attempts = maintenance.and_then(|maintenance| maintenance.attempts);
    let station_pause_attempts = attempts.map_or(1, |attempts| attempts.count);
    let station_pause_interval = millis(attempts.map(|attempts| attempts.interval_millis));
    let require_nonzero_rfpll_correction =
        maintenance.is_some_and(|maintenance| maintenance.require_nonzero_rfpll_correction);
    let fixture_guard_interval = if workload.fixed_fixture_guard_interval {
        link.guard_interval
    } else {
        Default::default()
    };
    match workload.offer.direction() {
        Direction::Rx => traffic::rx_traffic::run(
            traffic::rx_traffic::Config {
                require_post_maintenance_echo: maintenance
                    .is_some_and(|maintenance| maintenance.require_post_maintenance_echo),
                maximum_rx_silence_ms: criteria.maximum_rx_silence_ms,
                require_nonzero_rfpll_correction,
                station_pause,
                station_pause_after,
                station_pause_attempts,
                station_pause_interval,
                duration,
                payload,
                phy: link.phy,
                expected_rx_format: match link.phy {
                    PhyExpectation::He20 => 4,
                    PhyExpectation::Ht20 | PhyExpectation::Ht40 => 2,
                },
                rate_bps: workload.offer.rx_bps.expect("validated RX offer"),
                minimum_rate_bps: criteria.minimum_rx_bps,
                maximum_idle_channel_utilization_255: criteria.maximum_idle_channel_utilization_255,
                ..Default::default()
            },
            output,
            context,
            traffic::rx_traffic::EvidencePolicy {
                require_exact_delivery: criteria.exact_delivery,
                require_no_beacon_loss: criteria.require_no_beacon_loss,
                require_driver_observation: image.requires_driver_observation(),
                capture_openwrt_tx_monitor: workload.observation.openwrt_tx_monitor,
                capture_independent_laptop_monitor: workload.observation.independent_air_monitor,
                minimum_mcs: link.minimum_mcs,
                guard_interval: link.guard_interval,
                fixture_guard_interval,
            },
        ),
        Direction::Tx => {
            let (bandwidth_mhz, minimum_rate_kbps) = match link.phy {
                PhyExpectation::He20 => (20, 114_700),
                PhyExpectation::Ht40 | PhyExpectation::Ht20 => (40, 135_000),
            };
            traffic::tx_traffic::run(
                traffic::tx_traffic::Config {
                    require_nonzero_rfpll_correction,
                    station_pause,
                    station_pause_after,
                    station_pause_attempts,
                    station_pause_interval,
                    duration,
                    payload,
                    bandwidth_mhz,
                    minimum_rate_kbps,
                    offered_rate_bps: workload.offer.tx_bps,
                    throughput_floor_bps: criteria.minimum_tx_bps,
                    maximum_idle_channel_utilization_255: criteria
                        .maximum_idle_channel_utilization_255,
                    ..Default::default()
                },
                output,
                context,
                criteria.exact_delivery,
                criteria.require_no_beacon_loss,
                image.requires_driver_observation(),
            )
        }
        Direction::Bidirectional => traffic::bidirectional::run(
            traffic::bidirectional::Config {
                maximum_rx_silence_ms: criteria.maximum_rx_silence_ms,
                require_nonzero_rfpll_correction,
                station_pause,
                station_pause_after,
                station_pause_attempts,
                station_pause_interval,
                duration,
                payload,
                phy: if link.phy == PhyExpectation::He20 {
                    traffic::bidirectional::Phy::He20
                } else {
                    traffic::bidirectional::Phy::Ht40
                },
                rate_bps: workload.offer.rx_bps.expect("validated RX offer"),
                tx_rate_bps: workload.offer.tx_bps,
                rx_floor_bps: criteria.minimum_rx_bps,
                tx_floor_bps: criteria.minimum_tx_bps,
                combined_floor_bps: criteria.minimum_combined_bps,
                ..Default::default()
            },
            output,
            context,
            traffic::bidirectional::RunPolicy {
                require_exact_delivery: criteria.exact_delivery,
                require_no_beacon_loss: criteria.require_no_beacon_loss,
                capture_openwrt_tx_monitor_rx: workload.observation.openwrt_tx_monitor,
                capture_independent_laptop_air_monitor: workload
                    .observation
                    .independent_air_monitor,
                require_driver_observation: image.requires_driver_observation(),
                minimum_mcs: link.minimum_mcs,
                guard_interval: link.guard_interval,
                fixture_guard_interval,
            },
        ),
    }
}

fn station_tcp(workload: &StationTcp, output: &Path, context: &Context<'_>) -> Result<()> {
    let direction = match workload.offer.direction() {
        Direction::Rx => oer_hil_protocol::Direction::Rx,
        Direction::Tx => oer_hil_protocol::Direction::Tx,
        Direction::Bidirectional => oer_hil_protocol::Direction::Bidirectional,
    };
    let defaults = traffic::tcp_traffic::Config::for_direction(direction);
    traffic::tcp_traffic::run(
        traffic::tcp_traffic::Config {
            duration: Duration::from_secs(u64::from(workload.duration_seconds)),
            chunk_bytes: usize::from(workload.chunk_bytes),
            rx_rate_bps: workload.offer.rx_bps,
            tx_rate_bps: workload.offer.tx_bps,
            rx_floor_bps: workload.criteria.minimum_rx_bps.or(defaults.rx_floor_bps),
            tx_floor_bps: workload.criteria.minimum_tx_bps.or(defaults.tx_floor_bps),
            ..defaults
        },
        output,
        context,
        workload.criteria.require_no_beacon_loss,
    )
}

fn station_icmp(workload: &StationIcmp, output: &Path, context: &Context<'_>) -> Result<()> {
    traffic::icmp_latency::run(
        traffic::icmp_latency::Config {
            count: workload.count,
            interval: Duration::from_millis(u64::from(workload.interval_ms)),
            timeout: Duration::from_millis(u64::from(workload.timeout_ms)),
            payload_bytes: usize::from(workload.payload_bytes),
            maximum_lost: workload.criteria.maximum_lost,
            maximum_p95: workload
                .criteria
                .maximum_p95_ms
                .map(|ms| Duration::from_millis(u64::from(ms))),
            ..Default::default()
        },
        output,
        context,
        workload.criteria.require_no_beacon_loss,
    )
}

fn station_reconnect(
    workload: &StationReconnect,
    output: &Path,
    context: &Context<'_>,
) -> Result<()> {
    ieee80211::station_lifecycle::run(
        ieee80211::station_lifecycle::Config {
            cycles: workload.cycles,
            boots: workload.boots,
            timeout: Duration::from_secs(u64::from(workload.timeout_seconds)),
            ..Default::default()
        },
        output,
        context,
        workload.criteria.require_no_beacon_loss,
    )
}

fn role(operation: RoleOperation, timeout: Duration) -> (Operation, ieee80211::control::Config) {
    let mut config = ieee80211::control::Config {
        timeout,
        restart_cycles: 1,
        monitor_channel: None,
        monitor_duration: Duration::from_secs(3),
        snapshot_length: 256,
    };
    let operation = match operation {
        RoleOperation::Stop {} => Operation::Stop,
        RoleOperation::Start {} => Operation::Start,
        RoleOperation::Restart { cycles } => {
            config.restart_cycles = cycles;
            Operation::Restart
        }
        RoleOperation::MaintenanceRestart { cycles } => {
            config.restart_cycles = cycles;
            Operation::MaintenanceRestart
        }
        RoleOperation::Retained { cycles } => {
            config.restart_cycles = cycles;
            Operation::Retained
        }
        RoleOperation::Scan {} => Operation::Scan,
        RoleOperation::Monitor {
            channel,
            dwell_seconds,
            snapshot_length,
        }
        | RoleOperation::Roundtrip {
            channel,
            dwell_seconds,
            snapshot_length,
        } => {
            config.monitor_channel = channel;
            config.monitor_duration = Duration::from_secs(u64::from(dwell_seconds));
            config.snapshot_length = snapshot_length;
            if matches!(operation, RoleOperation::Monitor { .. }) {
                Operation::Monitor
            } else {
                Operation::Roundtrip
            }
        }
        RoleOperation::AccessPoint {} => Operation::AccessPoint,
        RoleOperation::StationAccessPoint { dwell_seconds } => {
            config.monitor_duration = Duration::from_secs(u64::from(dwell_seconds));
            Operation::StationAccessPoint
        }
    };
    (operation, config)
}
