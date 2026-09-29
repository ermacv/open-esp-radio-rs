//! Passive, active and filtered scanning of a Linux scannable advertiser.
//!
//! The Linux adapter advertises a fresh marker as manufacturer data and a
//! fresh name, which BlueZ carries only in the scan response. A passive ESP
//! scan must report the advertisement and no scan response; an active scan
//! must also report the scan response with the name, which the adapter sends
//! only in answer to the ESP's `SCAN_REQ`. The unfiltered active scan also
//! hears other advertisers on the air. A third, active scan with filter
//! policy 1 and only the adapter on the Filter Accept List must report the
//! adapter and its named scan response and nothing from any other
//! advertiser, and the list must refuse a change while that scanner runs.
use super::hci::{self, Packet};
use crate::{
    Result,
    fixture::bluetooth::{advertiser::Advertiser, att},
};
use hil_core::{context::Context, session::SerialCapture};
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
const COMMAND_DISALLOWED: u8 = 0x0c;

/// The scanner's filter policy.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    /// Every advertiser.
    None,
    /// Only advertisers on the Filter Accept List.
    AcceptList,
}

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
        let cleanup = oer_process::cleanup(|| hci::command(capture, hci::RESET, &[]).map(|_| ()));
        report.insert("schema".into(), 2.into());
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

/// What one scan pass heard.
#[derive(Default)]
struct Heard {
    /// Every LE Advertising Report, from any advertiser.
    reports: u32,
    /// Reports by advertising event kind, from any advertiser.
    kinds: [u32; 5],
    other_events: u32,
    /// The adapter's address kind and address, once its marker was heard.
    advertiser: Option<[u8; 7]>,
    advertisements: u32,
    scan_responses: u32,
    named_scan_responses: u32,
    /// Reports from any other advertiser.
    others: u32,
    /// Status of a Filter Accept List change while the scanner ran.
    list_change_while_scanning: Option<u8>,
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
    let passive = pass(capture, false, Filter::None, None, name, marker)?;
    report.insert("passive".into(), passive.json());
    let active = pass(capture, true, Filter::None, None, name, marker)?;
    report.insert("active".into(), active.json());
    unfiltered(&passive, &active)?;
    let adapter = passive
        .advertiser
        .ok_or("the adapter's address was not heard")?;
    hci::command(capture, hci::LE_CLEAR_FILTER_ACCEPT_LIST, &[])?;
    hci::command(capture, hci::LE_ADD_DEVICE_TO_FILTER_ACCEPT_LIST, &adapter)?;
    let filtered = pass(
        capture,
        true,
        Filter::AcceptList,
        Some(adapter),
        name,
        marker,
    )?;
    report.insert("filtered".into(), filtered.json());
    accept_list_only(&filtered)
}

/// The unfiltered passes: only the active one gets the scan response, and
/// it also hears other advertisers, the control for the filtered pass.
fn unfiltered(passive: &Heard, active: &Heard) -> Result<()> {
    if passive.advertisements == 0 {
        return Err("the passive scan heard no advertisement from the adapter".into());
    }
    if passive.scan_responses != 0 {
        return Err("the passive scan reported a scan response".into());
    }
    if active.named_scan_responses == 0 {
        return Err("the active scan reported no scan response with the adapter's name".into());
    }
    if active.others == 0 {
        return Err("the unfiltered active scan heard no other advertiser to filter out".into());
    }
    Ok(())
}

/// The filtered pass reported only the listed adapter, with its named scan
/// response, and the list refused a change while the scanner ran.
fn accept_list_only(filtered: &Heard) -> Result<()> {
    if filtered.others != 0 {
        return Err(format!(
            "the filtered scan reported {} advertisements from unlisted advertisers",
            filtered.others
        )
        .into());
    }
    if filtered.advertisements == 0 || filtered.named_scan_responses == 0 {
        return Err("the filtered scan missed the listed adapter or its scan response".into());
    }
    if filtered.list_change_while_scanning != Some(COMMAND_DISALLOWED) {
        return Err(format!(
            "adding to the Filter Accept List while scanning returned {:?}",
            filtered.list_change_while_scanning
        )
        .into());
    }
    Ok(())
}

