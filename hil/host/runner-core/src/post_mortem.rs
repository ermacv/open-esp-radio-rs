//! How a failed repetition's target says it ended.
//!
//! After a repetition fails, the runner attaches to the target without
//! resetting it and asks for its boot evidence: the boot that follows a hang
//! watchdog reset reports the hang, and any boot reports why it started. A
//! hang becomes a `hang` failure with each hart's interrupted code named from
//! the image's ELF; a reset the scenario did not cause becomes an
//! `unexpected-reset` failure. Either names the last checkpoints the ended
//! boot passed. Nothing is reclassified when the target reports neither.

use std::{path::Path, time::Duration};

use oer_hil_protocol::{BootEvidence, Checkpoint, Fault, HangFault, ResetReason};

use crate::{
    evidence::run::{Failure, FailureKind},
    session::SerialCapture,
};

/// How long a target may take to answer after a failure: a hang is reset by
/// its watchdog about seven seconds after it starts.
const ANSWER_WITHIN: Duration = Duration::from_secs(20);
/// Checkpoints named in a failure message.
const NAMED_CHECKPOINTS: usize = 6;

/// What the target reported after a failed repetition.
#[derive(Clone)]
pub struct Finding {
    pub boot: BootEvidence,
    pub checkpoints: Vec<Checkpoint>,
    /// The failure the report establishes, when it establishes one.
    pub failure: Option<Failure>,
}

/// The MAC of the board attached at `port`, its USB serial number.
pub fn board_mac(port: &Path) -> Option<String> {
    oer_hil_arbiter::port_mac(port)
}

/// The target's current port: `port` while it exists, else the port of the
/// board with `mac` once it is attached again, since a reset can make a USB
/// Serial/JTAG port re-enumerate under another name. `None` after `within`.
pub fn current_port(
    port: &Path,
    mac: Option<&str>,
    within: Duration,
) -> Option<std::path::PathBuf> {
    let started = std::time::Instant::now();
    loop {
        if let Some(mac) = mac
            && let Some(attached) = oer_hil_arbiter::attached_ports()
                .into_iter()
                .find(|attached| attached.mac.as_deref() == Some(mac))
        {
            return Some(attached.port.into());
        }
        if mac.is_none() && port.exists() {
            return Some(port.to_owned());
        }
        if started.elapsed() >= within || oer_process::sleep(Duration::from_millis(250)).is_err() {
            return None;
        }
    }
}

/// Attach to the target of board `mac` at `port` without resetting it,
/// record the exchange under `output/post-mortem`, and classify what it
/// reports; `None` when it does not answer.
pub fn inspect(
    port: &Path,
    mac: Option<&str>,
    output: &Path,
    elf: Option<&Path>,
) -> Option<Finding> {
    let port = current_port(port, mac, ANSWER_WITHIN)?;
    let capture = SerialCapture::attach(&port, &output.join("post-mortem")).ok()?;
    let started = std::time::Instant::now();
    let answer = loop {
        // Attached without a reset, the capture has seen no hello: discovery,
        // the one command a target answers without its boot identity, names
        // it first. A target that rebooted has sent its hello already.
        let status = if capture.latest_boot_id().is_none() {
            capture
                .discover(Duration::from_secs(2))
                .and_then(|_| capture.boot_status())
        } else {
            capture.boot_status()
        };
        match status {
            Ok(boot) => break Some(boot),
            Err(_) if started.elapsed() < ANSWER_WITHIN => {
                if oer_process::sleep(Duration::from_millis(500)).is_err() {
                    break None;
                }
            }
            Err(_) => break None,
        }
    };
    let finding = answer.map(|boot| {
        let checkpoints = boot
            .post_mortem
            .as_ref()
            .and_then(|summary| capture.post_mortem_checkpoints(summary.checkpoints).ok())
            .unwrap_or_default();
        let symbols = elf.and_then(|elf| addr2line::Loader::new(elf).ok());
        let name = |address: u32| symbol(symbols.as_ref(), address);
        let failure = classify(&boot, &checkpoints, &name);
        drain_trace(&capture, &output.join("post-mortem"));
        Finding {
            boot,
            checkpoints,
            failure,
        }
    });
    let _ = capture.finish_with(Ok(()));
    finding
}

