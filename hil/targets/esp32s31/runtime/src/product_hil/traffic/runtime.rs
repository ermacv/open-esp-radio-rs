#![forbid(unsafe_code)]

use embassy_futures::select::select;
use embassy_net::{Stack, udp::PacketMetadata};
use embassy_sync::channel::Channel;
use embassy_time::Duration;
use open_esp_radio_esp32s31_embassy_wifi::Esp32s31QualificationSnapshot;
use static_cell::{ConstStaticCell, StaticCell};

use super::{
    BidirectionalResultChannel, BidirectionalSessionChannel, TcpBenchmarkConfig,
    UdpRxBenchmarkConfig, UdpRxSessionSource, UdpRxTelemetry, UdpSocketBuffers,
    UdpTxBenchmarkConfig, UdpTxSessionSource, observe_open_radio_task_polls,
    run_open_radio_bidirectional_session_coordinator, run_open_radio_tcp_benchmark,
    run_open_radio_udp_rx_benchmark, run_open_radio_udp_tx_benchmark,
};
use crate::product_hil::{
    AGGREGATE_TX, OPEN_RADIO_BIDIRECTIONAL_BENCH, OPEN_RADIO_TASK_POLL_TELEMETRY,
    OPEN_RADIO_TCP_BENCH, OPEN_RADIO_TX_BENCH, RX_PIPELINE, TASK_POLLS,
};

const UDP_PAYLOAD_CAPACITY: usize = 1_472;
const UDP_RX_QUEUE_DEPTH: usize = if OPEN_RADIO_TX_BENCH { 1 } else { 64 };
const UDP_TX_QUEUE_DEPTH: usize = if OPEN_RADIO_TX_BENCH { 16 } else { 1 };
const BIDIRECTIONAL_RX_QUEUE_DEPTH: usize = 64;
const BIDIRECTIONAL_TX_QUEUE_DEPTH: usize = 1;
const TCP_RX_BUFFER_CAPACITY: usize = 262_144;
const TCP_TX_BUFFER_CAPACITY: usize = 65_536;

static UDP_RX_METADATA: StaticCell<[PacketMetadata; UDP_RX_QUEUE_DEPTH]> = StaticCell::new();
static UDP_RX_BUFFER: ConstStaticCell<[u8; UDP_RX_QUEUE_DEPTH * UDP_PAYLOAD_CAPACITY]> =
    ConstStaticCell::new([0; UDP_RX_QUEUE_DEPTH * UDP_PAYLOAD_CAPACITY]);
static UDP_TX_METADATA: StaticCell<[PacketMetadata; UDP_TX_QUEUE_DEPTH]> = StaticCell::new();
static UDP_TX_BUFFER: ConstStaticCell<[u8; UDP_TX_QUEUE_DEPTH * UDP_PAYLOAD_CAPACITY]> =
    ConstStaticCell::new([0; UDP_TX_QUEUE_DEPTH * UDP_PAYLOAD_CAPACITY]);
static UDP_PACKET: StaticCell<[u8; UDP_PAYLOAD_CAPACITY]> = StaticCell::new();

static BIDIRECTIONAL_RX_METADATA: StaticCell<[PacketMetadata; BIDIRECTIONAL_RX_QUEUE_DEPTH]> =
    StaticCell::new();
static BIDIRECTIONAL_RX_BUFFER: ConstStaticCell<
    [u8; BIDIRECTIONAL_RX_QUEUE_DEPTH * UDP_PAYLOAD_CAPACITY],
> = ConstStaticCell::new([0; BIDIRECTIONAL_RX_QUEUE_DEPTH * UDP_PAYLOAD_CAPACITY]);
static BIDIRECTIONAL_TX_METADATA: StaticCell<[PacketMetadata; BIDIRECTIONAL_TX_QUEUE_DEPTH]> =
    StaticCell::new();
static BIDIRECTIONAL_TX_BUFFER: ConstStaticCell<
    [u8; BIDIRECTIONAL_TX_QUEUE_DEPTH * UDP_PAYLOAD_CAPACITY],
> = ConstStaticCell::new([0; BIDIRECTIONAL_TX_QUEUE_DEPTH * UDP_PAYLOAD_CAPACITY]);

static TCP_RX_BUFFER: ConstStaticCell<[u8; TCP_RX_BUFFER_CAPACITY]> =
    ConstStaticCell::new([0; TCP_RX_BUFFER_CAPACITY]);
