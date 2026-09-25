//! Connected station obligations for PHY fault tests.
use crate::{
    Result,
    context::Context,
    scenario::PhyExpectation,
    session::SerialCapture,
    workload::phy::fault_lifecycle::{self, Scenario},
};
use open_esp_radio_hil_protocol::{PhyFaultCommand, PhyFaultEvidence, PhyFaultMode, PhyFaultPhase};
use std::{path::Path, time::Duration};

pub(crate) fn run(output: &Path, context: &Context<'_>, phy: PhyExpectation) -> Result<()> {
    fault_lifecycle::run(output, context, Station { phy })
}
struct Station {
    phy: PhyExpectation,
}
impl Scenario for Station {
    const RADIO: &'static str = "wifi";
    fn prove_link(
        &self,
        capture: &SerialCapture,
        context: &Context<'_>,
        directory: &Path,
    ) -> Result<serde_json::Value> {
        std::fs::create_dir_all(directory)?;
        let phy = self.phy;
        context.lab.station_fixture.require_phy(phy)?;
        let timeout = Duration::from_secs(60);
        capture.prepare_station(context.target(), timeout)?;
        let connected = capture.wait_for_connected_station_link_after(0, timeout)?;
        crate::workload::ieee80211::control::require_station_link(connected, phy)?;
        crate::workload::ieee80211::control::data_path::prove_station_data_path(
            capture,
            context,
            timeout,
            "phy-watchdog",
            connected.event_cursor_after,
        )?;
        Ok(
            serde_json::json!({ "link_generation": connected.generation, "bidirectional_udp": true }),
        )
    }
    fn normal_maintenance(&self, capture: &SerialCapture) -> Result<serde_json::Value> {
        capture.require_invalid_pause_rejected()?;
        let maintained = capture.station_pause_round_trip(
            open_esp_radio_hil_protocol::StationPauseOperation::Calibration,
            Duration::from_secs(3),
        )?;
        require_calibrated(maintained.evidence)?;
        Ok(serde_json::to_value(maintained.evidence)?)
    }
    fn begin_fault(&self, capture: &SerialCapture, mode: PhyFaultMode) -> Result<PhyFaultEvidence> {
        let armed = capture.phy_fault(PhyFaultCommand::Arm(mode))?;
        if armed.phase != PhyFaultPhase::Armed {
            return Err("fault did not arm".into());
        }
        capture.start_fault_calibration()?;
        Ok(armed)
    }
}
fn require_calibrated(evidence: open_esp_radio_hil_protocol::StationPauseEvidence) -> Result<()> {
    if evidence.result == open_esp_radio_hil_protocol::StationPauseResult::Resumed
        && evidence
            .tracking
            .is_some_and(|t| !t.inhibited && t.common_calibrated && t.wifi_calibrated)
    {
        Ok(())
    } else {
        Err("normal Wi-Fi control did not complete both calibrations and restoration".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_esp_radio_hil_protocol::{
        StationPauseEvidence, StationPauseResult, StationPhyTrackingEvidence,
    };
    #[test]
    fn normal_control_requires_real_calibration_and_restoration() {
        let valid = StationPauseEvidence {
            timeline: None,
            result: StationPauseResult::Resumed,
            elapsed_micros: 1,
            timings: None,
            tracking: Some(StationPhyTrackingEvidence {
                inhibited: false,
                common_calibrated: true,
                wifi_calibrated: true,
                bluetooth_ieee802154_calibrated: false,
            }),
        };
        assert!(require_calibrated(valid).is_ok());
        assert!(
            require_calibrated(StationPauseEvidence {
                timeline: None,
                tracking: None,
                ..valid
            })
            .is_err()
        );
        assert!(
            require_calibrated(StationPauseEvidence {
                timeline: None,
                result: StationPauseResult::Busy,
                ..valid
            })
            .is_err()
        );
    }
}
