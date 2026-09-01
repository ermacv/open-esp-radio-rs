//! Typed host-owned HIL scenario catalog.

use std::path::PathBuf;

#[cfg(test)]
use std::{fs, path::Path};

use open_esp_radio_hil_protocol::{
    WifiAccessPointSecurity, WifiDataPlanePlacement, WifiRxAdmissionPolicy, WifiRxChecksumPolicy,
    WifiRxContinuationPolicy, WifiRxDispatchPolicy, WifiTxBufferPolicy, WifiTxUdpChecksumPolicy,
};
use serde::{Deserialize, Serialize};

use crate::Result;

mod catalog;
mod validation;

#[cfg(test)]
use validation::validate_direction_rates;

pub const SCENARIO_SCHEMA: u16 = 4;
// Diagnostic phase totals use u32 cycle accumulators. At 320 MHz, 12 seconds
// leaves margin below the 2^32 wrap point; a 14-second interval cannot.
const CORE0_RX_CYCLE_MAX_DURATION_SECONDS: u16 = 12;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImageClass {
    BootSmoke,
    Performance,
    Correctness,
    DiagnosticMacIrq,
    DiagnosticTaskResidence,
    DiagnosticTxArchitecture,
    DiagnosticTaskPoll,
    DiagnosticCore0RxCoarse,
    DiagnosticCore0RxCycles,
    DiagnosticRxDelivery,
    DiagnosticIeee802154EventStatus,
    DiagnosticIeee802154EdEvent,
}

impl ImageClass {
    pub const ALL: [Self; 12] = [
        Self::BootSmoke,
        Self::Performance,
        Self::Correctness,
        Self::DiagnosticMacIrq,
        Self::DiagnosticTaskResidence,
        Self::DiagnosticTxArchitecture,
        Self::DiagnosticTaskPoll,
        Self::DiagnosticCore0RxCoarse,
        Self::DiagnosticCore0RxCycles,
        Self::DiagnosticRxDelivery,
        Self::DiagnosticIeee802154EventStatus,
        Self::DiagnosticIeee802154EdEvent,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::BootSmoke => "boot-smoke",
            Self::Performance => "performance",
            Self::Correctness => "correctness",
            Self::DiagnosticMacIrq => "diagnostic-mac-irq",
            Self::DiagnosticTaskResidence => "diagnostic-task-residence",
            Self::DiagnosticTxArchitecture => "diagnostic-tx-architecture",
            Self::DiagnosticTaskPoll => "diagnostic-task-poll",
            Self::DiagnosticCore0RxCoarse => "diagnostic-core0-rx-coarse",
            Self::DiagnosticCore0RxCycles => "diagnostic-core0-rx-cycles",
            Self::DiagnosticRxDelivery => "diagnostic-rx-delivery",
            Self::DiagnosticIeee802154EventStatus => "diagnostic-ieee802154-event-status",
            Self::DiagnosticIeee802154EdEvent => "diagnostic-ieee802154-ed-event",
        }
    }

    pub const fn runtime_features(self) -> &'static str {
        match self {
            Self::BootSmoke => "boot-smoke,psram-task-stack,code-psram,profile-psram-data",
            Self::Performance => "open-radio-hil,psram-task-stack,code-psram,profile-psram-data",
            Self::Correctness => {
                "open-radio-hil,driver-observation,psram-task-stack,code-psram,profile-psram-data"
            }
            Self::DiagnosticMacIrq => {
                "open-radio-hil,psram-task-stack,mac-irq-telemetry,code-psram,profile-psram-data"
            }
            Self::DiagnosticTaskResidence => {
                "open-radio-hil,psram-task-stack,task-residence-telemetry,code-psram,profile-psram-data"
            }
            Self::DiagnosticTxArchitecture => {
                "open-radio-hil,psram-task-stack,tx-architecture-probes,code-psram,profile-psram-data"
            }
            Self::DiagnosticTaskPoll => {
                "open-radio-hil,psram-task-stack,task-poll-telemetry,code-psram,profile-psram-data"
            }
            Self::DiagnosticCore0RxCoarse => {
                "open-radio-hil,psram-task-stack,core0-rx-coarse-telemetry,code-psram,profile-psram-data"
            }
            Self::DiagnosticCore0RxCycles => {
                "open-radio-hil,psram-task-stack,core0-rx-cycle-telemetry,code-psram,profile-psram-data"
            }
            Self::DiagnosticRxDelivery => {
                "open-radio-hil,psram-task-stack,rx-delivery-telemetry,code-psram,profile-psram-data"
            }
            Self::DiagnosticIeee802154EventStatus => {
                "open-radio-hil,ieee802154-event-status-probe,psram-task-stack,code-psram,profile-psram-data"
            }
            Self::DiagnosticIeee802154EdEvent => {
                "open-radio-hil,ieee802154-ed-event-probe,psram-task-stack,code-psram,profile-psram-data"
            }
        }
    }

    pub const fn runtime_profile(self) -> &'static str {
        match self {
            Self::BootSmoke
            | Self::Performance
            | Self::Correctness
            | Self::DiagnosticMacIrq
            | Self::DiagnosticTaskResidence
            | Self::DiagnosticTxArchitecture
            | Self::DiagnosticTaskPoll
            | Self::DiagnosticCore0RxCoarse
            | Self::DiagnosticCore0RxCycles
            | Self::DiagnosticRxDelivery
            | Self::DiagnosticIeee802154EventStatus
            | Self::DiagnosticIeee802154EdEvent => "psram-code-psram-data-psram-stack",
        }
    }

    pub const fn uses_psram_task_stack(self) -> bool {
        true
    }

    /// Whether the image promises typed driver-internal evidence.
    ///
    /// The task-residence image is deliberately production-like: its only
    /// diagnostic boundary is executor residence, so it must use the same
    /// transport/external-link acceptance path as the performance image.
    pub const fn requires_driver_observation(self) -> bool {
        !matches!(
            self,
            Self::BootSmoke
                | Self::Performance
                | Self::DiagnosticTaskResidence
                | Self::DiagnosticTxArchitecture
                | Self::DiagnosticCore0RxCoarse
        )
    }
}

