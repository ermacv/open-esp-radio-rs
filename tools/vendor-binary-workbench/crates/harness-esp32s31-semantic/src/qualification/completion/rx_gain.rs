//! Deterministic completion methods for this PHY subsystem.

use super::*;

impl DeterministicPhyCompletion {
    pub(super) fn rx_gain(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_rx_gain::PhyRxGainInitAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_rx_gain::PhyRxGainInitCompletion> {
        use open_esp_radio_esp32s31_phy::phy_rx_gain::{
            PhyRxGainInitAction as Action, PhyRxGainInitCompletion as Completion,
        };
        Ok(match action {
            Action::CaptureAndClearDcControl => Completion::DcControlCleared { saved_field: 0 },
            Action::Dc(action) => Completion::Dc(self.rx_gain_dc(action)?),
            Action::RestoreDcControl { saved_field } => {
                Completion::DcControlRestored { saved_field }
            }
            Action::Publish(action) => Completion::Publish(self.rx_gain_publish(action)?),
            Action::ConfigureLimits { wifi_last_index } => {
                Completion::LimitsConfigured { wifi_last_index }
            }
            Action::EnableIqCorrection => Completion::IqCorrectionEnabled,
            unsupported => {
                return Err(format!(
                    "deterministic PHY environment does not yet model RX-gain action {unsupported:?}"
                )
                .into());
            }
        })
    }

    fn rx_dc_minimum(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_rx_dco::PhyRxDcMinimumAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_rx_dco::PhyRxDcMinimumCompletion> {
        use open_esp_radio_esp32s31_phy::phy_rx_dco::{
            PhyRxDcMinimumAction as Action, PhyRxDcMinimumCompletion as Completion,
        };
        Ok(match action {
            Action::DcIq(action) => Completion::DcIq(self.dc_iq(action)?),
            terminal => {
                return Err(format!(
                    "deterministic PHY environment received terminal RX-DC minimum action {terminal:?}"
                )
                .into());
            }
        })
    }

    fn rx_dc_calibration(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_rx_gain_cal::PhyRxDcCalibrationAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_rx_gain_cal::PhyRxDcCalibrationCompletion> {
        use open_esp_radio_esp32s31_phy::phy_rx_gain_cal::{
            PhyRxDcCalibrationAction as Action, PhyRxDcCalibrationCompletion as Completion,
        };
        Ok(match action {
            Action::MaskControl => Completion::ControlMasked { saved_field: 0 },
            Action::ReadPbus { selector, path } => Completion::PbusRead {
                selector,
                path,
                value: 0,
            },
            Action::ForcePbus(transaction) => Completion::PbusForceCompleted(transaction),
            Action::DelayMicros {
                measurement,
                micros,
            } => Completion::DelayElapsed {
                measurement,
                micros,
            },
            Action::Minimum(action) => Completion::Minimum(self.rx_dc_minimum(action)?),
            Action::RestoreControl { saved_field } => Completion::ControlRestored { saved_field },
            terminal => {
                return Err(format!(
                    "deterministic PHY environment received terminal RX-DC calibration action {terminal:?}"
                )
                .into());
            }
        })
    }

    fn rx_gain_dc(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_rx_gain_cal::PhyRxGainDcAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_rx_gain_cal::PhyRxGainDcCompletion> {
        use open_esp_radio_esp32s31_phy::phy_rx_gain_cal::{
            PhyRxGainDcAction as Action, PhyRxGainDcCompletion as Completion,
        };
        Ok(match action {
            Action::ConfigureRegisters { enabled } => Completion::RegistersConfigured { enabled },
            Action::Rfpll(action) => Completion::Rfpll(self.rfpll(action)),
            Action::ConfigurePbusDebugMode => Completion::PbusDebugModeConfigured,
            Action::ForcePbus { bank, transaction } => {
                Completion::PbusCompleted { bank, transaction }
            }
            Action::ConfigureClock { clock, enabled } => {
                Completion::ClockConfigured { clock, enabled }
            }
            Action::ReadPbus { selector, path } => Completion::PbusRead {
                selector,
                path,
                value: 0,
            },
            Action::I2c(action) => Completion::I2c(self.masked_i2c_write(action)?),
            Action::Calibration(action) => Completion::Calibration(self.rx_dc_calibration(action)?),
            Action::Minimum(action) => Completion::Minimum(self.rx_dc_minimum(action)?),
            Action::ConfigurePbusWorkMode => Completion::PbusWorkModeConfigured {
                settle_required: false,
            },
            Action::DelayMicros { phase, micros } => Completion::DelayElapsed { phase, micros },
            Action::ConfigurePbusWorkModePulse => Completion::PbusWorkModePulseConfigured,
            Action::ClearPbusWorkModePulse => Completion::PbusWorkModePulseCleared,
            terminal => {
                return Err(format!(
                    "deterministic PHY environment received terminal RX-gain DC action {terminal:?}"
                )
                .into());
            }
        })
    }

    fn rx_gain_publish(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_rx_gain::PhyRxGainPublishAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_rx_gain::PhyRxGainPublishCompletion> {
        use open_esp_radio_esp32s31_phy::phy_rx_gain::{
            PhyRxGainPublishAction as Action, PhyRxGainPublishCompletion as Completion,
        };
        Ok(match action {
            Action::ConfigurePbusDebugMode { bank } => Completion::PbusDebugModeConfigured { bank },
            Action::ForcePbus { bank, transaction } => {
                Completion::PbusCompleted { bank, transaction }
            }
            Action::ConfigureClock {
                bank,
                clock,
                enabled,
            } => Completion::ClockConfigured {
                bank,
                clock,
                enabled,
            },
            Action::ProgramEntry { bank, entry } => Completion::EntryProgrammed { bank, entry },
            Action::ConfigurePbusWorkMode { bank } => Completion::PbusWorkModeConfigured {
                bank,
                settle_required: false,
            },
            Action::DelayMicros { phase, micros } => Completion::DelayElapsed { phase, micros },
            Action::ConfigurePbusWorkModePulse { bank } => {
                Completion::PbusWorkModePulseConfigured { bank }
            }
            Action::ClearPbusWorkModePulse { bank } => {
                Completion::PbusWorkModePulseCleared { bank }
            }
            terminal => {
                return Err(format!(
                    "deterministic PHY environment received terminal RX-gain publication action {terminal:?}"
                )
                .into());
            }
        })
    }
}
