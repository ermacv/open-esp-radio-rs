#![forbid(unsafe_code)]

use embassy_futures::yield_now;
use embassy_net::{Stack, udp::UdpSocket};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use open_esp_radio_esp32s31_embassy_wifi::Esp32s31QualificationSnapshot;
use open_esp_radio_hil_esp32s31_telemetry::{
    rx_pipeline::RxPipelineCounters, task_poll::TaskPollSet,
};
use open_esp_radio_hil_protocol::{
    Direction as HilDirection, Event as HilEvent, ServiceInfo, SessionReady,
    Transport as HilTransport, TransportEvidence,
};

use super::UdpSocketBuffers;
use crate::{
    console::{complete_session, emergency_log, publish_event_reliably, receive_session_start},
    product_hil::traffic::{
        BidirectionalResultChannel, BidirectionalSessionChannel, OpenRadioBidirectionalDirection,
        UdpSequenceEvidence, complete_open_radio_bidirectional_direction, iperf2_udp_sequence,
        log_open_radio_rx_pipeline_interval, log_open_radio_task_poll_interval,
    },
};

#[derive(Clone, Copy)]
pub(in crate::product_hil) enum UdpRxSessionSource {
    Console,
    Bidirectional {
        sessions: &'static BidirectionalSessionChannel,
        results: &'static BidirectionalResultChannel,
    },
}

#[derive(Clone, Copy)]
pub(in crate::product_hil) struct UdpRxBenchmarkConfig {
    pub local_port: u16,
    pub queue_depth: usize,
    pub payload_capacity: usize,
    pub idle_timeout: Duration,
    pub task_poll_telemetry: bool,
    pub code_address: usize,
    pub session_source: UdpRxSessionSource,
}

#[derive(Clone, Copy)]
pub(in crate::product_hil) struct UdpRxTelemetry {
    pub qualification: Esp32s31QualificationSnapshot,
    pub pipeline: &'static RxPipelineCounters,
    pub task_polls: &'static TaskPollSet,
}

