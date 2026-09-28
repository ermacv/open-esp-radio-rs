//! Active scanning of a Linux scannable advertiser.
//!
//! The Linux adapter advertises a fresh marker as manufacturer data and a
//! fresh name, which BlueZ carries only in the scan response. A passive ESP
//! scan must report the advertisement and no scan response; an active scan
//! must also report the scan response with the name, which the adapter sends
//! only in answer to the ESP's `SCAN_REQ`.
use super::hci;
use crate::{
    Result,
    fixture::bluetooth::{advertiser::Advertiser, att},
};
use hil_core::{context::Context, session::SerialCapture};
use oer_hil_protocol::{BluetoothHciRequest, BluetoothHciResponse};
use std::{
    path::Path,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const ADVERTISING_REPORT: u8 = 0x02;
const SCAN_RSP: u8 = 0x04;
/// 60 ms interval and 30 ms window in 0.625 ms units.
const INTERVAL: u16 = 96;
const WINDOW: u16 = 48;
const LISTEN: Duration = Duration::from_secs(4);

pub fn run(output: &Path, context: &Context<'_>) -> Result<()> {
    let adapter = context
        .lab
        .bluetooth_adapter
        .ok_or("missing Bluetooth adapter")?;
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.subsec_nanos();
    let name = format!("oer-pc-{nonce:08x}");
    let marker = nonce.to_le_bytes();
    let mut owner = att::Owner::acquire(adapter, output)?;
    let result = context.with_capture(output, |capture| {
        let mut report = serde_json::Map::new();
        report.insert("scan_response_name".into(), name.clone().into());
        let probe = Advertiser::start(adapter, &name, &marker).and_then(|mut advertiser| {
            let result = exercise(capture, &name, &marker, &mut report);
            result.and(advertiser.stop())
        });
        // DIAGNOSTIC: drain the role's scan trace before Reset.
        if let Ok(status) = capture.trace_control(oer_hil_protocol::TraceControl::Status) {
            let entries = capture.trace_entries(status.entries).unwrap_or_default();
            let _ = hil_core::durable::atomic_json(
                &output.join("trace.json"),
                &serde_json::json!({"status": status, "entries": entries}),
            );
        }
        let cleanup = oer_process::cleanup(|| hci::command(capture, hci::RESET, &[]).map(|_| ()));
        report.insert("schema".into(), 1.into());
        report.insert("passed".into(), (probe.is_ok() && cleanup.is_ok()).into());
        report.insert(
            "error".into(),
            probe.as_ref().err().map(ToString::to_string).into(),
        );
        report.insert(
            "cleanup_error".into(),
            cleanup.as_ref().err().map(ToString::to_string).into(),
        );
        hil_core::durable::atomic_json(
            &output.join("active-scan.json"),
            &serde_json::Value::Object(report),
        )?;
        probe.and(cleanup)
    });
    let restore = oer_process::cleanup(|| owner.restore());
    result.and(restore)
}

/// What one scan pass heard from the Linux advertiser.
#[derive(Default)]
struct Heard {
    /// Every LE Advertising Report, from any advertiser.
    reports: u32,
    /// Reports by advertising event kind, from any advertiser.
    kinds: [u32; 5],
    /// Other events and a few advertisers, for diagnosis.
    other_events: u32,
    samples: Vec<String>,
    registers: Vec<String>,
    advertiser: Option<[u8; 7]>,
    advertisements: u32,
    scan_responses: u32,
    named_scan_responses: u32,
}

fn exercise(
    capture: &SerialCapture,
    name: &str,
    marker: &[u8],
    report: &mut serde_json::Map<String, serde_json::Value>,
) -> Result<()> {
    hci::require(capture)?;
    hci::command(capture, hci::RESET, &[])?;
    hci::command(capture, hci::SET_EVENT_MASK, &hci::EVENT_MASK_WITH_LE_META)?;
    let passive = pass(capture, false, name, marker)?;
    report.insert("passive".into(), passive.json());
    let active = pass(capture, true, name, marker)?;
    report.insert("active".into(), active.json());
    if passive.advertisements == 0 {
        return Err("the passive scan heard no advertisement from the adapter".into());
    }
    if passive.scan_responses != 0 {
        return Err("the passive scan reported a scan response".into());
    }
    if active.named_scan_responses == 0 {
        return Err("the active scan reported no scan response with the adapter's name".into());
    }
    Ok(())
}

impl Heard {
    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "reports": self.reports,
            "kinds": self.kinds,
            "other_events": self.other_events,
            "samples": self.samples,
            "registers": self.registers,
            "advertiser": self.advertiser.map(|address| format!("{address:02x?}")),
            "advertisements": self.advertisements,
            "scan_responses": self.scan_responses,
            "named_scan_responses": self.named_scan_responses,
        })
    }
}

fn pass(capture: &SerialCapture, active: bool, name: &str, marker: &[u8]) -> Result<Heard> {
    let mut parameters = [0; 7];
    parameters[0] = u8::from(active);
    parameters[1..3].copy_from_slice(&INTERVAL.to_le_bytes());
    parameters[3..5].copy_from_slice(&WINDOW.to_le_bytes());
    hci::command(capture, hci::LE_SET_SCAN_PARAMETERS, &parameters)?;
    hci::command(capture, hci::LE_SET_SCAN_ENABLE, &[1, 0])?;
    let mut heard = Heard::default();
    let deadline = Instant::now() + LISTEN;
    while Instant::now() < deadline {
        let BluetoothHciResponse::Event { packet, .. } =
            capture.bluetooth_hci(BluetoothHciRequest::NextEvent { wait_ms: 200 })?
        else {
            continue;
        };
        // Code, length, subevent, one report: type, address kind and
        // address, data length, data, RSSI.
        if packet.first() == Some(&0xff) {
            // DIAGNOSTIC: the target's register dump.
            heard.registers.push(format!("{:02x?}", &packet[2..]));
            continue;
        }
        if packet.len() < 13 || packet[0] != 0x3e || packet[2] != ADVERTISING_REPORT {
            heard.other_events += 1;
            continue;
        }
        heard.reports += 1;
        if let Some(count) = heard.kinds.get_mut(usize::from(packet[4])) {
            *count += 1;
        }
        if heard.samples.len() < 6 {
            heard
                .samples
                .push(format!("{:02x?}", &packet[4..packet.len().min(24)]));
        }
        let kind = packet[4];
        let mut address = [0; 7];
        address.copy_from_slice(&packet[5..12]);
        let length = usize::from(packet[12]);
        let Some(data) = packet.get(13..13 + length) else {
            continue;
        };
        let contains = |needle: &[u8]| data.windows(needle.len()).any(|window| window == needle);
        if kind != SCAN_RSP && contains(marker) {
            heard.advertiser = Some(address);
            heard.advertisements += 1;
        } else if kind == SCAN_RSP && heard.advertiser == Some(address) {
            heard.scan_responses += 1;
            if contains(name.as_bytes()) {
                heard.named_scan_responses += 1;
            }
        }
    }
    hci::command(capture, hci::LE_SET_SCAN_ENABLE, &[0, 0])?;
    Ok(heard)
}
