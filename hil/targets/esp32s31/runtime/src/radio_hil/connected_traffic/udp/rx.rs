#![forbid(unsafe_code)]

use core::{
    cell::RefCell,
    sync::atomic::{AtomicU32, Ordering},
};

use embassy_futures::yield_now;
use embassy_net::{Stack, udp::UdpSocket};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use open_esp_radio::{
    adapters::esp32s31::wifi_embassy::embassy_irq::EmbassyMacIrqRuntime,
    esp32s31::{hal::RadioRegisters, wifi::lmac::tx::TxPhyRate},
    wifi::ieee80211::station::StaAssociationPhy,
};
use open_esp_radio_hil_esp32s31_telemetry::{
    mac_irq::MacIrqClassificationCounters,
    rx_evidence::{RX_HE_MCS_BUCKETS, RxAmpduCounters, RxPhyCounters, RxSmpduCounters},
    rx_order::RxOrderCounters,
    rx_pipeline::RxPipelineCounters,
    task_poll::TaskPollSet,
};
use open_esp_radio_hil_protocol::{
    Direction as HilDirection, Event as HilEvent, ServiceInfo, SessionReady,
    Transport as HilTransport, TransportEvidence,
};

use super::UdpSocketBuffers;
use crate::{
    console::{complete_session, emergency_log, publish_event_reliably, receive_session_start},
    radio_hil::connected_traffic::{
        BidirectionalResultChannel, BidirectionalSessionChannel, OpenRadioBidirectionalDirection,
        UdpSequenceEvidence, complete_open_radio_bidirectional_direction, iperf2_udp_sequence,
        log_open_radio_rx_pipeline_interval, log_open_radio_task_poll_interval,
    },
};

#[derive(Clone, Copy)]
pub(in crate::radio_hil) enum UdpRxSessionSource {
    Standalone,
    Console,
    Bidirectional {
        sessions: &'static BidirectionalSessionChannel,
        results: &'static BidirectionalResultChannel,
    },
}

#[derive(Clone, Copy)]
pub(in crate::radio_hil) struct UdpRxBenchmarkConfig {
    pub local_port: u16,
    pub queue_depth: usize,
    pub payload_capacity: usize,
    pub idle_timeout: Duration,
    pub application_handoff_budget: Duration,
    pub task_poll_telemetry: bool,
    pub rx_order_telemetry: bool,
    pub code_address: usize,
    pub session_source: UdpRxSessionSource,
}

#[derive(Clone, Copy)]
pub(in crate::radio_hil) struct UdpRxTelemetry {
    pub last_format: &'static AtomicU32,
    pub last_phy: &'static AtomicU32,
    pub phy: &'static RxPhyCounters,
    pub s_mpdu: &'static RxSmpduCounters,
    pub beacon_s_mpdu: &'static RxSmpduCounters,
    pub ampdu: &'static RxAmpduCounters,
    pub order: &'static RxOrderCounters,
    pub pipeline: &'static RxPipelineCounters,
    pub task_polls: &'static TaskPollSet,
    pub reload_delays: &'static AtomicU32,
    pub irq_runtime: &'static EmbassyMacIrqRuntime<CriticalSectionRawMutex>,
    pub irq_entries: &'static AtomicU32,
    pub irq_classification: &'static MacIrqClassificationCounters,
}

