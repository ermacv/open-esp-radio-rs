#![forbid(unsafe_code)]

mod bidirectional;
#[cfg(feature = "core0-rx-cycle-telemetry")]
mod cache_performance;
mod evidence;
mod reporting;
mod runtime;
mod tcp;
mod udp;

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::Timer;
use open_esp_radio_hil_protocol::SessionLinkRequirements;
use open_esp_radio_hil_protocol::Transport;
use open_esp_radio_hil_protocol::WifiNetworkInterface;

use crate::console::{ActiveSession, receive_session_start};

pub(super) type SessionChannel = Channel<CriticalSectionRawMutex, ActiveSession, 1>;

async fn run_session_dispatcher(
    station_udp: &'static SessionChannel,
    station_tcp: &'static SessionChannel,
    access_point_udp: &'static SessionChannel,
    access_point_tcp: &'static SessionChannel,
) -> ! {
    loop {
        let session = receive_session_start().await;
        let (udp, tcp) = match session.config.network_interface {
            WifiNetworkInterface::Station => (station_udp, station_tcp),
            WifiNetworkInterface::AccessPoint => (access_point_udp, access_point_tcp),
        };
        match session.config.transport {
            Transport::Udp => udp.send(session).await,
            Transport::Tcp => tcp.send(session).await,
        }
    }
}

/// Wait until the production control/TX owners have proved every link
/// property requested by a measured session. There is deliberately no
/// fallback timeout into a slower mode: the host owns the qualification
/// timeout and must classify an unavailable AddBA precondition separately
/// from transport performance.
async fn wait_session_link_requirements(
    requirements: SessionLinkRequirements,
    network_interface: WifiNetworkInterface,
) {
    let Some(tid) = requirements.tx_block_ack_tid else {
        return;
    };
    assert_eq!(
        network_interface,
        WifiNetworkInterface::Station,
        "only the station role currently exports a functional TX BlockAck status",
    );
    let bit = 1_u32
        .checked_shl(u32::from(tid))
        .expect("validated BlockAck TID fits the functional status bitmap");
    while crate::product_hil::STATION_TX_BLOCK_ACK_OPERATIONAL_TIDS
        .load(core::sync::atomic::Ordering::Acquire)
        & bit
        == 0
    {
        Timer::after_millis(10).await;
    }
}

pub(super) use bidirectional::{
    BidirectionalResultChannel, BidirectionalSessionChannel, OpenRadioBidirectionalDirection,
    OpenRadioBidirectionalResult, complete_open_radio_bidirectional_direction,
    run_open_radio_bidirectional_session_coordinator,
};
pub(super) use evidence::{UdpSequenceEvidence, iperf2_udp_sequence};
pub(super) use reporting::{
    aggregate_tx_evidence, log_open_radio_ampdu_interval, log_open_radio_rx_pipeline_interval,
    log_open_radio_task_poll_interval, observe_open_radio_task_polls,
};
#[cfg(feature = "core0-rx-coarse-telemetry")]
pub(super) use reporting::{
    log_open_radio_core0_rx_coarse, log_open_radio_core1_tx_phases,
    observe_open_radio_core0_task_polls,
};
#[cfg(feature = "core0-rx-cycle-telemetry")]
pub(super) use reporting::{
    log_open_radio_core0_rx_cycles, log_open_radio_core0_rx_service_histogram,
    observe_open_radio_core0_task_polls,
};
pub(in crate::product_hil) use runtime::{start_connected_traffic, start_traffic_dispatcher};
pub(super) use tcp::{TcpBenchmarkConfig, run_open_radio_tcp_benchmark};
pub(super) use udp::{
    UdpRxBenchmarkConfig, UdpRxSessionSource, UdpRxTelemetry, UdpSocketBuffers,
    UdpTxBenchmarkConfig, UdpTxSessionSource, run_open_radio_udp_rx_benchmark,
    run_open_radio_udp_tx_benchmark,
};
