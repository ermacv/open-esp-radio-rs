//! The correctness image's hang watchdog: a stalled executor, or a console
//! that stops taking commands while both executors run, leaves a post-mortem
//! naming the stall, and the chip resets.
use crate::Result;
use oer_hil_execution::context::Context;
use oer_hil_protocol::{
    base::{Fault, HangFault, ResetReason, TaskSlot},
    system::HangTarget,
};
use std::{path::Path, time::Duration};

const TARGETS: [HangTarget; 3] = [
    HangTarget::ProtocolExecutor,
    HangTarget::NetworkExecutor,
    HangTarget::Console,
];

pub fn run(output: &Path, context: &Context<'_>) -> Result<()> {
    for target in TARGETS {
        let directory = output.join(scope(target));
        std::fs::create_dir_all(&directory)?;
        context.with_capture(&directory, |capture| {
            capture.prepare_startup(context.target())?;
            // An executor stall is detected after 3 s, a waiting command
            // after 5 s; sampling takes 4 s more.
            capture.expect_reboot(Duration::from_secs(5), Duration::from_secs(25))?;
            capture.inject_hang(target)?;
            if target == HangTarget::Console {
                // The console now takes no command: this one waits, unanswered,
                // until the watchdog resets the chip.
                let _ = capture.boot_status();
            }
            let reboot = capture.wait_expected_reboot()?;
            let fresh = capture.boot_status()?;
            if fresh.reset_reason != ResetReason::Software {
                return Err(format!(
                    "expected the watchdog's software reset, not {:?} (0x{:02x})",
                    fresh.reset_reason, fresh.raw_reset_reason
                )
                .into());
            }
            let Some(Fault::Hang(hang)) = fresh.post_mortem.as_ref().and_then(|p| p.fault.clone())
            else {
                return Err(format!("no hang post-mortem after the reset: {fresh:?}").into());
            };
            if !names(target, &hang) {
                return Err(
                    format!("the hang post-mortem does not name the stall: {hang:?}").into(),
                );
            }
            // The trace froze at the hang and waits, untouched, for the host.
            let trace = capture.trace_control(oer_hil_protocol::telemetry::TraceControl::Status)?;
            let hang_kind = <oer_hil_trace::Hang as oer_trace::Event>::KIND.raw();
            if !trace.holding_previous || trace.trigger.map(|(kind, _)| kind) != Some(hang_kind) {
                return Err(format!("the trace did not freeze at the hang: {trace:?}").into());
            }
            let entries = capture.trace_entries(trace.entries)?;
            let restarted =
                capture.trace_control(oer_hil_protocol::telemetry::TraceControl::Start {
                    mask: u64::MAX,
                })?;
            if !restarted.running || restarted.holding_previous {
                return Err(format!("the trace did not restart: {restarted:?}").into());
            }
            oer_hil_durable::atomic_json(
                &directory.join("hang.json"),
                &serde_json::json!({
                    "schema": 1, "target": target, "reboot": reboot, "fresh_boot": fresh,
                    "trace": trace, "trace_entries": entries.len(), "passed": true
                }),
            )
        })?;
    }
    Ok(())
}

/// Whether `hang` names the stall of `target` and where its hart was.
fn names(target: HangTarget, hang: &HangFault) -> bool {
    let (stalled, task, hart) = match target {
        // Core 1's timers are driven from core 0, so a stalled protocol
        // executor stalls the network executor's heartbeat too.
        HangTarget::ProtocolExecutor => (hang.stalled_executors & 0b01 != 0, true, 0),
        HangTarget::NetworkExecutor => (hang.stalled_executors & 0b10 != 0, true, 1),
        HangTarget::Console => (
            hang.stalled_executors == 0,
            hang.stalled_task
                .is_some_and(|stall| stall.slot == TaskSlot::Console),
            0,
        ),
    };
    let hart = &hang.harts[hart];
    stalled && task && hart.responded && hart.mepc != 0 && !hang.samples.contains(&0)
}

fn scope(target: HangTarget) -> &'static str {
    match target {
        HangTarget::ProtocolExecutor => "protocol-executor",
        HangTarget::NetworkExecutor => "network-executor",
        HangTarget::Console => "console",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_hil_protocol::base::{HartState, TaskStall};

    fn hang(stalled_executors: u8, stalled_task: Option<TaskSlot>) -> HangFault {
        let hart = HartState {
            responded: true,
            mepc: 0x4200_0000,
            ..HartState::default()
        };
        HangFault {
            detected_uptime_ms: 9_000,
            stalled_executors,
            harts: [hart; 2],
            samples: [0x4200_0000; 16],
            stalled_task: stalled_task.map(|slot| TaskStall {
                slot,
                pending_ms: 5_001,
            }),
        }
    }

    #[test]
    fn a_console_hang_must_name_the_console_and_no_executor() {
        assert!(names(
            HangTarget::Console,
            &hang(0, Some(TaskSlot::Console))
        ));
        assert!(!names(HangTarget::Console, &hang(0b01, None)));
        assert!(!names(
            HangTarget::Console,
            &hang(0, Some(TaskSlot::SessionEvidence))
        ));
        assert!(names(HangTarget::ProtocolExecutor, &hang(0b11, None)));
        assert!(!names(HangTarget::NetworkExecutor, &hang(0b01, None)));
    }
}
