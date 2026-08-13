#![forbid(unsafe_code)]

use embassy_executor::Spawner;
use embassy_net::{Stack, udp::PacketMetadata};
use embassy_sync::channel::Channel;
use embassy_time::Duration;
use static_cell::{ConstStaticCell, StaticCell};

use super::{
    BidirectionalResultChannel, BidirectionalSessionChannel, SessionChannel, TcpBenchmarkConfig,
    UdpRxBenchmarkConfig, UdpRxSessionSource, UdpRxTelemetry, UdpSocketBuffers,
    UdpTxBenchmarkConfig, UdpTxSessionSource, observe_open_radio_task_polls,
    run_open_radio_bidirectional_session_coordinator, run_open_radio_tcp_benchmark,
    run_open_radio_udp_rx_benchmark, run_open_radio_udp_tx_benchmark, run_session_dispatcher,
};
use crate::product_hil::{AGGREGATE_TX, OPEN_RADIO_TASK_POLL_TELEMETRY, RX_PIPELINE, TASK_POLLS};

const UDP_PAYLOAD_CAPACITY: usize = 1_472;
const UDP_RX_QUEUE_DEPTH: usize = 64;
// Feed the complete active + standby 32-frame A-MPDU pipeline. A sixteen
// packet socket queue made the HIL producer, rather than the negotiated radio
// window, the TX ceiling and fragmented full-duplex aggregate preparation.
// This CPU-only socket storage is placed by the PSRAM-data qualification
// profile; DMA descriptors and hardware-visible frame backing stay in SRAM.
const UDP_TX_QUEUE_DEPTH: usize = 64;
const UDP_AUXILIARY_QUEUE_DEPTH: usize = 1;
const TCP_RX_BUFFER_CAPACITY: usize = 262_144;
// This is larger than the link BDP for a separate reason: TCP must be able to
// retain enough unsent payload to feed both 32-frame A-MPDU arenas. A 64-KiB
// socket held only about 44 full-size segments, so the HIL application could
// impose a partial-aggregate boundary on an otherwise idle radio pipeline.
// The buffer is CPU-only storage placed in PSRAM by the qualification profile.
const TCP_TX_BUFFER_CAPACITY: usize = 131_072;

static UDP_SINK_RX_METADATA: StaticCell<[PacketMetadata; UDP_RX_QUEUE_DEPTH]> = StaticCell::new();
static UDP_SINK_RX_BUFFER: ConstStaticCell<[u8; UDP_RX_QUEUE_DEPTH * UDP_PAYLOAD_CAPACITY]> =
    ConstStaticCell::new([0; UDP_RX_QUEUE_DEPTH * UDP_PAYLOAD_CAPACITY]);
static UDP_SINK_TX_METADATA: StaticCell<[PacketMetadata; UDP_AUXILIARY_QUEUE_DEPTH]> =
    StaticCell::new();
static UDP_SINK_TX_BUFFER: ConstStaticCell<[u8; UDP_AUXILIARY_QUEUE_DEPTH * UDP_PAYLOAD_CAPACITY]> =
    ConstStaticCell::new([0; UDP_AUXILIARY_QUEUE_DEPTH * UDP_PAYLOAD_CAPACITY]);
static UDP_SOURCE_RX_METADATA: StaticCell<[PacketMetadata; UDP_AUXILIARY_QUEUE_DEPTH]> =
    StaticCell::new();
static UDP_SOURCE_RX_BUFFER: ConstStaticCell<
    [u8; UDP_AUXILIARY_QUEUE_DEPTH * UDP_PAYLOAD_CAPACITY],
> = ConstStaticCell::new([0; UDP_AUXILIARY_QUEUE_DEPTH * UDP_PAYLOAD_CAPACITY]);
static UDP_SOURCE_TX_METADATA: StaticCell<[PacketMetadata; UDP_TX_QUEUE_DEPTH]> = StaticCell::new();
static UDP_SOURCE_TX_BUFFER: ConstStaticCell<[u8; UDP_TX_QUEUE_DEPTH * UDP_PAYLOAD_CAPACITY]> =
    ConstStaticCell::new([0; UDP_TX_QUEUE_DEPTH * UDP_PAYLOAD_CAPACITY]);
// Non-divisor payload sizes can require an internal smoltcp ring-padding
// record. embassy-net's zero-copy `send_to_with` cannot safely retry if that
// padding consumes the final metadata entry after its FnOnce callback ran.
// Keep the repeatable fallback payload in static PSRAM rather than adding a
// 1,472-byte object to the Embassy task future/CPU stack.
static UDP_SOURCE_PACKET: ConstStaticCell<[u8; UDP_PAYLOAD_CAPACITY]> =
    ConstStaticCell::new([0; UDP_PAYLOAD_CAPACITY]);
static TCP_RX_BUFFER: ConstStaticCell<[u8; TCP_RX_BUFFER_CAPACITY]> =
    ConstStaticCell::new([0; TCP_RX_BUFFER_CAPACITY]);
static TCP_TX_BUFFER: ConstStaticCell<[u8; TCP_TX_BUFFER_CAPACITY]> =
    ConstStaticCell::new([0; TCP_TX_BUFFER_CAPACITY]);

static BIDIRECTIONAL_RX_SESSIONS: BidirectionalSessionChannel = Channel::new();
static BIDIRECTIONAL_TX_SESSIONS: BidirectionalSessionChannel = Channel::new();
static BIDIRECTIONAL_RESULTS: BidirectionalResultChannel = Channel::new();
static UDP_SESSIONS: SessionChannel = Channel::new();
static TCP_SESSIONS: SessionChannel = Channel::new();

