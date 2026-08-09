#![forbid(unsafe_code)]

use embassy_futures::join::join;
use embassy_net::{
    Stack,
    tcp::{TcpReader, TcpSocket, TcpWriter},
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex, signal::Signal};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use open_esp_radio_esp32s31_embassy_wifi::Esp32s31QualificationSnapshot;
use open_esp_radio_hil_esp32s31_telemetry::aggregate_tx::AggregateTxCounters;
use open_esp_radio_hil_esp32s31_telemetry::rx_pipeline::RxPipelineCounters;
use open_esp_radio_hil_protocol::{
    Completion as HilCompletion, Direction as HilDirection, Event as HilEvent, ServiceInfo,
    SessionReady, Transport as HilTransport, TransportEvidence, fill_stream_pattern,
    stream_pattern_matches,
};

use crate::console::{
    complete_session, emergency_log, publish_event_reliably, receive_session_start,
};
use crate::product_hil::OPEN_RADIO_TCP_CHUNK_CAPACITY;

use super::{log_open_radio_ampdu_interval, wait_session_link_requirements};

#[derive(Clone, Copy)]
pub(in crate::product_hil) struct TcpBenchmarkConfig {
    pub local_port: u16,
    pub maximum_payload_bytes: u16,
    pub receive_buffer_capacity: usize,
    pub transmit_buffer_capacity: usize,
    pub io_buffer_capacity: usize,
    pub idle_timeout: Duration,
}

#[derive(Clone, Copy, Default)]
struct StreamResult {
    bytes: u64,
    units: u64,
    errors: u32,
    eof: bool,
    pattern_ok: bool,
}

#[derive(Clone, Copy)]
struct RxPatternJob {
    length: usize,
    offset: u64,
}

#[derive(Clone, Copy)]
struct TxPatternJob {
    length: usize,
    offset: u64,
}

static TCP_RX_PATTERN_BUFFER: Mutex<CriticalSectionRawMutex, [u8; OPEN_RADIO_TCP_CHUNK_CAPACITY]> =
    Mutex::new([0; OPEN_RADIO_TCP_CHUNK_CAPACITY]);
static TCP_TX_PATTERN_BUFFER: Mutex<CriticalSectionRawMutex, [u8; OPEN_RADIO_TCP_CHUNK_CAPACITY]> =
    Mutex::new([0; OPEN_RADIO_TCP_CHUNK_CAPACITY]);
static TCP_RX_PATTERN_JOB: Signal<CriticalSectionRawMutex, RxPatternJob> = Signal::new();
static TCP_RX_PATTERN_RESULT: Signal<CriticalSectionRawMutex, bool> = Signal::new();
static TCP_TX_PATTERN_JOB: Signal<CriticalSectionRawMutex, TxPatternJob> = Signal::new();
static TCP_TX_PATTERN_RESULT: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Validate received stream bytes on Core 1 while Core 0 keeps servicing the
/// radio, embassy-net and the TCP socket owner.
#[embassy_executor::task]
pub(in crate::product_hil) async fn tcp_rx_pattern_worker_task() {
    loop {
        let job = TCP_RX_PATTERN_JOB.wait().await;
        let buffer = TCP_RX_PATTERN_BUFFER.lock().await;
        let matches = stream_pattern_matches(&buffer[..job.length], job.offset);
        drop(buffer);
        TCP_RX_PATTERN_RESULT.signal(matches);
    }
}

/// Prepare transmitted stream bytes on Core 1 while Core 0 keeps servicing
/// the radio, embassy-net and the TCP socket owner.
#[embassy_executor::task]
pub(in crate::product_hil) async fn tcp_tx_pattern_worker_task() {
    loop {
        let job = TCP_TX_PATTERN_JOB.wait().await;
        let mut buffer = TCP_TX_PATTERN_BUFFER.lock().await;
        fill_stream_pattern(&mut buffer[..job.length], job.offset);
        drop(buffer);
        TCP_TX_PATTERN_RESULT.signal(());
    }
}

