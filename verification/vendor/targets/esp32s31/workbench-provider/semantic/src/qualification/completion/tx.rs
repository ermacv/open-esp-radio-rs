//! Deterministic completion methods for this PHY subsystem.

use super::*;

impl DeterministicPhyCompletion {
    fn clear_scratch(&mut self) {
        self.tx_power_events.clear();
        self.txdc_events.clear();
        self.txdc_pwdet_events.clear();
    }

    pub(super) fn rfpll(&mut self, action: RfpllFrequencyAction) -> RfpllFrequencyCompletion {
        self.tx_power_events.clear();
        append_rfpll_action(&mut self.tx_power_events, action)
    }

    pub(super) fn tx_dc(&mut self, action: PhyTxDcAction) -> Result<PhyTxDcCompletion> {
        self.txdc_events.clear();
        bluetooth_txdc_action_completion(&mut self.txdc_events, action)
    }

    pub(super) fn tx_power(&mut self, action: PhyTxPowerAction) -> Result<PhyTxPowerCompletion> {
        self.tx_power_events.clear();
        Ok(match action {
            PhyTxPowerAction::Environment(action) => {
                PhyTxPowerCompletion::Environment(self.tx_environment(action)?)
            }
            PhyTxPowerAction::Rfpll(action) => PhyTxPowerCompletion::Rfpll(self.rfpll(action)),
            PhyTxPowerAction::WriteI2c { address, value } => {
                PhyTxPowerCompletion::I2cWritten { address, value }
            }
            PhyTxPowerAction::ConfigureTone {
                selector,
                attenuation,
                enabled,
            } => PhyTxPowerCompletion::ToneConfigured {
                selector,
                attenuation,
                enabled,
            },
            PhyTxPowerAction::WriteReferenceControl { value } => {
                PhyTxPowerCompletion::ReferenceControlWritten { value }
            }
            PhyTxPowerAction::ToneSar(action) => PhyTxPowerCompletion::ToneSar(
                tone_sar_completion(&mut self.tx_power_events, action),
            ),
            PhyTxPowerAction::Point(action) => {
                PhyTxPowerCompletion::Point(point_completion(&mut self.tx_power_events, action))
            }
            terminal => {
                return Err(format!(
                    "deterministic PHY environment received terminal TX-power action {terminal:?}"
                )
                .into());
            }
        })
    }

    pub(super) fn tx_dc_pwdet(
        &mut self,
        action: PhyTxDcPwdetAction,
    ) -> Result<PhyTxDcPwdetCompletion> {
        self.txdc_pwdet_events.clear();
        bluetooth_txdc_pwdet_action_completion(&mut self.txdc_pwdet_events, action)
    }

