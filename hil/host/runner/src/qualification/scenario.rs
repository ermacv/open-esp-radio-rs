//! Typed host-owned HIL scenario catalog.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use open_esp_radio_hil_protocol::{
    WifiDataPlanePlacement, WifiRxAdmissionPolicy, WifiRxChecksumPolicy,
};
use serde::{Deserialize, Serialize};

use crate::Result;

pub const SCENARIO_SCHEMA: u16 = 3;
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
    DiagnosticTaskPoll,
    DiagnosticCore0RxCycles,
    DiagnosticRxDelivery,
    DiagnosticIeee802154EventStatus,
    DiagnosticIeee802154EdEvent,
}

impl ImageClass {
    pub const ALL: [Self; 10] = [
        Self::BootSmoke,
        Self::Performance,
        Self::Correctness,
        Self::DiagnosticMacIrq,
        Self::DiagnosticTaskResidence,
        Self::DiagnosticTaskPoll,
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
            Self::DiagnosticTaskPoll => "diagnostic-task-poll",
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
            Self::DiagnosticTaskPoll => {
                "open-radio-hil,psram-task-stack,task-poll-telemetry,code-psram,profile-psram-data"
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
            | Self::DiagnosticTaskPoll
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
            Self::BootSmoke | Self::Performance | Self::DiagnosticTaskResidence
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
        traffic: AccessPointTraffic,
    },
    /// One equal-offer UDP bidirectional session on each endpoint of a live
    /// same-channel STA+AP epoch.
    StationAccessPoint {
        timeout_seconds: u16,
        duration_seconds: u16,
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

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Criteria {
    pub exact_delivery: bool,
    pub minimum_rx_bps: Option<u64>,
    pub minimum_tx_bps: Option<u64>,
    pub minimum_combined_bps: Option<u64>,
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
    pub independent_laptop_monitor_rx: bool,
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
    pub rx_admission: WifiRxAdmissionPolicy,
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

impl Scenario {
    fn validate(&self) -> Result<()> {
        if self.schema != SCENARIO_SCHEMA {
            return Err(format!(
                "{}: scenario schema {} is unsupported (expected {SCENARIO_SCHEMA})",
                self.source.display(),
                self.schema
            )
            .into());
        }
        if self.id.is_empty()
            || !self
                .id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(format!(
                "{}: invalid scenario id `{}`",
                self.source.display(),
                self.id
            )
            .into());
        }
        if self.description.trim().is_empty() {
            return Err(format!("{}: scenario description is empty", self.source.display()).into());
        }
        bounded(self.repetitions, 1, 20, self, "repetitions")?;
        if self.rx_checksum == WifiRxChecksumPolicy::AssumeValidDiagnostic
            && (self.image != ImageClass::DiagnosticTaskPoll
                || !matches!(
                    self.workload,
                    Workload::Udp {
                        direction: Direction::Rx,
                        ..
                    }
                ))
        {
            return Err(format!(
                "{}: assume-valid-diagnostic RX checksum policy is restricted to the UDP RX task-poll diagnostic",
                self.source.display()
            )
            .into());
        }
        if self.rx_admission == WifiRxAdmissionPolicy::DeferredReadyDiagnostic
            && self.image != ImageClass::DiagnosticCore0RxCycles
        {
            return Err(format!(
                "{}: deferred-ready-diagnostic RX admission is restricted to the Core0 RX cycle diagnostic",
                self.source.display()
            )
            .into());
        }
        if self.l1_cache_counters && self.image != ImageClass::DiagnosticCore0RxCycles {
            return Err(format!(
                "{}: L1 cache counters are restricted to the Core0 RX cycle diagnostic",
                self.source.display()
            )
            .into());
        }
        let boot_smoke_image = self.image == ImageClass::BootSmoke;
        if boot_smoke_image && !matches!(self.workload, Workload::BootSmoke) {
            return Err(format!(
                "{}: boot-smoke image accepts only boot-smoke workload",
                self.source.display()
            )
            .into());
        }
        if !boot_smoke_image && matches!(self.workload, Workload::BootSmoke) {
            return Err(format!(
                "{}: boot-smoke workload requires boot-smoke image",
                self.source.display()
            )
            .into());
        }
        let event_status_image = self.image == ImageClass::DiagnosticIeee802154EventStatus;
        let event_status_workload = matches!(self.workload, Workload::Ieee802154EventStatus { .. });
        if event_status_image != event_status_workload {
            return Err(format!(
                "{}: IEEE 802.15.4 EVENT_STATUS workload and diagnostic image must be selected together",
                self.source.display()
            )
            .into());
        }
        let ed_event_image = self.image == ImageClass::DiagnosticIeee802154EdEvent;
        let ed_event_workload = matches!(self.workload, Workload::Ieee802154EdEvent { .. });
        if ed_event_image != ed_event_workload {
            return Err(format!(
                "{}: IEEE 802.15.4 ED event workload and diagnostic image must be selected together",
                self.source.display()
            )
            .into());
        }
        if self.evidence.openwrt_tx_monitor_rx
            && (!matches!(
                self.image,
                ImageClass::DiagnosticTaskResidence
                    | ImageClass::DiagnosticRxDelivery
                    | ImageClass::DiagnosticCore0RxCycles
            ) || !matches!(
                self.workload,
                Workload::Udp {
                    direction: Direction::Rx | Direction::Bidirectional,
                    ..
                }
            ))
        {
            return Err(format!(
                "{}: OpenWrt TX-monitor RX evidence requires an RX-bearing UDP diagnostic with residence, delivery, or Core0-cycle evidence",
                self.source.display()
            )
            .into());
        }
        if self.evidence.independent_laptop_monitor_rx && !self.evidence.openwrt_tx_monitor_rx {
            return Err(format!(
                "{}: independent laptop RX evidence requires OpenWrt TX-monitor evidence for correlation",
                self.source.display()
            )
            .into());
        }
        if let Some(link) = self.link
            && !self.image.requires_driver_observation()
            && (link.minimum_mcs.is_some()
                || link.guard_interval != HtGuardIntervalExpectation::Any)
        {
            return Err(format!(
                "{}: per-frame MCS/GI requirements require a driver-observation image",
                self.source.display(),
            )
            .into());
        }
        if self.fixture_mutation.openwrt_fixed_guard_interval != HtGuardIntervalExpectation::Any {
            let Some(link) = self.link else {
                return Err(format!(
                    "{}: OpenWrt fixed-GI mutation requires an HT link expectation",
                    self.source.display(),
                )
                .into());
            };
            if !matches!(link.phy, PhyExpectation::Ht20 | PhyExpectation::Ht40)
                || link.guard_interval != self.fixture_mutation.openwrt_fixed_guard_interval
            {
                return Err(format!(
                    "{}: OpenWrt fixed-GI mutation must equal the strict HT guard-interval expectation",
                    self.source.display(),
                )
                .into());
            }
            if !self.evidence.openwrt_tx_monitor_rx || !self.evidence.independent_laptop_monitor_rx
            {
                return Err(format!(
                    "{}: OpenWrt fixed-GI mutation requires AP and independent air evidence",
                    self.source.display(),
                )
                .into());
            }
        }
        if self.isolation == Isolation::MatrixSession {
            return Err(format!(
                "{}: matrix-session requires a multi-cell workload, which schema {SCENARIO_SCHEMA} does not define",
                self.source.display()
            )
            .into());
        }
        let station_link_required = matches!(
            self.workload,
            Workload::Udp { .. }
                | Workload::Tcp { .. }
                | Workload::Icmp { .. }
                | Workload::StationReconnect { .. }
                | Workload::StationApLoss { .. }
                | Workload::StationApAbsence { .. }
                | Workload::WifiRole { .. }
                | Workload::MonitorCapture { .. }
                | Workload::StationAccessPoint { .. }
                | Workload::StationAccessPointReconnect { .. }
        );
        let link_allowed =
            station_link_required || matches!(self.workload, Workload::AccessPoint { .. });
        if (station_link_required && self.link.is_none()) || (!link_allowed && self.link.is_some())
        {
            return Err(format!(
                "{}: this workload has an invalid `[link]` expectation",
                self.source.display()
            )
            .into());
        }
        if let Some(link) = self.link
            && let Some(minimum_mcs) = link.minimum_mcs
        {
            if self.image == ImageClass::Performance {
                return Err(format!(
                    "{}: performance images do not collect the driver observation required by minimum_mcs",
                    self.source.display(),
                )
                .into());
            }
            let maximum_mcs = match link.phy {
                PhyExpectation::Ht20 | PhyExpectation::Ht40 => 7,
                PhyExpectation::He20 => 9,
            };
            if minimum_mcs > maximum_mcs {
                return Err(format!(
                    "{}: minimum_mcs={minimum_mcs} exceeds the {} capability MCS{maximum_mcs}",
                    self.source.display(),
                    link.phy.id(),
                )
                .into());
            }
        }
        if let Some(link) = self.link
            && link.guard_interval != HtGuardIntervalExpectation::Any
        {
            if !matches!(link.phy, PhyExpectation::Ht20 | PhyExpectation::Ht40) {
                return Err(format!(
                    "{}: guard_interval={} is valid only for an HT link",
                    self.source.display(),
                    link.guard_interval.id(),
                )
                .into());
            }
            if !self.image.requires_driver_observation() {
                return Err(format!(
                    "{}: strict guard_interval requires complete target-side driver observation",
                    self.source.display(),
                )
                .into());
            }
        }
        match &self.workload {
            Workload::BootSmoke => {}
            Workload::Timebase {
                boots,
                intervals,
                period_millis,
            } => {
                bounded(*boots, 1, 20, self, "boots")?;
                bounded(*intervals, 2, 100, self, "intervals")?;
                bounded(*period_millis, 1, 1_000, self, "period_millis")?;
            }
            Workload::Ieee802154EventStatus {
                boots,
                poll_limit,
                timer_threshold,
            } => {
                bounded(*boots, 1, 20, self, "boots")?;
                bounded(*poll_limit, 1, 1_000_000, self, "poll_limit")?;
                bounded(*timer_threshold, 1, 1_000, self, "timer_threshold")?;
                if self.criteria != Criteria::default() {
                    return self.criteria_error(
                        "IEEE 802.15.4 EVENT_STATUS diagnostic does not accept network criteria",
                    );
                }
                if self.evidence != EvidenceConfig::default() {
                    return self.criteria_error(
                        "IEEE 802.15.4 EVENT_STATUS diagnostic does not accept external Wi-Fi evidence",
                    );
                }
            }
            Workload::Ieee802154EdEvent {
                boots,
                poll_limit,
                timer_threshold,
            } => {
                bounded(*boots, 1, 20, self, "boots")?;
                bounded(*poll_limit, 1, 1_000_000, self, "poll_limit")?;
                bounded(*timer_threshold, 1, 1_000, self, "timer_threshold")?;
                if self.criteria != Criteria::default() {
                    return self.criteria_error(
                        "IEEE 802.15.4 ED event diagnostic does not accept network criteria",
                    );
                }
                if self.evidence != EvidenceConfig::default() {
                    return self.criteria_error(
                        "IEEE 802.15.4 ED event diagnostic does not accept external Wi-Fi evidence",
                    );
                }
            }
            Workload::Udp {
                direction,
                duration_seconds,
                rx_rate_bps,
                tx_rate_bps,
                payload_bytes,
                ..
            } => {
                bounded(*duration_seconds, 5, 300, self, "duration_seconds")?;
                if self.image == ImageClass::DiagnosticCore0RxCycles
                    && *duration_seconds > CORE0_RX_CYCLE_MAX_DURATION_SECONDS
                {
                    return Err(format!(
                        "{}: Core0 cycle diagnostic duration {} exceeds the u32-safe {} second interval",
                        self.source.display(),
                        duration_seconds,
                        CORE0_RX_CYCLE_MAX_DURATION_SECONDS,
                    )
                    .into());
                }
                bounded(*payload_bytes, 64, 1472, self, "payload_bytes")?;
                validate_direction_rates(*direction, *rx_rate_bps, *tx_rate_bps, self)?;
            }
            Workload::Tcp {
                direction,
                duration_seconds,
                rx_rate_bps,
                tx_rate_bps,
                chunk_bytes,
            } => {
                bounded(*duration_seconds, 5, 300, self, "duration_seconds")?;
                bounded(*chunk_bytes, 64, 32768, self, "chunk_bytes")?;
                validate_direction_rates(*direction, *rx_rate_bps, *tx_rate_bps, self)?;
            }
            Workload::Icmp {
                count,
                interval_ms,
                timeout_ms,
                payload_bytes,
            } => {
                bounded(*count, 1, u16::MAX, self, "count")?;
                bounded(*interval_ms, 1, 10_000, self, "interval_ms")?;
                bounded(*timeout_ms, 1, 60_000, self, "timeout_ms")?;
                bounded(*payload_bytes, 0, 1400, self, "payload_bytes")?;
            }
            Workload::StationReconnect {
                cycles,
                boots,
                timeout_seconds,
            } => {
                bounded(*cycles, 1, 8, self, "cycles")?;
                bounded(*boots, 1, 100, self, "boots")?;
                bounded(*timeout_seconds, 10, 300, self, "timeout_seconds")?;
            }
            Workload::StationApLoss { timeout_seconds }
            | Workload::StationApAbsence { timeout_seconds } => {
                bounded(*timeout_seconds, 30, 300, self, "timeout_seconds")?;
            }
            Workload::WifiRole {
                timeout_seconds,
                channel,
                dwell_seconds,
                snapshot_length,
                ..
            } => {
                bounded(*timeout_seconds, 10, 180, self, "timeout_seconds")?;
                if let Some(channel) = channel {
                    bounded(*channel, 1, 13, self, "channel")?;
                }
                if let Some(seconds) = dwell_seconds {
                    bounded(*seconds, 1, 30, self, "dwell_seconds")?;
                }
                if let Some(length) = snapshot_length {
                    bounded(*length, 0, 2304, self, "snapshot_length")?;
                }
            }
            Workload::MonitorCapture {
                timeout_seconds,
                duration_seconds,
                channel,
                snapshot_length,
            } => {
                bounded(*timeout_seconds, 10, 180, self, "timeout_seconds")?;
                bounded(*duration_seconds, 1, 30, self, "duration_seconds")?;
                if let Some(channel) = channel {
                    bounded(*channel, 1, 13, self, "channel")?;
                }
                bounded(*snapshot_length, 0, 2304, self, "snapshot_length")?;
            }
            Workload::AccessPoint {
                cycles,
                boots,
                timeout_seconds,
                client,
                traffic,
            } => {
                bounded(*cycles, 2, 8, self, "cycles")?;
                bounded(*boots, 1, 20, self, "boots")?;
                bounded(*timeout_seconds, 20, 180, self, "timeout_seconds")?;
                validate_access_point_traffic(traffic, self)?;
                if *client == AccessPointClient::OpenWrt {
                    if self.criteria.minimum_concurrent_ap_clients.unwrap_or(1) != 1 {
                        return self.criteria_error(
                            "an OpenWrt primary AP client requires exactly one concurrent client",
                        );
                    }
                    if !matches!(
                        traffic,
                        AccessPointTraffic::Udp { .. } | AccessPointTraffic::Icmp { .. }
                    ) {
                        return self.criteria_error(
                            "the OpenWrt primary AP client supports UDP and ICMP workloads",
                        );
                    }
                }
            }
            Workload::StationAccessPoint {
                timeout_seconds,
                duration_seconds,
                rate_bps_per_flow,
                minimum_bps_per_flow,
                maximum_fairness_skew_percent,
                payload_bytes,
            } => {
                bounded(*timeout_seconds, 30, 180, self, "timeout_seconds")?;
                bounded(*duration_seconds, 5, 120, self, "duration_seconds")?;
                bounded(*payload_bytes, 64, 1472, self, "payload_bytes")?;
                bounded(
                    *maximum_fairness_skew_percent,
                    1,
                    100,
                    self,
                    "maximum_fairness_skew_percent",
                )?;
                if !(100_000..=100_000_000).contains(rate_bps_per_flow) {
                    return self.criteria_error(
                        "rate_bps_per_flow must be within 100 Kbit/s..=100 Mbit/s",
                    );
                }
                if *minimum_bps_per_flow == 0 || minimum_bps_per_flow > rate_bps_per_flow {
                    return self.criteria_error(
                        "minimum_bps_per_flow must be nonzero and cannot exceed the offer",
                    );
                }
            }
            Workload::StationAccessPointReconnect { timeout_seconds } => {
                bounded(*timeout_seconds, 30, 180, self, "timeout_seconds")?;
            }
        }
        if self.image == ImageClass::Performance {
            if self.criteria.exact_delivery {
                return self.criteria_error(
                    "performance images cannot claim exact delivery without driver observation",
                );
            }
            if self.criteria.require_no_beacon_loss {
                return self.criteria_error(
                    "performance images cannot use the driver-observed beacon-loss verdict",
                );
            }
            if !matches!(
                self.workload,
                Workload::Udp { .. }
                    | Workload::Tcp { .. }
                    | Workload::Icmp { .. }
                    | Workload::AccessPoint { .. }
                    | Workload::StationAccessPoint { .. }
            ) {
                return self.criteria_error(
                    "performance images admit only externally measured network workloads",
                );
            }
        }
        self.validate_criteria()?;
        Ok(())
    }

    fn validate_criteria(&self) -> Result<()> {
        let (rx_offer, tx_offer, udp, bidirectional_udp, icmp, station_data_plane) = match &self
            .workload
        {
            Workload::Udp {
                direction,
                rx_rate_bps,
                tx_rate_bps,
                ..
            } => (
                *rx_rate_bps,
                *tx_rate_bps,
                true,
                *direction == Direction::Bidirectional,
                false,
                true,
            ),
            Workload::Tcp {
                rx_rate_bps,
                tx_rate_bps,
                ..
            } => (*rx_rate_bps, *tx_rate_bps, false, false, false, true),
            Workload::Icmp { .. } => (None, None, false, false, true, true),
            Workload::StationReconnect { .. } => (None, None, false, false, false, true),
            Workload::StationAccessPoint { .. } => (None, None, false, false, false, true),
            Workload::StationAccessPointReconnect { .. } => (None, None, false, false, false, true),
            Workload::AccessPoint { traffic, .. } => match traffic {
                AccessPointTraffic::Udp {
                    direction,
                    rx_rate_bps,
                    tx_rate_bps,
                    ..
                } => (
                    *rx_rate_bps,
                    *tx_rate_bps,
                    true,
                    *direction == Direction::Bidirectional,
                    false,
                    false,
                ),
                AccessPointTraffic::Tcp {
                    rx_rate_bps,
                    tx_rate_bps,
                    ..
                } => (*rx_rate_bps, *tx_rate_bps, false, false, false, false),
                AccessPointTraffic::Icmp { .. } => (None, None, false, false, true, false),
                AccessPointTraffic::None => (None, None, false, false, false, false),
            },
            _ => (None, None, false, false, false, false),
        };
        if self.criteria.exact_delivery && !udp {
            return self.criteria_error("exact_delivery is valid only for UDP workloads");
        }
        if let Some(floor) = self.criteria.minimum_rx_bps {
            let offer = rx_offer.ok_or_else(|| {
                format!(
                    "{}: minimum_rx_bps requires an RX data plane",
                    self.source.display()
                )
            })?;
            if floor > offer {
                return self.criteria_error("minimum_rx_bps cannot exceed rx_rate_bps");
            }
        }
        if let Some(floor) = self.criteria.minimum_tx_bps {
            if tx_offer.is_none() {
                return self.criteria_error("minimum_tx_bps requires a TX data plane");
            }
            if tx_offer.is_some_and(|offer| floor > offer) {
                return self.criteria_error("minimum_tx_bps cannot exceed tx_rate_bps");
            }
        }
        if let Some(floor) = self.criteria.minimum_combined_bps {
            if !bidirectional_udp {
                return self
                    .criteria_error("minimum_combined_bps requires a bidirectional UDP workload");
            }
            let offered_sum = rx_offer
                .and_then(|rx| tx_offer.and_then(|tx| rx.checked_add(tx)))
                .ok_or_else(|| {
                    format!(
                        "{}: minimum_combined_bps requires bounded RX and TX offers",
                        self.source.display()
                    )
                })?;
            if floor > offered_sum {
                return self
                    .criteria_error("minimum_combined_bps cannot exceed the RX+TX offered rate");
            }
        }
        if let Some(maximum) = self.criteria.maximum_idle_channel_utilization_255 {
            if maximum == 0 {
                return self.criteria_error("maximum_idle_channel_utilization_255 must be nonzero");
            }
            if !matches!(
                self.workload,
                Workload::Udp {
                    direction: Direction::Rx,
                    ..
                }
            ) {
                return self.criteria_error(
                    "maximum_idle_channel_utilization_255 requires a station UDP RX workload",
                );
            }
        }
        if (self.criteria.maximum_lost.is_some() || self.criteria.maximum_p95_ms.is_some()) && !icmp
        {
            return self.criteria_error("loss and latency criteria are valid only for ICMP");
        }
        if self.criteria.require_no_beacon_loss
            && !station_data_plane
            && !matches!(self.workload, Workload::AccessPoint { .. })
            && !matches!(self.workload, Workload::StationAccessPoint { .. })
        {
            return self.criteria_error(
                "require_no_beacon_loss requires a station or access-point data-plane workload",
            );
        }
        if let Some(minimum) = self.criteria.minimum_concurrent_ap_clients {
            if !matches!(self.workload, Workload::AccessPoint { .. }) {
                return self.criteria_error(
                    "minimum_concurrent_ap_clients requires an access-point workload",
                );
            }
            if !(1..=2).contains(&minimum) {
                return self
                    .criteria_error("current physical HIL supports 1..=2 concurrent AP clients");
            }
        }
        Ok(())
    }

    fn criteria_error<T>(&self, message: &str) -> Result<T> {
        Err(format!("{}: {message}", self.source.display()).into())
    }
}

const fn one_repetition() -> u8 {
    1
}

fn validate_access_point_traffic(traffic: &AccessPointTraffic, scenario: &Scenario) -> Result<()> {
    match traffic {
        AccessPointTraffic::None => Ok(()),
        AccessPointTraffic::Icmp {
            count,
            interval_ms,
            timeout_ms,
            payload_bytes,
        } => {
            bounded(*count, 1, u16::MAX, scenario, "traffic.count")?;
            bounded(*interval_ms, 1, 10_000, scenario, "traffic.interval_ms")?;
            bounded(*timeout_ms, 1, 60_000, scenario, "traffic.timeout_ms")?;
            bounded(*payload_bytes, 0, 1400, scenario, "traffic.payload_bytes")
        }
        AccessPointTraffic::Udp {
            direction,
            duration_seconds,
            rx_rate_bps,
            tx_rate_bps,
            payload_bytes,
        } => {
            bounded(
                *duration_seconds,
                5,
                300,
                scenario,
                "traffic.duration_seconds",
            )?;
            bounded(*payload_bytes, 64, 1472, scenario, "traffic.payload_bytes")?;
            validate_direction_rates(*direction, *rx_rate_bps, *tx_rate_bps, scenario)
        }
        AccessPointTraffic::Tcp {
            direction,
            duration_seconds,
            rx_rate_bps,
            tx_rate_bps,
            chunk_bytes,
        } => {
            bounded(
                *duration_seconds,
                5,
                300,
                scenario,
                "traffic.duration_seconds",
            )?;
            bounded(*chunk_bytes, 64, 32_768, scenario, "traffic.chunk_bytes")?;
            validate_direction_rates(*direction, *rx_rate_bps, *tx_rate_bps, scenario)
        }
    }
}

fn bounded<T>(value: T, minimum: T, maximum: T, scenario: &Scenario, field: &str) -> Result<()>
where
    T: Ord + std::fmt::Display,
{
    if value < minimum || value > maximum {
        return Err(format!(
            "{}: {field}={value} is outside {minimum}..={maximum}",
            scenario.source.display()
        )
        .into());
    }
    Ok(())
}

fn validate_direction_rates(
    direction: Direction,
    rx: Option<u64>,
    tx: Option<u64>,
    scenario: &Scenario,
) -> Result<()> {
    let valid = match direction {
        Direction::Rx => rx.is_some() && tx.is_none(),
        Direction::Tx => rx.is_none() && tx.is_some(),
        Direction::Bidirectional => rx.is_some() && tx.is_some(),
    };
    if !valid {
        return Err(format!(
            "{}: offered rates do not match {:?} workload",
            scenario.source.display(),
            direction
        )
        .into());
    }
    Ok(())
}

#[derive(Debug)]
pub struct Catalog {
    scenarios: Vec<Scenario>,
}

impl Catalog {
    pub fn load(directory: &Path) -> Result<Self> {
        let mut paths = fs::read_dir(directory)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        paths.retain(|path| {
            path.extension()
                .is_some_and(|extension| extension == "toml")
        });
        paths.sort();
        let mut scenarios = Vec::with_capacity(paths.len());
        let mut ids = BTreeSet::new();
        for path in paths {
            let text = fs::read_to_string(&path)?;
            let mut scenario: Scenario =
                toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
            scenario.source = path;
            scenario.validate()?;
            if !ids.insert(scenario.id.clone()) {
                return Err(format!("duplicate HIL scenario id `{}`", scenario.id).into());
            }
            scenarios.push(scenario);
        }
        if scenarios.is_empty() {
            return Err(format!("scenario catalog is empty: {}", directory.display()).into());
        }
        Ok(Self { scenarios })
    }

    pub fn all(&self) -> &[Scenario] {
        &self.scenarios
    }

    pub fn get(&self, id: &str) -> Result<&Scenario> {
        self.scenarios
            .iter()
            .find(|scenario| scenario.id == id)
            .ok_or_else(|| format!("unknown HIL scenario `{id}`").into())
    }
}

#[cfg(test)]
mod tests {
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
        assert_eq!(
            catalog
                .get("udp-bidirectional-ht40-single-core-baseline")
                .unwrap()
                .data_plane,
            WifiDataPlanePlacement::SingleCore
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
        assert!(
            validate_direction_rates(Direction::Bidirectional, Some(1), Some(1), scenario).is_ok()
        );
        assert!(
            validate_direction_rates(Direction::Bidirectional, Some(1), None, scenario).is_err()
        );
    }
}
