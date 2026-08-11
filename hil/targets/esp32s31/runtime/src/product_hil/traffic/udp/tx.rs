#![forbid(unsafe_code)]

use embassy_futures::yield_now;
use embassy_net::{Ipv4Address, Stack, udp::UdpSocket};
use embassy_time::{Duration, Instant, Timer};
use open_esp_radio_esp32s31_embassy_wifi::Esp32s31QualificationSnapshot;
use open_esp_radio_hil_esp32s31_telemetry::aggregate_tx::AggregateTxCounters;
use open_esp_radio_hil_protocol::{
    Completion as HilCompletion, Direction as HilDirection, Event as HilEvent, RadioEvidence,
    ServiceInfo, SessionReady, Transport as HilTransport, TransportEvidence, TxRadioEvidence,
};

use super::UdpSocketBuffers;
use crate::{
    console::{publish_event_reliably, runtime_log},
    product_hil::traffic::{
        BidirectionalResultChannel, BidirectionalSessionChannel, OpenRadioBidirectionalDirection,
        complete_open_radio_bidirectional_direction, log_open_radio_ampdu_interval,
        wait_session_link_requirements,
    },
};

#[derive(Clone, Copy)]
pub(in crate::product_hil) struct UdpTxSessionSource {
    pub sessions: &'static BidirectionalSessionChannel,
    pub results: &'static BidirectionalResultChannel,
}

#[derive(Clone, Copy)]
pub(in crate::product_hil) struct UdpTxBenchmarkConfig {
    pub source_port: u16,
    pub queue_depth: usize,
    pub payload_capacity: usize,
    /// Maximum application datagrams admitted before enforcing the next
    /// absolute offered-rate deadline. The composition root derives this from
    /// the active plus prepared-ahead A-MPDU arenas, not an arbitrary poll
    /// batch.
    pub pacing_group_datagrams: u8,
    pub drain: Duration,
    pub code_address: usize,
    pub qualification: Esp32s31QualificationSnapshot,
    pub session_source: UdpTxSessionSource,
}

