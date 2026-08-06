#![forbid(unsafe_code)]

use core::cell::RefCell;

use embassy_futures::select::select;
use embassy_net::{Stack, udp::PacketMetadata};
use embassy_time::Timer;
use open_esp_radio::{
    esp32s31::{hal::RadioRegisters, wifi::lmac::tx::TxPhyRate},
    wifi::ieee80211::station::StaAssociationPhy,
};
use static_cell::StaticCell;

use super::{
    TcpBenchmarkConfig, UdpRxBenchmarkConfig, UdpRxSessionSource, UdpRxTelemetry, UdpSocketBuffers,
    UdpTxBenchmarkConfig, UdpTxSessionSource, observe_open_radio_task_polls,
    run_open_radio_bidirectional_session_coordinator, run_open_radio_tcp_benchmark,
    run_open_radio_udp_rx_benchmark, run_open_radio_udp_tx_benchmark,
};
use crate::radio_hil::{
    OPEN_RADIO_BIDIRECTIONAL_BENCH, OPEN_RADIO_BIDIRECTIONAL_RESULTS,
    OPEN_RADIO_BIDIRECTIONAL_RX_QUEUE_DEPTH, OPEN_RADIO_BIDIRECTIONAL_RX_SESSIONS,
    OPEN_RADIO_BIDIRECTIONAL_TX_QUEUE_DEPTH, OPEN_RADIO_BIDIRECTIONAL_TX_SESSIONS,
    OPEN_RADIO_CONNECTED_TRAFFIC_START, OPEN_RADIO_CONNECTED_TRAFFIC_STOP,
    OPEN_RADIO_CONNECTED_TRAFFIC_STOPPED, OPEN_RADIO_IRQ_RUNTIME,
    OPEN_RADIO_MAC_IRQ_CLASSIFICATION, OPEN_RADIO_MAC_IRQ_ENTRIES, OPEN_RADIO_RAW_MAC_BENCH,
    OPEN_RADIO_RUNTIME_RX_SESSIONS, OPEN_RADIO_RUNTIME_TX_SESSIONS, OPEN_RADIO_RX_A_MPDU_COUNTERS,
    OPEN_RADIO_RX_APPLICATION_HANDOFF_BUDGET, OPEN_RADIO_RX_BEACON_S_MPDU_COUNTERS,
    OPEN_RADIO_RX_LAST_UDP_FORMAT, OPEN_RADIO_RX_LAST_UDP_PHY, OPEN_RADIO_RX_ORDER_COUNTERS,
    OPEN_RADIO_RX_ORDER_TELEMETRY, OPEN_RADIO_RX_PHY_COUNTERS, OPEN_RADIO_RX_PIPELINE_COUNTERS,
    OPEN_RADIO_RX_RELOAD_DELAYS, OPEN_RADIO_RX_S_MPDU_COUNTERS, OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH,
    OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH, OPEN_RADIO_TASK_POLL_TELEMETRY, OPEN_RADIO_TASK_POLLS,
    OPEN_RADIO_TCP_BENCH, OPEN_RADIO_TCP_CHUNK_CAPACITY, OPEN_RADIO_TCP_IDLE_TIMEOUT,
    OPEN_RADIO_TCP_IO_CAPACITY, OPEN_RADIO_TCP_PORT, OPEN_RADIO_TCP_RX_BUFFER_CAPACITY,
    OPEN_RADIO_TCP_TX_BUFFER_CAPACITY, OPEN_RADIO_TX_AGGREGATE_COUNTERS,
    OPEN_RADIO_TX_BENCH_RATE_KBPS, OPEN_RADIO_TX_BENCH_TARGET_IPV4,
    OPEN_RADIO_UDP_PAYLOAD_CAPACITY, OPEN_RADIO_UDP_RX_IDLE, OPEN_RADIO_UDP_RX_PORT,
    OPEN_RADIO_UDP_TX_BENCH_DURATION, OPEN_RADIO_UDP_TX_BENCH_PORT, OPEN_RADIO_UDP_TX_DRAIN,
    OPEN_RADIO_UDP_TX_QUEUE_DEPTH,
};

static OPEN_RADIO_UDP_RX_METADATA: StaticCell<[PacketMetadata; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH]> =
    StaticCell::new();
