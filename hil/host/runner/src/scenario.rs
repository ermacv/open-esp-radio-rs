//! Typed host-owned HIL scenario catalog.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use open_esp_radio_hil_protocol::WifiDataPlanePlacement;
use serde::{Deserialize, Serialize};

use crate::Result;

pub const SCENARIO_SCHEMA: u16 = 3;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImageClass {
    BootSmoke,
    Qualification,
    DiagnosticTaskPoll,
    DiagnosticRxDelivery,
}

impl ImageClass {
    pub const ALL: [Self; 4] = [
        Self::BootSmoke,
        Self::Qualification,
        Self::DiagnosticTaskPoll,
        Self::DiagnosticRxDelivery,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::BootSmoke => "boot-smoke",
            Self::Qualification => "qualification",
            Self::DiagnosticTaskPoll => "diagnostic-task-poll",
            Self::DiagnosticRxDelivery => "diagnostic-rx-delivery",
        }
    }

    pub const fn runtime_features(self) -> &'static str {
        match self {
            Self::BootSmoke => "boot-smoke,code-psram,profile-psram-data",
            Self::Qualification => "open-radio-hil,code-psram,profile-psram-data",
            Self::DiagnosticTaskPoll => {
                "open-radio-hil,task-poll-telemetry,network-scheduler-telemetry,single-core-diagnostic,code-psram,profile-psram-data"
            }
            Self::DiagnosticRxDelivery => {
                "open-radio-hil,rx-delivery-telemetry,code-psram,profile-psram-data"
            }
        }
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
        traffic: AccessPointTraffic,
    },
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Criteria {
    pub exact_delivery: bool,
    pub minimum_rx_bps: Option<u64>,
    pub minimum_tx_bps: Option<u64>,
    pub minimum_combined_bps: Option<u64>,
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
    pub tags: Vec<String>,
    pub link: Option<LinkExpectation>,
    pub workload: Workload,
    #[serde(default)]
    pub criteria: Criteria,
    #[serde(default)]
    pub evidence: EvidenceConfig,
    #[serde(skip)]
    pub source: PathBuf,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LinkExpectation {
    pub phy: PhyExpectation,
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
        if self.image == ImageClass::BootSmoke && !matches!(self.workload, Workload::BootSmoke) {
            return Err(format!(
                "{}: boot-smoke image accepts only boot-smoke workload",
                self.source.display()
            )
            .into());
        }
        if self.image != ImageClass::BootSmoke && matches!(self.workload, Workload::BootSmoke) {
            return Err(format!(
                "{}: boot-smoke workload requires boot-smoke image",
                self.source.display()
            )
            .into());
        }
        if self.image == ImageClass::DiagnosticTaskPoll
            && self.data_plane != WifiDataPlanePlacement::SingleCore
        {
            return Err(format!(
                "{}: diagnostic-task-poll image admits only single-core data-plane placement",
                self.source.display()
            )
            .into());
        }
        if self.evidence.openwrt_tx_monitor_rx
            && (self.image != ImageClass::DiagnosticRxDelivery
                || !matches!(
                    self.workload,
                    Workload::Udp {
                        direction: Direction::Bidirectional,
                        ..
                    }
                ))
        {
            return Err(format!(
                "{}: OpenWrt TX-monitor RX evidence requires the bidirectional UDP RX-delivery diagnostic",
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
        );
        if station_link_required != self.link.is_some() {
            return Err(format!(
                "{}: station workloads require exactly one `[link]` expectation",
                self.source.display()
            )
            .into());
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
            Workload::Udp {
                direction,
                duration_seconds,
                rx_rate_bps,
                tx_rate_bps,
                payload_bytes,
                ..
            } => {
                bounded(*duration_seconds, 5, 300, self, "duration_seconds")?;
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
                traffic,
            } => {
                bounded(*cycles, 2, 8, self, "cycles")?;
                bounded(*boots, 1, 20, self, "boots")?;
                bounded(*timeout_seconds, 20, 180, self, "timeout_seconds")?;
                validate_access_point_traffic(traffic, self)?;
            }
        }
        self.validate_criteria()?;
        Ok(())
    }

    fn validate_criteria(&self) -> Result<()> {
        let (rx_offer, tx_offer, udp, icmp, station_data_plane) = match &self.workload {
            Workload::Udp {
                rx_rate_bps,
                tx_rate_bps,
                ..
            } => (*rx_rate_bps, *tx_rate_bps, true, false, true),
            Workload::Tcp {
                rx_rate_bps,
                tx_rate_bps,
                ..
            } => (*rx_rate_bps, *tx_rate_bps, false, false, true),
            Workload::Icmp { .. } => (None, None, false, true, true),
            Workload::StationReconnect { .. } => (None, None, false, false, true),
            Workload::AccessPoint { traffic, .. } => match traffic {
                AccessPointTraffic::Udp {
                    rx_rate_bps,
                    tx_rate_bps,
                    ..
                } => (*rx_rate_bps, *tx_rate_bps, true, false, false),
                AccessPointTraffic::Tcp {
                    rx_rate_bps,
                    tx_rate_bps,
                    ..
                } => (*rx_rate_bps, *tx_rate_bps, false, false, false),
                AccessPointTraffic::Icmp { .. } => (None, None, false, true, false),
                AccessPointTraffic::None => (None, None, false, false, false),
            },
            _ => (None, None, false, false, false),
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
            if !matches!(
                self.workload,
                Workload::Udp {
                    direction: Direction::Bidirectional,
                    ..
                }
            ) {
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
        if (self.criteria.maximum_lost.is_some() || self.criteria.maximum_p95_ms.is_some()) && !icmp
        {
            return self.criteria_error("loss and latency criteria are valid only for ICMP");
        }
        if self.criteria.require_no_beacon_loss && !station_data_plane {
            return self
                .criteria_error("require_no_beacon_loss requires a station data-plane workload");
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
            validate_direction_rates(*direction, *rx_rate_bps, *tx_rate_bps, scenario)?;
            validate_ap_rates(*rx_rate_bps, *tx_rate_bps, scenario)
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
            validate_direction_rates(*direction, *rx_rate_bps, *tx_rate_bps, scenario)?;
            validate_ap_rates(*rx_rate_bps, *tx_rate_bps, scenario)
        }
    }
}

fn validate_ap_rates(rx: Option<u64>, tx: Option<u64>, scenario: &Scenario) -> Result<()> {
    for (name, rate) in [("traffic.rx_rate_bps", rx), ("traffic.tx_rate_bps", tx)] {
        if let Some(rate) = rate {
            bounded(rate, 100_000, 20_000_000, scenario, name)?;
        }
    }
    Ok(())
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
