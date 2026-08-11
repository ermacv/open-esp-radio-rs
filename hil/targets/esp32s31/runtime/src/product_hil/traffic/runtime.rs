#![forbid(unsafe_code)]

use embassy_futures::select::select;
use embassy_net::{Stack, udp::PacketMetadata};
use embassy_sync::channel::Channel;
use embassy_time::Duration;
use open_esp_radio_esp32s31_embassy_wifi::Esp32s31QualificationSnapshot;
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
const UDP_TX_QUEUE_DEPTH: usize = 16;
const UDP_AUXILIARY_QUEUE_DEPTH: usize = 1;
const TCP_RX_BUFFER_CAPACITY: usize = 262_144;
const TCP_TX_BUFFER_CAPACITY: usize = 65_536;

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
static UDP_PACKET: StaticCell<[u8; UDP_PAYLOAD_CAPACITY]> = StaticCell::new();

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

async fn run_workload(stack: Stack<'static>, qualification: Esp32s31QualificationSnapshot) -> ! {
    log::info!("OPEN_RADIO_HIL stage=traffic-workload-start mode=runtime-dispatch");
    let tcp = run_open_radio_tcp_benchmark(
        stack,
        qualification,
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
    );

    let sink_rx_metadata =
        UDP_SINK_RX_METADATA.init_with(|| [PacketMetadata::EMPTY; UDP_RX_QUEUE_DEPTH]);
    let sink_rx_buffer = UDP_SINK_RX_BUFFER.take();
    let sink_tx_metadata =
        UDP_SINK_TX_METADATA.init_with(|| [PacketMetadata::EMPTY; UDP_AUXILIARY_QUEUE_DEPTH]);
    let sink_tx_buffer = UDP_SINK_TX_BUFFER.take();
    let source_rx_metadata =
        UDP_SOURCE_RX_METADATA.init_with(|| [PacketMetadata::EMPTY; UDP_AUXILIARY_QUEUE_DEPTH]);
    let source_rx_buffer = UDP_SOURCE_RX_BUFFER.take();
    let source_tx_metadata =
        UDP_SOURCE_TX_METADATA.init_with(|| [PacketMetadata::EMPTY; UDP_TX_QUEUE_DEPTH]);
    let source_tx_buffer = UDP_SOURCE_TX_BUFFER.take();
    let rx_config = |session_source| UdpRxBenchmarkConfig {
        local_port: 4_323,
        queue_depth: UDP_RX_QUEUE_DEPTH,
        payload_capacity: UDP_PAYLOAD_CAPACITY,
        idle_timeout: Duration::from_millis(750),
        task_poll_telemetry: OPEN_RADIO_TASK_POLL_TELEMETRY,
        code_address: runtime_code_marker as *const () as usize,
        session_source,
    };
    let tx_config = |session_source| UdpTxBenchmarkConfig {
        source_port: 4_324,
        queue_depth: UDP_TX_QUEUE_DEPTH,
        payload_capacity: UDP_PAYLOAD_CAPACITY,
        pacing_group_datagrams: 64,
        drain: Duration::from_millis(250),
        code_address: runtime_code_marker as *const () as usize,
        qualification,
        session_source,
    };

    let packet = UDP_PACKET.init_with(|| [0x5a; UDP_PAYLOAD_CAPACITY]);
    let udp = select(
        run_open_radio_bidirectional_session_coordinator(
            &UDP_SESSIONS,
            &BIDIRECTIONAL_RX_SESSIONS,
            &BIDIRECTIONAL_TX_SESSIONS,
            &BIDIRECTIONAL_RESULTS,
        ),
        select(
            run_open_radio_udp_rx_benchmark(
                stack,
                UdpSocketBuffers::new(
                    sink_rx_metadata,
                    sink_rx_buffer,
                    sink_tx_metadata,
                    sink_tx_buffer,
                ),
                rx_config(UdpRxSessionSource {
                    sessions: &BIDIRECTIONAL_RX_SESSIONS,
                    results: &BIDIRECTIONAL_RESULTS,
                }),
                UdpRxTelemetry {
                    qualification,
                    pipeline: &RX_PIPELINE,
                    task_polls: &TASK_POLLS,
                },
            ),
            run_open_radio_udp_tx_benchmark(
                stack,
                UdpSocketBuffers::new(
                    source_rx_metadata,
                    source_rx_buffer,
                    source_tx_metadata,
                    source_tx_buffer,
                ),
                packet,
                tx_config(UdpTxSessionSource {
                    sessions: &BIDIRECTIONAL_TX_SESSIONS,
                    results: &BIDIRECTIONAL_RESULTS,
                }),
                &AGGREGATE_TX,
            ),
        ),
    );
    select(
        run_session_dispatcher(&UDP_SESSIONS, &TCP_SESSIONS),
        select(udp, tcp),
    )
    .await;
    unreachable!()
}

#[embassy_executor::task]
pub(in crate::product_hil) async fn connected_traffic_task(
    stack: Stack<'static>,
    qualification: Esp32s31QualificationSnapshot,
) {
    observe_open_radio_task_polls(
        run_workload(stack, qualification),
        TASK_POLLS.benchmark(),
        OPEN_RADIO_TASK_POLL_TELEMETRY,
    )
    .await
}
