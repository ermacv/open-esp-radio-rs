//! Peripheral link and idle-maintenance obligations for PHY fault tests.
use super::*;
use hil_core::workload::phy::fault_lifecycle::{self, Scenario};
use open_esp_radio_hil_protocol::{PhyFaultCommand, PhyFaultEvidence, PhyFaultMode};

pub fn run(output: &Path, context: &Context<'_>) -> Result<()> {
    fault_lifecycle::run(output, context, Peripheral)
}
struct Peripheral;
impl Scenario for Peripheral {
    const RADIO: &'static str = "bluetooth";
    fn prove_link(
        &self,
        capture: &SerialCapture,
        context: &Context<'_>,
        directory: &Path,
    ) -> Result<serde_json::Value> {
        std::fs::create_dir_all(directory)?;
        let adapter = context
            .lab
            .bluetooth_adapter
            .ok_or("Bluetooth peer required")?;
        let mut cycles = Vec::new();
        probe_peripheral(
            capture,
            adapter,
            PeripheralConfig {
                encrypted: false,
                key_refresh: false,
                encrypted_maintenance: false,
                boots: 1,
                connections: 1,
                hold_millis: 3000,
                termination: BluetoothPeripheralTermination::PeerReset,
                retire_after: false,
                restart_between_connections: false,
                maintain_between_connections: false,
                calibration_threshold: None,
            },
            directory,
            &mut cycles,
        )?;
        Ok(serde_json::to_value(cycles)?)
    }
    fn normal_maintenance(&self, capture: &SerialCapture) -> Result<serde_json::Value> {
        let maintained = capture
            .bluetooth_peripheral(BluetoothPeripheralOperation::Calibrate { threshold: 0 })?;
        require_calibrated(maintained.result)?;
        Ok(serde_json::to_value(maintained)?)
    }
    fn begin_fault(&self, capture: &SerialCapture, mode: PhyFaultMode) -> Result<PhyFaultEvidence> {
        // The idle peripheral handler acknowledges arming before entering
        // the real forced-calibration transaction.
        capture.phy_fault(PhyFaultCommand::Arm(mode))
    }
}
fn require_calibrated(
    result: open_esp_radio_hil_protocol::BluetoothPeripheralResult,
) -> Result<()> {
    if matches!(
        result,
        open_esp_radio_hil_protocol::BluetoothPeripheralResult::Maintained {
            common_calibrated: true,
            bluetooth_calibrated: true,
            ..
        }
    ) {
        Ok(())
    } else {
        Err("normal BLE control did not complete both calibrations".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normal_control_requires_both_calibrations() {
        for common_calibrated in [false, true] {
            for bluetooth_calibrated in [false, true] {
                let result = open_esp_radio_hil_protocol::BluetoothPeripheralResult::Maintained {
                    cycles: 1,
                    due_tracking_completed: true,
                    tracking_inhibited: false,
                    same_hci_reset_completed: true,
                    common_calibrated,
                    bluetooth_calibrated,
                };
                assert_eq!(
                    require_calibrated(result).is_ok(),
                    common_calibrated && bluetooth_calibrated
                );
            }
        }
    }
}