pub(in crate::product_hil) async fn run_open_radio_tcp_benchmark<'a>(
    stack: Stack<'a>,
    qualification: Esp32s31QualificationSnapshot,
    rx_buffer: &'a mut [u8],
    tx_buffer: &'a mut [u8],
    config: TcpBenchmarkConfig,
    pipeline_counters: &RxPipelineCounters,
    aggregate_counters: &AggregateTxCounters,
) -> ! {
    stack.wait_config_up().await;
    while stack.config_v4().is_none() {
        Timer::after_millis(100).await;
    }

    let mut socket = TcpSocket::new(stack, rx_buffer, tx_buffer);
    for direction in [
        HilDirection::Rx,
        HilDirection::Tx,
        HilDirection::Bidirectional,
    ] {
        publish_event_reliably(
            0,
            0,
            HilEvent::ServiceReady(ServiceInfo {
                transport: HilTransport::Tcp,
                direction,
                local_port: config.local_port,
                maximum_payload_bytes: config.maximum_payload_bytes,
            }),
        )
        .await;
    }
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=tcp-ready port={} rx_buffer={} \
         tx_buffer={} io_buffer={} runtime_session=1 directions=rx,tx,bidirectional",
        config.local_port,
        config.receive_buffer_capacity,
        config.transmit_buffer_capacity,
        config.io_buffer_capacity,
    ));

    loop {
        let session = receive_session_start().await;
        wait_session_link_requirements(session.config.link_requirements, aggregate_counters).await;
        publish_event_reliably(
            session.session_id,
            0,
            HilEvent::SessionReady(SessionReady {
                direction: session.config.direction,
                tx_block_ack_tid: session.config.link_requirements.tx_block_ack_tid,
            }),
        )
        .await;
        let duration_millis = match session.config.completion {
            HilCompletion::DurationMillis(duration) => duration,
            HilCompletion::TransferBytes(_) | HilCompletion::HostStop => {
                unreachable!("protocol owner accepts only duration-completed sessions")
            }
        };
        let duration = Duration::from_millis(u64::from(duration_millis));
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=PASS stage=tcp-session-start session={} \
             direction={:?} duration_ms={}",
            session.session_id, session.config.direction, duration_millis,
        ));

        let hardware_start = qualification.rx_statistics().map(|value| value.primary);
        let pipeline_start = pipeline_counters.snapshot();
        let aggregate_start = aggregate_counters.snapshot();
        let connection_timeout = duration + Duration::from_secs(5);
        let connected =
            match with_timeout(connection_timeout, socket.accept(config.local_port)).await {
                Ok(Ok(())) => true,
                Ok(Err(error)) => {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=tcp-accept error={error:?}"
                    ));
                    false
                }
                Err(_) => {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=tcp-accept error=Timeout"
                    ));
                    false
                }
            };
        socket.set_nagle_enabled(false);
        let started = Instant::now();
        let (mut rx, mut tx) = (StreamResult::default(), StreamResult::default());

        if connected {
            match session.config.direction {
                HilDirection::Rx => {
                    let flow = session
                        .config
                        .target_rx
                        .expect("validated TCP RX session carries an RX flow");
                    rx = receive_stream(
                        &mut socket,
                        usize::from(flow.payload_bytes),
                        config.idle_timeout,
                    )
                    .await;
                }
                HilDirection::Tx => {
                    let flow = session
                        .config
                        .target_tx
                        .expect("validated TCP TX session carries a TX flow");
                    tx = transmit_stream(
                        &mut socket,
                        usize::from(flow.payload_bytes),
                        flow.offered_rate_bps,
                        started,
                        duration,
                    )
                    .await;
                }
                HilDirection::Bidirectional => {
                    let rx_flow = session
                        .config
                        .target_rx
                        .expect("validated bidirectional TCP session carries an RX flow");
                    let tx_flow = session
                        .config
                        .target_tx
                        .expect("validated bidirectional TCP session carries a TX flow");
                    let (mut reader, mut writer) = socket.split();
                    (rx, tx) = join(
                        receive_stream(
                            &mut reader,
                            usize::from(rx_flow.payload_bytes),
                            config.idle_timeout,
                        ),
                        transmit_stream(
                            &mut writer,
                            usize::from(tx_flow.payload_bytes),
                            tx_flow.offered_rate_bps,
                            started,
                            duration,
                        ),
                    )
                    .await;
                }
            }
        } else {
            match session.config.direction {
                HilDirection::Rx => rx.errors = 1,
                HilDirection::Tx => tx.errors = 1,
                HilDirection::Bidirectional => {
                    rx.errors = 1;
                    tx.errors = 1;
                }
            }
        }

        if matches!(
            session.config.direction,
            HilDirection::Tx | HilDirection::Bidirectional
        ) {
            socket.close();
            if with_timeout(config.idle_timeout, socket.flush())
                .await
                .is_err()
            {
                tx.errors = tx.errors.saturating_add(1);
            }
        }
        let elapsed_us = started.elapsed().as_micros().max(1);
        socket.abort();

        let hardware_delta = qualification
            .rx_statistics()
            .map(|value| value.primary)
            .zip(hardware_start)
            .map(|(current, earlier)| current.wrapping_delta_since(earlier))
            .unwrap_or_default();
        let pipeline_end = pipeline_counters.snapshot();
        let enqueued = pipeline_end
            .network_enqueued
            .wrapping_sub(pipeline_start.network_enqueued);
        let queue_dropped = pipeline_end
            .network_dropped
            .wrapping_sub(pipeline_start.network_dropped);
        let health_errors = u32::from(hardware_delta.buffer_full)
            .saturating_add(u32::from(hardware_delta.fifo_overflow))
            .saturating_add(queue_dropped);
        let transport_errors = rx
            .errors
            .saturating_add(tx.errors)
            .saturating_add(health_errors);
        let passed = connected
            && transport_errors == 0
            && match session.config.direction {
                HilDirection::Rx => rx.units == 1 && rx.pattern_ok,
                HilDirection::Tx => tx.units == 1,
                HilDirection::Bidirectional => rx.units == 1 && tx.units == 1 && rx.pattern_ok,
            };
        emergency_log(format_args!(
            "OTCP dir={:?} rb={} tb={} ru={} tu={} u={} e={} bf={} fo={} enq={} drop={} eof={} pat={}",
            session.config.direction,
            rx.bytes,
            tx.bytes,
            rx.units,
            tx.units,
            elapsed_us,
            transport_errors,
            hardware_delta.buffer_full,
            hardware_delta.fifo_overflow,
            enqueued,
            queue_dropped,
            u8::from(rx.eof),
            u8::from(rx.pattern_ok),
        ));
        log_open_radio_ampdu_interval(aggregate_start, aggregate_counters);
        complete_session(
            session.session_id,
            TransportEvidence {
                rx_bytes: rx.bytes,
                tx_bytes: tx.bytes,
                rx_units: rx.units,
                tx_units: tx.units,
                elapsed_micros: elapsed_us,
                transport_errors,
            },
            passed,
        )
        .await;
    }
}

