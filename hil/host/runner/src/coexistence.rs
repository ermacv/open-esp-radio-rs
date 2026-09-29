//! The `[coexistence]` scenario table: Wi-Fi and Bluetooth LE traffic at
//! once on the joint image's shared radio.
//!
//! The target runs its Wi-Fi station and the Bluetooth LE GATT application
//! on one radio. The workload connects the Linux adapter to the GATT
//! application, then offers UDP to the station while an ATT echo load runs
//! over the Bluetooth connection for the same interval. Both radios must keep
//! working: the station must deliver at least the minimum UDP rate and the
//! echo load must complete its exchanges without a disconnect.

use std::{net::Ipv4Addr, path::Path, time::Duration};

use hil_bluetooth::workload::bluetooth::coexistence::Echo;
use hil_core::{
    context::Context,
    evidence::run::{Comparison, Measurement, MeasurementUnit, MeasurementVerdict},
    image::ImageClass,
    lab::{
        link::{PhyExpectation, WifiLabUse},
        requirements::Requirements,
    },
    scenario::{Plan, bounded},
    session::await_udp_rx_ready,
};
use hil_wifi::workload::traffic::{
    host_network::BenchmarkIpv4Route,
    paced_udp::{Config as PacedUdpConfig, send as send_paced_udp},
};
use oer_hil_protocol::{
    network::Completion, network::Direction, network::FlowConfig, network::SessionConfig,
    network::SessionFlowConfig, network::SessionLinkRequirements, network::Transport,
};
use serde::{Deserialize, Serialize};

use crate::Result;

const UDP_PORT: u16 = 4_323;
const DEVICE_READY_TIMEOUT: Duration = Duration::from_secs(45);
const CHECKS: [&str; 2] = ["coexistence.wifi.rx-rate", "coexistence.bluetooth.echoes"];

/// Concurrent station UDP receive and Bluetooth LE ATT echo traffic.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CoexistenceScenario {
    /// The station fixture link.
    pub phy: PhyExpectation,
    /// The shared traffic interval.
    pub duration_seconds: u8,
    /// UDP rate the host offers the station.
    pub rx_bps: u64,
    pub payload_bytes: u16,
    /// The station's minimum delivered UDP rate while Bluetooth runs.
    pub minimum_rx_bps: u64,
    /// The minimum completed ATT write/read echoes in the interval.
    pub minimum_echoes: u32,
}

impl CoexistenceScenario {
    pub(crate) fn validate(&self) -> Result<()> {
        bounded(self.duration_seconds, 4, 16, "duration_seconds")?;
        bounded(self.payload_bytes, 64, 1_472, "payload_bytes")?;
        if self.minimum_rx_bps == 0 || self.minimum_rx_bps > self.rx_bps {
            return Err("minimum_rx_bps must be positive and at most rx_bps".into());
        }
        if self.minimum_echoes == 0 {
            return Err("minimum_echoes must be positive".into());
        }
        Ok(())
    }

    pub(crate) fn plan(&self) -> Plan {
        Plan {
            requirements: Requirements {
                station_network: true,
                bluetooth_adapter: true,
                ..Requirements::default()
            },
            checks: CHECKS.to_vec(),
            wifi: WifiLabUse {
                link: Some(self.phy),
                ..WifiLabUse::default()
            },
            ..Plan::target_only(ImageClass::WifiBleCoex)
        }
    }

    pub(crate) fn run(&self, output: &Path, context: &Context<'_>) -> Result<()> {
        let adapter = context
            .lab
            .bluetooth_adapter
            .ok_or("set [bluetooth] adapter in the local lab configuration")?;
        let duration = Duration::from_secs(u64::from(self.duration_seconds));
        context.with_capture(output, |capture| {
            let ready = await_udp_rx_ready(
                capture,
                context.target(),
                Ipv4Addr::UNSPECIFIED,
                UDP_PORT,
                DEVICE_READY_TIMEOUT,
            )?;
            let route = BenchmarkIpv4Route::discover(ready.address, &context.lab.station_fixture)?;
            let mut echo = Echo::connect(capture, adapter, output)?;
            let session = capture.start_session(SessionConfig {
                network_interface: oer_hil_protocol::wifi::WifiNetworkInterface::Station,
                transport: Transport::Udp,
                direction: Direction::Rx,
                completion: Completion::DurationMillis(u32::try_from(duration.as_millis())?),
                flows: [
                    Some(SessionFlowConfig {
                        flow_id: 0,
                        peer: None,
                        target_rx: Some(FlowConfig {
                            payload_bytes: self.payload_bytes,
                            offered_rate_bps: Some(self.rx_bps),
                            pacing_group_datagrams: None,
                        }),
                        target_tx: None,
                        payload_identity: None,
                    }),
                    None,
                ],
                link_requirements: SessionLinkRequirements::NONE,
            })?;
            // The echo load runs on its own thread for the interval the host
            // offers UDP; the capture serves it only after the interval.
            let (host, echoes) = std::thread::scope(|scope| {
                let echoes = scope.spawn(|| echo.run(duration));
                let host = send_paced_udp(PacedUdpConfig {
                    address: ready.address,
                    port: UDP_PORT,
                    rate_bps: self.rx_bps,
                    duration,
                    payload: usize::from(self.payload_bytes),
                });
                (host, echoes.join())
            });
            let echoes = echoes.map_err(|_| "echo load panicked")??;
            route.verify_socket_source(host?.source)?;
            let structured =
                capture.wait_for_session(session, duration + Duration::from_secs(10))?;
            capture.acknowledge_session(session)?;
            echo.finish()?;
            let rx_bps = structured
                .transport
                .rx_bytes
                .saturating_mul(8)
                .saturating_mul(1_000_000)
                .checked_div(structured.transport.elapsed_micros.max(1))
                .unwrap_or(0);
            self.evaluate(context, rx_bps, echoes.echoes)
        })
    }

    fn evaluate(&self, context: &Context<'_>, rx_bps: u64, echoes: u32) -> Result<()> {
        let measurements = vec![
            Measurement::observed(CHECKS[0], rx_bps, MeasurementUnit::BitsPerSecond)
                .evaluated(Comparison::AtLeast, self.minimum_rx_bps),
            Measurement::observed(CHECKS[1], u64::from(echoes), MeasurementUnit::Count)
                .evaluated(Comparison::AtLeast, u64::from(self.minimum_echoes)),
        ];
        let failed = measurements
            .iter()
            .filter(|measurement| measurement.verdict == Some(MeasurementVerdict::Failed))
            .map(|measurement| format!("{}={}", measurement.name, measurement.value))
            .collect::<Vec<_>>();
        context.measurements.record(measurements);
        eprintln!("coexistence rx_bps={rx_bps} echoes={echoes}");
        if !failed.is_empty() {
            return Err(format!("shared-radio traffic fell short: {}", failed.join(", ")).into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
