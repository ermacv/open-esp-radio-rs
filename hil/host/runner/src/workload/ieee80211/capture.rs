//! Finite normalized 802.11 capture exported through the typed HIL protocol.

use crate::execution::context::Context;
mod assembly;
mod pcapng;

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use open_esp_radio_hil_protocol::WifiMonitorCaptureRequest;

use crate::{
    Result,
    fixture::controlled_ap::ControlledAp,
    scenario::PhyExpectation,
    session::{MonitorCaptureEvidence, SerialCapture},
    workload::ieee80211::control::{scan, start_station, stop_station},
};

pub(crate) struct Config {
    pub(crate) output: PathBuf,
    pub(crate) timeout: Duration,
    pub(crate) duration: Duration,
    pub(crate) channel: Option<u8>,
    pub(crate) snapshot_length: u16,
}

pub(crate) fn run(
    options: Config,
    artifact_dir: &Path,
    context: &Context<'_>,
    phy: PhyExpectation,
) -> Result<()> {
    let options = options.validate()?;
    let _access_point = Some(ControlledAp::start(
        &context.lab.station,
        &context.lab.station_fixture,
        phy,
    )?);
    fs::create_dir_all(artifact_dir)?;
    if let Some(parent) = options.output.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }

    let result = context.with_capture(artifact_dir, |serial| qualify(serial, context, &options))?;
    pcapng::write_capture(
        &options.output,
        &result.assembly.packets,
        result.host_anchor_micros,
    )?;
    if result.assembly.incomplete_frames != 0
        || result.capture.summary.exported_frames as usize != result.assembly.packets.len()
    {
        return Err(format!(
            "monitor export was incomplete: target_exported={} reconstructed={} incomplete={}",
            result.capture.summary.exported_frames,
            result.assembly.packets.len(),
            result.assembly.incomplete_frames,
        )
        .into());
    }
    eprintln!("wifi_capture=PASS");
    eprintln!("pcapng={}", options.output.display());
    eprintln!(
        "generation={} frames={} bytes={} full_drops={} oversized_drops={} discarded={}",
        result.capture.summary.generation,
        result.capture.summary.exported_frames,
        result.capture.summary.captured_bytes,
        result.capture.summary.full_drops,
        result.capture.summary.oversized_drops,
        result.capture.summary.discarded_frames,
    );
    eprintln!("uart_log={}", artifact_dir.join("uart.log").display());
    Ok(())
}

struct CaptureResult {
    capture: MonitorCaptureEvidence,
    assembly: assembly::AssemblyReport,
    host_anchor_micros: u64,
}

fn qualify(
    serial: &SerialCapture,
    context: &Context<'_>,
    options: &Config,
) -> Result<CaptureResult> {
    let capabilities = serial.prepare_station(context, options.timeout)?;
    if !capabilities.features.wifi_monitor_capture {
        return Err("firmware does not advertise typed monitor capture".into());
    }
    stop_station(serial, options.timeout)?;
    let channel = match options.channel {
        Some(channel) => channel,
        None => {
            let evidence = scan(serial, options.timeout)?;
            if !(1..=13).contains(&evidence.configured_ssid_channel) {
                start_station(serial, context, options.timeout)?;
                return Err("scan did not provide a capture channel".into());
            }
            evidence.configured_ssid_channel
        }
    };
    let duration_millis = u32::try_from(options.duration.as_millis())
        .map_err(|_| "capture duration does not fit the HIL protocol")?;
    let host_anchor_micros = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_micros()
        .try_into()
        .map_err(|_| "host wall clock does not fit PCAPNG timestamp")?;
    let handle = serial.request_monitor_capture(WifiMonitorCaptureRequest {
        channel,
        snapshot_length: options.snapshot_length,
        duration_millis,
    })?;
    let capture = serial.wait_monitor_capture(handle, options.timeout + options.duration);
    let restart = start_station(serial, context, options.timeout);
    let capture = match (capture, restart) {
        (Ok(capture), Ok(())) => capture,
        (Err(capture), Ok(())) => return Err(capture),
        (Ok(_), Err(restart)) => {
            return Err(format!("station restart after capture failed: {restart}").into());
        }
        (Err(capture), Err(restart)) => {
            return Err(format!(
                "monitor capture failed ({capture}); station restart also failed ({restart})"
            )
            .into());
        }
    };
    let stack = serial.query_stack_usage(options.timeout)?;
    eprintln!(
        "stack_stage=monitor-capture cpu0_free={}/{} cpu0_required={} cpu1_free={}/{} cpu1_required={}",
        stack.cpu0.free_bytes,
        stack.cpu0.capacity_bytes,
        stack.cpu0.minimum_free_bytes,
        stack.cpu1.free_bytes,
        stack.cpu1.capacity_bytes,
        stack.cpu1.minimum_free_bytes,
    );
    validate_summary(&capture, channel, options.duration)?;
    let assembly = assembly::assemble(capture.chunks.clone())?;
    Ok(CaptureResult {
        capture,
        assembly,
        host_anchor_micros,
    })
}

fn validate_summary(
    capture: &MonitorCaptureEvidence,
    channel: u8,
    requested_duration: Duration,
) -> Result<()> {
    let summary = capture.summary;
    if summary.channel != channel
        || summary.generation_mismatches != 0
        || summary.channel_mismatches != 0
    {
        return Err(format!("monitor returned inconsistent capture evidence: {summary:?}").into());
    }
    if summary.exported_frames == 0 || summary.captured_bytes == 0 {
        return Err("finite monitor capture exported no frames".into());
    }
    let requested_micros = requested_duration.as_micros().min(u128::from(u64::MAX)) as u64;
    let minimum = requested_micros * 95 / 100;
    let maximum = requested_micros * 110 / 100;
    if summary.elapsed_micros < minimum || summary.elapsed_micros > maximum {
        return Err(format!(
            "finite monitor timing is outside the qualified range: requested_us={requested_micros} observed_us={} range={minimum}..={maximum}",
            summary.elapsed_micros,
        )
        .into());
    }
    if summary.exported_frames != summary.captured_frames {
        return Err(format!(
            "target exported {} frames after consuming {}",
            summary.exported_frames, summary.captured_frames
        )
        .into());
    }
    if summary.published_frames
        != summary
            .exported_frames
            .saturating_add(summary.discarded_frames)
    {
        return Err(format!(
            "capture ownership accounting mismatch: published={} exported={} discarded={}",
            summary.published_frames, summary.exported_frames, summary.discarded_frames,
        )
        .into());
    }
    Ok(())
}

impl Config {
    fn validate(self) -> Result<Self> {
        if !(Duration::from_secs(10)..=Duration::from_secs(180)).contains(&self.timeout) {
            return Err("capture timeout must be in 10..=180 seconds".into());
        }
        if !(Duration::from_secs(1)..=Duration::from_secs(30)).contains(&self.duration) {
            return Err("capture duration must be in 1..=30 seconds".into());
        }
        if self
            .channel
            .is_some_and(|channel| !(1..=13).contains(&channel))
        {
            return Err("capture channel must be in 1..=13".into());
        }
        if self.snapshot_length > 2304 {
            return Err("snapshot length must be at most 2304".into());
        }
        if self.output.as_os_str().is_empty() {
            return Err("capture output is required".into());
        }
        Ok(self)
    }
}

#[cfg(test)]
mod tests;