/// Save the target's event trace in `directory` as `trace.json` (raw) and
/// `trace.txt` (decoded, oldest first): the previous boot's when the target
/// holds it, else the current boot's. A held trace is then restarted, so the
/// new boot records. A target without a trace, or one that does not answer,
/// leaves nothing.
fn drain_trace(capture: &SerialCapture, directory: &Path) {
    let Ok(status) = capture.trace_control(oer_hil_protocol::TraceControl::Status) else {
        return;
    };
    if !status.installed || status.stored_entries == 0 && status.stored_snapshots == 0 {
        return;
    }
    let entries = capture.trace_entries(status.entries).unwrap_or_default();
    let slots = (0..status.snapshot_slots)
        .filter_map(|slot| capture.trace_snapshot(slot).ok().flatten())
        .collect::<Vec<_>>();
    let snapshots = slots
        .iter()
        .map(|(header, words)| {
            serde_json::json!({"slot": header.slot, "point": header.point,
            "tag": header.tag, "t_us": header.t_us, "len": header.len,
            "truncated": header.truncated, "words": words})
        })
        .collect::<Vec<_>>();
    let _ = std::fs::create_dir_all(directory);
    let _ = crate::durable::atomic_json(
        &directory.join("trace.json"),
        &serde_json::json!({"schema": 1, "status": status, "entries": entries,
                            "snapshots": snapshots}),
    );
    let mut text = decode_trace(&status, &entries);
    for (header, words) in &slots {
        text.push_str(&describe_snapshot(header, words));
    }
    let _ = std::fs::write(directory.join("trace.txt"), text);
    if status.holding_previous {
        let _ = capture.trace_control(oer_hil_protocol::TraceControl::Start { mask: u64::MAX });
    }
}

/// The trace's entries, oldest first, one per line: time, event and words.
fn decode_trace(
    status: &oer_hil_protocol::TraceStatus,
    entries: &[oer_hil_protocol::TraceEntry],
) -> String {
    let mut records = entries
        .iter()
        .map(|entry| oer_trace::Record {
            tag: entry.tag,
            kind: entry.kind,
            t_us: entry.t_us,
            words: entry.words,
        })
        .collect::<Vec<_>>();
    oer_trace::oldest_first(&mut records);
    let mut text = format!(
        "# {} entries{}; {}\n",
        records.len(),
        status
            .trigger
            .map(|(kind, tag)| format!(", frozen by {} at tag {tag}", event_name(kind)))
            .unwrap_or_default(),
        if status.holding_previous {
            "left by the previous boot"
        } else {
            "of the current boot"
        }
    );
    for record in &records {
        text.push_str(&format!(
            "{:>12} us  tag {:>5}  {}\n",
            record.t_us,
            record.tag,
            oer_trace::Described {
                record,
                sets: TRACE_EVENT_SETS,
            }
        ));
    }
    text
}

/// One snapshot slot, one line: its point, the tag it was taken at, and the
/// decoded state when a domain knows the point, else its word count.
fn describe_snapshot(header: &oer_hil_protocol::TraceSnapshotPage, words: &[u32]) -> String {
    let decoded = (header.point == oer_phy_trace::PhySnapshot::POINT.raw() && !header.truncated)
        .then(|| oer_phy_trace::PhySnapshot::decode(words))
        .flatten()
        .map(|snapshot| snapshot.to_string());
    format!(
        "{:>12} us  tag {:>5}  snapshot {}: {}\n",
        header.t_us,
        header.tag,
        event_name(header.point),
        decoded.unwrap_or_else(|| format!(
            "{} words{}",
            words.len(),
            if header.truncated { ", truncated" } else { "" }
        ))
    )
}

/// The event sets a drained trace is described with; an event of another
/// domain is shown as its domain, id and words.
const TRACE_EVENT_SETS: &[oer_trace::Describer] = &[
    <oer_hil_target_core::trace::PlatformTrace as oer_trace::EventSet>::describe,
    <oer_ieee80211_trace::StationTrace as oer_trace::EventSet>::describe,
    <oer_phy_trace::PhyTrace as oer_trace::EventSet>::describe,
    <oer_ieee802154_trace::Ieee802154Trace as oer_trace::EventSet>::describe,
];

