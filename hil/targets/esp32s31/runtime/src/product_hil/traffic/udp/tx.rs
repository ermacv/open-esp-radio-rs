#![forbid(unsafe_code)]

use core::sync::atomic::{AtomicU8, Ordering};
use embassy_net::{Ipv4Address, Stack, udp::UdpSocket};
use embassy_time::{Duration, Instant, Timer, with_timeout};
#[cfg(feature = "core0-rx-coarse-telemetry")]
use open_esp_radio_embassy_net::TX_PERFORMANCE;
#[cfg(feature = "core0-rx-coarse-telemetry")]
use open_esp_radio_esp32s31_embassy_wifi::CORE0_PERFORMANCE;
#[cfg(any(
    feature = "core0-rx-cycle-telemetry",
    feature = "core0-rx-coarse-telemetry"
))]
use open_esp_radio_esp32s31_platform_pac::L1CachePerformanceCounters;
use open_esp_radio_hil_esp32s31_telemetry::aggregate_tx::AggregateTxCounters;
use open_esp_radio_hil_protocol::{
    Completion as HilCompletion, Direction as HilDirection, Event as HilEvent,
    FlowTransportEvidence, SESSION_FLOW_CAPACITY, ServiceInfo, SessionConfig, SessionReady,
    Transport as HilTransport, TransportEvidence,
};

use super::UdpSocketBuffers;
use crate::{
    console::{publish_event_reliably, runtime_log},
    product_hil::traffic::{
        BidirectionalResultChannel, BidirectionalSessionChannel, OpenRadioBidirectionalDirection,
        OpenRadioBidirectionalResult, aggregate_tx_evidence,
        complete_open_radio_bidirectional_direction, log_open_radio_ampdu_interval,
        log_open_radio_task_poll_interval, wait_session_link_requirements,
    },
    product_hil::{
        OPEN_RADIO_TASK_POLL_TELEMETRY, QualificationRequester, TASK_POLLS, qualification_sample,
    },
};

#[cfg(feature = "core0-rx-coarse-telemetry")]
use crate::product_hil::traffic::log_open_radio_core0_rx_coarse;

const MAX_PACING_CATCH_UP_GROUPS: u32 = 4;
const DEFAULT_MULTI_FLOW_BURST_DATAGRAMS: u8 = 1;
static MULTI_FLOW_BURST_DATAGRAMS: AtomicU8 =
    AtomicU8::new(DEFAULT_MULTI_FLOW_BURST_DATAGRAMS);

pub(in crate::product_hil) fn configure_multi_flow_burst_datagrams(datagrams: u8) {
    assert!(datagrams != 0, "multi-flow burst cannot be empty");
    MULTI_FLOW_BURST_DATAGRAMS.store(datagrams, Ordering::Release);
}

pub(in crate::product_hil) fn multi_flow_burst_datagrams() -> u8 {
    MULTI_FLOW_BURST_DATAGRAMS.load(Ordering::Acquire)
}

#[derive(Clone, Copy)]
pub(in crate::product_hil) struct UdpTxSessionSource {
    pub sessions: &'static BidirectionalSessionChannel,
    pub results: &'static BidirectionalResultChannel,
}

#[derive(Clone, Copy)]
pub(in crate::product_hil) struct UdpTxBenchmarkConfig {
    pub network_interface: open_esp_radio_hil_protocol::WifiNetworkInterface,
    pub source_port: u16,
    pub queue_depth: usize,
    pub payload_capacity: usize,
    /// Maximum application datagrams admitted before enforcing the next
    /// absolute offered-rate deadline. The composition root derives this from
    /// the active plus prepared-ahead A-MPDU arenas, not an arbitrary poll
    /// batch.
    pub pacing_group_datagrams: u8,
    /// Number of consecutive successful publications offered to one flow
    /// before rotating to the next ready flow. One preserves the ordinary
    /// datagram round-robin producer; BA-sized values isolate pre-DMA queue
    /// selection without changing driver or radio backing.
    pub multi_flow_burst_datagrams: u8,
    pub drain: Duration,
    pub code_address: usize,
    pub session_source: UdpTxSessionSource,
}