#[inline(never)]
fn runtime_code_marker() {}

#[embassy_executor::task]
async fn session_dispatcher_task() {
    run_session_dispatcher(&UDP_SESSIONS, &TCP_SESSIONS).await;
}

#[embassy_executor::task]
async fn udp_session_coordinator_task() {
    run_open_radio_bidirectional_session_coordinator(
        &UDP_SESSIONS,
        &BIDIRECTIONAL_RX_SESSIONS,
        &BIDIRECTIONAL_TX_SESSIONS,
        &BIDIRECTIONAL_RESULTS,
    )
    .await;
}

#[embassy_executor::task]
async fn udp_rx_task(stack: Stack<'static>) {
    let rx_metadata =
        UDP_SINK_RX_METADATA.init_with(|| [PacketMetadata::EMPTY; UDP_RX_QUEUE_DEPTH]);
    let tx_metadata =
        UDP_SINK_TX_METADATA.init_with(|| [PacketMetadata::EMPTY; UDP_AUXILIARY_QUEUE_DEPTH]);
    observe_open_radio_task_polls(
        run_open_radio_udp_rx_benchmark(
            stack,
            UdpSocketBuffers::new(
                rx_metadata,
                UDP_SINK_RX_BUFFER.take(),
                tx_metadata,
                UDP_SINK_TX_BUFFER.take(),
            ),
            UdpRxBenchmarkConfig {
                local_port: 4_323,
                queue_depth: UDP_RX_QUEUE_DEPTH,
                payload_capacity: UDP_PAYLOAD_CAPACITY,
                idle_timeout: Duration::from_millis(750),
                task_poll_telemetry: OPEN_RADIO_TASK_POLL_TELEMETRY,
                code_address: runtime_code_marker as *const () as usize,
                session_source: UdpRxSessionSource {
                    sessions: &BIDIRECTIONAL_RX_SESSIONS,
                    results: &BIDIRECTIONAL_RESULTS,
                },
            },
            UdpRxTelemetry {
                pipeline: &RX_PIPELINE,
                task_polls: &TASK_POLLS,
            },
        ),
        TASK_POLLS.udp_rx(),
        OPEN_RADIO_TASK_POLL_TELEMETRY,
    )
    .await;
}

#[embassy_executor::task]
async fn udp_tx_task(stack: Stack<'static>) {
    let rx_metadata =
        UDP_SOURCE_RX_METADATA.init_with(|| [PacketMetadata::EMPTY; UDP_AUXILIARY_QUEUE_DEPTH]);
    let tx_metadata =
        UDP_SOURCE_TX_METADATA.init_with(|| [PacketMetadata::EMPTY; UDP_TX_QUEUE_DEPTH]);
    let tx_buffer = UDP_SOURCE_TX_BUFFER.take();
    let packet = UDP_SOURCE_PACKET.take();
    // The TX benchmark payload is a fixed 0x5a pattern apart from its leading
    // sequence. Paint every reusable PSRAM socket slot once before readiness;
    // the measured hot path then writes only the four-byte sequence, without
    // retaining the 94-KiB pattern in the flash image.
    tx_buffer.fill(0x5a);
    packet.fill(0x5a);
    observe_open_radio_task_polls(
        run_open_radio_udp_tx_benchmark(
            stack,
            UdpSocketBuffers::new(
                rx_metadata,
                UDP_SOURCE_RX_BUFFER.take(),
                tx_metadata,
                tx_buffer,
            ),
            packet,
            UdpTxBenchmarkConfig {
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
                drain: Duration::from_millis(250),
                code_address: runtime_code_marker as *const () as usize,
                session_source: UdpTxSessionSource {
                    sessions: &BIDIRECTIONAL_TX_SESSIONS,
                    results: &BIDIRECTIONAL_RESULTS,
                },
            },
            &AGGREGATE_TX,
        ),
        TASK_POLLS.udp_tx(),
        OPEN_RADIO_TASK_POLL_TELEMETRY,
    )
    .await;
}

#[embassy_executor::task]
async fn tcp_task(stack: Stack<'static>) {
    log::info!("OPEN_RADIO_HIL stage=traffic-workload-start mode=runtime-dispatch");
    observe_open_radio_task_polls(
        run_open_radio_tcp_benchmark(
            stack,
            TCP_RX_BUFFER.take(),
            TCP_TX_BUFFER.take(),
            TcpBenchmarkConfig {
                local_port: 4_325,
                maximum_payload_bytes: crate::product_hil::OPEN_RADIO_TCP_CHUNK_CAPACITY as u16,
                receive_buffer_capacity: TCP_RX_BUFFER_CAPACITY,
                transmit_buffer_capacity: TCP_TX_BUFFER_CAPACITY,
                io_buffer_capacity: 65_536,
                idle_timeout: Duration::from_secs(3),
            },
            &RX_PIPELINE,
            &AGGREGATE_TX,
            &TCP_SESSIONS,
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
pub(in crate::product_hil) fn start_connected_traffic(spawner: Spawner, stack: Stack<'static>) {
    spawner.spawn(session_dispatcher_task().expect("session dispatcher task must allocate once"));
    spawner.spawn(udp_session_coordinator_task().expect("UDP coordinator task must allocate once"));
    spawner.spawn(udp_rx_task(stack).expect("UDP RX task must allocate once"));
    spawner.spawn(udp_tx_task(stack).expect("UDP TX task must allocate once"));
    spawner.spawn(tcp_task(stack).expect("TCP task must allocate once"));
}
