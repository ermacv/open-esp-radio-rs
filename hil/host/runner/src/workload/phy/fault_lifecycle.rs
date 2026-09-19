//! Real PHY owner checkpoints, independently terminated by the SoC service.
use crate::{Result, execution::context::Context, session::SerialCapture};
use open_esp_radio_hil_protocol::{
    PhyFaultCommand as Control, PhyFaultMode as Mode, PhyFaultPhase as Phase, ResetReason,
};
use std::{
    path::Path,
    time::{Duration, Instant},
};

/// Domain scenarios own link preparation, normal controls and maintenance entry.
/// This coordinator owns only the destructive checkpoint/reset sequence.
pub(in crate::workload) trait Scenario {
    const RADIO: &'static str;
    fn prove_link(
        &self,
        capture: &SerialCapture,
        context: &Context<'_>,
        directory: &Path,
    ) -> Result<serde_json::Value>;
    fn normal_maintenance(&self, capture: &SerialCapture) -> Result<serde_json::Value>;
    fn begin_fault(
        &self,
        capture: &SerialCapture,
        mode: Mode,
    ) -> Result<open_esp_radio_hil_protocol::PhyFaultEvidence>;
}

pub(in crate::workload) fn run<S: Scenario>(
    output: &Path,
    context: &Context<'_>,
    scenario: S,
) -> Result<()> {
    for (mode, name) in [
        (Mode::BlockedPoll, "blocked-poll"),
        (Mode::LostCompletion, "lost-completion"),
        (Mode::Restoration, "restoration"),
        (Mode::Cancelled, "cancelled"),
    ] {
        let directory = output.join(name);
        std::fs::create_dir_all(&directory)?;
        context.with_capture(&directory, |capture| {
            let mut evidence = serde_json::json!({
                "schema": 2, "mode": mode, "radio": S::RADIO,
                "budget_micros": 5_000_000, "rf_stop_bound_micros": null,
                "fault_boundary": if mode == Mode::Restoration { "PHY returned, IRQ/MAC not restored" }
                    else { "PBus clear completed, parent completion not published" },
                "passed": false,
            });
            let result = (|| {
                if !capture.request_capabilities(Duration::from_secs(10))?.features.phy_fault_injection {
                    return Err("image lacks real PHY fault checkpoints".into());
                }
                evidence["before"] = scenario.prove_link(capture, context, &directory.join("before"))?;
                evidence["normal_maintenance"] = scenario.normal_maintenance(capture)?;
                oer_process::sleep(Duration::from_secs(6))?;
                capture.request_capabilities(Duration::from_secs(2))?;
                capture.expect_reboot(Duration::from_secs(3), Duration::from_secs(15))?;
                let armed = scenario.begin_fault(capture, mode)?;
                if armed.phase != Phase::Armed { return Err("fault did not arm".into()); }
                let reached = wait_phase(capture, Phase::Reached)?;
                evidence["reached"] = serde_json::to_value(reached)?;
                let released = capture.phy_fault(Control::Release)?;
                if released.phase != Phase::Released { return Err("fault release not acknowledged".into()); }
                evidence["released"] = serde_json::to_value(released)?;
                if mode == Mode::Cancelled {
                    evidence["cancelled"] = serde_json::to_value(wait_phase(capture, Phase::Cancelled)?)?;
                }
                evidence["reboot"] = serde_json::to_value(capture.wait_expected_reboot()?)?;
                let fresh = capture.phy_fault(Control::Status)?;
                if fresh.phase != Phase::Idle || fresh.reset_reason != ResetReason::MainWatchdog1 {
                    return Err("requires clean new epoch after autonomous MWDT1, not software reset".into());
                }
                evidence["fresh"] = serde_json::to_value(fresh)?;
                evidence["after"] = scenario.prove_link(capture, context, &directory.join("after"))?;
                Ok(())
            })();
            evidence["passed"] = serde_json::json!(result.is_ok());
            evidence["error"] = serde_json::json!(result.as_ref().err().map(ToString::to_string));
            crate::evidence::run::atomic_json(&directory.join("phy-watchdog.json"), &evidence)?;
            result
        })?;
    }
    Ok(())
}

fn wait_phase(
    capture: &SerialCapture,
    phase: Phase,
) -> Result<open_esp_radio_hil_protocol::PhyFaultEvidence> {
    let started = Instant::now();
    loop {
        let observed = capture.phy_fault(Control::Status)?;
        if observed.phase == phase {
            return Ok(observed);
        }
        if started.elapsed() >= Duration::from_secs(2) {
            return Err(format!("PHY checkpoint {phase:?} not reached: {observed:?}").into());
        }
        oer_process::sleep(Duration::from_millis(10))?;
    }
}
