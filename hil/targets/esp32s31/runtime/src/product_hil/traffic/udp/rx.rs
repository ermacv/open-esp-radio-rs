#![forbid(unsafe_code)]

use core::sync::atomic::Ordering;

use embassy_futures::yield_now;
use embassy_net::{Stack, udp::UdpSocket};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use open_esp_radio_esp32s31_embassy_wifi::Esp32s31QualificationSnapshot;
use open_esp_radio_hil_esp32s31_telemetry::{
    rx_evidence::RX_HE_MCS_BUCKETS, rx_pipeline::RxPipelineCounters, task_poll::TaskPollSet,
};
use open_esp_radio_hil_protocol::{
    Direction as HilDirection, Event as HilEvent, ServiceInfo, SessionReady,
    Transport as HilTransport, TransportEvidence,
};

use super::UdpSocketBuffers;
use crate::{
    console::{complete_session, emergency_log, publish_event_reliably, receive_session_start},
    product_hil::{
        rx_qualification,
        traffic::{
            BidirectionalResultChannel, BidirectionalSessionChannel,
            OpenRadioBidirectionalDirection, UdpSequenceEvidence,
            complete_open_radio_bidirectional_direction, iperf2_udp_sequence,
            log_open_radio_rx_pipeline_interval, log_open_radio_task_poll_interval,
        },
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
        let irq_classification_start = crate::product_hil::MAC_IRQ.snapshot();
        let _ = crate::product_hil::MAC_IRQ.take_auxiliary_status_or();
        let _ = crate::product_hil::MAC_IRQ.take_unknown_status_or();
        let pipeline_start = telemetry.pipeline.snapshot();
        let task_poll_start = telemetry.task_polls.snapshot();
        rx_qualification::LAST_FORMAT.store(u32::MAX, Ordering::Relaxed);
        rx_qualification::LAST_PHY.store(u32::MAX, Ordering::Relaxed);
        let phy_start = rx_qualification::RX_PHY.snapshot();
        let s_mpdu_start = rx_qualification::RX_S_MPDU.snapshot();
        let beacon_s_mpdu_start = rx_qualification::BEACON_S_MPDU.snapshot();
        let ampdu_start = rx_qualification::RX_AMPDU.snapshot();
        #[cfg(feature = "rx-order-telemetry")]
        let order_start = rx_qualification::RX_ORDER.snapshot();
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
        let irq_classification = crate::product_hil::MAC_IRQ
            .snapshot()
            .wrapping_delta_since(irq_classification_start);
        let mac_irq_entries = irq_classification
            .spurious_entries
            .saturating_add(irq_classification.rx_only_entries)
            .saturating_add(irq_classification.rx_mixed_entries)
            .saturating_add(irq_classification.tx_only_entries)
            .saturating_add(irq_classification.tx_mixed_entries)
            .saturating_add(irq_classification.other_only_entries);
        let irq_auxiliary_status_or = crate::product_hil::MAC_IRQ.take_auxiliary_status_or();
        let irq_unknown_status_or = crate::product_hil::MAC_IRQ.take_unknown_status_or();
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
        emergency_log(format_args!(
            "ORXP f={} p={}",
            rx_qualification::LAST_FORMAT.load(Ordering::Relaxed),
            rx_qualification::LAST_PHY.load(Ordering::Relaxed),
        ));
        emergency_log(format_args!(
            "ORXQ first={} highest={} next={} gap_events={} forward_missing={} \
             maximum_gap={} maximum_gap_at={} first_gap_at={} last_gap_at={} backward={} \
             adjacent_duplicates={} unsequenced={} maximum_interarrival_us={} \
             maximum_interarrival_at={}",
            sequence.first.unwrap_or(u32::MAX),
            sequence.first.map(|_| sequence.highest).unwrap_or(u32::MAX),
            sequence
                .first
                .map(|_| sequence.expected)
                .unwrap_or(u32::MAX),
            sequence.gap_events,
            sequence.forward_missing,
            sequence.maximum_gap,
            sequence.maximum_gap_at.unwrap_or(u32::MAX),
            sequence.first_gap_at.unwrap_or(u32::MAX),
            sequence.last_gap_at.unwrap_or(u32::MAX),
            sequence.backward,
            sequence.adjacent_duplicates,
            sequence.unsequenced,
            sequence.maximum_interarrival_micros,
            sequence.maximum_interarrival_at.unwrap_or(u32::MAX),
        ));
        #[cfg(feature = "rx-order-telemetry")]
        {
            let order = rx_qualification::RX_ORDER
                .snapshot()
                .wrapping_delta_since(order_start);
            emergency_log(format_args!(
                "ORXO gap_events={} forward_missing={} backward={} adjacent_duplicates={} \
                 backward_mac_backward={} backward_mac_same={} backward_mac_forward={} \
                 backward_mac_other_tid={} backward_mac_unavailable={}",
                order.gap_events,
                order.forward_missing,
                order.backward,
                order.adjacent_duplicates,
                order.backward_mac_backward,
                order.backward_mac_same,
                order.backward_mac_forward,
                order.backward_mac_other_tid,
                order.backward_mac_unavailable,
            ));
        }
        let rx_s_mpdu = rx_qualification::RX_S_MPDU
            .snapshot()
            .wrapping_delta_since(s_mpdu_start);
        let beacon_s_mpdu = rx_qualification::BEACON_S_MPDU
            .snapshot()
            .wrapping_delta_since(beacon_s_mpdu_start);
        let rx_ampdu = rx_qualification::RX_AMPDU
            .snapshot()
            .wrapping_delta_since(ampdu_start);
        emergency_log(format_args!(
            "ORXSM s_mpdu={} not_s_mpdu={} unavailable={} beacon_s_mpdu={} \
             beacon_not_s_mpdu={} beacon_unavailable={}",
            rx_s_mpdu.s_mpdu_frames,
            rx_s_mpdu.not_s_mpdu_frames,
            rx_s_mpdu.unavailable_frames,
            beacon_s_mpdu.s_mpdu_frames,
            beacon_s_mpdu.not_s_mpdu_frames,
            beacon_s_mpdu.unavailable_frames,
        ));
        emergency_log(format_args!(
            "ORXAG ampdu={} not_ampdu={} hardware_ampdu={} hardware_not_ampdu={} \
             protocol_ampdu={} protocol_not_ampdu={} unavailable={}",
            rx_ampdu.ampdu_frames,
            rx_ampdu.not_ampdu_frames,
            rx_ampdu.hardware_ampdu_frames,
            rx_ampdu.hardware_not_ampdu_frames,
            rx_ampdu.protocol_ampdu_frames,
            rx_ampdu.protocol_not_ampdu_frames,
            rx_ampdu.unavailable_frames,
        ));
        let phy_end = rx_qualification::RX_PHY.snapshot();
        let mcs = core::array::from_fn::<_, RX_HE_MCS_BUCKETS, _>(|index| {
            phy_end.he_mcs[index].wrapping_sub(phy_start.he_mcs[index])
        });
        emergency_log(format_args!(
            "ORXM m0={} m1={} m2={} m3={} m4={} m5={} m6={} m7={} m8={} m9={} \
             m10={} m11={} other={}",
            mcs[0],
            mcs[1],
            mcs[2],
            mcs[3],
            mcs[4],
            mcs[5],
            mcs[6],
            mcs[7],
            mcs[8],
            mcs[9],
            mcs[10],
            mcs[11],
            phy_end.other.wrapping_sub(phy_start.other),
        ));
        log_open_radio_rx_pipeline_interval(
            pipeline_start,
            rx_irq_posts,
            mac_irq_entries,
            irq_classification,
            irq_auxiliary_status_or,
            irq_unknown_status_or,
            telemetry.pipeline,
        );
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
