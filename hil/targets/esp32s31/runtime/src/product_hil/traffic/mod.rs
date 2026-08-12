#![forbid(unsafe_code)]

mod bidirectional;
mod evidence;
mod reporting;
mod runtime;
mod tcp;
mod udp;

use embassy_futures::yield_now;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::{Duration, Instant, Timer};
use open_esp_radio_hil_esp32s31_telemetry::aggregate_tx::AggregateTxCounters;
use open_esp_radio_hil_protocol::SessionLinkRequirements;
use open_esp_radio_hil_protocol::Transport;

use crate::console::{ActiveSession, receive_session_start};

pub(super) type SessionChannel = Channel<CriticalSectionRawMutex, ActiveSession, 1>;

const WORKLOAD_SERVICE_BUDGET: Duration = Duration::from_micros(250);

/// Time-based cooperative boundary for HIL load generators and consumers.
/// The checkpoint is placed only after ownership of one socket unit has
/// completed; it is deliberately unrelated to A-MPDU or BlockAck dimensions.
pub(super) struct CooperativePollBudget {
    started: Instant,
}

impl CooperativePollBudget {
    pub(super) fn new() -> Self {
        Self {
            started: Instant::now(),
        }
    }

    pub(super) async fn checkpoint(&mut self) {
        if self.started.elapsed() >= WORKLOAD_SERVICE_BUDGET {
            yield_now().await;
            self.started = Instant::now();
        }
    }
}

async fn run_session_dispatcher(udp: &'static SessionChannel, tcp: &'static SessionChannel) -> ! {
    loop {
        let session = receive_session_start().await;
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
pub(super) use evidence::{UdpSequenceEvidence, iperf2_udp_sequence};
pub(super) use reporting::{
    aggregate_tx_evidence, log_open_radio_ampdu_interval, log_open_radio_rx_pipeline_interval,
    log_open_radio_task_poll_interval, observe_open_radio_task_polls,
};
pub(in crate::product_hil) use runtime::start_connected_traffic;
pub(super) use tcp::{TcpBenchmarkConfig, run_open_radio_tcp_benchmark};
pub(in crate::product_hil) use tcp::{tcp_rx_pattern_worker_task, tcp_tx_pattern_worker_task};
pub(super) use udp::{
    UdpRxBenchmarkConfig, UdpRxSessionSource, UdpRxTelemetry, UdpSocketBuffers,
    UdpTxBenchmarkConfig, UdpTxSessionSource, run_open_radio_udp_rx_benchmark,
    run_open_radio_udp_tx_benchmark,
};