pub(in crate::product_hil) async fn run_open_radio_udp_rx_benchmark<'a>(
    stack: Stack<'a>,
    buffers: UdpSocketBuffers<'a>,
    config: UdpRxBenchmarkConfig,
    telemetry: UdpRxTelemetry,
) -> ! {
    stack.wait_config_up().await;
    while stack.config_v4().is_none() {
        Timer::after_millis(100).await;
    }
    let mut socket = UdpSocket::new(
        stack,
        buffers.rx_metadata,
        buffers.rx,
        buffers.tx_metadata,
        buffers.tx,
    );
    socket
        .bind(config.local_port)
        .unwrap_or_else(|error| panic!("production UDP RX socket bind failed: {error:?}"));
    publish_event_reliably(
        0,
        0,
        HilEvent::ServiceReady(ServiceInfo {
            transport: HilTransport::Udp,
            direction: HilDirection::Rx,
            local_port: config.local_port,
            maximum_payload_bytes: config.payload_capacity as u16,
        }),
    )
    .await;
    emergency_log(format_args!(
        "OPEN_RADIO_HIL result=PASS stage=udp-rx-ready port={} queue={} payload={}",
        config.local_port, config.queue_depth, config.payload_capacity,
    ));

    loop {
        let session = match config.session_source {
            UdpRxSessionSource::Console => receive_session_start().await,
            UdpRxSessionSource::Bidirectional { sessions, .. } => sessions.receive().await,
        };
        publish_event_reliably(
            session.session_id,
            0,
            HilEvent::SessionReady(SessionReady {
                direction: HilDirection::Rx,
                tx_block_ack_tid: None,
            }),
        )
        .await;
        yield_now().await;

        let hardware_start = telemetry
            .qualification
            .rx_statistics()
            .map(|value| value.primary);
        let irq_start = telemetry.qualification.rx_interrupt_posts();
        let pipeline_start = telemetry.pipeline.snapshot();
        let task_poll_start = telemetry.task_polls.snapshot();
        let expected_payload_bytes = usize::from(
            session
                .config
                .target_rx
                .expect("validated RX session carries an RX flow")
                .payload_bytes,
        );

        let (first_length, first_sequence) = loop {
            let (length, sequence) = socket
                .recv_from_with(|packet, _| (packet.len(), iperf2_udp_sequence(packet)))
                .await;
            if sequence.is_some_and(|sequence| sequence < 0) {
                continue;
            }
            break (length, sequence);
        };
        let started = Instant::now();
        let mut last_packet = started;
        let mut bytes = first_length as u64;
        let mut datagrams = 1_u64;
        let mut receive_errors = u32::from(first_length != expected_payload_bytes);
        let mut terminal_seen = false;
        let mut sequence = UdpSequenceEvidence::default();
        sequence.observe(first_sequence);

        loop {
            match with_timeout(
                config.idle_timeout,
                socket.recv_from_with(|packet, _| (packet.len(), iperf2_udp_sequence(packet))),
            )
            .await
            {
                Ok((_, Some(value))) if value < 0 => {
                    terminal_seen = true;
                    break;
                }
                Ok((length, packet_sequence)) => {
                    receive_errors =
                        receive_errors.saturating_add(u32::from(length != expected_payload_bytes));
                    let received_at = Instant::now();
                    sequence.observe(packet_sequence);
                    sequence.observe_interarrival(
                        packet_sequence,
                        received_at.duration_since(last_packet).as_micros(),
                    );
                    bytes = bytes.saturating_add(length as u64);
                    datagrams = datagrams.saturating_add(1);
                    last_packet = received_at;
                }
                Err(_) => break,
            }
        }

        let elapsed_us = last_packet.duration_since(started).as_micros().max(1);
        let hardware = telemetry
            .qualification
            .rx_statistics()
            .map(|value| value.primary)
            .zip(hardware_start)
            .map(|(current, earlier)| current.wrapping_delta_since(earlier))
            .unwrap_or_default();
        let rx_irq_posts = telemetry
            .qualification
            .rx_interrupt_posts()
            .wrapping_sub(irq_start);
        let pipeline_end = telemetry.pipeline.snapshot();
        let queue_dropped = pipeline_end
            .network_dropped
            .wrapping_sub(pipeline_start.network_dropped);
        receive_errors = receive_errors.saturating_add(queue_dropped);
        let throughput_kbps = bytes
            .saturating_mul(8_000)
            .checked_div(elapsed_us)
            .unwrap_or(0);
        emergency_log(format_args!(
            "ORX b={bytes} d={datagrams} u={elapsed_us} k={throughput_kbps} e={receive_errors} \
             terminal={} first={} highest={} missing={} backward={} duplicate={} gap_us={} \
             mpdu={} success={} fcs={} full={} overflow={} irq={} code={}",
            u8::from(terminal_seen),
            sequence.first.unwrap_or(u32::MAX),
            sequence.highest,
            sequence.forward_missing,
            sequence.backward,
            sequence.adjacent_duplicates,
            sequence.maximum_interarrival_micros,
            hardware.mpdu_count,
            hardware.data_success,
            hardware.fcs_error,
            hardware.buffer_full,
            hardware.fifo_overflow,
            rx_irq_posts,
            config.code_address,
        ));
        log_open_radio_rx_pipeline_interval(pipeline_start, rx_irq_posts, telemetry.pipeline);
        log_open_radio_task_poll_interval(
            task_poll_start,
            config.task_poll_telemetry,
            telemetry.task_polls,
        );
        let evidence = TransportEvidence {
            rx_bytes: bytes,
            tx_bytes: 0,
            rx_units: datagrams,
            tx_units: 0,
            elapsed_micros: elapsed_us,
            transport_errors: receive_errors,
        };
        let passed = terminal_seen && receive_errors == 0;
        match config.session_source {
            UdpRxSessionSource::Bidirectional { results, .. } => {
                complete_open_radio_bidirectional_direction(
                    results,
                    session.session_id,
                    OpenRadioBidirectionalDirection::Rx,
                    evidence,
                    passed,
                )
                .await;
            }
            UdpRxSessionSource::Console => {
                complete_session(session.session_id, evidence, passed).await;
            }
        }
    }
}
