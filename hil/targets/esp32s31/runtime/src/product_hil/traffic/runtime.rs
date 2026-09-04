#![forbid(unsafe_code)]

use embassy_executor::Spawner;
use embassy_net::Stack;
use embassy_sync::channel::Channel;
use embassy_time::Duration;
use open_esp_radio_esp32s31_platform_pac::L1CachePerformanceCounters;
use static_cell::ConstStaticCell;

use super::{
    BidirectionalResultChannel, BidirectionalSessionChannel, SessionChannel, TcpBenchmarkConfig,
    UdpRxBenchmarkConfig, UdpRxSessionSource, UdpRxTelemetry, UdpTxBenchmarkConfig,
    UdpTxSessionSource, multi_flow_burst_datagrams, observe_open_radio_task_polls,
    run_open_radio_bidirectional_session_coordinator, run_open_radio_tcp_benchmark,
    run_open_radio_udp_rx_benchmark, run_open_radio_udp_tx_benchmark, run_session_dispatcher,
};
use crate::product_hil::{AGGREGATE_TX, OPEN_RADIO_TASK_POLL_TELEMETRY, RX_PIPELINE, TASK_POLLS};
use open_esp_radio_hil_protocol::WifiNetworkInterface;

const UDP_PAYLOAD_CAPACITY: usize = 1_472;
const UDP_RX_QUEUE_DEPTH: usize = 64;
// Feed the complete active + standby 32-frame A-MPDU pipeline. A sixteen
// packet socket queue made the HIL producer, rather than the negotiated radio
// window, the TX ceiling and fragmented full-duplex aggregate preparation.
// This CPU-owned PSRAM backlog feeds two complete aggregate horizons without
// changing the fixed DMA-visible SRAM working set.
const UDP_TX_QUEUE_DEPTH: usize = 128;
const TCP_RX_BUFFER_CAPACITY: usize = 262_144;
// This is larger than the link BDP for a separate reason: TCP must be able to
// retain enough unsent payload to feed both 32-frame A-MPDU arenas. A 64-KiB
// socket held only about 44 full-size segments, so the HIL application could
// impose a partial-aggregate boundary on an otherwise idle radio pipeline.
// The buffer is CPU-only storage placed in PSRAM by the qualification profile.
const TCP_TX_BUFFER_CAPACITY: usize = 131_072;

struct ConnectedTrafficResources {
    tcp_rx_buffer: ConstStaticCell<[u8; TCP_RX_BUFFER_CAPACITY]>,
    tcp_tx_buffer: ConstStaticCell<[u8; TCP_TX_BUFFER_CAPACITY]>,
    bidirectional_rx_sessions: BidirectionalSessionChannel,
    bidirectional_tx_sessions: BidirectionalSessionChannel,
    bidirectional_results: BidirectionalResultChannel,
    udp_sessions: SessionChannel,
    tcp_sessions: SessionChannel,
}

impl ConnectedTrafficResources {
    const fn new() -> Self {
        Self {
            tcp_rx_buffer: ConstStaticCell::new([0; TCP_RX_BUFFER_CAPACITY]),
            tcp_tx_buffer: ConstStaticCell::new([0; TCP_TX_BUFFER_CAPACITY]),
            bidirectional_rx_sessions: Channel::new(),
            bidirectional_tx_sessions: Channel::new(),
            bidirectional_results: Channel::new(),
            udp_sessions: Channel::new(),
            tcp_sessions: Channel::new(),
        }
    }
}

static STATION_TRAFFIC: ConnectedTrafficResources = ConnectedTrafficResources::new();
static ACCESS_POINT_TRAFFIC: ConnectedTrafficResources = ConnectedTrafficResources::new();

fn resources(network_interface: WifiNetworkInterface) -> &'static ConnectedTrafficResources {
    match network_interface {
        WifiNetworkInterface::Station => &STATION_TRAFFIC,
        WifiNetworkInterface::AccessPoint => &ACCESS_POINT_TRAFFIC,
    }
}

#[inline(never)]
fn runtime_code_marker() {}

#[embassy_executor::task]
async fn session_dispatcher_task() {
    run_session_dispatcher(
        &STATION_TRAFFIC.udp_sessions,
        &STATION_TRAFFIC.tcp_sessions,
        &ACCESS_POINT_TRAFFIC.udp_sessions,
        &ACCESS_POINT_TRAFFIC.tcp_sessions,
    )
    .await;
}

#[embassy_executor::task(pool_size = 2)]
#[allow(
    large_assignments,
    reason = "the bounded session coordinator future is moved once into its static Embassy task arena"
)]
async fn udp_session_coordinator_task(network_interface: WifiNetworkInterface) {
    let resources = resources(network_interface);
    run_open_radio_bidirectional_session_coordinator(
        &resources.udp_sessions,
        &resources.bidirectional_rx_sessions,
        &resources.bidirectional_tx_sessions,
        &resources.bidirectional_results,
    )
    .await;
}

