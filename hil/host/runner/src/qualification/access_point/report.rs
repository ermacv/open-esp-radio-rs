//! Serialized AP qualification report model.

use serde::Serialize;

use open_esp_radio_hil_protocol::{Ipv4Endpoint, SESSION_FLOW_CAPACITY};

use crate::{
    qualification::scenario::Direction,
    transport::{
        controlled_openwrt_client::{
            OpenWrtClientFixturePreparation, OpenWrtClientLinkEvidence,
            SecondaryClientProbeEvidence,
        },
        local_air_monitor::LocalAirMonitorEvidence,
    },
};

pub(super) const ACCESS_POINT_REPORT_SCHEMA: u8 = 6;

#[derive(Serialize)]
pub(super) struct AccessPointReport {
    pub(super) schema: u8,
    pub(super) fixture_preparation: Option<OpenWrtClientFixturePreparation>,
    pub(super) boots: Vec<BootReport>,
}

#[derive(Serialize)]
pub(super) struct BootReport {
    pub(super) boot: u8,
    pub(super) cycles: Vec<CycleReport>,
}

#[derive(Serialize)]
pub(super) struct CycleReport {
    pub(super) cycle: u8,
    pub(super) traffic: TrafficReport,
    pub(super) secondary_client: Option<SecondaryClientProbeEvidence>,
    pub(super) primary_client_link: Option<OpenWrtClientLinkEvidence>,
    pub(super) secondary_client_link: Option<OpenWrtClientLinkEvidence>,
    pub(super) independent_air: Option<LocalAirMonitorEvidence>,
    /// Intrusive driver-internal evidence is absent from observer-free
    /// performance images. An omitted field is deliberately different from a
    /// diagnostic snapshot whose counters all happened to be zero.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) access_point: Option<open_esp_radio_hil_protocol::WifiAccessPointEvidence>,
    /// Diagnostic-only comparison of stack classification with the AP role's
    /// independently admitted peer identity. This never changes admission.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) egress_identity: Option<ApEgressIdentityReport>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub(super) struct ApEgressIdentityReport {
    pub(super) exact: u64,
    pub(super) unclassified: u64,
    pub(super) non_associated: u64,
    pub(super) role_unbound: u64,
    pub(super) interface_mismatch: u64,
    pub(super) peer_slot_mismatch: u64,
    pub(super) peer_generation_mismatch: u64,
    pub(super) traffic_class_mismatch: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) terminal_current_aggregates: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) terminal_current_frames: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) terminal_stale_aggregates: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) terminal_stale_frames: Option<u64>,
}

impl ApEgressIdentityReport {
    pub(super) fn all_from_log(log: &str) -> Result<Vec<Self>, String> {
        log.lines()
            .filter(|line| line.starts_with("ORC0TXI ") || line.contains(" ORC0TXI "))
            .map(Self::from_line)
            .collect()
    }

    fn from_line(line: &str) -> Result<Self, String> {
        let terminal = [
            optional_numeric_field(line, "terminal_current_aggregates")?,
            optional_numeric_field(line, "terminal_current_frames")?,
            optional_numeric_field(line, "terminal_stale_aggregates")?,
            optional_numeric_field(line, "terminal_stale_frames")?,
        ];
        if terminal.iter().any(Option::is_some) && terminal.iter().any(Option::is_none) {
            return Err(format!(
                "ORC0TXI terminal identity fields must be all present or all absent in {line:?}"
            ));
        }
        Ok(Self {
            exact: numeric_field(line, "exact")?,
            unclassified: numeric_field(line, "unclassified")?,
            non_associated: numeric_field(line, "non_associated")?,
            role_unbound: numeric_field(line, "role_unbound")?,
            interface_mismatch: numeric_field(line, "interface_mismatch")?,
            peer_slot_mismatch: numeric_field(line, "peer_slot_mismatch")?,
            peer_generation_mismatch: numeric_field(line, "peer_generation_mismatch")?,
            traffic_class_mismatch: numeric_field(line, "traffic_class_mismatch")?,
            terminal_current_aggregates: terminal[0],
            terminal_current_frames: terminal[1],
            terminal_stale_aggregates: terminal[2],
            terminal_stale_frames: terminal[3],
        })
    }
}

fn numeric_field(line: &str, key: &str) -> Result<u64, String> {
    optional_numeric_field(line, key)?
        .ok_or_else(|| format!("ORC0TXI field {key:?} is missing in {line:?}"))
}

fn optional_numeric_field(line: &str, key: &str) -> Result<Option<u64>, String> {
    let value = line.split_ascii_whitespace().find_map(|token| {
        let (candidate, value) = token.split_once('=')?;
        (candidate == key).then_some(value)
    });
    value
        .map(|value| {
            value
                .parse()
                .map_err(|error| format!("invalid ORC0TXI field {key:?}={value:?}: {error}"))
        })
        .transpose()
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(super) enum TrafficReport {
    None,
    Icmp {
        transmitted: u16,
        received: u16,
        lost: u16,
        p50_micros: u64,
        p95_micros: u64,
        p99_micros: u64,
    },
    Udp(SessionReport),
    UdpMultiClient(Box<MultiClientSessionReport>),
    Tcp(SessionReport),
}

#[derive(Serialize)]
pub(super) struct SessionReport {
    pub(super) direction: Direction,
    pub(super) rx_bytes: u64,
    pub(super) tx_bytes: u64,
    pub(super) rx_units: u64,
    pub(super) tx_units: u64,
    pub(super) elapsed_micros: u64,
}

#[derive(Serialize)]
pub(super) struct MultiClientSessionReport {
    pub(super) direction: Direction,
    pub(super) aggregate: SessionReport,
    pub(super) flows: [MultiClientFlowReport; SESSION_FLOW_CAPACITY],
}

#[derive(Clone, Copy, Serialize)]
pub(super) struct MultiClientFlowReport {
    pub(super) flow_id: u8,
    pub(super) peer: Ipv4Endpoint,
    pub(super) rx_bytes: u64,
    pub(super) tx_bytes: u64,
    pub(super) rx_units: u64,
    pub(super) tx_units: u64,
    pub(super) elapsed_micros: u64,
    pub(super) rx_bps: u64,
    pub(super) tx_bps: u64,
    pub(super) host_tx_started_at_zero: Option<bool>,
    pub(super) host_tx_missing: Option<u64>,
    pub(super) host_tx_reordered: Option<u64>,
    pub(super) host_tx_duplicates: Option<u64>,
    pub(super) host_tx_maximum_interarrival_us: Option<u64>,
    pub(super) host_tx_sequence_after_maximum_interarrival: Option<u32>,
}
