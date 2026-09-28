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
        match capture.boot_status() {
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
        Finding {
            boot,
            checkpoints,
            failure,
        }
    });
    let _ = capture.finish_with(Ok(()));
    finding
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
    let stalled = match (protocol, network) {
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
    fn core_one_stalled_with_core_zero_is_reported_as_dependent() {
        let failure = classify(&boot(ResetReason::Software, Some(hang(0b11))), &[], &name).unwrap();
        assert!(
            failure.message.contains("its timers depend on core 0"),
            "{}",
            failure.message
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