/// A trace kind's name: the platform's own, else its domain and event id.
fn event_name(kind: u16) -> String {
    match oer_trace::Kind::from_raw(kind) {
        Some(kind) => oer_hil_target_core::trace::platform_name(kind).map_or_else(
            || format!("{:?}.{}", kind.domain, kind.event).to_lowercase(),
            str::to_owned,
        ),
        None => format!("unknown-kind-{kind:#06x}"),
    }
}

/// The failure `boot` establishes, naming code addresses with `name`.
pub fn classify(
    boot: &BootEvidence,
    checkpoints: &[Checkpoint],
    name: &dyn Fn(u32) -> String,
) -> Option<Failure> {
    let trail = trail(checkpoints);
    if let Some(Fault::Hang(hang)) = boot.post_mortem.as_ref().and_then(|p| p.fault.as_ref()) {
        return Some(Failure::new(
            FailureKind::Hang,
            format!("{}{trail}", describe_hang(hang, name)),
        ));
    }
    match boot.reset_reason {
        // The runner's own resets and those it cannot attribute.
        ResetReason::UsbSerialJtag | ResetReason::Jtag | ResetReason::Other => None,
        reason => Some(Failure::new(
            FailureKind::UnexpectedReset,
            format!(
                "unexpected reset: {reason:?} (reset code {:#04x}){trail}",
                boot.raw_reset_reason
            ),
        )),
    }
}

/// Which executors stopped and where each hart was. Core 1's timers are
/// driven from core 0, so while core 0 is stalled a stalled core 1 is
/// reported as depending on it rather than as stalled on its own.
fn describe_hang(hang: &HangFault, name: &dyn Fn(u32) -> String) -> String {
    let protocol = hang.stalled_executors & 0b01 != 0;
    let network = hang.stalled_executors & 0b10 != 0;
    let task;
    let stalled = match (protocol, network) {
        (false, false) if let Some(stall) = hang.stalled_task => {
            task = format!(
                "task {} made no progress for {} ms while executors ran",
                stall.slot.id(),
                stall.pending_ms
            );
            task.as_str()
        }
        (true, true) => {
            "the core 0 protocol executor stalled (core 1's network heartbeat \
                         stopped with it: its timers depend on core 0)"
        }
        (true, false) => "the core 0 protocol executor stalled",
        (false, true) => "the core 1 network executor stalled",
        (false, false) => "an executor stalled",
    };
    let hart = |index: usize| {
        let state = &hang.harts[index];
        if state.responded {
            format!(
                "core {index} at {} called from {} (sp {:#010x})",
                name(state.mepc),
                name(state.ra),
                state.sp
            )
        } else {
            format!("core {index} did not answer: its interrupts are masked")
        }
    };
    // A stalled task is sampled on core 0, like a stalled core 0.
    let sampled = if network && !protocol { 1 } else { 0 };
    let mut samples = hang.samples.to_vec();
    samples.sort_unstable();
    samples.dedup();
    let mut loop_at = Vec::new();
    for named in samples.iter().map(|address| name(*address)) {
        if !loop_at.contains(&named) && loop_at.len() < 4 {
            loop_at.push(named);
        }
    }
    let loop_at = loop_at.join(", ");
    format!(
        "hang after {} ms: {stalled}; {}; {}; core {sampled} kept running {loop_at}",
        hang.detected_uptime_ms,
        hart(0),
        hart(1)
    )
}