/// Host-to-device UDP throughput baseline for the fully open data path.
///
/// A sample starts on the first payload datagram and closes after a quiet
/// interval. The first four bytes are interpreted only to discard iperf2's
/// negative terminal/report datagrams; ordinary UDP payloads remain valid.
pub(in crate::radio_hil) async fn run_open_radio_udp_rx_benchmark<'a>(
    stack: Stack<'a>,
    association_phy: StaAssociationPhy,
    data_tx_rate: TxPhyRate,
    registers: &RefCell<&mut RadioRegisters>,
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
    if let Err(error) = socket.bind(config.local_port) {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=FAIL stage=udp-rx-bind port={} error={error:?}",
            config.local_port,
        ));
        loop {
            Timer::after_secs(60).await;
        }
    }
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
        "OPEN_RADIO_PHY_HIL result=PASS stage=udp-rx-ready port={} queue={} \
         payload_capacity={} bandwidth_mhz={} phy={} rate_code={:#04x} rate_kbps={}",
        config.local_port,
        config.queue_depth,
        config.payload_capacity,
        association_phy.bandwidth_mhz(),
        association_phy.name(),
        data_tx_rate.code(),
        data_tx_rate.nominal_kbps(),
    ));

    let mut last_radio_handoff = Instant::now();
    loop {
        let session = match config.session_source {
            UdpRxSessionSource::Standalone => None,
            UdpRxSessionSource::Console => Some(receive_session_start().await),
            UdpRxSessionSource::Bidirectional { sessions, .. } => Some(sessions.receive().await),
        };
        if let Some(session) = session {
            publish_event_reliably(
                session.session_id,
                0,
                HilEvent::SessionReady(SessionReady {
                    direction: HilDirection::Rx,
                    tx_block_ack_tid: None,
                }),
            )
            .await;
        }
        yield_now().await;
        telemetry.last_format.store(u32::MAX, Ordering::Relaxed);
        telemetry.last_phy.store(u32::MAX, Ordering::Relaxed);
        let hardware_start = registers.borrow().rx_statistics_snapshot().primary;
        let phy_start = telemetry.phy.snapshot();
        let s_mpdu_start = telemetry.s_mpdu.snapshot();
        let beacon_s_mpdu_start = telemetry.beacon_s_mpdu.snapshot();
        let ampdu_start = telemetry.ampdu.snapshot();
        let order_start = telemetry.order.snapshot();
        let pipeline_start = telemetry.pipeline.snapshot();
        let task_poll_start = telemetry.task_polls.snapshot();
        let reload_delay_start = telemetry.reload_delays.load(Ordering::Relaxed);
        let irq_start = telemetry.irq_runtime.rx_post_count();
        let irq_entry_start = telemetry.irq_entries.load(Ordering::Relaxed);
        let irq_classification_start = telemetry.irq_classification.snapshot();
        let _ = telemetry.irq_classification.take_auxiliary_status_or();
        let _ = telemetry.irq_classification.take_unknown_status_or();
        let (first_length, first_sequence) = loop {
            let received = socket
                .recv_from_with(|packet, _| (packet.len(), iperf2_udp_sequence(packet)))
                .await;
            yield_to_pending_radio_rx(
                &mut last_radio_handoff,
                telemetry.irq_runtime,
                config.application_handoff_budget,
            )
            .await;
            let (length, sequence) = received;
            if sequence.is_some_and(|sequence| sequence < 0) {
                continue;
            }
            break (length, sequence);
        };
        let started = Instant::now();
        let mut last_packet = started;
        let mut bytes = first_length as u64;
        let mut datagrams = 1_u64;
        let expected_payload_bytes = session.map(|session| {
            usize::from(
                session
                    .config
                    .target_rx
                    .expect("validated RX session carries a target RX flow")
                    .payload_bytes,
            )
        });
        let mut receive_errors =
            u32::from(expected_payload_bytes.is_some_and(|expected| first_length != expected));
        let mut terminal_seen = false;
        let mut sequence_evidence = UdpSequenceEvidence::default();
        sequence_evidence.observe(first_sequence);

        loop {
            let received = with_timeout(
                config.idle_timeout,
                socket.recv_from_with(|packet, _| (packet.len(), iperf2_udp_sequence(packet))),
            )
            .await;
            yield_to_pending_radio_rx(
                &mut last_radio_handoff,
                telemetry.irq_runtime,
                config.application_handoff_budget,
            )
            .await;
            match received {
                Ok((length, sequence)) => {
                    if sequence.is_some_and(|sequence| sequence < 0) {
                        terminal_seen = true;
                        break;
                    }
                    receive_errors = receive_errors.saturating_add(u32::from(
                        expected_payload_bytes.is_some_and(|expected| length != expected),
                    ));
                    let received_at = Instant::now();
                    sequence_evidence.observe(sequence);
                    sequence_evidence.observe_interarrival(
                        sequence,
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
        let throughput_kbps = bytes
            .saturating_mul(8)
            .saturating_mul(1_000)
            .checked_div(elapsed_us)
            .unwrap_or(0);
        let hardware_delta = registers
            .borrow()
            .rx_statistics_snapshot()
            .primary
            .wrapping_delta_since(hardware_start);
        let pipeline_end = telemetry.pipeline.snapshot();
        let enqueued = pipeline_end
            .network_enqueued
            .wrapping_sub(pipeline_start.network_enqueued);
        let queue_dropped = pipeline_end
            .network_dropped
            .wrapping_sub(pipeline_start.network_dropped);
        let reload_delays = telemetry
            .reload_delays
            .load(Ordering::Relaxed)
            .wrapping_sub(reload_delay_start);
        let rx_irqs = telemetry
            .irq_runtime
            .rx_post_count()
            .wrapping_sub(irq_start);
        let irq_entries = telemetry
            .irq_entries
            .load(Ordering::Relaxed)
            .wrapping_sub(irq_entry_start);
        let irq_classification = telemetry
            .irq_classification
            .snapshot()
            .wrapping_delta_since(irq_classification_start);
        let irq_auxiliary_status_or = telemetry.irq_classification.take_auxiliary_status_or();
        let irq_unknown_status_or = telemetry.irq_classification.take_unknown_status_or();
        let rx_format = telemetry.last_format.load(Ordering::Relaxed);
        let rx_phy = telemetry.last_phy.load(Ordering::Relaxed);
        let rx_he_valid = rx_phy >> 31;
        let rx_rate = (rx_phy >> 4) & 0x1f;
        let rx_mcs = (rx_phy >> 9) & 0x0f;
        let rx_gi_ltf = (rx_phy >> 13) & 0x03;
        let rx_bandwidth_mhz = 20_u32 << ((rx_phy >> 15) & 0x03);
        let rx_dcm = (rx_phy >> 17) & 1;
        let rx_ldpc = (rx_phy >> 18) & 1;
        let phy_end = telemetry.phy.snapshot();
        let rx_mcs_histogram = core::array::from_fn::<_, RX_HE_MCS_BUCKETS, _>(|index| {
            phy_end.he_mcs[index].wrapping_sub(phy_start.he_mcs[index])
        });
        let rx_other_phy = phy_end.other.wrapping_sub(phy_start.other);
        let rx_s_mpdu = telemetry
            .s_mpdu
            .snapshot()
            .wrapping_delta_since(s_mpdu_start);
        let beacon_s_mpdu = telemetry
            .beacon_s_mpdu
            .snapshot()
            .wrapping_delta_since(beacon_s_mpdu_start);
        let rx_ampdu = telemetry.ampdu.snapshot().wrapping_delta_since(ampdu_start);
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=BENCH stage=udp-rx bytes={bytes} datagrams={datagrams} \
             elapsed_us={elapsed_us} throughput_kbps={throughput_kbps} \
             receive_errors={receive_errors} terminal={} bandwidth_mhz={} phy={} \
             rate_code={:#04x} rate_kbps={} code_address={}",
            u8::from(terminal_seen),
            association_phy.bandwidth_mhz(),
            association_phy.name(),
            data_tx_rate.code(),
            data_tx_rate.nominal_kbps(),
            config.code_address,
        ));
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=BENCH stage=udp-rx-path \
             mpdu={} data_success={} fcs_error={} buffer_full={} fifo_overflow={} \
             enqueued={enqueued} queue_dropped={queue_dropped} rx_irqs={rx_irqs} \
             reload_delays={reload_delays} rx_format={rx_format} rx_rate={rx_rate} \
             rx_he_valid={rx_he_valid} rx_mcs={rx_mcs} rx_gi_ltf={rx_gi_ltf} \
             rx_bandwidth_mhz={rx_bandwidth_mhz} rx_dcm={rx_dcm} rx_ldpc={rx_ldpc}",
            hardware_delta.mpdu_count,
            hardware_delta.data_success,
            hardware_delta.fcs_error,
            hardware_delta.buffer_full,
            hardware_delta.fifo_overflow,
        ));
        emergency_log(format_args!(
            "ORXQ first={} highest={} next={} gap_events={} forward_missing={} \
             maximum_gap={} maximum_gap_at={} first_gap_at={} last_gap_at={} backward={} \
             adjacent_duplicates={} unsequenced={} maximum_interarrival_us={} \
             maximum_interarrival_at={}",
            sequence_evidence.first.unwrap_or(u32::MAX),
            sequence_evidence
                .first
                .map(|_| sequence_evidence.highest)
                .unwrap_or(u32::MAX),
            sequence_evidence
                .first
                .map(|_| sequence_evidence.expected)
                .unwrap_or(u32::MAX),
            sequence_evidence.gap_events,
            sequence_evidence.forward_missing,
            sequence_evidence.maximum_gap,
            sequence_evidence.maximum_gap_at.unwrap_or(u32::MAX),
            sequence_evidence.first_gap_at.unwrap_or(u32::MAX),
            sequence_evidence.last_gap_at.unwrap_or(u32::MAX),
            sequence_evidence.backward,
            sequence_evidence.adjacent_duplicates,
            sequence_evidence.unsequenced,
            sequence_evidence.maximum_interarrival_micros,
            sequence_evidence
                .maximum_interarrival_at
                .unwrap_or(u32::MAX),
        ));
        if config.rx_order_telemetry {
            let order = telemetry.order.snapshot().wrapping_delta_since(order_start);
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
        emergency_log(format_args!(
            "ORXSM s_mpdu={} not_s_mpdu={} unavailable={} \
             beacon_s_mpdu={} beacon_not_s_mpdu={} beacon_unavailable={}",
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
        emergency_log(format_args!(
            "ORXM m0={} m1={} m2={} m3={} m4={} m5={} m6={} m7={} m8={} \
             m9={} m10={} m11={} other={rx_other_phy}",
            rx_mcs_histogram[0],
            rx_mcs_histogram[1],
            rx_mcs_histogram[2],
            rx_mcs_histogram[3],
            rx_mcs_histogram[4],
            rx_mcs_histogram[5],
            rx_mcs_histogram[6],
            rx_mcs_histogram[7],
            rx_mcs_histogram[8],
            rx_mcs_histogram[9],
            rx_mcs_histogram[10],
            rx_mcs_histogram[11],
        ));
        log_open_radio_rx_pipeline_interval(
            pipeline_start,
            rx_irqs,
            irq_entries,
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
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=PASS stage=udp-rx-interval-complete \
             datagrams={datagrams} terminal={}",
            u8::from(terminal_seen),
        ));
        if let Some(session) = session {
            let evidence = TransportEvidence {
                rx_bytes: bytes,
                tx_bytes: 0,
                rx_units: datagrams,
                tx_units: 0,
                elapsed_micros: elapsed_us,
                transport_errors: receive_errors,
            };
            match config.session_source {
                UdpRxSessionSource::Bidirectional { results, .. } => {
                    complete_open_radio_bidirectional_direction(
                        results,
                        session.session_id,
                        OpenRadioBidirectionalDirection::Rx,
                        evidence,
                        terminal_seen && receive_errors == 0,
                    )
                    .await;
                }
                UdpRxSessionSource::Console | UdpRxSessionSource::Standalone => {
                    complete_session(
                        session.session_id,
                        evidence,
                        terminal_seen && receive_errors == 0,
                    )
                    .await;
                }
            }
        }
    }
}

async fn yield_to_pending_radio_rx(
    last_handoff: &mut Instant,
    irq_runtime: &EmbassyMacIrqRuntime<CriticalSectionRawMutex>,
    application_handoff_budget: Duration,
) {
    if irq_runtime.rx_signaled() && last_handoff.elapsed() >= application_handoff_budget {
        yield_now().await;
        *last_handoff = Instant::now();
    }
}