impl Heard {
    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "reports": self.reports,
            "kinds": self.kinds,
            "other_events": self.other_events,
            "advertiser": self.advertiser.map(|address| format!("{address:02x?}")),
            "advertisements": self.advertisements,
            "scan_responses": self.scan_responses,
            "named_scan_responses": self.named_scan_responses,
            "others": self.others,
            "list_change_while_scanning": self.list_change_while_scanning,
        })
    }

    /// Account one LE Advertising Report carrying a single report: code,
    /// length, subevent, count, then event kind, address kind and address,
    /// data length, data and RSSI.
    fn report(&mut self, packet: &[u8], name: &str, marker: &[u8]) {
        if packet.len() < 13 || packet[0] != 0x3e || packet[2] != ADVERTISING_REPORT {
            self.other_events += 1;
            return;
        }
        self.reports += 1;
        let kind = packet[4];
        if let Some(count) = self.kinds.get_mut(usize::from(kind)) {
            *count += 1;
        }
        let address: [u8; 7] = packet[5..12].try_into().expect("seven octets");
        let length = usize::from(packet[12]);
        let Some(data) = packet.get(13..13 + length) else {
            return;
        };
        let contains = |needle: &[u8]| data.windows(needle.len()).any(|window| window == needle);
        if kind != SCAN_RSP && contains(marker) {
            self.advertiser = Some(address);
            self.advertisements += 1;
        } else if self.advertiser == Some(address) {
            if kind == SCAN_RSP {
                self.scan_responses += 1;
                if contains(name.as_bytes()) {
                    self.named_scan_responses += 1;
                }
            }
        } else {
            self.others += 1;
        }
    }
}

fn pass(
    capture: &SerialCapture,
    active: bool,
    filter: Filter,
    listed: Option<[u8; 7]>,
    name: &str,
    marker: &[u8],
) -> Result<Heard> {
    let mut parameters = [0; 7];
    parameters[0] = u8::from(active);
    parameters[1..3].copy_from_slice(&INTERVAL.to_le_bytes());
    parameters[3..5].copy_from_slice(&WINDOW.to_le_bytes());
    parameters[6] = u8::from(filter == Filter::AcceptList);
    hci::command(capture, hci::LE_SET_SCAN_PARAMETERS, &parameters)?;
    hci::command(capture, hci::LE_SET_SCAN_ENABLE, &[1, 0])?;
    let mut heard = Heard::default();
    if let Some(listed) = listed {
        let (status, _) =
            hci::command_complete(capture, hci::LE_ADD_DEVICE_TO_FILTER_ACCEPT_LIST, &listed)?;
        heard.list_change_while_scanning = Some(status);
    }
    let deadline = Instant::now() + LISTEN;
    while Instant::now() < deadline {
        if let Some(Packet::Event(packet)) = hci::next_packet(capture, Duration::from_millis(200))?
        {
            heard.report(&packet, name, marker);
        }
    }
    hci::command(capture, hci::LE_SET_SCAN_ENABLE, &[0, 0])?;
    Ok(heard)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MARKER: [u8; 4] = [1, 2, 3, 4];
    const NAME: &str = "oer-pc-1";
    const ADAPTER: [u8; 7] = [0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6];
    const OTHER: [u8; 7] = [1, 0xb1, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6];

    fn report(kind: u8, address: [u8; 7], data: &[u8]) -> Vec<u8> {
        let mut packet = vec![0x3e, 0, ADVERTISING_REPORT, 1, kind];
        packet.extend_from_slice(&address);
        packet.push(data.len() as u8);
        packet.extend_from_slice(data);
        packet.push(0xc0);
        packet[1] = (packet.len() - 2) as u8;
        packet
    }

    fn heard(packets: &[Vec<u8>]) -> Heard {
        let mut heard = Heard::default();
        for packet in packets {
            heard.report(packet, NAME, &MARKER);
        }
        heard
    }

    #[test]
    fn reports_split_into_the_adapter_its_scan_response_and_others() {
        let heard = heard(&[
            report(0x02, ADAPTER, &MARKER),
            report(SCAN_RSP, ADAPTER, NAME.as_bytes()),
            report(0x00, OTHER, &[9]),
        ]);
        assert_eq!(heard.advertiser, Some(ADAPTER));
        assert_eq!(heard.advertisements, 1);
        assert_eq!(heard.named_scan_responses, 1);
        assert_eq!(heard.others, 1);
    }

    #[test]
    fn the_filtered_pass_needs_the_adapter_no_other_and_a_refused_change() {
        let mut filtered = heard(&[
            report(0x02, ADAPTER, &MARKER),
            report(SCAN_RSP, ADAPTER, NAME.as_bytes()),
        ]);
        filtered.list_change_while_scanning = Some(COMMAND_DISALLOWED);
        assert!(accept_list_only(&filtered).is_ok());
        filtered.list_change_while_scanning = Some(0);
        assert!(accept_list_only(&filtered).is_err());
        filtered.list_change_while_scanning = Some(COMMAND_DISALLOWED);
        filtered.report(&report(0x00, OTHER, &[9]), NAME, &MARKER);
        assert!(accept_list_only(&filtered).is_err());
    }

    #[test]
    fn the_unfiltered_control_must_hear_another_advertiser() {
        let passive = heard(&[report(0x02, ADAPTER, &MARKER)]);
        let mut active = heard(&[
            report(0x02, ADAPTER, &MARKER),
            report(SCAN_RSP, ADAPTER, NAME.as_bytes()),
        ]);
        assert!(unfiltered(&passive, &active).is_err());
        active.report(&report(0x00, OTHER, &[9]), NAME, &MARKER);
        assert!(unfiltered(&passive, &active).is_ok());
    }
}
