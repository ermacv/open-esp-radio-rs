//! Deterministic completion methods for this PHY subsystem.

use super::*;

impl DeterministicPhyCompletion {
    fn rx_iq_rfpll(&mut self, action: RfpllFrequencyAction) -> Result<RfpllFrequencyCompletion> {
        Ok(match action {
            RfpllFrequencyAction::WriteMasked {
                address,
                high_bit,
                low_bit,
                ..
            } => RfpllFrequencyCompletion::MaskedWrite {
                address,
                high_bit,
                low_bit,
            },
            RfpllFrequencyAction::WriteByte { address, .. } => {
                RfpllFrequencyCompletion::ByteWrite { address }
            }
            RfpllFrequencyAction::ReadMasked {
                address,
                high_bit,
                low_bit,
            } => RfpllFrequencyCompletion::MaskedRead {
                address,
                high_bit,
                low_bit,
                value: u8::from(high_bit == 1),
            },
            RfpllFrequencyAction::ReadByte { address } => {
                let value = if (address.block(), address.register()) == (0x62, 5) {
                    0xc8
                } else if (address.block(), address.register()) == (0x62, 0x0c) {
                    self.rx_iq_cap_status_reads = self.rx_iq_cap_status_reads.wrapping_add(1);
                    if self.rx_iq_cap_status_reads & 1 == 1 {
                        0
                    } else {
                        4
                    }
                } else {
                    4
                };
                RfpllFrequencyCompletion::ByteRead { address, value }
            }
            RfpllFrequencyAction::DelayMicros(micros) => {
                RfpllFrequencyCompletion::DelayElapsed(micros)
            }
            terminal => {
                return Err(format!(
                    "deterministic PHY environment received terminal RXIQ RFPLL action {terminal:?}"
                )
                .into());
            }
        })
    }