#[derive(Clone, Copy)]
struct MultiTxFlowState {
    flow_id: u8,
    server: Ipv4Address,
    server_port: u16,
    payload_bytes: usize,
    offered_rate_bps: Option<u64>,
    pacing_group_datagrams: u8,
    next_send: Instant,
    bytes: u64,
    datagrams: u64,
    errors: u32,
}

fn multi_tx_flow_states(
    session_config: SessionConfig,
    started: Instant,
    default_pacing_group_datagrams: u8,
) -> [Option<MultiTxFlowState>; SESSION_FLOW_CAPACITY] {
    session_config.flows.map(|flow| {
        flow.map(|flow| {
            let peer = flow
                .peer
                .expect("validated multi-flow TX session carries a peer");
            let target_tx = flow
                .target_tx
                .expect("validated multi-flow TX session carries a TX flow");
            MultiTxFlowState {
                flow_id: flow.flow_id,
                server: Ipv4Address::from_octets(peer.address),
                server_port: peer.port,
                payload_bytes: usize::from(target_tx.payload_bytes),
                offered_rate_bps: target_tx.offered_rate_bps,
                pacing_group_datagrams: target_tx
                    .pacing_group_datagrams
                    .unwrap_or(default_pacing_group_datagrams),
                next_send: started,
                bytes: 0,
                datagrams: 0,
                errors: 0,
            }
        })
    })
}

fn ready_multi_tx_flow(
    states: &[Option<MultiTxFlowState>; SESSION_FLOW_CAPACITY],
    cursor: usize,
    now: Instant,
) -> Option<usize> {
    (0..SESSION_FLOW_CAPACITY)
        .map(|offset| (cursor + offset) % SESSION_FLOW_CAPACITY)
        .find(|index| {
            states[*index]
                .is_some_and(|state| state.offered_rate_bps.is_none() || state.next_send <= now)
        })
}

fn earliest_multi_tx_deadline(
    states: &[Option<MultiTxFlowState>; SESSION_FLOW_CAPACITY],
) -> Option<Instant> {
    states
        .iter()
        .flatten()
        .filter(|state| state.offered_rate_bps.is_some())
        .map(|state| state.next_send)
        .min()
}

