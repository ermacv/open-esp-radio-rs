//! Host sender and report writer for production RX-only qualification.

use std::{
    fs,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use open_esp_radio_hil_protocol::{
    Completion, Direction, FlowConfig, SessionConfig, SessionLinkRequirements, Transport,
};

use crate::{
    Result,
    bidirectional::{
        assess_rx_log, rx_order_markdown, rx_reorder_markdown, task_poll_markdown,
        udp_sequence_markdown, validate_exact_rx_delivery,
    },
    invalidate_previous_report,
    paced_udp::{Config as PacedUdpConfig, send as send_paced_udp},
    traffic_capture::{SerialCapture, await_udp_rx_ready},
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
    duration: Duration,
    payload: usize,
    serial: PathBuf,
    expected_rx_format: u8,
    phy: &'static str,
}

pub(crate) fn run(arguments: Vec<String>, root: &Path) -> Result<()> {
    if arguments
        .first()
        .is_some_and(|value| matches!(value.as_str(), "help" | "--help" | "-h"))
    {
        print_help();
        return Ok(());
    }
    let mut options = parse_options(&arguments)?;
    let output = root.join("target/hil/esp32s31/qualification/open-radio-rx");
    fs::create_dir_all(&output)?;
    invalidate_previous_report(&output)?;
    let capture = SerialCapture::start_with_reset(&options.serial);
    let discovered_address = match await_udp_rx_ready(
        &capture,
        options.address,
        options.port,
        DEVICE_READY_TIMEOUT,
    ) {
        Ok(address) => address,
        Err(error) => {
            let log = capture.finish();
            fs::write(output.join("uart.log"), &log)?;
            return Err(error);
        }
    };
    options.address = discovered_address.address;
    let session = if discovered_address.runtime_session {
        let duration_millis = u32::try_from(options.duration.as_millis())?;
        Some(capture.start_session(SessionConfig {
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
        })?)
    } else {
        None
    };
    let host = send_paced_udp(PacedUdpConfig {
        address: options.address,
        port: options.port,
        rate_bps: options.rate_bps,
        duration: options.duration,
        payload: options.payload,
    })?;
    let structured = if let Some(session) = session {
        let evidence = match capture.wait_for_session(
            session,
            options.duration.saturating_add(Duration::from_secs(10)),
        ) {
            Ok(evidence) => evidence,
            Err(error) => {
                let log = capture.finish();
                fs::write(output.join("uart.log"), &log)?;
                return Err(error);
            }
        };
        if let Err(error) = capture.acknowledge_session(session) {
            let log = capture.finish();
            fs::write(output.join("uart.log"), &log)?;
            return Err(error);
        }
        Some(evidence)
    } else {
        thread::sleep(Duration::from_secs(5));
        None
    };
    let log = capture.finish();
    fs::write(output.join("uart.log"), &log)?;
    let assessment = assess_rx_log(&log, options.expected_rx_format)?;
    let rx = assessment.rx;
    let minimum_bps = options.rate_bps.saturating_mul(9) / 10;
    let structured_failure = structured.and_then(|evidence| {
        let structured_throughput_kbps = evidence
            .transport
            .rx_bytes
            .saturating_mul(8)
            .saturating_mul(1_000)
            .checked_div(evidence.transport.elapsed_micros.max(1))
            .unwrap_or(0);
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
        } else if evidence.transport.rx_units != rx.received_datagrams {
            Some(format!(
                "typed/text RX datagram mismatch: {}/{}",
                evidence.transport.rx_units, rx.received_datagrams
            ))
        } else if evidence.transport.rx_bytes != expected_bytes {
            Some(format!(
                "typed RX byte count {} does not match {} full payload datagrams",
                evidence.transport.rx_bytes, evidence.transport.rx_units
            ))
        } else if evidence.transport.rx_units != host.datagrams
            || evidence.transport.rx_bytes != host.bytes
        {
            Some(format!(
                "host/target RX delivery mismatch: host={}/{} target={}/{}",
                host.bytes,
                host.datagrams,
                evidence.transport.rx_bytes,
                evidence.transport.rx_units
            ))
        } else if structured_throughput_kbps != rx.throughput_median_kbps {
            Some(format!(
                "typed/text RX throughput mismatch: {structured_throughput_kbps}/{} kbit/s",
                rx.throughput_median_kbps
            ))
        } else {
            None
        }
    });
    let delivery_failure =
        validate_exact_rx_delivery(host.datagrams, rx.received_datagrams, rx.sequence, rx.order)
            .err()
            .map(|error| error.to_string());
    let acceptance_failure = if host.throughput_bps() < minimum_bps {
        Some(String::from(
            "host failed to offer at least 90% of the requested RX rate",
        ))
    } else if rx.throughput_median_kbps < minimum_bps / 1_000 {
        Some(format!(
            "device RX {} kbit/s is below the acceptance floor",
            rx.throughput_median_kbps,
        ))
    } else {
        delivery_failure
    };
    let failure = assessment
        .failure
        .or(structured_failure)
        .or(acceptance_failure);
    let result = if failure.is_some() { "FAIL" } else { "PASS" };
    let failure_report = failure
        .as_ref()
        .map(|failure| format!("- Acceptance failure: `{failure}`\n"))
        .unwrap_or_default();
    let structured_report = structured
        .map(|evidence| {
            format!(
                "- Typed session evidence: `{}` bytes / `{}` datagrams / `{}` us; CRC32C `0x{:08x}`\n\
                 - Stack minimum free: CPU0 `{}/{}` bytes; CPU1 `{}/{}` bytes; required `{}%`\n",
                evidence.transport.rx_bytes,
                evidence.transport.rx_units,
                evidence.transport.elapsed_micros,
                evidence.finished.evidence_crc32c,
                evidence.stack.cpu0.free_bytes,
                evidence.stack.cpu0.capacity_bytes,
                evidence.stack.cpu1.free_bytes,
                evidence.stack.cpu1.capacity_bytes,
                evidence.stack.minimum_free_percent,
            )
        })
        .unwrap_or_else(|| String::from("- Typed session evidence: compatibility mode\n"));
    let pipeline = rx.pipeline;
    let irq = rx.irq;
    let average_service_us = pipeline.service_us as f64 / pipeline.admitted_frames.max(1) as f64;
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
    fs::write(
        output.join("report.md"),
        format!(
            "# Open-radio {} RX-only HIL\n\n\
             - Result: `{result}`\n\
             {failure_report}\
             - Device: `{}`\n\
             - Requested/actual host offer: `{:.3}` / `{:.3} Mbit/s`\n\
             - Host payload: `{}` bytes in `{}` datagrams\n\
             {structured_report}\
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
             ## RX pipeline\n\n\
             - DMA service calls/frontier/admitted: `{}` / `{}` / `{}`; max frontier/admitted: `{}` / `{}`\n\
             - Service-observed BUFFER_FULL increments/samples: `{}` / `{}`; last boot service/counter/frontier/admitted/pool/queue/service time: `{}` / `{}` / `{}` / `{}` / `{}` / `{}` / `{} us`\n\
             - Frontier service buckets 0 / 1 / 2-3 / 4-7 / 8-15 / 16-31 / 32+: `{}` / `{}` / `{}` / `{}` / `{}` / `{}` / `{}`\n\
             - RX IRQ posts/wake epochs/hard entries/coalesced/sampled services/clock-skew rejects: `{}` / `{}` / `{}` / `{}` / `{}` / `{}`; sampled IRQ-to-service: `{:.2} us` average, `{}` us boot maximum\n\
             - MAC entry causes spurious / RX-work-only / RX-mixed / TX-only / TX-mixed / auxiliary-or-unknown-only: `{}` / `{}` / `{}` / `{}` / `{}` / `{}`; classified `{}` entries; extra snapshots `{}`, loop saturations `{}`, auxiliary STATUS OR `0x{:08x}`, unknown STATUS OR `0x{:08x}`\n\
             - Staged bytes: `{}`; invalid empty/oversize units recycled: `{}` / `{}`; service: `{:.2} us/frame` average, `{}` us boot maximum\n\
             - Backpressured services: `{}`; pool/queue credit limited: `{}` / `{}`; maximum deferred frames: `{}`; minimum pool/queue credits: `{}` / `{}`\n\
             - Protocol frames/data: `{}` / `{}`; dispatch: `{:.2} us/frame` average, `{}` us boot maximum\n\
             - A-MSDU MPDUs/subframes: `{}` / `{}`; raw unit buckets <=1700 / 1701-3400 / >3400 bytes: `{}` / `{}` / `{}`; boot maximum: `{}` bytes\n\
             - Network publications/bytes: `{}` / `{}`; copy+publish: `{:.2} us/frame` average, `{}` us boot maximum\n\
             - Network-ready waits: `{}`; `{:.2} us` average, `{}` us boot maximum\n\n\
             {task_poll_report}\
             UART evidence is in [`uart.log`](uart.log).\n",
            options.phy.to_uppercase(),
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
            pipeline.dma_buffer_full_last_service,
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
    println!(
        "OPENRADIOHOST result=PASS mode={}-rx offered_kbps={} host_kbps={} \
         rx_median_kbps={} enqueued={} dropped=0 report={}",
        options.phy,
        options.rate_bps / 1_000,
        host.throughput_bps() / 1_000,
        rx.throughput_median_kbps,
        rx.enqueued,
        output.join("report.md").display(),
    );
    Ok(())
}

fn print_help() {
    println!(
        "cargo hil traffic rx <device-ipv4> [options]\n\
         \n\
         --rate <bps>       paced host-to-device rate (default 20M)\n\
         --seconds <5..300> traffic duration (default 12)\n\
         --payload <64..1472> UDP payload bytes (default 1200)\n\
         --port <port>      device UDP sink (default 4323)\n\
         --serial <path>    diagnostics device (default /dev/ttyACM0)\n\
         --phy <he20|ht20|ht40> expected RX vector (default he20)\n\n\
         Flash `cargo hil flash radio` and wait for DHCP first."
    );
}

fn parse_options(arguments: &[String]) -> Result<Options> {
    let address = arguments
        .first()
        .ok_or("missing ESP32-S31 IPv4 address")?
        .parse::<Ipv4Addr>()?;
    let mut options = Options {
        address,
        port: DEFAULT_PORT,
        rate_bps: DEFAULT_RATE_BPS,
        duration: DEFAULT_DURATION,
        payload: DEFAULT_PAYLOAD,
        serial: PathBuf::from("/dev/ttyACM0"),
        expected_rx_format: 4,
        phy: "he20",
    };
    let mut index = 1;
    while index < arguments.len() {
        let value = arguments
            .get(index + 1)
            .ok_or("RX option requires a value")?;
        match arguments[index].as_str() {
            "--rate" => options.rate_bps = parse_rate(value)?,
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
            "--serial" => options.serial = PathBuf::from(value),
            "--phy" => match value.as_str() {
                "he20" => {
                    options.expected_rx_format = 4;
                    options.phy = "he20";
                }
                "ht20" => {
                    options.expected_rx_format = 2;
                    options.phy = "ht20";
                }
                "ht40" => {
                    options.expected_rx_format = 2;
                    options.phy = "ht40";
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
        let options = parse_options(&[
            "192.168.178.141".into(),
            "--rate".into(),
            "40M".into(),
            "--phy".into(),
            "ht40".into(),
        ])
        .unwrap();
        assert_eq!(options.rate_bps, 40_000_000);
        assert_eq!(options.expected_rx_format, 2);
        let ht20 =
            parse_options(&["192.168.178.141".into(), "--phy".into(), "ht20".into()]).unwrap();
        assert_eq!(ht20.expected_rx_format, 2);
        assert_eq!(ht20.phy, "ht20");
    }
}