    fn rx_iq_estimator(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_rxiq::PhyRxIqEstimatorAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_rxiq::PhyRxIqEstimatorCompletion> {
        use open_esp_radio_esp32s31_phy::phy_rxiq::{
            PhyRxIqEstimatorAction as Action, PhyRxIqEstimatorCompletion as Completion,
            PhyRxIqMismatchSnapshot,
        };
        Ok(match action {
            Action::Configure(request) => Completion::Configured(request),
            Action::SetEnable {
                request,
                phase,
                enabled,
            } => Completion::EnableSet {
                request,
                phase,
                enabled,
            },
            Action::DelayMicros {
                request,
                phase,
                micros,
            } => Completion::DelayElapsed {
                request,
                phase,
                micros,
            },
            Action::AwaitReadinessEdge { request, .. } => Completion::ReadinessObserved {
                request,
                snapshot: PhyDcIqReadinessSnapshot {
                    ready: true,
                    activity: false,
                },
            },
            Action::ReadTotalPower(request) => Completion::TotalPowerRead {
                request,
                value: 0x12_3400,
            },
            Action::ReadMismatch(request) => Completion::MismatchRead {
                request,
                snapshot: PhyRxIqMismatchSnapshot {
                    sum_i: 0,
                    difference_i: 0,
                    difference_q: 0,
                    sum_q: 0,
                },
            },
            terminal => {
                return Err(format!(
                    "deterministic PHY environment received terminal RXIQ estimator action {terminal:?}"
                )
                .into());
            }
        })
    }

    pub(super) fn rx_iq_loopback(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_txiq::PhyTxIqLoopbackAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_txiq::PhyTxIqLoopbackCompletion> {
        use open_esp_radio_esp32s31_phy::{
            phy_i2c::{MaskedI2cWriteAction, MaskedI2cWriteCompletion},
            phy_txiq::{PhyTxIqLoopbackAction as Action, PhyTxIqLoopbackCompletion as Completion},
        };
        Ok(match action {
            Action::I2c(MaskedI2cWriteAction::ReadByte { address }) => {
                Completion::I2c(MaskedI2cWriteCompletion::I2cReadCompleted { address, value: 0 })
            }
            Action::I2c(MaskedI2cWriteAction::WriteByte { address, .. }) => {
                Completion::I2c(MaskedI2cWriteCompletion::I2cWriteCompleted { address })
            }
            Action::ConfigureTxClock { enabled } => Completion::TxClockConfigured { enabled },
            Action::ConfigureRxClock { enabled } => Completion::RxClockConfigured { enabled },
            terminal => {
                return Err(format!(
                    "deterministic PHY environment received terminal RXIQ loopback action {terminal:?}"
                )
                .into());
            }
        })
    }

    pub(super) fn dc_iq(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_dc_iq::PhyDcIqAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_dc_iq::PhyDcIqCompletion> {
        use open_esp_radio_esp32s31_phy::phy_dc_iq::{
            PhyDcIqAction as Action, PhyDcIqCompletion as Completion,
        };
        Ok(match action {
            Action::Configure(request) => Completion::Configured(request),
            Action::SetEnable {
                request,
                phase,
                enabled,
            } => Completion::EnableSet {
                request,
                phase,
                enabled,
            },
            Action::DelayMicros {
                request,
                phase,
                micros,
            } => Completion::DelayElapsed {
                request,
                phase,
                micros,
            },
            Action::AwaitReadinessEdge { request, .. } => Completion::ReadinessObserved {
                request,
                snapshot: PhyDcIqReadinessSnapshot {
                    ready: true,
                    activity: false,
                },
            },
            Action::ReadAccumulators(request) => Completion::AccumulatorsRead {
                request,
                snapshot: PhyDcIqAccumulatorSnapshot {
                    i: 0,
                    q: 0,
                    power: 0,
                },
            },
            terminal => {
                return Err(format!(
                    "deterministic PHY environment received terminal DC/IQ action {terminal:?}"
                )
                .into());
            }
        })
    }

    fn rx_dco(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_rx_dco::PhyRxDcoAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_rx_dco::PhyRxDcoCompletion> {
        use open_esp_radio_esp32s31_phy::phy_rx_dco::{
            PhyRxDcoAction as Action, PhyRxDcoCompletion as Completion,
        };
        Ok(match action {
            Action::MaskRxDcoControl => Completion::RxDcoControlMasked { saved_field: 0 },
            Action::ReadPbus { selector, path } => Completion::PbusRead {
                selector,
                path,
                value: 0,
            },
            Action::ForcePbus(transaction) => Completion::PbusForceCompleted(transaction),
            Action::DelayMicros { iteration, micros } => {
                Completion::DelayElapsed { iteration, micros }
            }
            Action::DcIq(action) => Completion::DcIq(self.dc_iq(action)?),
            Action::RestoreRxDcoControl { saved_field } => {
                Completion::RxDcoControlRestored { saved_field }
            }
            terminal => {
                return Err(format!(
                    "deterministic PHY environment received terminal RX-DCO action {terminal:?}"
                )
                .into());
            }
        })
    }

    fn rx_iq_cover(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_rxiq::PhyRxIqCoverAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_rxiq::PhyRxIqCoverCompletion> {
        use open_esp_radio_esp32s31_phy::phy_rxiq::{
            PhyRxIqCoverAction as Action, PhyRxIqCoverCompletion as Completion,
        };
        Ok(match action {
            Action::ConfigureCoefficient {
                identity,
                iteration,
                kind,
                value,
                final_value,
            } => Completion::CoefficientConfigured {
                identity,
                iteration,
                kind,
                value,
                final_value,
            },
            Action::Estimator(action) => Completion::Estimator(self.rx_iq_estimator(action)?),
            terminal => {
                return Err(format!(
                    "deterministic PHY environment received terminal RXIQ cover action {terminal:?}"
                )
                .into());
            }
        })
    }

    fn rx_iq_rf_calibration(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_rxiq::PhyRxIqRfCalibrationAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_rxiq::PhyRxIqRfCalibrationCompletion> {
        use open_esp_radio_esp32s31_phy::phy_rxiq::{
            PhyRxIqRfCalibrationAction as Action, PhyRxIqRfCalibrationCompletion as Completion,
        };
        Ok(match action {
            Action::ConfigureCalibrationMode => Completion::CalibrationModeConfigured,
            Action::ConfigureTone {
                enabled,
                selector,
                attenuation,
            } => Completion::ToneConfigured {
                enabled,
                selector,
                attenuation,
            },
            Action::Cover(action) => Completion::Cover(self.rx_iq_cover(action)?),
            terminal => {
                return Err(format!(
                    "deterministic PHY environment received terminal RXIQ RF-calibration action {terminal:?}"
                )
                .into());
            }
        })
    }

    fn rx_iq_gain(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_rxiq::PhyRxIqGainAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_rxiq::PhyRxIqGainCompletion> {
        use open_esp_radio_esp32s31_phy::phy_rxiq::{
            PhyRxIqAdjustedTxAction, PhyRxIqAdjustedTxCompletion, PhyRxIqDataAction,
            PhyRxIqDataCompletion, PhyRxIqGainAction as Action,
            PhyRxIqGainCompletion as Completion,
        };
        use open_esp_radio_esp32s31_phy::phy_txiq::PhyTxIqCoefficientKind;
        Ok(match action {
            Action::ForcePbus { pass, transaction } => {
                Completion::PbusCompleted { pass, transaction }
            }
            Action::WriteI2c { address, value } => Completion::I2cWritten { address, value },
            Action::AdjustTx(PhyRxIqAdjustedTxAction::ReadI2cMasked {
                address,
                high_bit,
                low_bit,
            }) => Completion::AdjustTx(PhyRxIqAdjustedTxCompletion::I2cMaskedRead {
                address,
                high_bit,
                low_bit,
                value: 0,
            }),
            Action::ConfigureTxIq { kind, value } => {
                if kind == PhyTxIqCoefficientKind::Phase {
                    self.rx_iq_configured_phase = Some(value);
                }
                Completion::TxIqConfigured { kind, value }
            }
            Action::Dco(action) => Completion::Dco(self.rx_dco(action)?),
            Action::ConfigureTone {
                enabled,
                selector,
                attenuation,
            } => Completion::ToneConfigured {
                enabled,
                selector,
                attenuation,
            },
            Action::Estimator(action) => Completion::Estimator(self.rx_iq_estimator(action)?),
            Action::Data(PhyRxIqDataAction::Calibration(action)) => Completion::Data(
                PhyRxIqDataCompletion::Calibration(self.rx_iq_rf_calibration(action)?),
            ),
            terminal => {
                return Err(format!(
                    "deterministic PHY environment received terminal RXIQ gain action {terminal:?}"
                )
                .into());
            }
        })
    }

    pub(super) fn rx_iq(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_rxiq::PhyRxIqInitAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_rxiq::PhyRxIqInitCompletion> {
        use open_esp_radio_esp32s31_phy::phy_rxiq::{
            PhyRxIqInitAction as Action, PhyRxIqInitCompletion as Completion,
        };
        Ok(match action {
            Action::Rfpll(action) => Completion::Rfpll(self.rx_iq_rfpll(action)?),
            Action::WriteTxCap { address, value } => Completion::TxCapWritten { address, value },
            Action::ConfigureRootStatus => Completion::RootStatusConfigured,
            Action::ConfigurePbusDebugMode => Completion::PbusDebugModeConfigured,
            Action::ForcePbus(transaction) => Completion::PbusCompleted(transaction),
            Action::Loopback(action) => Completion::Loopback(self.rx_iq_loopback(action)?),
            Action::ConfigureCorrection { begin } => Completion::CorrectionConfigured { begin },
            Action::Gain(action) => Completion::Gain(self.rx_iq_gain(action)?),
            Action::ConfigurePbusWorkMode => Completion::PbusWorkModeConfigured {
                settle_required: false,
            },
            Action::DelayMicros { phase, micros } => Completion::DelayElapsed { phase, micros },
            Action::ConfigurePbusWorkModePulse => Completion::PbusWorkModePulseConfigured,
            Action::ClearPbusWorkModePulse => Completion::PbusWorkModePulseCleared,
            terminal => {
                return Err(format!(
                    "deterministic PHY environment received terminal RXIQ action {terminal:?}"
                )
                .into());
            }
        })
    }
}
