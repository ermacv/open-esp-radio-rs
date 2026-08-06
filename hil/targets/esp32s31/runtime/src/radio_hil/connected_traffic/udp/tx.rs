#![forbid(unsafe_code)]

use embassy_net::{Ipv4Address, Stack, udp::UdpSocket};
use embassy_time::{Duration, Instant, Timer};
use open_esp_radio::{
    esp32s31::wifi::lmac::tx::TxPhyRate, wifi::ieee80211::station::StaAssociationPhy,
};
use open_esp_radio_hil_esp32s31_telemetry::aggregate_tx::AggregateTxCounters;
use open_esp_radio_hil_protocol::{
    Completion as HilCompletion, Direction as HilDirection, Event as HilEvent, ServiceInfo,
    Transport as HilTransport, TransportEvidence,
};

use super::UdpSocketBuffers;
use crate::{
    console::{complete_session, emergency_log, publish_event, receive_session_start},
    radio_hil::connected_traffic::{
        BidirectionalResultChannel, BidirectionalSessionChannel, OpenRadioBidirectionalDirection,
        complete_open_radio_bidirectional_direction, log_open_radio_ampdu_interval,
    },
};

#[derive(Clone, Copy)]
pub(in crate::radio_hil) enum UdpTxSessionSource {
    Standalone,
    Console,
    Bidirectional {
        sessions: &'static BidirectionalSessionChannel,
        results: &'static BidirectionalResultChannel,
    },
}

#[derive(Clone, Copy)]
pub(in crate::radio_hil) struct UdpTxBenchmarkConfig {
    pub source_port: u16,
    pub queue_depth: usize,
    pub payload_capacity: usize,
    pub default_target: [u8; 4],
    pub default_port: u16,
    pub default_duration: Duration,
    pub default_offered_rate_bps: Option<u64>,
    pub drain: Duration,
    pub code_address: usize,
    pub session_source: UdpTxSessionSource,
}

/// Device-to-host UDP load through Embassy and the open TX scheduler.
pub(in crate::radio_hil) async fn run_open_radio_udp_tx_benchmark<'a>(
    stack: Stack<'a>,
    association_phy: StaAssociationPhy,
    data_tx_rate: TxPhyRate,
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
    publish_event(
        0,
        0,
        HilEvent::ServiceReady(ServiceInfo {
            transport: HilTransport::Udp,
            direction: HilDirection::Tx,
            local_port: config.source_port,
            maximum_payload_bytes: config.payload_capacity as u16,
        }),
    );
    if !matches!(config.session_source, UdpTxSessionSource::Standalone) {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=PASS stage=udp-tx-ready \
             source_port={} queue={} payload_capacity={} \
             tx_mode=ampdu runtime_session=1 rate_code={:#04x} rate_kbps={}",
            config.source_port,
            config.queue_depth,
            config.payload_capacity,
            data_tx_rate.code(),
            data_tx_rate.nominal_kbps(),
        ));
    } else {
        let server = Ipv4Address::from_octets(config.default_target);
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=PASS stage=udp-tx-ready \
             target={server}:{} queue={} payload={} tx_mode=ampdu \
             offered_tx_kbps={:?} rate_code={:#04x} rate_kbps={}",
            config.default_port,
            config.queue_depth,
            config.payload_capacity,
            config.default_offered_rate_bps.map(|rate| rate / 1_000),
            data_tx_rate.code(),
            data_tx_rate.nominal_kbps(),
        ));
    }
    loop {
        let session = match config.session_source {
            UdpTxSessionSource::Standalone => None,
            UdpTxSessionSource::Console => Some(receive_session_start().await),
            UdpTxSessionSource::Bidirectional { sessions, .. } => Some(sessions.receive().await),
        };
        let (server, server_port, payload_bytes, duration, offered_rate_bps) =
            if let Some(session) = session {
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
                (
                    Ipv4Address::from_octets(peer.address),
                    peer.port,
                    usize::from(flow.payload_bytes),
                    Duration::from_millis(u64::from(duration_millis)),
                    flow.offered_rate_bps,
                )
            } else {
                (
                    Ipv4Address::from_octets(config.default_target),
                    config.default_port,
                    config.payload_capacity,
                    config.default_duration,
                    config.default_offered_rate_bps,
                )
            };
        if let Some(session) = session {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=udp-tx-session-start \
                 session={} target={server}:{server_port} payload={payload_bytes} \
                 duration_ms={} offered_bps={offered_rate_bps:?}",
                session.session_id,
                duration.as_millis(),
            ));
        }
        let started = Instant::now();
        let aggregate_start = (!matches!(
            config.session_source,
            UdpTxSessionSource::Bidirectional { .. }
        ))
        .then(|| aggregate_counters.snapshot());
        let mut next_send = started;
        let mut bytes = 0_u64;
        let mut datagrams = 0_u64;
        let mut send_errors = 0_u32;
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
            if let Some(rate_bps) = offered_rate_bps {
                // Pace absolute microsecond deadlines so a temporarily
                // blocking network queue does not produce a compensating
                // burst after it becomes writable.
                let interval_us = (payload_bytes as u64)
                    .saturating_mul(8_000_000)
                    .saturating_add(rate_bps - 1)
                    / rate_bps;
                next_send += Duration::from_micros(interval_us);
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
        if session.is_some() {
            Timer::after(config.drain).await;
        }
        emergency_log(format_args!(
            "OTX b={bytes} d={datagrams} u={elapsed_us} k={throughput_kbps} \
             e={send_errors} p={} w={} r={} g={} x={} l={} a={}",
            offered_rate_bps.unwrap_or(0) / 1_000,
            association_phy.bandwidth_mhz(),
            data_tx_rate.nominal_kbps(),
            match data_tx_rate {
                TxPhyRate::He(rate) => rate.guard_interval_and_ltf().encoding(),
                TxPhyRate::Legacy(_) | TxPhyRate::Ht(_) => u8::MAX,
            },
            match data_tx_rate {
                TxPhyRate::He(rate) => rate.is_dcm() as u8,
                TxPhyRate::Legacy(_) | TxPhyRate::Ht(_) => u8::MAX,
            },
            match data_tx_rate {
                TxPhyRate::He(rate) => rate.is_ldpc() as u8,
                TxPhyRate::Legacy(_) | TxPhyRate::Ht(_) => u8::MAX,
            },
            config.code_address,
        ));
        if let Some(aggregate_start) = aggregate_start {
            log_open_radio_ampdu_interval(aggregate_start, aggregate_counters);
        }
        if let Some(session) = session {
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
                UdpTxSessionSource::Console | UdpTxSessionSource::Standalone => {
                    complete_session(session.session_id, evidence, send_errors == 0).await;
                }
            }
        } else {
            Timer::after_secs(2).await;
        }
    }
}
