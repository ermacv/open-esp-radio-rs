#![forbid(unsafe_code)]

use core::cell::RefCell;

use embassy_net::{Stack, tcp::TcpSocket};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use open_esp_radio::esp32s31::hal::RadioRegisters;
use open_esp_radio_hil_esp32s31_telemetry::rx_pipeline::RxPipelineCounters;
use open_esp_radio_hil_protocol::{
    Completion as HilCompletion, Direction as HilDirection, Event as HilEvent, ServiceInfo,
    Transport as HilTransport, TransportEvidence,
};

use crate::console::{complete_session, emergency_log, publish_event, receive_session_start};

#[derive(Clone, Copy)]
pub(in crate::radio_hil) struct TcpRxBenchmarkConfig {
    pub local_port: u16,
    pub maximum_payload_bytes: u16,
    pub receive_buffer_capacity: usize,
    pub read_capacity: usize,
    pub idle_timeout: Duration,
}

pub(in crate::radio_hil) async fn run_open_radio_tcp_rx_benchmark<'a>(
    stack: Stack<'a>,
    registers: &RefCell<&mut RadioRegisters>,
    rx_buffer: &'a mut [u8],
    tx_buffer: &'a mut [u8],
    read_buffer: &mut [u8],
    config: TcpRxBenchmarkConfig,
    pipeline_counters: &RxPipelineCounters,
) -> ! {
    stack.wait_config_up().await;
    while stack.config_v4().is_none() {
        Timer::after_millis(100).await;
    }

    let mut socket = TcpSocket::new(stack, rx_buffer, tx_buffer);
    publish_event(
        0,
        0,
        HilEvent::ServiceReady(ServiceInfo {
            transport: HilTransport::Tcp,
            direction: HilDirection::Rx,
            local_port: config.local_port,
            maximum_payload_bytes: config.maximum_payload_bytes,
        }),
    );
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=tcp-rx-ready port={} \
         receive_buffer={} read_capacity={} runtime_session=1",
        config.local_port, config.receive_buffer_capacity, config.read_capacity,
    ));

    loop {
        let session = receive_session_start().await;
        let flow = session
            .config
            .target_rx
            .expect("validated TCP RX session carries a target RX flow");
        let duration_millis = match session.config.completion {
            HilCompletion::DurationMillis(duration) => duration,
            HilCompletion::TransferBytes(_) | HilCompletion::HostStop => {
                unreachable!("protocol owner accepts only duration-completed sessions")
            }
        };
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=PASS stage=tcp-rx-session-start session={} \
             chunk={} duration_ms={} offered_bps={:?}",
            session.session_id, flow.payload_bytes, duration_millis, flow.offered_rate_bps,
        ));

        let hardware_start = registers.borrow().rx_statistics_snapshot().primary;
        let pipeline_start = pipeline_counters.snapshot();
        let accept_timeout =
            Duration::from_millis(u64::from(duration_millis)) + Duration::from_secs(5);
        let mut bytes = 0_u64;
        let mut read_errors = 0_u32;
        let mut eof = false;
        let accepted = matches!(
            with_timeout(accept_timeout, socket.accept(config.local_port)).await,
            Ok(Ok(()))
        );
        let started = Instant::now();
        if accepted {
            loop {
                match with_timeout(config.idle_timeout, socket.read(read_buffer)).await {
                    Ok(Ok(0)) => {
                        eof = true;
                        break;
                    }
                    Ok(Ok(length)) => bytes = bytes.saturating_add(length as u64),
                    Ok(Err(_)) | Err(_) => {
                        read_errors = read_errors.saturating_add(1);
                        break;
                    }
                }
            }
        } else {
            read_errors = read_errors.saturating_add(1);
        }
        let elapsed_us = started.elapsed().as_micros().max(1);
        socket.abort();

        let hardware_delta = registers
            .borrow()
            .rx_statistics_snapshot()
            .primary
            .wrapping_delta_since(hardware_start);
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
        let transport_errors = read_errors.saturating_add(health_errors);
        let throughput_kbps = bytes
            .saturating_mul(8)
            .saturating_mul(1_000)
            .checked_div(elapsed_us)
            .unwrap_or(0);
        emergency_log(format_args!(
            "OTCPRX b={bytes} s={} u={elapsed_us} k={throughput_kbps} e={transport_errors} \
             bf={} fo={} enq={enqueued} drop={queue_dropped} eof={}",
            u8::from(accepted),
            hardware_delta.buffer_full,
            hardware_delta.fifo_overflow,
            u8::from(eof),
        ));
        let passed = accepted && eof && bytes != 0 && transport_errors == 0;
        complete_session(
            session.session_id,
            TransportEvidence {
                rx_bytes: bytes,
                tx_bytes: 0,
                rx_units: u64::from(accepted && eof),
                tx_units: 0,
                elapsed_micros: elapsed_us,
                transport_errors,
            },
            passed,
        )
        .await;
    }
}