static TCP_TX_BUFFER: ConstStaticCell<[u8; TCP_TX_BUFFER_CAPACITY]> =
    ConstStaticCell::new([0; TCP_TX_BUFFER_CAPACITY]);

static BIDIRECTIONAL_RX_SESSIONS: BidirectionalSessionChannel = Channel::new();
static BIDIRECTIONAL_TX_SESSIONS: BidirectionalSessionChannel = Channel::new();
static BIDIRECTIONAL_RESULTS: BidirectionalResultChannel = Channel::new();

#[inline(never)]
fn runtime_code_marker() {}

async fn run_workload(stack: Stack<'static>, qualification: Esp32s31QualificationSnapshot) -> ! {
    if OPEN_RADIO_TCP_BENCH {
        run_open_radio_tcp_benchmark(
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
        )
        .await
    }

    let rx_metadata = UDP_RX_METADATA.init_with(|| [PacketMetadata::EMPTY; UDP_RX_QUEUE_DEPTH]);
    let rx_buffer = UDP_RX_BUFFER.take();
    let tx_metadata = UDP_TX_METADATA.init_with(|| [PacketMetadata::EMPTY; UDP_TX_QUEUE_DEPTH]);
    let tx_buffer = UDP_TX_BUFFER.take();
    let rx_config = |session_source| UdpRxBenchmarkConfig {
        local_port: 4_323,
        queue_depth: if OPEN_RADIO_BIDIRECTIONAL_BENCH {
            BIDIRECTIONAL_RX_QUEUE_DEPTH
        } else {
            UDP_RX_QUEUE_DEPTH
        },
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

    if OPEN_RADIO_BIDIRECTIONAL_BENCH {
        let packet = UDP_PACKET.init_with(|| [0x5a; UDP_PAYLOAD_CAPACITY]);
        let bidirectional_rx_metadata = BIDIRECTIONAL_RX_METADATA
            .init_with(|| [PacketMetadata::EMPTY; BIDIRECTIONAL_RX_QUEUE_DEPTH]);
        let bidirectional_tx_metadata = BIDIRECTIONAL_TX_METADATA
            .init_with(|| [PacketMetadata::EMPTY; BIDIRECTIONAL_TX_QUEUE_DEPTH]);
        select(
            run_open_radio_bidirectional_session_coordinator(
                &BIDIRECTIONAL_RX_SESSIONS,
                &BIDIRECTIONAL_TX_SESSIONS,
                &BIDIRECTIONAL_RESULTS,
            ),
            select(
                run_open_radio_udp_rx_benchmark(
                    stack,
                    UdpSocketBuffers::new(
                        bidirectional_rx_metadata,
                        BIDIRECTIONAL_RX_BUFFER.take(),
                        bidirectional_tx_metadata,
                        BIDIRECTIONAL_TX_BUFFER.take(),
                    ),
                    rx_config(UdpRxSessionSource::Bidirectional {
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
                    UdpSocketBuffers::new(rx_metadata, rx_buffer, tx_metadata, tx_buffer),
                    packet,
                    tx_config(UdpTxSessionSource::Bidirectional {
                        sessions: &BIDIRECTIONAL_TX_SESSIONS,
                        results: &BIDIRECTIONAL_RESULTS,
                    }),
                    &AGGREGATE_TX,
                ),
            ),
        )
        .await;
        loop {
            core::hint::spin_loop();
        }
    } else if OPEN_RADIO_TX_BENCH {
        run_open_radio_udp_tx_benchmark(
            stack,
            UdpSocketBuffers::new(rx_metadata, rx_buffer, tx_metadata, tx_buffer),
            UDP_PACKET.init_with(|| [0x5a; UDP_PAYLOAD_CAPACITY]),
            tx_config(UdpTxSessionSource::Console),
            &AGGREGATE_TX,
        )
        .await
    } else {
        run_open_radio_udp_rx_benchmark(
            stack,
            UdpSocketBuffers::new(rx_metadata, rx_buffer, tx_metadata, tx_buffer),
            rx_config(UdpRxSessionSource::Console),
            UdpRxTelemetry {
                qualification,
                pipeline: &RX_PIPELINE,
                task_polls: &TASK_POLLS,
            },
        )
        .await
    }
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