impl std::str::FromStr for ImageClass {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|class| class.id() == value)
            .ok_or_else(|| format!("unknown image class `{value}`"))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Isolation {
    Reset,
    MatrixSession,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    Rx,
    Tx,
    Bidirectional,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccessPointClient {
    #[default]
    Laptop,
    OpenWrt,
}

const fn default_access_point_security() -> WifiAccessPointSecurity {
    WifiAccessPointSecurity::Wpa2Personal
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum AccessPointTraffic {
    None,
    Icmp {
        count: u16,
        interval_ms: u16,
        timeout_ms: u16,
        payload_bytes: u16,
    },
    Udp {
        direction: Direction,
        duration_seconds: u16,
        rx_rate_bps: Option<u64>,
        tx_rate_bps: Option<u64>,
        payload_bytes: u16,
    },
    UdpMultiClient {
        direction: Direction,
        duration_seconds: u16,
        rx_rate_bps_per_flow: Option<u64>,
        tx_rate_bps_per_flow: Option<u64>,
        /// Optional offer for the second physical client. Absence keeps the
        /// ordinary equal-offer fairness workload.
        secondary_rx_rate_bps: Option<u64>,
        secondary_tx_rate_bps: Option<u64>,
        /// Optional second-client target-TX pacing group. This describes a
        /// sparse burst independently of its average offered rate.
        secondary_tx_pacing_group_datagrams: Option<u8>,
        payload_bytes: u16,
    },
    Tcp {
        direction: Direction,
        duration_seconds: u16,
        rx_rate_bps: Option<u64>,
        tx_rate_bps: Option<u64>,
        chunk_bytes: u16,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PhyExpectation {
    He20,
    Ht20,
    Ht40,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WifiOperation {
    Stop,
    Start,
    Scan,
    Monitor,
    AccessPoint,
    StationAccessPoint,
    Roundtrip,
}

impl WifiOperation {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Start => "start",
            Self::Scan => "scan",
            Self::Monitor => "monitor",
            Self::AccessPoint => "ap",
            Self::StationAccessPoint => "sta-ap",
            Self::Roundtrip => "roundtrip",
        }
    }
}

impl PhyExpectation {
    pub const fn id(self) -> &'static str {
        match self {
            Self::He20 => "he20",
            Self::Ht20 => "ht20",
            Self::Ht40 => "ht40",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Workload {
    BootSmoke,
    Timebase {
        boots: u8,
        intervals: u16,
        period_millis: u16,
    },
    Ieee802154EventStatus {
        boots: u8,
        poll_limit: u32,
        timer_threshold: u32,
    },
    Ieee802154EdEvent {
        boots: u8,
        poll_limit: u32,
        timer_threshold: u32,
    },
    Udp {
        direction: Direction,
        duration_seconds: u16,
        rx_rate_bps: Option<u64>,
        tx_rate_bps: Option<u64>,
        payload_bytes: u16,
    },
    Tcp {
        direction: Direction,
        duration_seconds: u16,
        rx_rate_bps: Option<u64>,
        tx_rate_bps: Option<u64>,
        chunk_bytes: u16,
    },
    Icmp {
        count: u16,
        interval_ms: u16,
        timeout_ms: u16,
        payload_bytes: u16,
    },
    StationReconnect {
        cycles: u8,
        boots: u8,
        timeout_seconds: u16,
    },
    StationApLoss {
        timeout_seconds: u16,
    },
    StationApAbsence {
        timeout_seconds: u16,
    },
    WifiRole {
        operation: WifiOperation,
        timeout_seconds: u16,
        channel: Option<u8>,
        dwell_seconds: Option<u8>,
        snapshot_length: Option<u16>,
    },
    MonitorCapture {
        timeout_seconds: u16,
        duration_seconds: u8,
        channel: Option<u8>,
        snapshot_length: u16,
    },
    AccessPoint {
        cycles: u8,
        boots: u8,
        timeout_seconds: u16,
        #[serde(default)]
        client: AccessPointClient,
        #[serde(default = "default_access_point_security")]
        security: WifiAccessPointSecurity,
        traffic: AccessPointTraffic,
    },
    /// One equal-offer UDP session in the selected direction on each endpoint
    /// of a live same-channel STA+AP epoch.
    StationAccessPoint {
        timeout_seconds: u16,
        duration_seconds: u16,
        direction: Direction,
        rate_bps_per_flow: u64,
        minimum_bps_per_flow: u64,
        maximum_fairness_skew_percent: u8,
        payload_bytes: u16,
    },
    /// Controlled loss of the upstream AP tears down the complete same-channel
    /// pair; restoring the fixture permits one explicit fresh paired start.
    StationAccessPointReconnect {
        timeout_seconds: u16,
    },
}

fn is_rx_only_udp_workload(workload: &Workload) -> bool {
    matches!(
        workload,
        Workload::Udp {
            direction: Direction::Rx,
            ..
        } | Workload::AccessPoint {
            traffic: AccessPointTraffic::Udp {
                direction: Direction::Rx,
                ..
            } | AccessPointTraffic::UdpMultiClient {
                direction: Direction::Rx,
                ..
            },
            ..
        } | Workload::StationAccessPoint {
            direction: Direction::Rx,
            ..
        }
    )
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Criteria {
    pub exact_delivery: bool,
    pub minimum_rx_bps: Option<u64>,
    pub minimum_tx_bps: Option<u64>,
    pub minimum_combined_bps: Option<u64>,
    pub minimum_bps_per_flow: Option<u64>,
    pub maximum_flow_skew_percent: Option<u8>,
    /// Host-observed upper bound between consecutive datagrams of the second
    /// AP TX flow. Used by sparse-peer service tests, not as an air-latency
    /// estimate.
    pub maximum_secondary_tx_interarrival_ms: Option<u16>,
    pub minimum_secondary_tx_datagrams: Option<u16>,
    /// Maximum pre-workload channel utilization reported by hostapd, in the
    /// native 0..=255 BSS-load scale. This is deliberately a ceiling-scenario
    /// criterion rather than an inferred throughput failure.
    pub maximum_idle_channel_utilization_255: Option<u8>,
    pub maximum_lost: Option<u32>,
    pub maximum_p95_ms: Option<u16>,
    pub require_no_beacon_loss: bool,
    pub minimum_concurrent_ap_clients: Option<u8>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct EvidenceConfig {
    /// Capture the OpenWrt AP's own TX monitor tap. Diagnostic only.
    pub openwrt_tx_monitor_rx: bool,
    /// Capture the same radio channel through the laptop's independent adapter.
    pub independent_laptop_air_monitor: bool,
}

/// Explicit, diagnostic-only mutations of a managed laboratory fixture.
///
/// Link expectations never change the peer: they only validate target-side
/// observations. Keeping the mutation separate prevents a ceiling scenario
/// from silently entering a vendor firmware's fixed-rate-control mode.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct FixtureMutationConfig {
    pub openwrt_fixed_guard_interval: HtGuardIntervalExpectation,
    /// Fix the controlled OpenWrt AP client's HT transmit MCS.
    ///
    /// This is an explicit diagnostic mutation, not a link expectation. The
    /// scoped client interface owns the mask and deleting that interface
    /// restores automatic rate control.
    pub openwrt_client_fixed_ht_mcs: Option<u8>,
    /// Fix the controlled OpenWrt AP client's transmit guard interval.
    pub openwrt_client_fixed_guard_interval: HtGuardIntervalExpectation,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub schema: u16,
    pub id: String,
    pub description: String,
    #[serde(default = "one_repetition")]
    pub repetitions: u8,
    pub image: ImageClass,
    pub isolation: Isolation,
    #[serde(default)]
    pub data_plane: WifiDataPlanePlacement,
    #[serde(default)]
    pub rx_checksum: WifiRxChecksumPolicy,
    #[serde(default)]
    pub tx_udp_checksum: WifiTxUdpChecksumPolicy,
    #[serde(default)]
    pub tx_buffer: WifiTxBufferPolicy,
    #[serde(default)]
    pub rx_admission: WifiRxAdmissionPolicy,
    #[serde(default)]
    pub rx_dispatch: WifiRxDispatchPolicy,
    #[serde(default)]
    pub rx_continuation: WifiRxContinuationPolicy,
    #[serde(default)]
    pub l1_cache_counters: bool,
    #[serde(default)]
    pub tags: Vec<String>,
    pub link: Option<LinkExpectation>,
    pub workload: Workload,
    #[serde(default)]
    pub criteria: Criteria,
    #[serde(default)]
    pub evidence: EvidenceConfig,
    #[serde(default)]
    pub fixture_mutation: FixtureMutationConfig,
    #[serde(skip)]
    pub source: PathBuf,
}

const fn one_repetition() -> u8 {
    1
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LinkExpectation {
    pub phy: PhyExpectation,
    #[serde(default)]
    pub minimum_mcs: Option<u8>,
    #[serde(default)]
    pub guard_interval: HtGuardIntervalExpectation,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HtGuardIntervalExpectation {
    #[default]
    Any,
    Long,
    Short,
}

impl HtGuardIntervalExpectation {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Long => "long",
            Self::Short => "short",
        }
    }
}

#[derive(Debug)]
pub struct Catalog {
    scenarios: Vec<Scenario>,
}

#[cfg(test)]
mod tests;