    pub(super) fn power_detector(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_pwdet::PhyPwdetAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_pwdet::PhyPwdetCompletion> {
        use open_esp_radio_esp32s31_phy::phy_pwdet::{
            PhyPwdetAction as Action, PhyPwdetCompletion as Completion,
        };
        Ok(match action {
            Action::ConfigurePbusDebugMode => Completion::PbusDebugModeConfigured,
            Action::ForcePbus(transaction) => Completion::PbusCompleted(transaction),
            Action::ConfigureTxClock { enabled } => Completion::TxClockConfigured { enabled },
            Action::ConfigurePowerDetector => Completion::PowerDetectorConfigured,
            Action::ConfigureCalibrationMode => Completion::CalibrationModeConfigured,
            Action::ConfigureTone => Completion::ToneConfigured,
            Action::WriteReferenceControl { value } => {
                Completion::ReferenceControlWritten { value }
            }
            Action::ArmTone {
                measurement_index,
                sample_index,
            } => Completion::ToneArmed {
                measurement_index,
                sample_index,
            },
            Action::DelayMicros { phase, micros } => Completion::DelayElapsed { phase, micros },
            Action::TriggerSar {
                measurement_index,
                sample_index,
            } => Completion::SarTriggered {
                measurement_index,
                sample_index,
            },
            Action::PollSarReady {
                measurement_index,
                sample_index,
            } => Completion::SarReadySampled {
                measurement_index,
                sample_index,
                ready: true,
            },
            Action::ClearToneArm {
                measurement_index,
                sample_index,
            } => Completion::ToneArmCleared {
                measurement_index,
                sample_index,
            },
            Action::ReadSarSample {
                measurement_index,
                sample_index,
            } => Completion::SarSampled {
                measurement_index,
                sample_index,
                value: 0,
            },
            Action::StopTone => Completion::ToneStopped,
            Action::ConfigurePbusWorkMode => Completion::PbusWorkModeConfigured {
                settle_required: false,
            },
            Action::ConfigurePbusWorkModePulse => Completion::PbusWorkModePulseConfigured,
            Action::ClearPbusWorkModePulse => Completion::PbusWorkModePulseCleared,
            terminal => {
                return Err(format!(
                    "deterministic PHY environment received terminal power-detector action {terminal:?}"
                )
                .into());
            }
        })
    }

    fn tx_environment(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_tx_cal::PhyTxCalibrationEnvironmentAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_tx_cal::PhyTxCalibrationEnvironmentCompletion>
    {
        use open_esp_radio_esp32s31_phy::phy_tx_cal::{
            PhyTxCalibrationEnvironmentAction as Action,
            PhyTxCalibrationEnvironmentCompletion as Completion,
        };
        Ok(match action {
            Action::ConfigurePbusDebugMode => Completion::PbusDebugModeConfigured,
            Action::ForcePbus(transaction) => Completion::PbusCompleted(transaction),
            Action::ConfigureTxClock { enabled } => Completion::TxClockConfigured { enabled },
            Action::ConfigurePowerDetector => Completion::PowerDetectorConfigured,
            Action::ConfigureCalibrationMode => Completion::CalibrationModeConfigured,
            Action::StopTone => Completion::ToneStopped,
            Action::ConfigurePbusWorkMode => Completion::PbusWorkModeConfigured {
                settle_required: false,
            },
            Action::DelayMicros { phase, micros } => Completion::DelayElapsed { phase, micros },
            Action::ConfigurePbusWorkModePulse => Completion::PbusWorkModePulseConfigured,
            Action::ClearPbusWorkModePulse => Completion::PbusWorkModePulseCleared,
            terminal => {
                return Err(format!(
                    "deterministic PHY environment received terminal TX environment action {terminal:?}"
                )
                .into());
            }
        })
    }

    pub(super) fn masked_i2c_write(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_i2c::MaskedI2cWriteAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_i2c::MaskedI2cWriteCompletion> {
        use open_esp_radio_esp32s31_phy::phy_i2c::{
            MaskedI2cWriteAction as Action, MaskedI2cWriteCompletion as Completion,
        };
        Ok(match action {
            Action::ReadByte { address } => Completion::I2cReadCompleted { address, value: 0 },
            Action::WriteByte { address, .. } => Completion::I2cWriteCompleted { address },
            Action::Complete => {
                return Err(
                    "deterministic PHY environment received terminal masked-I2C action".into(),
                );
            }
        })
    }

    fn power_attenuation(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_tx_cal::PhyPowerAttenuationAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_tx_cal::PhyPowerAttenuationCompletion> {
        use open_esp_radio_esp32s31_phy::phy_tx_cal::{
            PhyPowerAttenuationAction as Action, PhyPowerAttenuationCompletion as Completion,
        };
        Ok(match action {
            Action::ConfigureTone {
                iteration,
                selector,
                attenuation,
            } => Completion::ToneConfigured {
                iteration,
                selector,
                attenuation,
            },
            Action::ToneSar(action) => {
                self.tx_power_events.clear();
                Completion::ToneSar(tone_sar_completion(&mut self.tx_power_events, action))
            }
            terminal => {
                return Err(format!(
                    "deterministic PHY environment received terminal attenuation action {terminal:?}"
                )
                .into());
            }
        })
    }

    fn tx_cap_search(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_tx_cal::PhyTxCapSearchAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_tx_cal::PhyTxCapSearchCompletion> {
        use open_esp_radio_esp32s31_phy::phy_tx_cal::{
            PhyTxCapSearchAction as Action, PhyTxCapSearchCompletion as Completion,
        };
        Ok(match action {
            Action::ConfigureTone {
                selector,
                attenuation,
                enabled,
            } => Completion::ToneConfigured {
                selector,
                attenuation,
                enabled,
            },
            Action::I2c(action) => Completion::I2c(self.masked_i2c_write(action)?),
            Action::ToneSar(action) => {
                self.tx_power_events.clear();
                Completion::ToneSar(tone_sar_completion(&mut self.tx_power_events, action))
            }
            terminal => {
                return Err(format!(
                    "deterministic PHY environment received terminal TX-cap search action {terminal:?}"
                )
                .into());
            }
        })
    }

    pub(super) fn tx_cap(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_tx_cal::PhyTxCapAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_tx_cal::PhyTxCapCompletion> {
        use open_esp_radio_esp32s31_phy::phy_tx_cal::{
            PhyTxCapAction as Action, PhyTxCapCompletion as Completion,
        };
        Ok(match action {
            Action::Environment(action) => Completion::Environment(self.tx_environment(action)?),
            Action::Rfpll(action) => Completion::Rfpll(self.rfpll(action)),
            Action::I2c(action) => Completion::I2c(self.masked_i2c_write(action)?),
            Action::Attenuation(action) => Completion::Attenuation(self.power_attenuation(action)?),
            Action::Search(action) => Completion::Search(self.tx_cap_search(action)?),
            terminal => {
                return Err(format!(
                    "deterministic PHY environment received terminal TX-cap action {terminal:?}"
                )
                .into());
            }
        })
    }

    pub(super) fn dcode(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_dcode::PhyDcodeAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_dcode::PhyDcodeCompletion> {
        use open_esp_radio_esp32s31_phy::phy_dcode::{
            PhyDcodeAction as Action, PhyDcodeCompletion as Completion,
        };
        Ok(match action {
            Action::Rfpll(action) => Completion::Rfpll(self.rfpll(action)),
            Action::ConfigureNrx { frequency_code } => Completion::NrxConfigured { frequency_code },
            Action::WriteMasked {
                address,
                high_bit,
                low_bit,
                value,
            } => Completion::MaskedWrite {
                address,
                high_bit,
                low_bit,
                value,
            },
            Action::ReadMasked {
                address,
                high_bit,
                low_bit,
            } => Completion::MaskedRead {
                address,
                high_bit,
                low_bit,
                value: 0,
            },
            terminal => {
                return Err(format!(
                    "deterministic PHY environment received terminal D-code action {terminal:?}"
                )
                .into());
            }
        })
    }

    fn tx_iq_linear(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_txiq::PhyTxIqLinearPowerAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_txiq::PhyTxIqLinearPowerCompletion> {
        use open_esp_radio_esp32s31_phy::phy_txiq::{
            PhyTxIqLinearPowerAction as Action, PhyTxIqLinearPowerCompletion as Completion,
        };
        Ok(match action {
            Action::ToneSar(action) => {
                self.tx_power_events.clear();
                Completion::ToneSar(tone_sar_completion(&mut self.tx_power_events, action))
            }
            terminal => {
                return Err(format!(
                    "deterministic PHY environment received terminal TXIQ linear-power action {terminal:?}"
                )
                .into());
            }
        })
    }

    fn tx_iq_mis_power(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_txiq::PhyTxIqMisPowerAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_txiq::PhyTxIqMisPowerCompletion> {
        use open_esp_radio_esp32s31_phy::phy_txiq::{
            PhyTxIqMisPowerAction as Action, PhyTxIqMisPowerCompletion as Completion,
        };
        Ok(match action {
            Action::Configure {
                identity,
                first,
                polarity,
                attenuation,
                selector,
            } => Completion::Configured {
                identity,
                first,
                polarity,
                attenuation,
                selector,
            },
            Action::DelayMicros {
                identity,
                phase,
                micros,
            } => Completion::DelayElapsed {
                identity,
                phase,
                micros,
            },
            Action::LinearPower(action) => Completion::LinearPower(self.tx_iq_linear(action)?),
            terminal => {
                return Err(format!(
                    "deterministic PHY environment received terminal TXIQ mismatch-power action {terminal:?}"
                )
                .into());
            }
        })
    }

    fn tx_iq_cover(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_txiq::PhyTxIqCoverAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_txiq::PhyTxIqCoverCompletion> {
        use open_esp_radio_esp32s31_phy::phy_txiq::{
            PhyTxIqCoverAction as Action, PhyTxIqCoverCompletion as Completion,
        };
        Ok(match action {
            Action::ConfigureCoefficient {
                identity,
                iteration,
                kind,
                value,
            } => Completion::CoefficientConfigured {
                identity,
                iteration,
                kind,
                value,
            },
            Action::MisPower(action) => Completion::MisPower(self.tx_iq_mis_power(action)?),
            terminal => {
                return Err(format!(
                    "deterministic PHY environment received terminal TXIQ cover action {terminal:?}"
                )
                .into());
            }
        })
    }

    fn tx_iq_calibration(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_txiq::PhyTxIqCalibrationAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_txiq::PhyTxIqCalibrationCompletion> {
        use open_esp_radio_esp32s31_phy::phy_txiq::{
            PhyTxIqCalibrationAction as Action, PhyTxIqCalibrationCompletion as Completion,
        };
        Ok(match action {
            Action::ConfigureCorrection { begin } => Completion::CorrectionConfigured { begin },
            Action::ConfigurePbusDebugMode => Completion::PbusDebugModeConfigured,
            Action::Loopback(action) => Completion::Loopback(self.rx_iq_loopback(action)?),
            Action::ForcePbus(transaction) => Completion::PbusCompleted(transaction),
            Action::Environment(action) => Completion::Environment(self.tx_environment(action)?),
            Action::CaptureToneControl => Completion::ToneControlCaptured { value: 0xa5a5_5a5a },
            Action::PowerAttenuation(action) => {
                Completion::PowerAttenuation(self.power_attenuation(action)?)
            }
            Action::Cover(action) => Completion::Cover(self.tx_iq_cover(action)?),
            Action::RestoreToneControl { saved } => Completion::ToneControlRestored { saved },
            terminal => {
                return Err(format!(
                    "deterministic PHY environment received terminal TXIQ calibration action {terminal:?}"
                )
                .into());
            }
        })
    }

    pub(super) fn tx_iq(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_txiq::PhyTxIqInitAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_txiq::PhyTxIqInitCompletion> {
        use open_esp_radio_esp32s31_phy::phy_txiq::{
            PhyTxIqInitAction as Action, PhyTxIqInitCompletion as Completion,
        };
        Ok(match action {
            Action::Rfpll(action) => Completion::Rfpll(self.rfpll(action)),
            Action::WriteI2c { address, value } => Completion::I2cWritten { address, value },
            Action::ReadI2cMasked {
                address,
                high_bit,
                low_bit,
            } => Completion::I2cMaskedRead {
                address,
                high_bit,
                low_bit,
                value: 0,
            },
            Action::Calibration(action) => Completion::Calibration(self.tx_iq_calibration(action)?),
            Action::Temperature(action) => Completion::Temperature(self.temperature(action)?),
            terminal => {
                return Err(format!(
                    "deterministic PHY environment received terminal TXIQ init action {terminal:?}"
                )
                .into());
            }
        })
    }

    pub(super) fn bluetooth_tx_power(
        &mut self,
        action: PhyBluetoothTxPowerAction,
    ) -> Result<PhyBluetoothTxPowerCompletion> {
        self.tx_power_events.clear();
        bluetooth_tx_power_action_completion(&mut self.tx_power_events, action)
    }

    pub(crate) fn bluetooth_tx_gain(
        &mut self,
        action: PhyBluetoothTxGainInitAction,
    ) -> Result<PhyBluetoothTxGainInitCompletion> {
        self.clear_scratch();
        Ok(match action {
            PhyBluetoothTxGainInitAction::Rfpll(action) => {
                PhyBluetoothTxGainInitCompletion::Rfpll(self.rfpll(action))
            }
            PhyBluetoothTxGainInitAction::TxCap(action) => {
                PhyBluetoothTxGainInitCompletion::TxCap(self.tx_power(action)?)
            }
            PhyBluetoothTxGainInitAction::TxDc(action) => {
                PhyBluetoothTxGainInitCompletion::TxDc(self.tx_dc(action)?)
            }
            PhyBluetoothTxGainInitAction::TxPower(action) => {
                PhyBluetoothTxGainInitCompletion::TxPower(self.bluetooth_tx_power(action)?)
            }
            PhyBluetoothTxGainInitAction::TxDcPwdet(action) => {
                PhyBluetoothTxGainInitCompletion::TxDcPwdet(self.tx_dc_pwdet(action)?)
            }
            PhyBluetoothTxGainInitAction::Publish(publication) => {
                PhyBluetoothTxGainInitCompletion::Published(publication)
            }
        })
    }
}