/// Device-to-host UDP load through Embassy and the open TX scheduler.
pub(in crate::product_hil) async fn run_open_radio_udp_tx_benchmark<'a>(
    stack: Stack<'a>,
    buffers: UdpSocketBuffers<'a>,
    packet: &mut [u8],
    config: UdpTxBenchmarkConfig,
    aggregate_counters: &AggregateTxCounters,
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
    if let Err(error) = socket.bind(config.source_port) {
        runtime_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=FAIL stage=udp-tx-bind error={error:?}"
        ));
        loop {
            Timer::after_secs(60).await;
        }
    }
    // Complete the connected-data-path settle before advertising readiness.
    // `Start` must mean the benchmark task can consume its session without a
    // hidden post-acceptance delay.
    Timer::after_secs(1).await;
    publish_event_reliably(
        0,
        0,
        HilEvent::ServiceReady(ServiceInfo {
            transport: HilTransport::Udp,
            direction: HilDirection::Tx,
            local_port: config.source_port,
            maximum_payload_bytes: config.payload_capacity as u16,
        }),
    )
    .await;
    runtime_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=udp-tx-ready \
         source_port={} queue={} payload_capacity={} tx_mode=ampdu session_protocol=required",
        config.source_port, config.queue_depth, config.payload_capacity,
    ));
    loop {
        let session = config.session_source.sessions.receive().await;
        wait_session_link_requirements(session.config.link_requirements, aggregate_counters).await;
        publish_event_reliably(
            session.session_id,
            0,
            HilEvent::SessionReady(SessionReady {
                direction: HilDirection::Tx,
                tx_block_ack_tid: session.config.link_requirements.tx_block_ack_tid,
            }),
        )
        .await;
        let peer = session
            .config
            .peer
            .expect("validated TX session carries a peer");
        let flow = session
            .config
            .target_tx
            .expect("validated TX session carries a target TX flow");
        let duration_millis = match session.config.completion {
            HilCompletion::DurationMillis(duration) => duration,
            HilCompletion::TransferBytes(_) | HilCompletion::HostStop => {
                unreachable!("protocol owner accepts only duration-completed sessions")
            }
        };
        let server = Ipv4Address::from_octets(peer.address);
        let server_port = peer.port;
        let payload_bytes = usize::from(flow.payload_bytes);
        let duration = Duration::from_millis(u64::from(duration_millis));
        let offered_rate_bps = flow.offered_rate_bps;
        // Group pacing is a physical credit for the active+standby A-MPDU
        // arenas, not a generic UDP batching knob. AP v1 declares no
        // BlockAck link requirement and owns one ordinary TX descriptor, so
        // carrying the STA credit of 64 into an AP session creates artificial
        // burst/idle periods instead of the requested offered load.
        let pacing_group_datagrams = if session.config.link_requirements.tx_block_ack_tid.is_some()
        {
            config.pacing_group_datagrams
        } else {
            1
        };
        runtime_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=PASS stage=udp-tx-session-start \
             session={} target={server}:{server_port} payload={payload_bytes} \
             duration_ms={} offered_bps={offered_rate_bps:?}",
            session.session_id,
            duration.as_millis(),
        ));
        let started = Instant::now();
        // TX owns A-MPDU evidence because its post-measurement drain proves
        // that the last publication reached a terminal BlockAck outcome.
        // The RX sibling can finish on the host terminal datagram while a
        // target aggregate is still in flight, so sampling there can tear one
        // logical publication across independent diagnostic atomics.
        let aggregate_start = match session.config.direction {
            HilDirection::Rx => None,
            _ => Some(aggregate_counters.snapshot()),
        };
        let mut next_send = started;
        let mut bytes = 0_u64;
        let mut datagrams = 0_u64;
        let mut send_errors = 0_u32;
        let cooperative_bidirectional = session.config.direction == HilDirection::Bidirectional;
        while started.elapsed() < duration {
            packet[..4].copy_from_slice(&(datagrams as u32).to_be_bytes());
            match socket
                .send_to(&packet[..payload_bytes], (server, server_port))
                .await
            {
                Ok(()) => {
                    bytes = bytes.saturating_add(payload_bytes as u64);
                    datagrams = datagrams.saturating_add(1);
                }
                Err(_) => send_errors = send_errors.saturating_add(1),
            }
            if cooperative_bidirectional {
                // TX and RX are sibling futures in the same HIL task. Once
                // the socket owns this datagram, yield one poll edge so a
                // continuously writable TX socket cannot starve RX dequeue.
                yield_now().await;
            }
            if let Some(rate_bps) = offered_rate_bps
                && datagrams.is_multiple_of(u64::from(pacing_group_datagrams))
            {
                // Enforce one byte-budget deadline per active+standby A-MPDU
                // pipeline capacity. Sleeping after every datagram prevented
                // the network queue from ever presenting an aggregate-sized
                // workload and halved throughput. A missed deadline is reset
                // after this bounded physical credit, so a scheduler stall
                // cannot trigger unbounded catch-up bursts.
                let group_nanos = u64::from(pacing_group_datagrams)
                    .saturating_mul(u64::try_from(payload_bytes).unwrap_or(u64::MAX))
                    .saturating_mul(8_000_000_000)
                    .saturating_add(rate_bps - 1)
                    / rate_bps;
                next_send += Duration::from_nanos(group_nanos);
                let now = Instant::now();
                if now < next_send {
                    Timer::at(next_send).await;
                } else {
                    next_send = now;
                }
            }
        }
        let elapsed_us = started.elapsed().as_micros().max(1);
        let throughput_kbps = bytes
            .saturating_mul(8)
            .saturating_mul(1_000)
            .checked_div(elapsed_us)
            .unwrap_or(0);
        // UDP enqueue completion precedes MAC acknowledgement. Keep draining
        // outside the measured interval so the structured result cannot race
        // the final network queue and A-MPDU exchange.
        Timer::after(config.drain).await;
        let tx_vector = config.qualification.tx_vector();
        // A link vector belongs to the associated-STA A-MPDU datapath. AP v1
        // intentionally exposes only legacy unicast TX, so a session whose
        // link requirements are NONE has no such vector. Conversely, a host
        // that explicitly required BlockAck must never receive a successful
        // session without the associated-link evidence it requested.
        assert!(
            session.config.link_requirements.tx_block_ack_tid.is_none() || tx_vector.is_some(),
            "BlockAck-qualified TX session retains its associated link vector",
        );
        runtime_log(format_args!(
            "OTX b={bytes} d={datagrams} u={elapsed_us} k={throughput_kbps} \
             e={send_errors} p={} pg={} w={} r={} code={}",
            offered_rate_bps.unwrap_or(0) / 1_000,
            pacing_group_datagrams,
            tx_vector.map_or(0, |vector| vector.bandwidth_mhz),
            tx_vector.map_or(0, |vector| vector.aggregate_rate_kbps),
            config.code_address,
        ));
        let aggregate = aggregate_start
            .map(|earlier| aggregate_counters.snapshot().wrapping_delta_since(earlier));
        if let Some(aggregate_start) = aggregate_start {
            log_open_radio_ampdu_interval(aggregate_start, aggregate_counters).await;
        }
        let evidence = TransportEvidence {
            rx_bytes: 0,
            tx_bytes: bytes,
            rx_units: 0,
            tx_units: datagrams,
            elapsed_micros: elapsed_us,
            transport_errors: send_errors,
        };
        let radio = aggregate
            .zip(tx_vector)
            .map(|(aggregate, tx_vector)| RadioEvidence {
                rx: None,
                tx: Some(TxRadioEvidence {
                    bandwidth_mhz: tx_vector.bandwidth_mhz,
                    aggregate_rate_kbps: u32::try_from(tx_vector.aggregate_rate_kbps)
                        .unwrap_or(u32::MAX),
                    aggregates_prepared: aggregate.aggregates_prepared,
                    aggregate_publications: aggregate.aggregate_publications,
                    aggregates_completed: aggregate.aggregates_completed,
                    subframes_prepared: aggregate.prepared_subframe_total(),
                    subframes_acknowledged: aggregate.subframes_acknowledged,
                    individual_retries: aggregate.individual_retries,
                    hardware_timeouts: aggregate.hardware_timeouts,
                    collisions: aggregate.collisions,
                    minimum_subframes: aggregate.minimum_prepared_subframes().unwrap_or(0),
                    maximum_subframes: aggregate.maximum_prepared_subframes().unwrap_or(0),
                    prepared_histogram: [
                        aggregate.prepared_in_range(1, 1),
                        aggregate.prepared_in_range(2, 3),
                        aggregate.prepared_in_range(4, 7),
                        aggregate.prepared_in_range(8, 15),
                        aggregate.prepared_in_range(16, 23),
                        aggregate.prepared_in_range(24, 30),
                        aggregate.prepared_in_range(31, 31),
                        aggregate.prepared_in_range(32, 32),
                    ],
                    stopped_at_frame_limit: aggregate.stopped_at_frame_limit,
                    stopped_at_capacity_limit: aggregate.stopped_at_capacity_limit,
                    stopped_on_empty_queue: aggregate.stopped_on_empty_queue,
                    preparation_micros: aggregate.preparation_micros,
                    publication_micros: aggregate.publication_program_micros,
                    exchange_micros: aggregate.exchange_micros,
                    block_ack_samples: aggregate.block_ack_samples,
                    block_ack_received: aggregate.block_ack_received,
                    success_without_block_ack: aggregate.success_without_block_ack,
                    nonzero_block_ack_control: aggregate.nonzero_block_ack_control,
                    full_block_ack: aggregate.full_block_ack,
                    partial_block_ack: aggregate.partial_block_ack,
                    empty_block_ack: aggregate.empty_block_ack,
                    tx_irq_epochs: aggregate.tx_irq_epochs,
                    tx_irq_service_samples: aggregate.tx_irq_service_samples,
                    tx_irq_clock_skew_samples: aggregate.tx_irq_clock_skew_samples,
                    tx_publication_to_irq_samples: aggregate.tx_publication_to_irq_samples,
                }),
            });
        complete_open_radio_bidirectional_direction(
            config.session_source.results,
            session.session_id,
            OpenRadioBidirectionalDirection::Tx,
            evidence,
            radio,
            None,
            send_errors == 0,
        )
        .await;
    }
}
