#![forbid(unsafe_code)]

use embassy_futures::yield_now;
use embassy_net::{Ipv4Address, Stack, udp::UdpSocket};
use embassy_time::{Duration, Instant, Timer};
use open_esp_radio_hil_esp32s31_telemetry::aggregate_tx::AggregateTxCounters;
use open_esp_radio_hil_protocol::{
    Completion as HilCompletion, Direction as HilDirection, Event as HilEvent, ServiceInfo,
    SessionReady, Transport as HilTransport, TransportEvidence,
};

use super::UdpSocketBuffers;
use crate::{
    console::{complete_session, emergency_log, publish_event_reliably, receive_session_start},
    product_hil::traffic::{
        BidirectionalResultChannel, BidirectionalSessionChannel, OpenRadioBidirectionalDirection,
        complete_open_radio_bidirectional_direction, log_open_radio_ampdu_interval,
        wait_session_link_requirements,
    },
};

#[derive(Clone, Copy)]
pub(in crate::product_hil) enum UdpTxSessionSource {
    Console,
    Bidirectional {
        sessions: &'static BidirectionalSessionChannel,
        results: &'static BidirectionalResultChannel,
    },
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
        emergency_log(format_args!(
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
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=udp-tx-ready \
         source_port={} queue={} payload_capacity={} tx_mode=ampdu runtime_session=1",
        config.source_port, config.queue_depth, config.payload_capacity,
    ));
    loop {
        let session = match config.session_source {
            UdpTxSessionSource::Console => receive_session_start().await,
            UdpTxSessionSource::Bidirectional { sessions, .. } => sessions.receive().await,
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
        emergency_log(format_args!(
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
        let aggregate_start = match config.session_source {
            UdpTxSessionSource::Bidirectional { .. }
                if session.config.direction == HilDirection::Rx =>
            {
                None
            }
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
                && datagrams.is_multiple_of(u64::from(config.pacing_group_datagrams))
            {
                // Enforce one byte-budget deadline per active+standby A-MPDU
                // pipeline capacity. Sleeping after every datagram prevented
                // the network queue from ever presenting an aggregate-sized
                // workload and halved throughput. A missed deadline is reset
                // after this bounded physical credit, so a scheduler stall
                // cannot trigger unbounded catch-up bursts.
                let group_nanos = u64::from(config.pacing_group_datagrams)
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
        emergency_log(format_args!(
            "OTX b={bytes} d={datagrams} u={elapsed_us} k={throughput_kbps} \
             e={send_errors} p={} pg={} code={}",
            offered_rate_bps.unwrap_or(0) / 1_000,
            config.pacing_group_datagrams,
            config.code_address,
        ));
        if let Some(aggregate_start) = aggregate_start {
            log_open_radio_ampdu_interval(aggregate_start, aggregate_counters);
        }
        let evidence = TransportEvidence {
            rx_bytes: 0,
            tx_bytes: bytes,
            rx_units: 0,
            tx_units: datagrams,
            elapsed_micros: elapsed_us,
            transport_errors: send_errors,
        };
        match config.session_source {
            UdpTxSessionSource::Bidirectional { results, .. } => {
                complete_open_radio_bidirectional_direction(
                    results,
                    session.session_id,
                    OpenRadioBidirectionalDirection::Tx,
                    evidence,
                    send_errors == 0,
                )
                .await;
            }
            UdpTxSessionSource::Console => {
                complete_session(session.session_id, evidence, send_errors == 0).await;
            }
        }
    }
}
