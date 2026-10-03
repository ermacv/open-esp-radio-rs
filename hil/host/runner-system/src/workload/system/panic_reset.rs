//! The panic-reset image: the platform's product panic entry records an
//! intentional panic and resets the chip; the boot ROM and bootloader start
//! again, and the next boot reports the record.
use crate::Result;
use oer_hil_execution::context::Context;
use oer_hil_protocol::base::{PlatformPanic, ResetReason};
use std::{path::Path, time::Duration};

/// The agent source that raises the panic, as its record ends.
const PANIC_FILE: &str = "system/panic_reset.rs";

pub fn run(output: &Path, context: &Context<'_>) -> Result<()> {
    context.with_capture(output, |capture| {
        if !capture
            .request_image_keys(Duration::from_secs(10))?
            .has::<oer_hil_protocol::system::InjectPanic>()
        {
            return Err("requires the exclusive panic-reset image".into());
        }
        // The reset follows the acknowledgement at once; the next Hello
        // comes after the ROM, the bootloader and both boot stages.
        capture.expect_reboot(Duration::from_millis(100), Duration::from_secs(15))?;
        let mark = capture.console_length();
        capture.inject_panic()?;
        let reboot = capture.wait_expected_reboot()?;
        let fresh = capture.boot_status()?;
        if fresh.reset_reason != ResetReason::Software {
            return Err(format!(
                "expected the panic entry's software reset, not {:?} (0x{:02x})",
                fresh.reset_reason, fresh.raw_reset_reason
            )
            .into());
        }
        if !capture.console_shows_boot_since(mark) {
            return Err("the console shows no ROM or bootloader start after the reset".into());
        }
        let Some(panic) = fresh.platform_panic.as_ref() else {
            return Err(format!("no platform panic record after the reset: {fresh:?}").into());
        };
        names_the_injection(panic)?;
        oer_hil_durable::atomic_json(
            &output.join("panic-reset.json"),
            &serde_json::json!({
                "schema": 1, "reboot": reboot, "fresh_boot": fresh,
                "boot_console_after_reset": true, "passed": true
            }),
        )
    })
}

/// Whether `panic` is the injected one: raised in thread context on hart 0,
/// where the protocol executor runs, at a line of the agent's panic source.
fn names_the_injection(panic: &PlatformPanic) -> Result<()> {
    if panic.file.ends_with(PANIC_FILE)
        && panic.line != 0
        && panic.hart == 0
        && !panic.in_interrupt
        && panic.interrupted_pc.is_none()
    {
        Ok(())
    } else {
        Err(format!("the platform panic record does not name the injected panic: {panic:?}").into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(file: &str, hart: u8, interrupted_pc: Option<u32>) -> PlatformPanic {
        PlatformPanic {
            hart,
            in_interrupt: interrupted_pc.is_some(),
            interrupted_pc,
            file: file.try_into().unwrap(),
            line: 57,
            column: 5,
        }
    }

    #[test]
    fn only_the_injected_thread_panic_matches() {
        let injected = "agent/src/system/panic_reset.rs";
        assert!(names_the_injection(&record(injected, 0, None)).is_ok());
        assert!(names_the_injection(&record("src/main.rs", 0, None)).is_err());
        assert!(names_the_injection(&record(injected, 1, None)).is_err());
        assert!(names_the_injection(&record(injected, 0, Some(0x4000_0000))).is_err());
    }
}
