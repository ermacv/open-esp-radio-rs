#![forbid(unsafe_code)]

mod bidirectional;
mod evidence;
mod reporting;
mod runtime;
mod tcp;
mod udp;

use embassy_time::Timer;
use open_esp_radio_hil_esp32s31_telemetry::aggregate_tx::AggregateTxCounters;
use open_esp_radio_hil_protocol::SessionLinkRequirements;

/// Wait until the production control/TX owners have proved every link
/// property requested by a measured session. There is deliberately no
/// fallback timeout into a slower mode: the host owns the qualification
/// timeout and must classify an unavailable AddBA precondition separately
/// from transport performance.
async fn wait_session_link_requirements(
    requirements: SessionLinkRequirements,
    aggregate_counters: &AggregateTxCounters,
) {
    let Some(tid) = requirements.tx_block_ack_tid else {
        return;
    };
    while !aggregate_counters.snapshot().block_ack_operational(tid) {
        Timer::after_millis(10).await;
    }
}

pub(super) use bidirectional::{
    BidirectionalResultChannel, BidirectionalSessionChannel, OpenRadioBidirectionalDirection,
    complete_open_radio_bidirectional_direction, run_open_radio_bidirectional_session_coordinator,
};
pub(super) use evidence::{
    UdpSequenceEvidence, iperf2_udp_sequence, ipv4_udp_destination_port, ipv4_udp_sequence,
    public_qos_sequence,
};
pub(super) use reporting::{
    log_open_radio_ampdu_interval, log_open_radio_rx_pipeline_interval,
    log_open_radio_task_poll_interval, observe_open_radio_task_polls,
};
pub(in crate::radio_hil) use runtime::{RadioHilConnectedTrafficConfig, connected_traffic_task};
pub(super) use tcp::{TcpBenchmarkConfig, run_open_radio_tcp_benchmark};
pub(super) use udp::{
    UdpRxBenchmarkConfig, UdpRxSessionSource, UdpRxTelemetry, UdpSocketBuffers,
    UdpTxBenchmarkConfig, UdpTxSessionSource, run_open_radio_udp_rx_benchmark,
    run_open_radio_udp_tx_benchmark,
};
