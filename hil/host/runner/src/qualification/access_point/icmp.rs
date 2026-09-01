//! ICMP workload execution and assessment.

use std::{net::Ipv4Addr, process::Command};

use super::report::TrafficReport;
use crate::{Result, qualification::scenario::Criteria};

pub(super) fn qualify_icmp(
    target: Ipv4Addr,
    bind_wifi_interface: bool,
    count: u16,
    interval_ms: u16,
    timeout_ms: u16,
    payload_bytes: u16,
    criteria: &Criteria,
) -> Result<TrafficReport> {
    let interval_seconds = format!("{:.3}", f64::from(interval_ms) / 1_000.0);
    let timeout_seconds = format!("{:.3}", f64::from(timeout_ms) / 1_000.0);
    let mut command = Command::new("ping");
    command.env("LC_ALL", "C");
    if bind_wifi_interface {
        command.args(["-I", "wlan0"]);
    }
    let output = command
        .arg("-c")
        .arg(count.to_string())
        .args(["-i", &interval_seconds, "-W", &timeout_seconds, "-s"])
        .arg(payload_bytes.to_string())
        .arg(target.to_string())
        .output()?;
    let stdout = String::from_utf8(output.stdout)?;
    let mut samples_micros = stdout
        .lines()
        .filter_map(ping_sample_micros)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    samples_micros.sort_unstable();
    let received = u16::try_from(samples_micros.len())?;
    let lost = count.saturating_sub(received);
    let allowed_lost = criteria.maximum_lost.unwrap_or(0);
    if u32::from(lost) > allowed_lost {
        return Err(format!(
            "AP ICMP lost {lost}/{count} packets (allowed {allowed_lost}); output: {}",
            stdout.trim()
        )
        .into());
    }
    if received == 0 {
        return Err(format!("AP client received no ICMP replies from {target}").into());
    }
    let p50_micros = percentile_micros(&samples_micros, 50);
    let p95_micros = percentile_micros(&samples_micros, 95);
    let p99_micros = percentile_micros(&samples_micros, 99);
    if let Some(maximum_ms) = criteria.maximum_p95_ms
        && p95_micros > u64::from(maximum_ms) * 1_000
    {
        return Err(format!(
            "AP ICMP p50={p50_micros} us p95={p95_micros} us p99={p99_micros} us; \
             p95 exceeds {maximum_ms} ms"
        )
        .into());
    }
    if !output.status.success() && lost == 0 {
        return Err(format!("ping failed despite complete replies: {}", stdout.trim()).into());
    }
    Ok(TrafficReport::Icmp {
        transmitted: count,
        received,
        lost,
        p50_micros,
        p95_micros,
        p99_micros,
    })
}

pub(super) fn percentile_micros(sorted_samples: &[u64], percentile: usize) -> u64 {
    let index = (sorted_samples.len() * percentile)
        .div_ceil(100)
        .saturating_sub(1);
    sorted_samples[index]
}

pub(super) fn ping_sample_micros(
    line: &str,
) -> Option<std::result::Result<u64, std::num::ParseFloatError>> {
    let suffix = line
        .split_once("time=")
        .or_else(|| line.split_once("time<"))?
        .1;
    let value = suffix.split_whitespace().next()?;
    Some(
        value
            .parse::<f64>()
            .map(|millis| (millis * 1_000.0).round() as u64),
    )
}
