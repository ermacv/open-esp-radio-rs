//! Pcap-based checks are independent of successful raw-socket send calls.
use super::model;
use crate::Result;
use serde::Serialize;
use std::{collections::BTreeSet, fs, path::Path, process::Command, time::Duration};

#[derive(Debug, Default, Serialize)]
struct Evidence {
    fixed_source_requests: usize,
    rotating_source_requests: usize,
    responses: usize,
    fixed_source_responses: usize,
    rotating_source_responses: usize,
    retry_responses: usize,
    maximum_responses_in_one_second: usize,
}
fn fixture_mac(mac: &str) -> bool {
    mac.starts_with("02:4f:45:52:")
}
fn fixed_mac(mac: &str) -> bool {
    mac == "02:4f:45:52:00:00"
}
fn parse(text: &str, bssid: &str) -> Result<Evidence> {
    let mut evidence = Evidence::default();
    let mut fixed = BTreeSet::new();
    let mut rotating = BTreeSet::new();
    let mut times = Vec::new();
    let mut response_sequences = BTreeSet::new();
    for line in text.lines() {
        let columns: Vec<_> = line.split('\t').collect();
        if columns.len() != 6 {
            return Err("incomplete probe capture fields".into());
        }
        let [time, kind, source, receiver, sequence, retry] = columns[..] else {
            unreachable!()
        };
        if kind == "0x0004" && fixture_mac(source) {
            let sequence = sequence.parse::<u16>()?;
            let expected = model::request(sequence).ok_or("unexpected probe sequence")?;
            if source != mac(expected.source) {
                return Err("probe source/sequence does not match the workload".into());
            }
            let key = (source.to_owned(), sequence);
            if fixed_mac(source) {
                fixed.insert(key);
            } else {
                rotating.insert(key);
            }
        }
        if kind == "0x0005" && source == bssid && fixture_mac(receiver) {
            evidence.responses += 1;
            if fixed_mac(receiver) {
                evidence.fixed_source_responses += 1;
            } else {
                evidence.rotating_source_responses += 1;
            }
            match retry {
                "True" | "1" => evidence.retry_responses += 1,
                "False" | "0" => {}
                _ => return Err("missing response retry flag".into()),
            }
            if !response_sequences.insert((receiver.to_owned(), sequence.parse::<u16>()?)) {
                return Err("duplicate probe response sequence".into());
            }
            let timestamp = time.parse::<f64>()?;
            if !timestamp.is_finite() {
                return Err("invalid probe capture timestamp".into());
            }
            times.push(timestamp);
        }
    }
    evidence.fixed_source_requests = fixed.len();
    evidence.rotating_source_requests = rotating.len();
    times.sort_by(f64::total_cmp);
    let mut start = 0;
    for end in 0..times.len() {
        while times[end] - times[start] >= 1.0 {
            start += 1;
        }
        evidence.maximum_responses_in_one_second = evidence
            .maximum_responses_in_one_second
            .max(end - start + 1);
    }
    Ok(evidence)
}
impl Evidence {
    fn validate(&self) -> Result<()> {
        if self.fixed_source_requests < 201 || self.rotating_source_requests < 200 {
            return Err(
                "probe air evidence incomplete: the observer did not see the full source workload"
                    .into(),
            );
        }
        if self.fixed_source_responses == 0 || self.rotating_source_responses == 0 {
            return Err("probe air evidence lacks responses for both source modes".into());
        }
        if self.retry_responses != 0 || self.maximum_responses_in_one_second > 105 {
            return Err("probe responses exceed the retry/admission budget".into());
        }
        Ok(())
    }
}
fn mac(address: [u8; 6]) -> String {
    address
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}
pub(crate) fn verify(output: &Path) -> Result<()> {
    let report: model::Report =
        serde_json::from_slice(&fs::read(output.join("probe-source.json"))?)?;
    report.validate()?;
    let capture: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("management.json"))?)?;
    if capture["kernel_dropped"].as_u64() != Some(0) {
        return Err("probe observer dropped capture frames".into());
    }
    let mut command = Command::new("tshark");
    command.arg("-r").arg(output.join("management.pcap")).args([
        "-Y",
        "wlan.fc.type_subtype == 4 || wlan.fc.type_subtype == 5",
        "-T",
        "fields",
    ]);
    for field in [
        "frame.time_epoch",
        "wlan.fc.type_subtype",
        "wlan.ta",
        "wlan.ra",
        "wlan.seq",
        "wlan.fc.retry",
    ] {
        command.args(["-e", field]);
    }
    let result = oer_process::output(&mut command, Some(Duration::from_secs(15)))?;
    if !result.status.success() {
        return Err("cannot decode probe air evidence".into());
    }
    let evidence = parse(&String::from_utf8(result.stdout)?, &mac(report.bssid))?;
    fs::write(
        output.join("probe-air.json"),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    evidence.validate()
}
#[cfg(test)]
mod tests;