#[embassy_executor::task(pool_size = 2)]
#[allow(
    large_assignments,
    reason = "the bounded UDP RX owner future is constructed in its final PSRAM-backed Embassy task arena"
)]
async fn udp_rx_task(
    stack: Stack<'static>,
    network_interface: WifiNetworkInterface,
    _l1_cache: &'static L1CachePerformanceCounters,
) {
    let resources = resources(network_interface);
    observe_open_radio_task_polls(
        run_open_radio_udp_rx_benchmark(
            stack,
            UdpRxBenchmarkConfig {
                network_interface,
                local_port: 4_323,
                queue_depth: UDP_RX_QUEUE_DEPTH,
                payload_capacity: UDP_PAYLOAD_CAPACITY,
                idle_timeout: Duration::from_millis(750),
                task_poll_telemetry: OPEN_RADIO_TASK_POLL_TELEMETRY,
                code_address: runtime_code_marker as *const () as usize,
                session_source: UdpRxSessionSource {
                    sessions: &resources.bidirectional_rx_sessions,
                    results: &resources.bidirectional_results,
                },
            },
            UdpRxTelemetry {
                pipeline: &RX_PIPELINE,
                task_polls: &TASK_POLLS,
                #[cfg(feature = "core0-rx-cycle-telemetry")]
                l1_cache: _l1_cache,
            },
        ),
        TASK_POLLS.udp_rx(),
        OPEN_RADIO_TASK_POLL_TELEMETRY,
    )
    .await;
}

#[embassy_executor::task(pool_size = 2)]
#[allow(
    large_assignments,
    reason = "the bounded UDP TX owner future is constructed in its final PSRAM-backed Embassy task arena"
)]
async fn udp_tx_task(
    stack: Stack<'static>,
    network_interface: WifiNetworkInterface,
    _l1_cache: &'static L1CachePerformanceCounters,
) {
    let resources = resources(network_interface);
    observe_open_radio_task_polls(
        run_open_radio_udp_tx_benchmark(
            stack,
            UdpTxBenchmarkConfig {
                network_interface,
                source_port: 4_324,
                queue_depth: UDP_TX_QUEUE_DEPTH,
                payload_capacity: UDP_PAYLOAD_CAPACITY,
                // A 1,472-byte HE20/MCS9 publication reaches the configured TXOP
                // byte ceiling at 31 MPDUs. Pacing in 64-packet bursts produced a
                // stable 31+1 pattern: every second exchange paid A-MPDU overhead
                // for one frame despite a negotiated 32-entry BlockAck window.
                // Keep the complete CPU-side UDP socket queue available between
                // pacing deadlines. The radio still enforces the negotiated
                // 32-entry BlockAck window and the smaller per-publication HE
                // TXOP byte/duration ceiling; this value only prevents the load
                // generator from manufacturing a 31+1 burst boundary.
                pacing_group_datagrams: UDP_TX_QUEUE_DEPTH as u8,
                multi_flow_burst_datagrams: multi_flow_burst_datagrams(),
                drain: Duration::from_millis(250),
                code_address: runtime_code_marker as *const () as usize,
                session_source: UdpTxSessionSource {
                    sessions: &resources.bidirectional_tx_sessions,
                    results: &resources.bidirectional_results,
                },
            },
            &AGGREGATE_TX,
            #[cfg(any(
                feature = "core0-rx-cycle-telemetry",
                feature = "core0-rx-coarse-telemetry"
            ))]
            _l1_cache,
        ),
        TASK_POLLS.udp_tx(),
        OPEN_RADIO_TASK_POLL_TELEMETRY,
    )
    .await;
}

#[embassy_executor::task(pool_size = 2)]
#[allow(
    large_assignments,
    reason = "the bounded TCP owner future is constructed in its final PSRAM-backed Embassy task arena"
)]
async fn tcp_task(stack: Stack<'static>, network_interface: WifiNetworkInterface) {
    let resources = resources(network_interface);
    log::info!("OPEN_RADIO_HIL stage=traffic-workload-start mode=runtime-dispatch");
    observe_open_radio_task_polls(
        run_open_radio_tcp_benchmark(
            stack,
            resources.tcp_rx_buffer.take(),
            resources.tcp_tx_buffer.take(),
            TcpBenchmarkConfig {
                network_interface,
                local_port: 4_325,
                maximum_payload_bytes: crate::product_hil::OPEN_RADIO_TCP_CHUNK_CAPACITY as u16,
                receive_buffer_capacity: TCP_RX_BUFFER_CAPACITY,
                transmit_buffer_capacity: TCP_TX_BUFFER_CAPACITY,
                io_buffer_capacity: 65_536,
                idle_timeout: Duration::from_secs(3),
            },
            &RX_PIPELINE,
            &AGGREGATE_TX,
            &resources.tcp_sessions,
        ),
        TASK_POLLS.tcp(),
        OPEN_RADIO_TASK_POLL_TELEMETRY,
    )
    .await;
}

/// Materialize independent traffic owners. Keeping RX and TX as sibling
/// futures under `embassy_futures::select` is incorrect for sustained
/// full-duplex traffic: that combinator polls RX first, so either continuously
/// ready branch can distort progress of the other branch. Separate Embassy
/// tasks retain bounded socket ownership and let the executor schedule each
/// ready data plane independently.
pub(in crate::product_hil) fn start_traffic_dispatcher(spawner: Spawner) {
    spawner.spawn(session_dispatcher_task().expect("session dispatcher task must allocate once"));
}

pub(in crate::product_hil) fn start_connected_traffic(
    spawner: Spawner,
    stack: Stack<'static>,
    network_interface: WifiNetworkInterface,
    l1_cache: &'static L1CachePerformanceCounters,
) {
    spawner.spawn(
        udp_session_coordinator_task(network_interface)
            .expect("UDP coordinator task pool must fit both roles"),
    );
    spawner.spawn(
        udp_rx_task(stack, network_interface, l1_cache)
            .expect("UDP RX task pool must fit both roles"),
    );
    spawner.spawn(
        udp_tx_task(stack, network_interface, l1_cache)
            .expect("UDP TX task pool must fit both roles"),
    );
    spawner.spawn(tcp_task(stack, network_interface).expect("TCP task pool must fit both roles"));
}