/// `; last checkpoints: a(1)@10ms, b(2)@20ms` for the newest checkpoints.
fn trail(checkpoints: &[Checkpoint]) -> String {
    if checkpoints.is_empty() {
        return String::new();
    }
    let newest = checkpoints
        .iter()
        .rev()
        .take(NAMED_CHECKPOINTS)
        .rev()
        .map(|checkpoint| {
            format!(
                "{}({})@{}ms",
                checkpoint.name, checkpoint.arg, checkpoint.uptime_ms
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("; last checkpoints: {newest}")
}

/// `function (file.rs:34)` for `address` in the image from its debug
/// information, naming the function it was inlined into as `in outer`; the
/// address when the image does not describe it.
pub(crate) fn symbol(loader: Option<&addr2line::Loader>, address: u32) -> String {
    let raw = format!("{address:#010x}");
    let Some(loader) = loader else {
        return raw;
    };
    let Ok(mut frames) = loader.find_frames(u64::from(address)) else {
        return raw;
    };
    let mut names = Vec::new();
    let mut location = None;
    while let Ok(Some(frame)) = frames.next() {
        if location.is_none() {
            location = frame.location.as_ref().and_then(|location| {
                let file = Path::new(location.file?)
                    .file_name()?
                    .to_string_lossy()
                    .into_owned();
                Some(format!("{file}:{}", location.line?))
            });
        }
        if let Some(function) = frame.function.as_ref().and_then(|f| f.demangle().ok()) {
            names.push(function.into_owned());
        }
    }
    let Some(innermost) = names.first() else {
        return raw;
    };
    let mut text = innermost.clone();
    if let Some(outer) = names.last().filter(|_| names.len() > 1) {
        text.push_str(&format!(" in {outer}"));
    }
    if let Some(location) = location {
        text.push_str(&format!(" ({location})"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_hil_protocol::{HartState, PostMortemSummary};

    fn slot(point: u16, words: &[u32], truncated: bool) -> oer_hil_protocol::TraceSnapshotPage {
        oer_hil_protocol::TraceSnapshotPage {
            slot: 0,
            point,
            tag: 7,
            t_us: 1_000,
            len: words.len() as u16,
            truncated,
            offset: 0,
            words: Default::default(),
        }
    }

    #[test]
    fn a_phy_snapshot_is_decoded_and_another_point_shows_its_size() {
        use oer_phy_trace::{
            BusRead, Client, Clients, Fault, FaultStage, PhySnapshot, Poison, PoisonedBy, Slot,
            TemperatureReferences,
        };
        let snapshot = PhySnapshot {
            poison: Poison {
                by: PoisonedBy::Tracking,
                fault: Fault {
                    stage: FaultStage::Deadline,
                    detail: 0xbeef,
                },
            },
            slot: Slot::Pending,
            clients: Clients::NONE.with(Client::Wifi),
            temperatures: TemperatureReferences {
                rfpll: 24,
                calibration: 23,
                transmit: 25,
                power: 22,
                observed: 26,
            },
            bus: BusRead::DomainOff,
            platform_clocks: [1, 1, 1, 0, 0, 0, 0, 0],
        };
        let words = snapshot.encode();
        let point = PhySnapshot::POINT.raw();
        let line = describe_snapshot(&slot(point, &words, false), &words);
        assert!(line.contains(&snapshot.to_string()), "{line}");
        // A truncated or foreign slot is shown by size, never misdecoded.
        let truncated = describe_snapshot(&slot(point, &words, true), &words);
        assert!(truncated.contains("14 words, truncated"), "{truncated}");
        let other = describe_snapshot(&slot(0x0181, &[1, 2, 3], false), &[1, 2, 3]);
        assert!(other.ends_with("3 words\n"), "{other}");
    }

    fn boot(reset_reason: ResetReason, fault: Option<Fault>) -> BootEvidence {
        BootEvidence {
            reset_reason,
            raw_reset_reason: 0x03,
            post_mortem: Some(PostMortemSummary {
                boot_count: 2,
                checkpoints: 1,
                fault,
            }),
        }
    }

    fn hang(stalled: u8) -> Fault {
        let hart = |mepc| HartState {
            responded: true,
            mepc,
            ra: mepc + 4,
            sp: 0x2f01_9ef0,
            mcause: 0,
            mstatus: 0,
        };
        Fault::Hang(HangFault {
            detected_uptime_ms: 7505,
            stalled_executors: stalled,
            harts: [hart(0x5000_0010), hart(0x5000_0020)],
            samples: [0x5000_0020; 16],
            stalled_task: None,
        })
    }

    fn name(address: u32) -> String {
        format!("f{address:x}")
    }

    fn checkpoint(name: &str) -> Checkpoint {
        Checkpoint {
            name: name.try_into().unwrap(),
            arg: 1,
            uptime_ms: 10,
            hart: 0,
        }
    }

    #[test]
    fn a_hang_names_each_hart_and_the_checkpoints_before_it() {
        let failure = classify(
            &boot(ResetReason::Software, Some(hang(0b10))),
            &[checkpoint("coex.grant")],
            &name,
        )
        .unwrap();
        assert_eq!(failure.kind, FailureKind::Hang);
        let message = failure.message;
        assert!(
            message.contains("core 1 network executor stalled"),
            "{message}"
        );
        assert!(
            message.contains("core 0 at f50000010 called from f50000014"),
            "{message}"
        );
        assert!(
            message.contains("core 1 kept running f50000020"),
            "{message}"
        );
        assert!(
            message.contains("last checkpoints: coex.grant(1)@10ms"),
            "{message}"
        );
    }

    #[test]
    fn a_stalled_task_is_named_with_how_long_its_work_waited() {
        let Fault::Hang(mut stall) = hang(0) else {
            unreachable!()
        };
        stall.stalled_task = Some(oer_hil_protocol::TaskStall {
            slot: oer_hil_protocol::TaskSlot::Console,
            pending_ms: 5_250,
        });
        let failure = classify(
            &boot(ResetReason::Software, Some(Fault::Hang(stall))),
            &[],
            &name,
        )
        .unwrap();
        assert_eq!(failure.kind, FailureKind::Hang);
        assert!(
            failure.message.contains(
                "hang after 7505 ms: task console made no progress for 5250 ms while executors ran"
            ),
            "{}",
            failure.message
        );
    }

    #[test]
    fn core_one_stalled_with_core_zero_is_reported_as_dependent() {
        let failure = classify(&boot(ResetReason::Software, Some(hang(0b11))), &[], &name).unwrap();
        assert!(
            failure.message.contains("its timers depend on core 0"),
            "{}",
            failure.message
        );
    }

    #[test]
    fn a_trace_is_decoded_oldest_first_with_platform_names() {
        let hang = <oer_hil_target_core::trace::Hang as oer_trace::Event>::KIND.raw();
        let station = oer_trace::Kind::new(oer_trace::Domain::Bluetooth, 9).raw();
        let entry = |tag, kind, t_us| oer_hil_protocol::TraceEntry {
            tag,
            kind,
            t_us,
            // The platform's events carry no words.
            words: if kind == hang { [0, 0] } else { [1, 2] },
        };
        let status = oer_hil_protocol::TraceStatus {
            installed: true,
            entries: 4,
            snapshot_slots: 0,
            snapshot_words: 0,
            running: false,
            frozen: false,
            mask: u64::MAX,
            trigger: Some((hang, 3)),
            holding_previous: true,
            stored_entries: 2,
            stored_snapshots: 0,
        };
        // Storage order is not time order: the ring wrapped.
        let text = decode_trace(&status, &[entry(3, hang, 200), entry(2, station, 100)]);
        let lines = text.lines().collect::<Vec<_>>();
        assert!(lines[0].contains("frozen by platform.hang"), "{text}");
        assert!(lines[0].contains("previous boot"), "{text}");
        assert!(
            lines[1].contains("bluetooth.9 [0x00000001, 0x00000002]"),
            "{text}"
        );
        assert!(lines[2].contains("platform.hang"), "{text}");
        // A station event is described by the station's event set.
        let exit = oer_trace::Record {
            tag: 1,
            kind: <oer_ieee80211_trace::ControlExit as oer_trace::Event>::KIND.raw(),
            t_us: 0,
            words: [0, 0],
        };
        let described = oer_trace::Described {
            record: &exit,
            sets: TRACE_EVENT_SETS,
        }
        .to_string();
        assert!(!described.starts_with("ieee80211."), "{described}");
        // So is an IEEE 802.15.4 driver event, by the driver's event set.
        let change = oer_ieee802154_trace::StateChange {
            from: oer_ieee802154_trace::MacState::Rx,
            to: oer_ieee802154_trace::MacState::Tx,
        };
        let record = oer_trace::Record {
            tag: 1,
            kind: <oer_ieee802154_trace::StateChange as oer_trace::Event>::KIND.raw(),
            t_us: 0,
            words: oer_trace::Event::encode(&change),
        };
        assert_eq!(
            oer_trace::Described {
                record: &record,
                sets: TRACE_EVENT_SETS,
            }
            .to_string(),
            change.to_string()
        );
    }

    #[test]
    fn only_resets_the_runner_did_not_cause_are_unexpected() {
        let brownout = classify(&boot(ResetReason::Brownout, None), &[], &name).unwrap();
        assert_eq!(brownout.kind, FailureKind::UnexpectedReset);
        assert!(
            brownout.message.contains("Brownout"),
            "{}",
            brownout.message
        );
        for expected in [
            ResetReason::UsbSerialJtag,
            ResetReason::Jtag,
            ResetReason::Other,
        ] {
            assert!(classify(&boot(expected, None), &[], &name).is_none());
        }
    }
}
