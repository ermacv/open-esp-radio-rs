//! SoC reset evidence without a radio peer; distinct from DTM RF evidence.
use crate::{Result, context::Context};
use open_esp_radio_hil_protocol::{ResetReason, WatchdogTestMode as Mode};
use std::{path::Path, time::Duration};

pub(crate) fn run(output: &Path, context: &Context<'_>) -> Result<()> {
    for mode in [
        Mode::Complete,
        Mode::BlockedPoll,
        Mode::Cancelled,
        Mode::LostCompletion,
        Mode::LateRestoration,
    ] {
        let directory = output.join(scope(mode));
        std::fs::create_dir_all(&directory)?;
        context.with_capture(&directory, |capture| {
            let features = capture
                .request_capabilities(Duration::from_secs(10))?
                .features;
            if !features.system_watchdog {
                return Err("requires the exclusive watchdog diagnostic image".into());
            }
            if mode != Mode::Complete {
                capture.expect_reboot(Duration::from_millis(500), Duration::from_secs(12))?;
            }
            capture.system_watchdog_test(mode)?;
            let reboot = if mode == Mode::Complete {
                oer_process::sleep(Duration::from_secs(3))?;
                capture.request_capabilities(Duration::from_secs(5))?;
                None
            } else {
                Some(capture.wait_expected_reboot()?)
            };
            let fresh = capture.boot_status()?;
            if reboot.is_some() && fresh.reset_reason != ResetReason::MainWatchdog1 {
                return Err("expected autonomous MWDT1 reset, not software reset".into());
            }
            crate::durable::atomic_json(
                &directory.join("watchdog.json"),
                &serde_json::json!({
                    "schema": 2, "mode": mode, "budget_micros": 1_000_000,
                    "reboot": reboot, "fresh_boot": fresh, "rf_stop_bound_micros": null,
                    "scope": "SoC service without a radio; not a PHY restoration fault injection",
                    "passed": true
                }),
            )
        })?;
    }
    Ok(())
}

fn scope(mode: Mode) -> &'static str {
    match mode {
        Mode::Complete => "complete",
        Mode::BlockedPoll => "blocked-poll",
        Mode::Cancelled => "cancelled",
        Mode::LostCompletion => "lost-completion",
        Mode::LateRestoration => "late-restoration",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_scopes_are_valid_and_distinct() {
        let modes = [
            Mode::Complete,
            Mode::BlockedPoll,
            Mode::Cancelled,
            Mode::LostCompletion,
            Mode::LateRestoration,
        ];
        let names: std::collections::BTreeSet<_> = modes.map(scope).into_iter().collect();
        assert_eq!(names.len(), modes.len());
        assert!(
            names
                .iter()
                .all(|s| s.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'))
        );
    }
}
