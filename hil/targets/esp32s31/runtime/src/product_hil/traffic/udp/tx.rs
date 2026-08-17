#![forbid(unsafe_code)]

use embassy_net::{Ipv4Address, Stack, udp::UdpSocket};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use open_esp_radio_hil_esp32s31_telemetry::aggregate_tx::AggregateTxCounters;
use open_esp_radio_hil_protocol::{
    Completion as HilCompletion, Direction as HilDirection, Event as HilEvent, ServiceInfo,
    SessionReady, Transport as HilTransport, TransportEvidence,
};

use super::UdpSocketBuffers;
use crate::{
    console::{publish_event_reliably, runtime_log},
    product_hil::traffic::{
        BidirectionalResultChannel, BidirectionalSessionChannel, OpenRadioBidirectionalDirection,
        aggregate_tx_evidence, complete_open_radio_bidirectional_direction,
        log_open_radio_ampdu_interval, log_open_radio_task_poll_interval,
        wait_session_link_requirements,
    },
    product_hil::{
        OPEN_RADIO_TASK_POLL_TELEMETRY, QualificationRequester, TASK_POLLS, qualification_sample,
    },
};

const MAX_PACING_CATCH_UP_GROUPS: u32 = 4;

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
    pub session_source: UdpTxSessionSource,
}