trait TcpRead {
    async fn read(&mut self, buffer: &mut [u8]) -> Result<usize, embassy_net::tcp::Error>;
}

impl TcpRead for TcpSocket<'_> {
    async fn read(&mut self, buffer: &mut [u8]) -> Result<usize, embassy_net::tcp::Error> {
        TcpSocket::read(self, buffer).await
    }
}

impl TcpRead for TcpReader<'_> {
    async fn read(&mut self, buffer: &mut [u8]) -> Result<usize, embassy_net::tcp::Error> {
        TcpReader::read(self, buffer).await
    }
}

trait TcpWrite {
    async fn write(&mut self, buffer: &[u8]) -> Result<usize, embassy_net::tcp::Error>;
    async fn flush(&mut self) -> Result<(), embassy_net::tcp::Error>;
}

impl TcpWrite for TcpSocket<'_> {
    async fn write(&mut self, buffer: &[u8]) -> Result<usize, embassy_net::tcp::Error> {
        TcpSocket::write(self, buffer).await
    }

    async fn flush(&mut self) -> Result<(), embassy_net::tcp::Error> {
        TcpSocket::flush(self).await
    }
}

impl TcpWrite for TcpWriter<'_> {
    async fn write(&mut self, buffer: &[u8]) -> Result<usize, embassy_net::tcp::Error> {
        TcpWriter::write(self, buffer).await
    }

    async fn flush(&mut self) -> Result<(), embassy_net::tcp::Error> {
        TcpWriter::flush(self).await
    }
}

async fn receive_stream(
    reader: &mut impl TcpRead,
    chunk_bytes: usize,
    idle_timeout: Duration,
) -> StreamResult {
    let mut result = StreamResult {
        pattern_ok: true,
        ..StreamResult::default()
    };
    loop {
        let read = {
            let mut buffer = TCP_RX_PATTERN_BUFFER.lock().await;
            with_timeout(idle_timeout, reader.read(&mut buffer[..chunk_bytes])).await
        };
        match read {
            Ok(Ok(0)) => {
                result.eof = true;
                result.units = 1;
                break;
            }
            Ok(Ok(length)) => {
                TCP_RX_PATTERN_JOB.signal(RxPatternJob {
                    length,
                    offset: result.bytes,
                });
                result.pattern_ok &= TCP_RX_PATTERN_RESULT.wait().await;
                result.bytes = result.bytes.saturating_add(length as u64);
            }
            Ok(Err(_)) | Err(_) => {
                result.errors = result.errors.saturating_add(1);
                break;
            }
        }
    }
    result
}

async fn transmit_stream(
    writer: &mut impl TcpWrite,
    chunk_bytes: usize,
    offered_rate_bps: Option<u64>,
    started: Instant,
    duration: Duration,
) -> StreamResult {
    let mut result = StreamResult::default();
    let mut pending_offset = chunk_bytes;
    while started.elapsed() < duration {
        if pending_offset == chunk_bytes {
            TCP_TX_PATTERN_JOB.signal(TxPatternJob {
                length: chunk_bytes,
                offset: result.bytes,
            });
            let _ = TCP_TX_PATTERN_RESULT.wait().await;
            pending_offset = 0;
        }
        let elapsed = started.elapsed();
        if elapsed >= duration {
            break;
        }
        let remaining = duration - elapsed;
        let write = {
            let buffer = TCP_TX_PATTERN_BUFFER.lock().await;
            with_timeout(
                remaining,
                writer.write(&buffer[pending_offset..chunk_bytes]),
            )
            .await
        };
        match write {
            Err(_) if started.elapsed() >= duration => break,
            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => {
                result.errors = result.errors.saturating_add(1);
                break;
            }
            Ok(Ok(length)) => {
                pending_offset += length;
                result.bytes = result.bytes.saturating_add(length as u64);
            }
        }
        if let Some(rate_bps) = offered_rate_bps {
            let elapsed_us = result.bytes.saturating_mul(8_000_000) / rate_bps;
            let deadline = started + Duration::from_micros(elapsed_us);
            if Instant::now() < deadline {
                Timer::at(deadline).await;
            }
        }
    }
    if writer.flush().await.is_err() {
        result.errors = result.errors.saturating_add(1);
    }
    result.units = u64::from(result.bytes != 0 && result.errors == 0);
    result
}
