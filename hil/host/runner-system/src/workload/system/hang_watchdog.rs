//! The correctness image's hang watchdog: a stalled executor leaves a
//! post-mortem naming the stall, and the chip resets.
use crate::Result;
use hil_core::context::Context;
use oer_hil_protocol::{Fault, HangTarget, ResetReason};
use std::{path::Path, time::Duration};

pub fn run(output: &Path, context: &Context<'_>) -> Result<()> {
    for target in [HangTarget::ProtocolExecutor, HangTarget::NetworkExecutor] {
        let directory = output.join(scope(target));
        std::fs::create_dir_all(&directory)?;
        context.with_capture(&directory, |capture| {
            capture.prepare_startup(context.target())?;
            // Detection takes 3 s of stall and 4 s of sampling.
            capture.expect_reboot(Duration::from_secs(5), Duration::from_secs(20))?;
            capture.inject_hang(target)?;
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
            let stalled = stalled_bit(target);
            let hart = &hang.harts[usize::from(stalled.trailing_zeros() as u8)];
            // Core 1's timers are driven from core 0, so a stalled protocol
            // executor stalls the network executor's heartbeat too.
            if hang.stalled_executors & stalled == 0
                || !hart.responded
                || hart.mepc == 0
                || hang.samples.contains(&0)
            {
                return Err(
                    format!("the hang post-mortem does not name the stall: {hang:?}").into(),
                );
            }
            hil_core::durable::atomic_json(
                &directory.join("hang.json"),
                &serde_json::json!({
                    "schema": 1, "target": target, "reboot": reboot, "fresh_boot": fresh,
                    "passed": true
                }),
            )
        })?;
    }
    Ok(())
}

/// The watchdog's executor bit for `target`: bit 0 core 0, bit 1 core 1.
fn stalled_bit(target: HangTarget) -> u8 {
    match target {
        HangTarget::ProtocolExecutor => 0b01,
        HangTarget::NetworkExecutor => 0b10,
    }
}

fn scope(target: HangTarget) -> &'static str {
    match target {
        HangTarget::ProtocolExecutor => "protocol-executor",
        HangTarget::NetworkExecutor => "network-executor",
    }
}