/// Device-to-host UDP load through Embassy and the open TX scheduler.
pub(in crate::product_hil) async fn run_open_radio_udp_tx_benchmark<'a>(
    stack: Stack<'a>,
    buffers: UdpSocketBuffers<'a>,
    packet: &'a mut [u8],
    config: UdpTxBenchmarkConfig,
    aggregate_counters: &AggregateTxCounters,
) -> ! {
    stack.wait_config_up().await;
    while stack.config_v4().is_none() {
        Timer::after_millis(100).await;
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
    // A typed session supplies the fixed datagram size before its SessionReady
    // event. Size the active payload ring to an exact multiple of that value:
    // smoltcp then never needs a metadata-bearing wrap-padding record and its
    // zero-copy FnOnce callback cannot be consumed by a retryable post-callback
    // `BufferFull`. This configures only the HIL load generator, not the radio.
    let first_session = config.session_source.sessions.receive().await;
    let first_payload_bytes = usize::from(
        first_session
            .config
            .target_tx
            .expect("validated TX session carries a target TX flow")
            .payload_bytes,
    );
    let active_tx_bytes = config
        .queue_depth
        .checked_mul(first_payload_bytes)
        .filter(|bytes| *bytes <= buffers.tx.len())
        .expect("validated TX session fits the static UDP payload arena");
    let socket_payload = &mut buffers.tx[..active_tx_bytes];
    let mut socket = UdpSocket::new(
        stack,
        buffers.rx_metadata,
        buffers.rx,
        buffers.tx_metadata,
        socket_payload,
    );
    let socket_payload_bytes = socket.payload_send_capacity();
    if let Err(error) = socket.bind(config.source_port) {
        runtime_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=FAIL stage=udp-tx-bind error={error:?}"
        ));
        loop {
            Timer::after_secs(60).await;
        }
    }
    let mut first_session = Some(first_session);
    loop {
        let session = match first_session.take() {
            Some(session) => session,
            None => config.session_source.sessions.receive().await,
        };
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
        // Group pacing is a CPU-side load-generator property, not evidence of
        // a negotiated radio capability. The bounded socket/WDEV queues own
        // admission while interval telemetry independently proves whether TX
        // actually used BlockAck/A-MPDU. Waiting after every datagram makes
        // the Embassy timer itself the throughput ceiling at high offered
        // rates.
        let pacing_group_datagrams = config.pacing_group_datagrams;
        runtime_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=PASS stage=udp-tx-session-start \
             session={} target={server}:{server_port} payload={payload_bytes} \
             duration_ms={} offered_bps={offered_rate_bps:?}",
            session.session_id,
            duration.as_millis(),
        ));
        let started = Instant::now();
        let task_poll_start = TASK_POLLS.snapshot();
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
        // One session timer bounds a permanently blocked socket admission.
        // Installing and cancelling a timeout for every datagram added about
        // 80 us to the 700-us 16-Mbit/s packet interval and made HIL itself
        // the measured throughput ceiling.
        let _session_elapsed = with_timeout(duration, async {
            loop {
                let sequence = (datagrams as u32).to_be_bytes();
                let publication = async {
                    if socket_payload_bytes.is_multiple_of(payload_bytes) {
                        socket
                            .send_to_with(payload_bytes, (server, server_port), |packet| {
                                // An exact divisor never requires smoltcp's
                                // metadata-bearing ring padding, so the FnOnce
                                // zero-copy callback cannot be consumed by a
                                // retryable post-callback `BufferFull` result.
                                packet[..sequence.len()].copy_from_slice(&sequence);
                                (payload_bytes, ())
                            })
                            .await
                    } else {
                        // `send_to` copies from a stable source and is therefore
                        // retryable when a variable-size payload needs ring
                        // padding. This scratch resides in static PSRAM.
                        packet[..sequence.len()].copy_from_slice(&sequence);
                        socket
                            .send_to(&packet[..payload_bytes], (server, server_port))
                            .await
                    }
                };
                match publication.await {
                    Ok(()) => {
                        bytes = bytes.saturating_add(payload_bytes as u64);
                        datagrams = datagrams.saturating_add(1);
                    }
                    Err(_) => send_errors = send_errors.saturating_add(1),
                }
                if let Some(rate_bps) = offered_rate_bps
                    && datagrams.is_multiple_of(u64::from(pacing_group_datagrams))
                {
                    // Enforce one byte-budget deadline per bounded socket-queue
                    // group. Keep small timer/executor lateness on the absolute
                    // schedule. A four-group token-bucket horizon prevents a
                    // genuine pause from becoming an unbounded line-rate burst.
                    let group_nanos = u64::from(pacing_group_datagrams)
                        .saturating_mul(u64::try_from(payload_bytes).unwrap_or(u64::MAX))
                        .saturating_mul(8_000_000_000)
                        .saturating_add(rate_bps - 1)
                        / rate_bps;
                    let group_duration = Duration::from_nanos(group_nanos);
                    next_send += group_duration;
                    let now = Instant::now();
                    if now < next_send {
                        Timer::at(next_send).await;
                    } else if now - next_send > group_duration * MAX_PACING_CATCH_UP_GROUPS {
                        next_send = now;
                    }
                }
            }
        })
        .await;
        let elapsed_us = started.elapsed().as_micros().max(1);
        let throughput_kbps = bytes
            .saturating_mul(8)
            .saturating_mul(1_000)
            .checked_div(elapsed_us)
            .unwrap_or(0);
        // UDP admission is not network-stack drainage. Prove that every
        // admitted datagram left the socket before reporting its count; a
        // fixed sleep raced the final 17 AP frames at a full 16-Mbit/s offer.
        // Timeout remains a failed transport observation rather than turning
        // a stopped radio into an unbounded HIL task.
        if with_timeout(config.drain, socket.flush()).await.is_err() {
            send_errors = send_errors.saturating_add(1);
        }
        // Socket drainage can leave the final driver publication in flight.
        // Keep that terminal MAC/BlockAck interval outside measured time.
        Timer::after(config.drain).await;
        let tx_vector = qualification_sample(QualificationRequester::UdpTx)
            .await
            .tx_vector;
        // This live link vector belongs to the associated-STA datapath. AP
        // rate/A-MPDU evidence is owned by its terminal role report instead.
        // A station session that explicitly required BlockAck must never
        // succeed without the associated-link evidence it requested.
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
        log_open_radio_task_poll_interval(
            task_poll_start,
            OPEN_RADIO_TASK_POLL_TELEMETRY,
            &TASK_POLLS,
        )
        .await;
        let evidence = TransportEvidence {
            rx_bytes: 0,
            tx_bytes: bytes,
            rx_units: 0,
            tx_units: datagrams,
            elapsed_micros: elapsed_us,
            transport_errors: send_errors,
        };
        let aggregate_evidence = aggregate.zip(tx_vector).map(|(aggregate, tx_vector)| {
            aggregate_tx_evidence(
                aggregate,
                tx_vector.bandwidth_mhz,
                u32::try_from(tx_vector.aggregate_rate_kbps).unwrap_or(u32::MAX),
            )
        });
        complete_open_radio_bidirectional_direction(
            config.session_source.results,
            session.session_id,
            OpenRadioBidirectionalDirection::Tx,
            evidence,
            aggregate_evidence.map(|(radio, _)| radio),
            aggregate_evidence.map(|(_, timing)| timing),
            None,
            send_errors == 0,
        )
        .await;
    }
}