async fn transmit_multi_flow(
    socket: &mut UdpSocket<'_>,
    socket_payload_bytes: usize,
    packet: &mut [u8],
    session_config: SessionConfig,
    started: Instant,
    duration: Duration,
    pacing_group_datagrams: u8,
    multi_flow_burst_datagrams: u8,
) -> [Option<FlowTransportEvidence>; SESSION_FLOW_CAPACITY] {
    assert!(multi_flow_burst_datagrams != 0, "multi-flow burst cannot be empty");
    let mut states = multi_tx_flow_states(session_config, started, pacing_group_datagrams);
    let mut cursor = 0_usize;
    let mut burst_flow: Option<usize> = None;
    let mut burst_remaining = 0_u8;
    let _session_elapsed = with_timeout(duration, async {
        loop {
            let now = Instant::now();
            let continuing = burst_flow.filter(|index| {
                burst_remaining != 0
                    && states[*index]
                        .is_some_and(|state| state.offered_rate_bps.is_none() || state.next_send <= now)
            });
            let Some(index) = continuing.or_else(|| ready_multi_tx_flow(&states, cursor, now)) else {
                if let Some(deadline) = earliest_multi_tx_deadline(&states) {
                    Timer::at(deadline).await;
                }
                continue;
            };
            if continuing.is_none() {
                burst_flow = Some(index);
                burst_remaining = multi_flow_burst_datagrams;
            }
            let state = states[index].expect("selected multi-flow TX state remains active");
            let sequence = (state.datagrams as u32).to_be_bytes();
            let publication = if socket_payload_bytes.is_multiple_of(state.payload_bytes) {
                socket
                    .send_to_with(
                        state.payload_bytes,
                        (state.server, state.server_port),
                        |payload| {
                            payload[..sequence.len()].copy_from_slice(&sequence);
                            (state.payload_bytes, ())
                        },
                    )
                    .await
            } else {
                packet[..sequence.len()].copy_from_slice(&sequence);
                socket
                    .send_to(
                        &packet[..state.payload_bytes],
                        (state.server, state.server_port),
                    )
                    .await
            };

            let state = states[index]
                .as_mut()
                .expect("selected multi-flow TX state remains active");
            match publication {
                Ok(()) => {
                    state.bytes = state.bytes.saturating_add(state.payload_bytes as u64);
                    state.datagrams = state.datagrams.saturating_add(1);
                    if let Some(rate_bps) = state.offered_rate_bps
                        && state
                            .datagrams
                            .is_multiple_of(u64::from(state.pacing_group_datagrams))
                    {
                        let group_nanos = u64::from(state.pacing_group_datagrams)
                            .saturating_mul(u64::try_from(state.payload_bytes).unwrap_or(u64::MAX))
                            .saturating_mul(8_000_000_000)
                            .saturating_add(rate_bps - 1)
                            / rate_bps;
                        let group_duration = Duration::from_nanos(group_nanos);
                        state.next_send += group_duration;
                        let now = Instant::now();
                        if now > state.next_send
                            && now - state.next_send > group_duration * MAX_PACING_CATCH_UP_GROUPS
                        {
                            state.next_send = now;
                        }
                    }
                    burst_remaining = burst_remaining.saturating_sub(1);
                }
                Err(_) => {
                    state.errors = state.errors.saturating_add(1);
                    burst_remaining = 0;
                }
            }
            if burst_remaining == 0 {
                burst_flow = None;
                cursor = (index + 1) % SESSION_FLOW_CAPACITY;
            }
        }
    })
    .await;
    let elapsed_micros = started.elapsed().as_micros().max(1);
    states.map(|state| {
        state.map(|state| FlowTransportEvidence {
            flow_id: state.flow_id,
            rx_bytes: 0,
            tx_bytes: state.bytes,
            rx_units: 0,
            tx_units: state.datagrams,
            elapsed_micros,
            transport_errors: state.errors,
        })
    })
}

