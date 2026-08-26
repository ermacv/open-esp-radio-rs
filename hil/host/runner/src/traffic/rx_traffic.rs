//! Host sender and report writer for production RX-only qualification.

use std::{
    env, fs,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    time::Duration,
};

use open_esp_radio_hil_protocol::{
    Completion, Direction, FlowConfig, SessionConfig, SessionLinkRequirements, Transport,
};

use crate::{
    Result, evidence,
    evidence::traffic_capture::{SerialCapture, await_udp_rx_ready},
    qualification::scenario::PhyExpectation,
    traffic::bidirectional::{
        RxQualification, assess_rx_log, rx_order_markdown, rx_reorder_markdown, task_poll_markdown,
        udp_sequence_markdown,
    },
    traffic::host_network::BenchmarkIpv4Route,
    traffic::paced_udp::{Config as PacedUdpConfig, send as send_paced_udp},
    transport::lab_config::{LabConfig, StationFixtureConfig},
    transport::local_air_monitor::{LocalAirMonitorCapture, LocalAirMonitorEvidence},
    transport::openwrt_tx_monitor::{OpenWrtTxMonitorCapture, OpenWrtTxMonitorEvidence},
    transport::station_fixture::RxCapture,
};

const DEFAULT_PORT: u16 = 4_323;
const DEFAULT_RATE_BPS: u64 = 20_000_000;
const DEFAULT_DURATION: Duration = Duration::from_secs(12);
const DEFAULT_PAYLOAD: usize = 1_200;
const DEVICE_READY_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, Eq, PartialEq)]
struct Options {
    address: Ipv4Addr,
    port: u16,
    rate_bps: u64,
    minimum_rate_bps: Option<u64>,
    duration: Duration,
    payload: usize,
    serial: PathBuf,
    expected_rx_format: u8,
    phy: PhyExpectation,
    maximum_idle_channel_utilization_255: Option<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EvidencePolicy {
    pub(crate) require_exact_delivery: bool,
    pub(crate) require_no_beacon_loss: bool,
    pub(crate) require_driver_observation: bool,
    pub(crate) capture_openwrt_tx_monitor: bool,
    pub(crate) capture_independent_laptop_monitor: bool,
}

pub(crate) fn run(
    arguments: Vec<String>,
    output: &Path,
    lab: &LabConfig,
    evidence_policy: EvidencePolicy,
) -> Result<()> {
    let mut options = parse_options(&arguments, lab)?;
    let require_exact_delivery = evidence_policy.require_exact_delivery;
    fs::create_dir_all(output)?;
    let capture = SerialCapture::start_with_reset(&options.serial);
    let discovered_address = match await_udp_rx_ready(
        &capture,
        lab,
        options.address,
        options.port,
        DEVICE_READY_TIMEOUT,
    ) {
        Ok(address) => address,
        Err(error) => {
            capture.finish_to(output)?;
            return Err(error);
        }
    };
    options.address = discovered_address.address;
    let host_route = match BenchmarkIpv4Route::discover(options.address, &lab.station_fixture) {
        Ok(route) => route,
        Err(error) => {
            capture.finish_to(output)?;
            return Err(error);
        }
    };
    let same_boot_probes = env::var("OPEN_RADIO_RX_SAME_BOOT_PROBES")
        .ok()
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(0);
    for probe in 1..=same_boot_probes {
        let fixture_capture = RxCapture::start(
            &lab.station_fixture,
            options.address,
            options.port,
            options.duration,
            options.phy,
            options.maximum_idle_channel_utilization_255,
        )?;
        let duration_millis = u32::try_from(options.duration.as_millis())?;
        let session = capture.start_session(SessionConfig {
            network_interface: open_esp_radio_hil_protocol::WifiNetworkInterface::Station,
            transport: Transport::Udp,
            direction: Direction::Rx,
            completion: Completion::DurationMillis(duration_millis),
            peer: None,
            target_rx: Some(FlowConfig {
                payload_bytes: u16::try_from(options.payload)?,
                offered_rate_bps: Some(options.rate_bps),
            }),
            target_tx: None,
            link_requirements: SessionLinkRequirements::NONE,
        })?;
        let host = send_paced_udp(PacedUdpConfig {
            address: options.address,
            port: options.port,
            rate_bps: options.rate_bps,
            duration: options.duration,
            payload: options.payload,
        })?;
        host_route.verify_socket_source(host.source)?;
        let structured = capture.wait_for_session(
            session,
            options.duration.saturating_add(Duration::from_secs(10)),
        )?;
        capture.acknowledge_session(session)?;
        let fixture = fixture_capture.map(RxCapture::finish).transpose()?;
        let throughput_kbps = structured
            .transport
            .rx_bytes
            .saturating_mul(8)
            .saturating_mul(1_000)
            .checked_div(structured.transport.elapsed_micros.max(1))
            .unwrap_or(0);
        eprintln!(
            "OPENRADIOHOST same_boot_probe={probe}/{same_boot_probes} rx_kbps={throughput_kbps} rx_units={} elapsed_us={}",
            structured.transport.rx_units, structured.transport.elapsed_micros,
        );
        if let Some(fixture) = fixture {
            eprintln!(
                "OPENRADIOHOST same_boot_probe={probe}/{same_boot_probes} {}",
                fixture.markdown().trim(),
            );
        }
    }
    let fixture_capture = RxCapture::start(
        &lab.station_fixture,
        options.address,
        options.port,
        options.duration,
        options.phy,
        options.maximum_idle_channel_utilization_255,
    )?;
    let tx_monitor_capture = if evidence_policy.capture_openwrt_tx_monitor {
        let StationFixtureConfig::OpenWrt(config) = &lab.station_fixture else {
            return Err("OpenWrt TX-monitor evidence requires an OpenWrt station fixture".into());
        };
        Some(OpenWrtTxMonitorCapture::start(
            config,
            options.address,
            options.port,
            options.duration,
            output,
        )?)
    } else {
        None
    };
    let independent_air_capture = if evidence_policy.capture_independent_laptop_monitor {
        let StationFixtureConfig::OpenWrt(config) = &lab.station_fixture else {
            return Err("independent laptop evidence requires an OpenWrt station fixture".into());
        };
        Some(LocalAirMonitorCapture::start(
            config,
            options.address,
            options.duration,
            output,
        )?)
    } else {
        None
    };
    let duration_millis = u32::try_from(options.duration.as_millis())?;
    let session = capture.start_session(SessionConfig {
        network_interface: open_esp_radio_hil_protocol::WifiNetworkInterface::Station,
        transport: Transport::Udp,
        direction: Direction::Rx,
        completion: Completion::DurationMillis(duration_millis),
        peer: None,
        target_rx: Some(FlowConfig {
            payload_bytes: u16::try_from(options.payload)?,
            offered_rate_bps: Some(options.rate_bps),
        }),
        target_tx: None,
        link_requirements: SessionLinkRequirements::NONE,
    })?;
    let host = send_paced_udp(PacedUdpConfig {
        address: options.address,
        port: options.port,
        rate_bps: options.rate_bps,
        duration: options.duration,
        payload: options.payload,
    })?;
    host_route.verify_socket_source(host.source)?;
    host_route.record(output, options.address, host.source)?;
    let structured = match capture.wait_for_session(
        session,
        options.duration.saturating_add(Duration::from_secs(10)),
    ) {
        Ok(evidence) => evidence,
        Err(error) => {
            capture.finish_to(output)?;
            return Err(error);
        }
    };
    if let Err(error) = capture.acknowledge_session(session) {
        capture.finish_to(output)?;
        return Err(error);
    }
    let fixture = fixture_capture.map(RxCapture::finish).transpose()?;
    let tx_monitor_rx = tx_monitor_capture
        .map(|capture| capture.finish(host.datagrams))
        .transpose()?;
    let independent_air_rx = independent_air_capture
        .map(LocalAirMonitorCapture::finish)
        .transpose()?;
    let beacon_loss = evidence_policy
        .require_no_beacon_loss
        .then(|| capture.require_no_beacon_loss());
    let log = capture.finish_to(output)?;
    if let Some(result) = beacon_loss {
        result?;
    }
    let minimum_bps = options
        .minimum_rate_bps
        .unwrap_or_else(|| options.rate_bps.saturating_mul(9) / 10);
    let typed_rx_kbps = structured
        .transport
        .rx_bytes
        .saturating_mul(8)
        .saturating_mul(1_000)
        .checked_div(structured.transport.elapsed_micros.max(1))
        .unwrap_or(0);
    if !evidence_policy.require_driver_observation {
        if structured.radio.is_some()
            || structured.tx_timing.is_some()
            || structured.rx_delivery.is_some()
            || structured.network_scheduler.is_some()
        {
            return Err("performance image published driver-internal evidence".into());
        }
        let expected_bytes = structured
            .transport
            .rx_units
            .saturating_mul(options.payload as u64);
        let link_failure = if options.phy == PhyExpectation::Ht40 {
            fixture.as_ref().map_or_else(
                || {
                    Some(String::from(
                        "HT40 performance requires a managed fixture link snapshot",
                    ))
                },
                |fixture| {
                    fixture
                        .require_ht40_downlink()
                        .err()
                        .map(|error| error.to_string())
                },
            )
        } else {
            None
        };
        let transport_failure = if !structured.finished.summary.passed {
            Some(String::from(
                "target did not complete the typed RX session normally",
            ))
        } else if structured.transport.tx_bytes != 0 || structured.transport.tx_units != 0 {
            Some(String::from(
                "RX-only session reported unexpected transmitted traffic",
            ))
        } else if structured.transport.transport_errors != 0 {
            Some(format!(
                "typed RX session reported {} transport errors",
                structured.transport.transport_errors
            ))
        } else if structured.transport.rx_bytes != expected_bytes {
            Some(format!(
                "typed RX byte count {} does not match {} full payload datagrams",
                structured.transport.rx_bytes, structured.transport.rx_units
            ))
        } else if host.throughput_bps() < minimum_bps {
            Some(String::from(
                "host failed to offer at least 90% of the requested RX rate",
            ))
        } else if typed_rx_kbps < minimum_bps / 1_000 {
            Some(format!(
                "device RX {typed_rx_kbps} kbit/s is below the acceptance floor"
            ))
        } else {
            None
        };
        let failure = link_failure.or(transport_failure);
        let result = if failure.is_some() { "FAIL" } else { "PASS" };
        let failure_report = failure
            .as_ref()
            .map(|failure| format!("- Acceptance failure: `{failure}`\n"))
            .unwrap_or_default();
        let fixture_report = fixture.as_ref().map_or_else(
            || String::from("- AP-side link vector: `external AP; not observed`\n"),
            |fixture| fixture.markdown(),
        );
        fs::write(
            output.join("report.md"),
            format!(
                "# Open-radio {} RX performance HIL\n\n\
                 - Result: `{result}`\n\
                 {failure_report}\
                 - Evidence boundary: `transport, external host offer, stack watermark; driver observation not collected`\n\
                 {fixture_report}\
                 - Device: `{}`\n\
                 - Requested/actual host offer: `{:.3}` / `{:.3} Mbit/s`\n\
                 - Host payload: `{}` bytes in `{}` datagrams\n\
                 - Target transport: `{}` bytes / `{}` datagrams / `{}` us (`{:.3} Mbit/s`)\n\
                 - Stack minimum free: CPU0 `{}/{}` bytes (required `{}`); CPU1 `{}/{}` bytes (required `{}`)\n\
                 - Evidence CRC32C: `0x{:08x}`\n\
                 - Host pacing maximum lateness/catch-up/deadline resets: `{} us` / `{}` datagrams / `{}`\n\n\
                 UART evidence is in [`uart.log`](uart.log).\n",
                options.phy.id().to_uppercase(),
                options.address,
                options.rate_bps as f64 / 1_000_000.0,
                host.throughput_bps() as f64 / 1_000_000.0,
                host.bytes,
                host.datagrams,
                structured.transport.rx_bytes,
                structured.transport.rx_units,
                structured.transport.elapsed_micros,
                typed_rx_kbps as f64 / 1_000.0,
                structured.stack.cpu0.free_bytes,
                structured.stack.cpu0.capacity_bytes,
                structured.stack.cpu0.minimum_free_bytes,
                structured.stack.cpu1.free_bytes,
                structured.stack.cpu1.capacity_bytes,
                structured.stack.cpu1.minimum_free_bytes,
                structured.finished.evidence_crc32c,
                host.maximum_lateness_us(),
                host.maximum_catch_up_datagrams,
                host.deadline_resets,
            ),
        )?;
        if let Some(failure) = failure {
            return Err(failure.into());
        }
        eprintln!(
            "OPENRADIOHOST result=PASS mode={}-rx-performance offered_kbps={} host_kbps={} rx_kbps={typed_rx_kbps} report={}",
            options.phy.id(),
            options.rate_bps / 1_000,
            host.throughput_bps() / 1_000,
            output.join("report.md").display(),
        );
        return Ok(());
    }
    let raw_rx_radio = structured
        .radio
        .and_then(|evidence| evidence.rx)
        .ok_or("session did not publish typed RX radio evidence")?;
    let typed_radio_failure = if require_exact_delivery {
        structured.require_rx_radio(options.expected_rx_format, host.datagrams)
    } else {
        structured.require_rx_radio_health(options.expected_rx_format)
    }
    .err()
    .map(|error| error.to_string());
    // Text telemetry enriches the report when present. Typed transport/radio
    // evidence alone decides qualification and therefore remains authoritative
    // even if the bounded diagnostic stream is truncated.
    let text_assessment = assess_rx_log(&log, options.expected_rx_format).ok();
    if let Some(failure) = text_assessment
        .as_ref()
        .and_then(|assessment| assessment.failure.as_deref())
    {
        eprintln!("diagnostic_text_warning={failure}");
    }
    let rx = text_assessment
        .map(|assessment| assessment.rx)
        .unwrap_or_else(|| RxQualification::from_typed(structured.transport, raw_rx_radio));
    let structured_failure = {
        let evidence = structured;
        let expected_bytes = evidence
            .transport
            .rx_units
            .saturating_mul(options.payload as u64);
        if !evidence.finished.summary.passed {
            Some(String::from(
                "target did not complete the typed RX session normally",
            ))
        } else if evidence.transport.tx_bytes != 0 || evidence.transport.tx_units != 0 {
            Some(String::from(
                "RX-only session reported unexpected transmitted traffic",
            ))
        } else if evidence.transport.transport_errors != 0 {
            Some(format!(
                "typed RX session reported {} transport errors",
                evidence.transport.transport_errors
            ))
        } else if evidence.transport.rx_bytes != expected_bytes {
            Some(format!(
                "typed RX byte count {} does not match {} full payload datagrams",
                evidence.transport.rx_bytes, evidence.transport.rx_units
            ))
        } else if require_exact_delivery
            && (evidence.transport.rx_units != host.datagrams
                || evidence.transport.rx_bytes != host.bytes)
        {
            Some(format!(
                "host/target RX delivery mismatch: host={}/{} target={}/{}",
                host.bytes,
                host.datagrams,
                evidence.transport.rx_bytes,
                evidence.transport.rx_units
            ))
        } else {
            None
        }
    };
    let typed_delivery_failure = if require_exact_delivery {
        structured.rx_delivery.and_then(|delivery| {
            let assessment = evidence::rx_delivery::assess(host.datagrams, delivery);
            (!assessment.exact()).then_some(format!(
                "typed RX delivery frontier is {}",
                assessment.frontier()
            ))
        })
    } else {
        None
    };
    let acceptance_failure = if host.throughput_bps() < minimum_bps {
        Some(String::from(
            "host failed to offer at least 90% of the requested RX rate",
        ))
    } else if typed_rx_kbps < minimum_bps / 1_000 {
        Some(format!(
            "device RX {} kbit/s is below the acceptance floor",
            typed_rx_kbps,
        ))
    } else {
        None
    };
    let fixture_failure = if require_exact_delivery {
        fixture.as_ref().and_then(|fixture| {
            let expected = host.datagrams.saturating_add(1);
            (fixture.wireless_packets() != expected).then_some({
                format!(
                    "host/AP Wi-Fi egress mismatch: expected={} observed={} packets",
                    expected,
                    fixture.wireless_packets()
                )
            })
        })
    } else {
        None
    };
    let failure = fixture_failure
        .or(typed_delivery_failure)
        .or(typed_radio_failure)
        .or(structured_failure)
        .or(acceptance_failure);
    let result = if failure.is_some() { "FAIL" } else { "PASS" };
    let failure_report = failure
        .as_ref()
        .map(|failure| format!("- Acceptance failure: `{failure}`\n"))
        .unwrap_or_default();
    let structured_report = format!(
        "- Typed session evidence: `{}` bytes / `{}` datagrams / `{}` us; CRC32C `0x{:08x}`\n\
                 - Stack minimum free: CPU0 `{}/{}` bytes (required `{}`); CPU1 `{}/{}` bytes (required `{}`)\n",
        structured.transport.rx_bytes,
        structured.transport.rx_units,
        structured.transport.elapsed_micros,
        structured.finished.evidence_crc32c,
        structured.stack.cpu0.free_bytes,
        structured.stack.cpu0.capacity_bytes,
        structured.stack.cpu0.minimum_free_bytes,
        structured.stack.cpu1.free_bytes,
        structured.stack.cpu1.capacity_bytes,
        structured.stack.cpu1.minimum_free_bytes,
    );
    let fixture_report = fixture.as_ref().map_or_else(
        || String::from("- AP-side evidence: `external AP; not observed`\n"),
        |fixture| fixture.markdown(),
    );
    let air_report = rx_air_evidence_markdown(tx_monitor_rx.as_ref(), independent_air_rx.as_ref());
    let pipeline = rx.pipeline;
    let irq = rx.irq;
    let average_service_us = pipeline.service_us as f64 / pipeline.admitted_frames.max(1) as f64;
    let average_reload_us = pipeline.reload_us as f64 / pipeline.reload_transactions.max(1) as f64;
    let average_dispatch_us = pipeline.dispatch_us as f64 / pipeline.protocol_frames.max(1) as f64;
    let average_publish_us =
        pipeline.network_publish_us as f64 / pipeline.network_publications.max(1) as f64;
    let average_wait_us =
        pipeline.network_ready_wait_us as f64 / pipeline.network_ready_waits.max(1) as f64;
    let average_irq_service_us =
        pipeline.rx_irq_to_service_us as f64 / pipeline.rx_irq_service_samples.max(1) as f64;
    let task_poll_report = task_poll_markdown(rx.task_polls);
    let udp_sequence_report = udp_sequence_markdown(rx.sequence, host.datagrams);
    let rx_order_report = rx_order_markdown(rx.order);
    let rx_reorder_report = rx_reorder_markdown(rx.reorder);
    let typed_delivery_report = structured
        .rx_delivery
        .map(|evidence| evidence::rx_delivery::markdown(host.datagrams, evidence))
        .unwrap_or_else(|| {
            String::from(
                "## Typed RX delivery frontier\n\nNot collected in this image. Use the explicit RX-delivery profile.\n\n",
            )
        });
    fs::write(
        output.join("report.md"),
        format!(
            "# Open-radio {} RX-only HIL\n\n\
             - Result: `{result}`\n\
             {failure_report}\
             - Delivery contract: `{}`\n\
             - Device: `{}`\n\
             - Requested/actual host offer: `{:.3}` / `{:.3} Mbit/s`\n\
             - Host payload: `{}` bytes in `{}` datagrams\n\
             {structured_report}\
             {fixture_report}\
             {air_report}\
             - Host pacing maximum lateness/catch-up/deadline resets: `{} us` / `{}` datagrams / `{}`\n\
             - Device RX median: `{:.3} Mbit/s` across `{}` samples; received UDP datagrams: `{}`\n\
             - Enqueued/software-dropped frames: `{}` / `{}`\n\
             - Sampled HE-SU MCS0..11 frame histogram: `{:?}`; other sampled PHY frames: `{}`\n\
             - Benchmark UDP datagrams marked S-MPDU / not S-MPDU / unavailable provenance: `{}` / `{}` / `{}`\n\
             - Connected beacons marked S-MPDU / not S-MPDU / unavailable provenance: `{}` / `{}` / `{}`\n\
             - Benchmark UDP datagrams marked A-MPDU / not A-MPDU / unavailable provenance: `{}` / `{}` / `{}`\n\
             - A-MPDU provenance hardware true/false, protocol true/false: `{}` / `{}`, `{}` / `{}`\n\
             - Hardware BUFFER_FULL/FIFO_OVERFLOW: `{}` / `{}`\n\n\
             {udp_sequence_report}\
             {rx_order_report}\
             {rx_reorder_report}\
             {typed_delivery_report}\
             ## RX pipeline\n\n\
             - DMA service calls/frontier/admitted: `{}` / `{}` / `{}`; max frontier/admitted: `{}` / `{}`\n\
             - Service-observed BUFFER_FULL increments/samples: `{}` / `{}`; between/during: `{}` / `{}` increments across `{}` / `{}` services; last boot service/phase/counter/frontier/admitted/pool/queue/service time: `{}` / `{}` / `{}` / `{}` / `{}` / `{}` / `{}` / `{} us`\n\
             - Frontier service buckets 0 / 1 / 2-3 / 4-7 / 8-15 / 16-31 / 32+: `{}` / `{}` / `{}` / `{}` / `{}` / `{}` / `{}`\n\
             - RX IRQ posts/wake epochs/hard entries/coalesced/sampled services/clock-skew rejects: `{}` / `{}` / `{}` / `{}` / `{}` / `{}`; sampled IRQ-to-service: `{:.2} us` average, `{}` us boot maximum\n\
             - MAC entry causes spurious / RX-work-only / RX-mixed / TX-only / TX-mixed / auxiliary-or-unknown-only: `{}` / `{}` / `{}` / `{}` / `{}` / `{}`; classified `{}` entries; extra snapshots `{}`, loop saturations `{}`, auxiliary STATUS OR `0x{:08x}`, unknown STATUS OR `0x{:08x}`\n\
             - Staged bytes: `{}`; invalid empty/oversize units recycled: `{}` / `{}`; service: `{:.2} us/frame` average, `{}` us boot maximum\n\
             - Safe reload transactions: `{}`; `{:.2} us` average, `{}` us boot maximum; `{}` us total\n\
             - Backpressured services: `{}`; pool/queue credit limited: `{}` / `{}`; maximum deferred frames: `{}`; minimum pool/queue credits: `{}` / `{}`\n\
             - Protocol frames/data: `{}` / `{}`; dispatch: `{:.2} us/frame` average, `{}` us boot maximum\n\
             - A-MSDU MPDUs/subframes: `{}` / `{}`; raw unit buckets <=1700 / 1701-3400 / >3400 bytes: `{}` / `{}` / `{}`; boot maximum: `{}` bytes\n\
             - Network publications/bytes: `{}` / `{}`; copy+publish: `{:.2} us/frame` average, `{}` us boot maximum\n\
             - Network-ready waits: `{}`; `{:.2} us` average, `{}` us boot maximum\n\n\
             {task_poll_report}\
             UART evidence is in [`uart.log`](uart.log).\n",
            options.phy.id().to_uppercase(),
            if require_exact_delivery {
                "exact"
            } else {
                "performance-health"
            },
            options.address,
            options.rate_bps as f64 / 1_000_000.0,
            host.throughput_bps() as f64 / 1_000_000.0,
            host.bytes,
            host.datagrams,
            host.maximum_lateness_us(),
            host.maximum_catch_up_datagrams,
            host.deadline_resets,
            rx.throughput_median_kbps as f64 / 1_000.0,
            rx.sample_count,
            rx.received_datagrams,
            rx.enqueued,
            rx.dropped,
            rx.he_mcs_histogram,
            rx.other_phy_frames,
            rx.s_mpdu.s_mpdu_datagrams,
            rx.s_mpdu.not_s_mpdu_datagrams,
            rx.s_mpdu.unavailable_datagrams,
            rx.s_mpdu.s_mpdu_beacons,
            rx.s_mpdu.not_s_mpdu_beacons,
            rx.s_mpdu.unavailable_beacons,
            rx.ampdu.ampdu_datagrams,
            rx.ampdu.not_ampdu_datagrams,
            rx.ampdu.unavailable_datagrams,
            rx.ampdu.hardware_ampdu_datagrams,
            rx.ampdu.hardware_not_ampdu_datagrams,
            rx.ampdu.protocol_ampdu_datagrams,
            rx.ampdu.protocol_not_ampdu_datagrams,
            rx.buffer_full,
            rx.fifo_overflow,
            pipeline.service_calls,
            pipeline.frontier_frames,
            pipeline.admitted_frames,
            pipeline.maximum_frontier,
            pipeline.maximum_admitted,
            pipeline.dma_buffer_full_increments,
            pipeline.dma_buffer_full_service_samples,
            pipeline.dma_buffer_full_between_services,
            pipeline.dma_buffer_full_during_services,
            pipeline.dma_buffer_full_between_service_samples,
            pipeline.dma_buffer_full_during_service_samples,
            pipeline.dma_buffer_full_last_service,
            pipeline.dma_buffer_full_last_phase,
            pipeline.dma_buffer_full_last_counter,
            pipeline.dma_buffer_full_last_frontier,
            pipeline.dma_buffer_full_last_admitted,
            pipeline.dma_buffer_full_last_pool_credits,
            pipeline.dma_buffer_full_last_queue_credits,
            pipeline.dma_buffer_full_last_service_us,
            pipeline.frontier_zero_services,
            pipeline.frontier_one_services,
            pipeline.frontier_two_three_services,
            pipeline.frontier_four_seven_services,
            pipeline.frontier_eight_fifteen_services,
            pipeline.frontier_sixteen_thirty_one_services,
            pipeline.frontier_thirty_two_plus_services,
            pipeline.rx_irq_posts,
            pipeline.rx_irq_epochs,
            pipeline.mac_irq_entries,
            pipeline.rx_irq_coalesced_posts,
            pipeline.rx_irq_service_samples,
            pipeline.rx_irq_clock_skew_samples,
            average_irq_service_us,
            pipeline.rx_irq_to_service_max_us,
            irq.spurious_entries,
            irq.rx_only_entries,
            irq.rx_mixed_entries,
            irq.tx_only_entries,
            irq.tx_mixed_entries,
            irq.other_only_entries,
            irq.classified_entries(),
            irq.extra_nonzero_snapshots,
            irq.saturated_entries,
            irq.auxiliary_status_or,
            irq.unknown_status_or,
            pipeline.staged_bytes,
            pipeline.stage_empty_discards,
            pipeline.stage_too_long_discards,
            average_service_us,
            pipeline.service_max_us,
            pipeline.reload_transactions,
            average_reload_us,
            pipeline.reload_max_us,
            pipeline.reload_us,
            pipeline.backpressured_services,
            pipeline.pool_credit_limited_services,
            pipeline.queue_credit_limited_services,
            pipeline.maximum_deferred_frames,
            pipeline.minimum_backpressured_pool_credits,
            pipeline.minimum_backpressured_queue_credits,
            pipeline.protocol_frames,
            pipeline.protocol_data_frames,
            average_dispatch_us,
            pipeline.dispatch_max_us,
            pipeline.protocol_amsdu_mpdus,
            pipeline.protocol_amsdu_subframes,
            pipeline.protocol_units_le_1700,
            pipeline.protocol_units_1701_3400,
            pipeline.protocol_units_over_3400,
            pipeline.protocol_unit_max_bytes,
            pipeline.network_publications,
            pipeline.network_published_bytes,
            average_publish_us,
            pipeline.network_publish_max_us,
            pipeline.network_ready_waits,
            average_wait_us,
            pipeline.network_ready_wait_max_us,
        ),
    )?;
    if let Some(failure) = failure {
        return Err(failure.into());
    }
    eprintln!(
        "OPENRADIOHOST result=PASS mode={}-rx offered_kbps={} host_kbps={} \
         rx_median_kbps={} enqueued={} dropped=0 report={}",
        options.phy.id(),
        options.rate_bps / 1_000,
        host.throughput_bps() / 1_000,
        typed_rx_kbps,
        rx.enqueued,
        output.join("report.md").display(),
    );
    Ok(())
}

fn rx_air_evidence_markdown(
    tx_monitor: Option<&OpenWrtTxMonitorEvidence>,
    independent: Option<&LocalAirMonitorEvidence>,
) -> String {
    let ap = tx_monitor.map_or_else(
        || String::from("- OpenWrt TX monitor: `not collected`\n"),
        |evidence| {
            format!(
                "- OpenWrt TX monitor frames/kernel drops: `{}` / `{}`; UDP unique/duplicates/unrecovered: `{}` / `{}` / `{}`; MAC retry publications: `{}`\n",
                evidence.captured_frames,
                evidence.kernel_dropped,
                evidence.unique_units,
                evidence.duplicates,
                evidence.unrecovered,
                evidence.mac_retry_publications,
            )
        },
    );
    let observer = independent.map_or_else(
        || String::from("- Independent air observer: `not collected`\n"),
        |evidence| {
            format!(
                "- Independent air observer frames/kernel drops: `{}` / `{}`; logical decoded data MPDUs/retry attempts/missing metadata: `{}` / `{}` / `{}`; BlockAck full/partial/unique MPDUs/backward starts: `{}` / `{}` / `{}` / `{}`\n",
                evidence.captured_frames,
                evidence.kernel_dropped,
                evidence.logical_data_units,
                evidence.retry_attempts,
                evidence.missing_mac_metadata,
                evidence.full_block_ack_frames,
                evidence.partial_block_ack_frames,
                evidence.unique_block_acked_mpdus,
                evidence.backward_block_ack_starts,
            )
        },
    );
    format!("{ap}{observer}")
}

fn parse_options(arguments: &[String], lab: &LabConfig) -> Result<Options> {
    let mut options = Options {
        address: Ipv4Addr::UNSPECIFIED,
        port: DEFAULT_PORT,
        rate_bps: DEFAULT_RATE_BPS,
        minimum_rate_bps: None,
        duration: DEFAULT_DURATION,
        payload: DEFAULT_PAYLOAD,
        serial: lab.device.serial.clone(),
        expected_rx_format: 4,
        phy: PhyExpectation::He20,
        maximum_idle_channel_utilization_255: None,
    };
    let mut index = 0;
    while index < arguments.len() {
        let value = arguments
            .get(index + 1)
            .ok_or("RX option requires a value")?;
        match arguments[index].as_str() {
            "--rate" => options.rate_bps = parse_rate(value)?,
            "--floor" => options.minimum_rate_bps = Some(parse_rate(value)?),
            "--seconds" => {
                let seconds = value.parse::<u64>()?;
                if !(5..=300).contains(&seconds) {
                    return Err("--seconds must be in 5..=300".into());
                }
                options.duration = Duration::from_secs(seconds);
            }
            "--payload" => {
                options.payload = value.parse::<usize>()?;
                if !(64..=1_472).contains(&options.payload) {
                    return Err("--payload must be in 64..=1472".into());
                }
            }
            "--port" => options.port = value.parse::<u16>()?,
            "--max-idle-channel-utilization-255" => {
                let maximum = value.parse::<u8>()?;
                if maximum == 0 {
                    return Err("--max-idle-channel-utilization-255 must be nonzero".into());
                }
                options.maximum_idle_channel_utilization_255 = Some(maximum);
            }
            "--phy" => match value.as_str() {
                "he20" => {
                    options.expected_rx_format = 4;
                    options.phy = PhyExpectation::He20;
                }
                "ht20" => {
                    options.expected_rx_format = 2;
                    options.phy = PhyExpectation::Ht20;
                }
                "ht40" => {
                    options.expected_rx_format = 2;
                    options.phy = PhyExpectation::Ht40;
                }
                _ => return Err("--phy must be he20, ht20 or ht40".into()),
            },
            other => return Err(format!("unknown RX option `{other}`").into()),
        }
        index += 2;
    }
    if options.port == 0 {
        return Err("--port must be nonzero".into());
    }
    if options
        .minimum_rate_bps
        .is_some_and(|floor| floor > options.rate_bps)
    {
        return Err("--floor cannot exceed --rate".into());
    }
    Ok(options)
}

fn parse_rate(value: &str) -> Result<u64> {
    let (digits, multiplier) = match value.as_bytes().last().copied() {
        Some(b'k' | b'K') => (&value[..value.len() - 1], 1_000_u64),
        Some(b'm' | b'M') => (&value[..value.len() - 1], 1_000_000_u64),
        Some(b'g' | b'G') => (&value[..value.len() - 1], 1_000_000_000_u64),
        _ => (value, 1),
    };
    let rate = digits
        .parse::<u64>()?
        .checked_mul(multiplier)
        .ok_or("rate overflow")?;
    if !(100_000..=500_000_000).contains(&rate) {
        return Err("--rate must be in 100K..=500M".into());
    }
    Ok(rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rx_options_and_rates() {
        assert_eq!(parse_rate("20M").unwrap(), 20_000_000);
        let lab = test_lab_config();
        let options = parse_options(
            &["--rate".into(), "40M".into(), "--phy".into(), "ht40".into()],
            &lab,
        )
        .unwrap();
        assert_eq!(options.rate_bps, 40_000_000);
        assert_eq!(options.expected_rx_format, 2);
        let guarded = parse_options(
            &["--max-idle-channel-utilization-255".into(), "64".into()],
            &lab,
        )
        .unwrap();
        assert_eq!(guarded.maximum_idle_channel_utilization_255, Some(64));
        assert!(
            parse_options(
                &["--max-idle-channel-utilization-255".into(), "0".into(),],
                &lab,
            )
            .is_err()
        );
        let ht20 = parse_options(&["--phy".into(), "ht20".into()], &lab).unwrap();
        assert_eq!(ht20.expected_rx_format, 2);
        assert_eq!(ht20.phy, PhyExpectation::Ht20);
    }

    fn test_lab_config() -> LabConfig {
        LabConfig::for_test()
    }
}