static OPEN_RADIO_UDP_RX_BUFFER: StaticCell<
    [u8; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
> = StaticCell::new();
static OPEN_RADIO_UDP_TX_METADATA: StaticCell<[PacketMetadata; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH]> =
    StaticCell::new();
static OPEN_RADIO_UDP_TX_BUFFER: StaticCell<
    [u8; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
> = StaticCell::new();
static OPEN_RADIO_UDP_PACKET: StaticCell<[u8; OPEN_RADIO_UDP_PAYLOAD_CAPACITY]> = StaticCell::new();
static OPEN_RADIO_TCP_RX_BUFFER: StaticCell<[u8; OPEN_RADIO_TCP_RX_BUFFER_CAPACITY]> =
    StaticCell::new();
static OPEN_RADIO_TCP_TX_BUFFER: StaticCell<[u8; OPEN_RADIO_TCP_TX_BUFFER_CAPACITY]> =
    StaticCell::new();
static OPEN_RADIO_TCP_IO_BUFFER: StaticCell<[u8; OPEN_RADIO_TCP_IO_CAPACITY]> = StaticCell::new();
static OPEN_RADIO_BIDIRECTIONAL_RX_METADATA: StaticCell<
    [PacketMetadata; OPEN_RADIO_BIDIRECTIONAL_RX_QUEUE_DEPTH],
> = StaticCell::new();
static OPEN_RADIO_BIDIRECTIONAL_RX_BUFFER: StaticCell<
    [u8; OPEN_RADIO_BIDIRECTIONAL_RX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
> = StaticCell::new();
static OPEN_RADIO_BIDIRECTIONAL_TX_METADATA: StaticCell<
    [PacketMetadata; OPEN_RADIO_BIDIRECTIONAL_TX_QUEUE_DEPTH],
> = StaticCell::new();
static OPEN_RADIO_BIDIRECTIONAL_TX_BUFFER: StaticCell<
    [u8; OPEN_RADIO_BIDIRECTIONAL_TX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
> = StaticCell::new();

// Keep one ordinary-code symbol alive so the host HIL can prove the runtime
// memory profile from periodic UART evidence. In the required
// psram-code-psram-data image its address is in 0x5000_0000..0x5100_0000; a
// directly linked app/Flash-XIP image reports 0x4000_0000..0x5000_0000.
#[inline(never)]
fn open_radio_runtime_code_marker() {}

fn open_radio_udp_tx_benchmark_config(session_source: UdpTxSessionSource) -> UdpTxBenchmarkConfig {
    UdpTxBenchmarkConfig {
        source_port: 4_324,
        queue_depth: OPEN_RADIO_UDP_TX_QUEUE_DEPTH,
        payload_capacity: OPEN_RADIO_UDP_PAYLOAD_CAPACITY,
        default_target: OPEN_RADIO_TX_BENCH_TARGET_IPV4,
        default_port: OPEN_RADIO_UDP_TX_BENCH_PORT,
        default_duration: OPEN_RADIO_UDP_TX_BENCH_DURATION,
        default_offered_rate_bps: OPEN_RADIO_TX_BENCH_RATE_KBPS
            .map(|rate| rate.saturating_mul(1_000)),
        drain: OPEN_RADIO_UDP_TX_DRAIN,
        code_address: open_radio_runtime_code_marker as *const () as usize,
        session_source,
    }
}

fn open_radio_udp_rx_benchmark_config(
    queue_depth: usize,
    session_source: UdpRxSessionSource,
) -> UdpRxBenchmarkConfig {
    UdpRxBenchmarkConfig {
        local_port: OPEN_RADIO_UDP_RX_PORT,
        queue_depth,
        payload_capacity: OPEN_RADIO_UDP_PAYLOAD_CAPACITY,
        idle_timeout: OPEN_RADIO_UDP_RX_IDLE,
        application_handoff_budget: OPEN_RADIO_RX_APPLICATION_HANDOFF_BUDGET,
        task_poll_telemetry: OPEN_RADIO_TASK_POLL_TELEMETRY,
        rx_order_telemetry: OPEN_RADIO_RX_ORDER_TELEMETRY,
        code_address: open_radio_runtime_code_marker as *const () as usize,
        session_source,
    }
}

fn open_radio_udp_rx_telemetry() -> UdpRxTelemetry {
    UdpRxTelemetry {
        last_format: &OPEN_RADIO_RX_LAST_UDP_FORMAT,
        last_phy: &OPEN_RADIO_RX_LAST_UDP_PHY,
        phy: &OPEN_RADIO_RX_PHY_COUNTERS,
        s_mpdu: &OPEN_RADIO_RX_S_MPDU_COUNTERS,
        beacon_s_mpdu: &OPEN_RADIO_RX_BEACON_S_MPDU_COUNTERS,
        ampdu: &OPEN_RADIO_RX_A_MPDU_COUNTERS,
        order: &OPEN_RADIO_RX_ORDER_COUNTERS,
        pipeline: &OPEN_RADIO_RX_PIPELINE_COUNTERS,
        task_polls: &OPEN_RADIO_TASK_POLLS,
        reload_delays: &OPEN_RADIO_RX_RELOAD_DELAYS,
        irq_runtime: &OPEN_RADIO_IRQ_RUNTIME,
        irq_entries: &OPEN_RADIO_MAC_IRQ_ENTRIES,
        irq_classification: &OPEN_RADIO_MAC_IRQ_CLASSIFICATION,
        aggregate_tx: &OPEN_RADIO_TX_AGGREGATE_COUNTERS,
    }
}

async fn run_connected_traffic_workload(
    stack: Stack<'static>,
    association_phy: StaAssociationPhy,
    data_tx_rate: TxPhyRate,
    registers: &RefCell<&mut RadioRegisters>,
    buffers: &mut RadioHilConnectedTrafficBuffers,
) -> ! {
    match buffers {
        RadioHilConnectedTrafficBuffers::Raw => loop {
            Timer::after_secs(60).await;
        },
        RadioHilConnectedTrafficBuffers::Tcp { rx, tx, read } => {
            run_open_radio_tcp_benchmark(
                stack,
                registers,
                &mut **rx,
                &mut **tx,
                &mut **read,
                TcpBenchmarkConfig {
                    local_port: OPEN_RADIO_TCP_PORT,
                    maximum_payload_bytes: OPEN_RADIO_TCP_CHUNK_CAPACITY as u16,
                    receive_buffer_capacity: OPEN_RADIO_TCP_RX_BUFFER_CAPACITY,
                    transmit_buffer_capacity: OPEN_RADIO_TCP_TX_BUFFER_CAPACITY,
                    io_buffer_capacity: OPEN_RADIO_TCP_IO_CAPACITY,
                    idle_timeout: OPEN_RADIO_TCP_IDLE_TIMEOUT,
                },
                &OPEN_RADIO_RX_PIPELINE_COUNTERS,
                &OPEN_RADIO_TX_AGGREGATE_COUNTERS,
            )
            .await
        }
        RadioHilConnectedTrafficBuffers::UdpRx {
            rx_metadata,
            rx,
            tx_metadata,
            tx,
        } => {
            run_open_radio_udp_rx_benchmark(
                stack,
                association_phy,
                data_tx_rate,
                registers,
                UdpSocketBuffers::new(&mut **rx_metadata, &mut **rx, &mut **tx_metadata, &mut **tx),
                open_radio_udp_rx_benchmark_config(
                    OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH,
                    if OPEN_RADIO_RUNTIME_RX_SESSIONS {
                        UdpRxSessionSource::Console
                    } else {
                        UdpRxSessionSource::Standalone
                    },
                ),
                open_radio_udp_rx_telemetry(),
            )
            .await
        }
        RadioHilConnectedTrafficBuffers::UdpTx {
            rx_metadata,
            rx,
            tx_metadata,
            tx,
            packet,
        } => {
            run_open_radio_udp_tx_benchmark(
                stack,
                association_phy,
                data_tx_rate,
                UdpSocketBuffers::new(&mut **rx_metadata, &mut **rx, &mut **tx_metadata, &mut **tx),
                &mut **packet,
                open_radio_udp_tx_benchmark_config(if OPEN_RADIO_RUNTIME_TX_SESSIONS {
                    UdpTxSessionSource::Console
                } else {
                    UdpTxSessionSource::Standalone
                }),
                &OPEN_RADIO_TX_AGGREGATE_COUNTERS,
            )
            .await
        }
        RadioHilConnectedTrafficBuffers::Bidirectional {
            tx_rx_metadata,
            tx_rx,
            tx_tx_metadata,
            tx_tx,
            packet,
            rx_rx_metadata,
            rx_rx,
            rx_tx_metadata,
            rx_tx,
        } => match select(
            run_open_radio_bidirectional_session_coordinator(
                &OPEN_RADIO_BIDIRECTIONAL_RX_SESSIONS,
                &OPEN_RADIO_BIDIRECTIONAL_TX_SESSIONS,
                &OPEN_RADIO_BIDIRECTIONAL_RESULTS,
            ),
            select(
                run_open_radio_udp_tx_benchmark(
                    stack,
                    association_phy,
                    data_tx_rate,
                    UdpSocketBuffers::new(
                        &mut **tx_rx_metadata,
                        &mut **tx_rx,
                        &mut **tx_tx_metadata,
                        &mut **tx_tx,
                    ),
                    &mut **packet,
                    open_radio_udp_tx_benchmark_config(UdpTxSessionSource::Bidirectional {
                        sessions: &OPEN_RADIO_BIDIRECTIONAL_TX_SESSIONS,
                        results: &OPEN_RADIO_BIDIRECTIONAL_RESULTS,
                    }),
                    &OPEN_RADIO_TX_AGGREGATE_COUNTERS,
                ),
                run_open_radio_udp_rx_benchmark(
                    stack,
                    association_phy,
                    data_tx_rate,
                    registers,
                    UdpSocketBuffers::new(
                        &mut **rx_rx_metadata,
                        &mut **rx_rx,
                        &mut **rx_tx_metadata,
                        &mut **rx_tx,
                    ),
                    open_radio_udp_rx_benchmark_config(
                        OPEN_RADIO_BIDIRECTIONAL_RX_QUEUE_DEPTH,
                        UdpRxSessionSource::Bidirectional {
                            sessions: &OPEN_RADIO_BIDIRECTIONAL_RX_SESSIONS,
                            results: &OPEN_RADIO_BIDIRECTIONAL_RESULTS,
                        },
                    ),
                    open_radio_udp_rx_telemetry(),
                ),
            ),
        )
        .await {},
    }
}

// These concrete wrappers belong to the HIL composition root. The reusable
// driver crates expose owned runners but do not choose an executor, task
// storage or benchmark policy. Keeping each long-running future in its own
// Embassy task gives it an independent waker and removes the fixed outer poll
// order that previously coupled stack, protocol and PAC progress.
#[derive(Clone, Copy)]
pub(in crate::radio_hil) struct RadioHilConnectedTrafficConfig {
    pub(in crate::radio_hil) association_phy: StaAssociationPhy,
    pub(in crate::radio_hil) data_tx_rate: TxPhyRate,
}

enum RadioHilConnectedTrafficBuffers {
    Raw,
    Tcp {
        rx: &'static mut [u8; OPEN_RADIO_TCP_RX_BUFFER_CAPACITY],
        tx: &'static mut [u8; OPEN_RADIO_TCP_TX_BUFFER_CAPACITY],
        read: &'static mut [u8; OPEN_RADIO_TCP_IO_CAPACITY],
    },
    UdpRx {
        rx_metadata: &'static mut [PacketMetadata; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH],
        rx: &'static mut [u8; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
        tx_metadata: &'static mut [PacketMetadata; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH],
        tx: &'static mut [u8; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
    },
    UdpTx {
        rx_metadata: &'static mut [PacketMetadata; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH],
        rx: &'static mut [u8; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
        tx_metadata: &'static mut [PacketMetadata; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH],
        tx: &'static mut [u8; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
        packet: &'static mut [u8; OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
    },
    Bidirectional {
        tx_rx_metadata: &'static mut [PacketMetadata; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH],
        tx_rx:
            &'static mut [u8; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
        tx_tx_metadata: &'static mut [PacketMetadata; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH],
        tx_tx:
            &'static mut [u8; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
        packet: &'static mut [u8; OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
        rx_rx_metadata: &'static mut [PacketMetadata; OPEN_RADIO_BIDIRECTIONAL_RX_QUEUE_DEPTH],
        rx_rx: &'static mut [u8; OPEN_RADIO_BIDIRECTIONAL_RX_QUEUE_DEPTH
                         * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
        rx_tx_metadata: &'static mut [PacketMetadata; OPEN_RADIO_BIDIRECTIONAL_TX_QUEUE_DEPTH],
        rx_tx: &'static mut [u8; OPEN_RADIO_BIDIRECTIONAL_TX_QUEUE_DEPTH
                         * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
    },
}

impl RadioHilConnectedTrafficBuffers {
    fn init() -> Self {
        if OPEN_RADIO_TCP_BENCH {
            Self::Tcp {
                rx: OPEN_RADIO_TCP_RX_BUFFER.init_with(|| [0; OPEN_RADIO_TCP_RX_BUFFER_CAPACITY]),
                tx: OPEN_RADIO_TCP_TX_BUFFER.init_with(|| [0; OPEN_RADIO_TCP_TX_BUFFER_CAPACITY]),
                read: OPEN_RADIO_TCP_IO_BUFFER.init_with(|| [0; OPEN_RADIO_TCP_IO_CAPACITY]),
            }
        } else if OPEN_RADIO_RAW_MAC_BENCH {
            Self::Raw
        } else if OPEN_RADIO_BIDIRECTIONAL_BENCH {
            Self::Bidirectional {
                tx_rx_metadata: OPEN_RADIO_UDP_RX_METADATA
                    .init_with(|| [PacketMetadata::EMPTY; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH]),
                tx_rx: OPEN_RADIO_UDP_RX_BUFFER.init_with(|| {
                    [0; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY]
                }),
                tx_tx_metadata: OPEN_RADIO_UDP_TX_METADATA
                    .init_with(|| [PacketMetadata::EMPTY; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH]),
                tx_tx: OPEN_RADIO_UDP_TX_BUFFER.init_with(|| {
                    [0; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY]
                }),
                packet: OPEN_RADIO_UDP_PACKET.init_with(|| [0x5a; OPEN_RADIO_UDP_PAYLOAD_CAPACITY]),
                rx_rx_metadata: OPEN_RADIO_BIDIRECTIONAL_RX_METADATA
                    .init_with(|| [PacketMetadata::EMPTY; OPEN_RADIO_BIDIRECTIONAL_RX_QUEUE_DEPTH]),
                rx_rx: OPEN_RADIO_BIDIRECTIONAL_RX_BUFFER.init_with(|| {
                    [0; OPEN_RADIO_BIDIRECTIONAL_RX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY]
                }),
                rx_tx_metadata: OPEN_RADIO_BIDIRECTIONAL_TX_METADATA
                    .init_with(|| [PacketMetadata::EMPTY; OPEN_RADIO_BIDIRECTIONAL_TX_QUEUE_DEPTH]),
                rx_tx: OPEN_RADIO_BIDIRECTIONAL_TX_BUFFER.init_with(|| {
                    [0; OPEN_RADIO_BIDIRECTIONAL_TX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY]
                }),
            }
        } else {
            let rx_metadata = OPEN_RADIO_UDP_RX_METADATA
                .init_with(|| [PacketMetadata::EMPTY; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH]);
            let rx = OPEN_RADIO_UDP_RX_BUFFER.init_with(|| {
                [0; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY]
            });
            let tx_metadata = OPEN_RADIO_UDP_TX_METADATA
                .init_with(|| [PacketMetadata::EMPTY; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH]);
            let tx = OPEN_RADIO_UDP_TX_BUFFER.init_with(|| {
                [0; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY]
            });
            if option_env!("OPEN_RADIO_TX_BENCH").is_some() {
                Self::UdpTx {
                    rx_metadata,
                    rx,
                    tx_metadata,
                    tx,
                    packet: OPEN_RADIO_UDP_PACKET
                        .init_with(|| [0x5a; OPEN_RADIO_UDP_PAYLOAD_CAPACITY]),
                }
            } else {
                Self::UdpRx {
                    rx_metadata,
                    rx,
                    tx_metadata,
                    tx,
                }
            }
        }
    }
}

#[embassy_executor::task]
pub(in crate::radio_hil) async fn connected_traffic_task(
    stack: Stack<'static>,
    registers: &'static RefCell<&'static mut RadioRegisters>,
) {
    let mut buffers = RadioHilConnectedTrafficBuffers::init();
    loop {
        let config = OPEN_RADIO_CONNECTED_TRAFFIC_START.receive().await;
        let _ = select(
            OPEN_RADIO_CONNECTED_TRAFFIC_STOP.wait(),
            observe_open_radio_task_polls(
                run_connected_traffic_workload(
                    stack,
                    config.association_phy,
                    config.data_tx_rate,
                    registers,
                    &mut buffers,
                ),
                OPEN_RADIO_TASK_POLLS.benchmark(),
                OPEN_RADIO_TASK_POLL_TELEMETRY,
            ),
        )
        .await;
        OPEN_RADIO_CONNECTED_TRAFFIC_STOPPED.signal(());
    }
}