/// Device-to-host UDP load through Embassy and the open TX scheduler.
pub(in crate::product_hil) async fn run_open_radio_udp_tx_benchmark<'a>(
    stack: Stack<'a>,
    buffers: UdpSocketBuffers<'a>,
    packet: &'a mut [u8],
    config: UdpTxBenchmarkConfig,
    aggregate_counters: &AggregateTxCounters,
    #[cfg(any(
        feature = "core0-rx-cycle-telemetry",
        feature = "core0-rx-coarse-telemetry"
    ))]
    l1_cache: &'static L1CachePerformanceCounters,
) -> ! {
    // Complete the connected-data-path settle before advertising readiness.
    // `Start` must mean the benchmark task can consume its session without a
    // hidden post-acceptance delay.
    Timer::after_secs(1).await;
    publish_event_reliably(
        0,
        0,
        HilEvent::ServiceReady(ServiceInfo {
            network_interface: config.network_interface,
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
    // xarxa then never needs a metadata-bearing wrap-padding record and its
    // zero-copy FnOnce callback cannot be consumed by a retryable post-callback
    // `BufferFull`. This configures only the HIL load generator, not the radio.
    let first_session = config.session_source.sessions.receive().await;
    let first_payload_bytes = usize::from(
        first_session
            .config
            .primary_flow()
            .expect("validated TX session carries a primary flow")
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
        wait_session_link_requirements(session.config.link_requirements, config.network_interface)
            .await;
        publish_event_reliably(
            session.session_id,
            0,
            HilEvent::SessionReady(SessionReady {
                direction: HilDirection::Tx,
                tx_block_ack_tid: session.config.link_requirements.tx_block_ack_tid,
            }),
        )
        .await;
        let session_flow = session
            .config
            .primary_flow()
            .expect("validated TX session carries a primary flow");
        let peer = session_flow
            .peer
            .expect("validated TX session carries a peer");
        let flow = session_flow
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
        // a negotiated radio capability. The bounded socket/DATAPATH queues own
        // admission while interval telemetry independently proves whether TX
        // actually used BlockAck/A-MPDU. Waiting after every datagram makes
        // the Embassy timer itself the throughput ceiling at high offered
        // rates.
        let pacing_group_datagrams = flow
            .pacing_group_datagrams
            .unwrap_or(config.pacing_group_datagrams);
        runtime_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=PASS stage=udp-tx-session-start \
             session={} target={server}:{server_port} payload={payload_bytes} \
             duration_ms={} offered_bps={offered_rate_bps:?}",
            session.session_id,
            duration.as_millis(),
        ));
        #[cfg(any(
            feature = "core0-rx-cycle-telemetry",
            feature = "core0-rx-coarse-telemetry"
        ))]
        if crate::product_hil::L1_CACHE_COUNTERS_ENABLED.load(Ordering::Relaxed) {
            l1_cache.enable();
        }
        #[cfg(any(
            feature = "core0-rx-cycle-telemetry",
            feature = "core0-rx-coarse-telemetry"
        ))]
        let cache_start = l1_cache.snapshot();
        let started = Instant::now();
        let task_poll_start = TASK_POLLS.snapshot();
        #[cfg(feature = "core0-rx-coarse-telemetry")]
        let network_scheduler_start =
            crate::product_hil::traffic::network_scheduler::snapshot();
        #[cfg(feature = "core0-rx-coarse-telemetry")]
        let core0_performance_start = CORE0_PERFORMANCE.snapshot();
        #[cfg(feature = "core0-rx-coarse-telemetry")]
        let core1_tx_performance_start = TX_PERFORMANCE.snapshot();
        #[cfg(feature = "core0-rx-coarse-telemetry")]
        let egress_control_start = (
            open_esp_radio_esp32s31_embassy_wifi::station_egress_control_snapshot(),
            open_esp_radio_esp32s31_embassy_wifi::access_point_egress_control_snapshot(),
        );
        #[cfg(feature = "core0-rx-coarse-telemetry")]
        let egress_policy_start =
            open_esp_radio_esp32s31_embassy_wifi::egress_policy_shadow_snapshot();
        #[cfg(feature = "tx-architecture-probes")]
        let tx_core1_materializer_start =
            open_esp_radio_embassy_net::TX_CORE1_MATERIALIZER_COUNTERS.snapshot();
        // TX owns A-MPDU evidence because its post-measurement drain proves
        // that the last publication reached a terminal BlockAck outcome.
        // The RX sibling can finish on the host terminal datagram while a
        // target aggregate is still in flight, so sampling there can tear one
        // logical publication across independent diagnostic atomics.
        let aggregate_start = if crate::product_hil::OPEN_RADIO_DRIVER_OBSERVATION
            && session.config.direction != HilDirection::Rx
        {
            aggregate_counters.begin_interval();
            Some(aggregate_counters.snapshot())
        } else {
            None
        };
        let (bytes, datagrams, mut send_errors, mut flow_evidence) =
            if session.config.active_flow_count() == 1 {
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
                                        // An exact divisor never requires xarxa's
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
                            } else if now - next_send > group_duration * MAX_PACING_CATCH_UP_GROUPS
                            {
                                next_send = now;
                            }
                        }
                    }
                })
                .await;
                let elapsed_micros = started.elapsed().as_micros().max(1);
                let transport = TransportEvidence {
                    rx_bytes: 0,
                    tx_bytes: bytes,
                    rx_units: 0,
                    tx_units: datagrams,
                    elapsed_micros,
                    transport_errors: send_errors,
                };
                (
                    bytes,
                    datagrams,
                    send_errors,
                    [
                        Some(FlowTransportEvidence::from_session_total(
                            session_flow.flow_id,
                            transport,
                        )),
                        None,
                    ],
                )
            } else {
                let flows = transmit_multi_flow(
                    &mut socket,
                    socket_payload_bytes,
                    packet,
                    session.config,
                    started,
                    duration,
                    pacing_group_datagrams,
                    config.multi_flow_burst_datagrams,
                )
                .await;
                let aggregate = TransportEvidence::from_flows(flows);
                (
                    aggregate.tx_bytes,
                    aggregate.tx_units,
                    aggregate.transport_errors,
                    flows,
                )
            };
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
            if let Some(primary) = flow_evidence.iter_mut().flatten().next() {
                primary.transport_errors = primary.transport_errors.saturating_add(1);
            }
        }
        // Socket drainage can leave the final driver publication in flight.
        // Keep that terminal MAC/BlockAck interval outside measured time.
        Timer::after(config.drain).await;
        let tx_vector = qualification_sample(QualificationRequester::UdpTx)
            .await
            .tx_vector;
        #[cfg(any(
            feature = "core0-rx-cycle-telemetry",
            feature = "core0-rx-coarse-telemetry"
        ))]
        let cache_interval = l1_cache.snapshot().wrapping_delta_since(cache_start);
        #[cfg(feature = "core0-rx-coarse-telemetry")]
        let network_scheduler =
            crate::product_hil::traffic::network_scheduler::interval_since(
                network_scheduler_start,
            );
        // This live link vector belongs to the associated-STA datapath. AP
        // rate/A-MPDU evidence is owned by its terminal role report instead.
        // A station session that explicitly required BlockAck must never
        // succeed without the associated-link evidence it requested.
        assert!(
            !crate::product_hil::OPEN_RADIO_DRIVER_OBSERVATION
                || session.config.link_requirements.tx_block_ack_tid.is_none()
                || tx_vector.is_some(),
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
        #[cfg(feature = "core0-rx-coarse-telemetry")]
        {
            runtime_log(format_args!(
                "ONSCHED polls={} ingress_calls={} ingress_packets={} egress_passes={} \
                 egress_tokens={} blocked={} ingress_budget={} egress_budget={} \
                 start_ingress={} start_egress={} exit_drained={} exit_work={} exit_credit={}",
                network_scheduler.polls,
                network_scheduler.ingress_calls,
                network_scheduler.ingress_packets,
                network_scheduler.egress_passes,
                network_scheduler.egress_tx_tokens,
                network_scheduler.egress_blocked,
                network_scheduler.ingress_budget_exhausted,
                network_scheduler.egress_budget_exhausted,
                network_scheduler.started_with_ingress,
                network_scheduler.started_with_egress,
                network_scheduler.exit_drained,
                network_scheduler.exit_work_budget,
                network_scheduler.exit_egress_credit,
            ));
        }
        #[cfg(feature = "core0-rx-coarse-telemetry")]
        log_open_radio_core0_rx_coarse(core0_performance_start).await;
        #[cfg(feature = "core0-rx-coarse-telemetry")]
        let egress_policy = Some(
            crate::product_hil::traffic::log_open_radio_core1_tx_phases(
                core1_tx_performance_start,
                egress_control_start,
                egress_policy_start,
            )
            .await,
        );
        #[cfg(not(feature = "core0-rx-coarse-telemetry"))]
        let egress_policy = None;
        #[cfg(any(
            feature = "core0-rx-cycle-telemetry",
            feature = "core0-rx-coarse-telemetry"
        ))]
        crate::product_hil::traffic::reporting::log_open_radio_l1_cache_interval(cache_interval)
            .await;
        #[cfg(feature = "tx-architecture-probes")]
        crate::product_hil::traffic::reporting::log_open_radio_tx_core1_materializer(
            tx_core1_materializer_start,
        )
        .await;
        let aggregate_evidence = aggregate
            .filter(|aggregate| aggregate.rate_selections != 0)
            .map(aggregate_tx_evidence);
        complete_open_radio_bidirectional_direction(
            config.session_source.results,
            OpenRadioBidirectionalResult::new(
                session.session_id,
                OpenRadioBidirectionalDirection::Tx,
                flow_evidence,
                aggregate_evidence.map(|(radio, _)| radio),
                aggregate_evidence.map(|(_, timing)| timing),
                None,
                egress_policy,
                send_errors == 0,
            ),
        )
        .await;
    }
}
