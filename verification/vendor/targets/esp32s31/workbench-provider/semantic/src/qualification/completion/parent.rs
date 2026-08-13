//! Deterministic completion methods for this PHY subsystem.

use super::*;

impl DeterministicPhyCompletion {
    pub(crate) fn register(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_register::PhyRegisterAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_register::PhyRegisterCompletion> {
        use open_esp_radio_esp32s31_phy::phy_register::{
            PhyRegisterAction as Action, PhyRegisterCompletion as Completion,
            PhyRegisterMmioCompletion,
        };
        Ok(match action {
            Action::Mmio(action) => Completion::Mmio(PhyRegisterMmioCompletion { action }),
            Action::DelayMicros { phase, micros } => Completion::DelayElapsed { phase, micros },
            Action::SampleI2cMasterReset { index, sample } => Completion::I2cMasterResetSampled {
                index,
                sample,
                busy: false,
            },
            Action::Rf(action) => {
                let binding = PhyColdExternalBinding::lower(action).map_err(|error| {
                    format!("cannot lower register RF action {action:?}: {error:?}")
                })?;
                Completion::Rf(complete_rf_init_external(binding)?)
            }
            Action::Baseband(action) => Completion::Baseband(self.baseband(action)?),
            Action::Temperature(action) => Completion::Temperature(self.temperature(action)?),
            Action::ReadFinalI2c { address } => Completion::FinalI2cRead { address, value: 0 },
        })
    }

    pub(super) fn channel(
        &mut self,
        action: PhyChipChannelAction,
    ) -> Result<PhyChipChannelCompletion> {
        Ok(match action {
            PhyChipChannelAction::SetAgc { enabled } => {
                PhyChipChannelCompletion::AgcSet { enabled }
            }
            PhyChipChannelAction::SetBbpllCalibration { enabled } => {
                PhyChipChannelCompletion::BbpllCalibrationSet { enabled }
            }
            PhyChipChannelAction::Temperature(action) => {
                PhyChipChannelCompletion::Temperature(self.temperature(action)?)
            }
            PhyChipChannelAction::StartFrequencySwitch {
                frequency_index,
                crystal_selector,
            } => PhyChipChannelCompletion::FrequencySwitchStarted {
                frequency_index,
                crystal_selector,
            },
            PhyChipChannelAction::DelayMicros { phase, micros } => {
                PhyChipChannelCompletion::DelayElapsed { phase, micros }
            }
            PhyChipChannelAction::ClearFrequencySwitch => {
                PhyChipChannelCompletion::FrequencySwitchCleared
            }
            PhyChipChannelAction::AwaitFrequencyReadyEdge { .. } => {
                PhyChipChannelCompletion::FrequencyReadyObserved { ready: true }
            }
            PhyChipChannelAction::ConfigureNrx { frequency_mhz } => {
                PhyChipChannelCompletion::NrxConfigured { frequency_mhz }
            }
            PhyChipChannelAction::ConfigureBssCbw { cbw } => {
                PhyChipChannelCompletion::BssCbwConfigured { cbw }
            }
            PhyChipChannelAction::ConfigureRxCompensation => {
                PhyChipChannelCompletion::RxCompensationConfigured
            }
            PhyChipChannelAction::WriteI2c {
                phase,
                address,
                value,
            } => PhyChipChannelCompletion::I2cWriteCompleted {
                phase,
                address,
                value,
            },
            PhyChipChannelAction::CalculateTxGain(request) => {
                PhyChipChannelCompletion::TxGainCalculated {
                    request,
                    image: calculate_wifi_tx_gain(request),
                }
            }
            PhyChipChannelAction::PublishTxGain(_) => PhyChipChannelCompletion::TxGainPublished,
            PhyChipChannelAction::ReadI2c { phase, address } => {
                PhyChipChannelCompletion::I2cReadCompleted {
                    phase,
                    address,
                    value: TX_CAP_READ,
                }
            }
            PhyChipChannelAction::PublishTxCapCommandMemory { value } => {
                PhyChipChannelCompletion::TxCapCommandMemoryPublished { value }
            }
            PhyChipChannelAction::ConfigureChannelCbw { cbw } => {
                PhyChipChannelCompletion::ChannelCbwConfigured { cbw }
            }
            PhyChipChannelAction::ClearDcMemory => PhyChipChannelCompletion::DcMemoryCleared,
            terminal @ (PhyChipChannelAction::Complete(_) | PhyChipChannelAction::Failed(_)) => {
                return Err(format!(
                    "deterministic PHY environment received terminal channel action {terminal:?}"
                )
                .into());
            }
        })
    }

    #[allow(
        dead_code,
        reason = "the staged baseband parent contract consumes this boundary next"
    )]
    pub(crate) fn baseband(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_bb::PhyBbInitAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_bb::PhyBbInitCompletion> {
        use open_esp_radio_esp32s31_phy::phy_bb::{
            PhyBbInitAction as Action, PhyBbInitCompletion as Completion,
        };
        Ok(match action {
            Action::Mmio(action) => {
                Completion::Mmio(open_esp_radio_esp32s31_phy::phy_bb::PhyBbMmioCompletion {
                    action,
                })
            }
            Action::TxDc(action) => Completion::TxDc(self.tx_dc(action)?),
            Action::Pwdet(action) => Completion::Pwdet(self.power_detector(action)?),
            Action::TxCap(action) => Completion::TxCap(self.tx_cap(action)?),
            Action::Temperature(action) => Completion::Temperature(self.temperature(action)?),
            Action::TxPower(action) => Completion::TxPower(self.tx_power(action)?),
            Action::TxDcPwdet(action) => Completion::TxDcPwdet(self.tx_dc_pwdet(action)?),
            Action::Dcode(action) => Completion::Dcode(self.dcode(action)?),
            Action::TxIq(action) => Completion::TxIq(self.tx_iq(action)?),
            Action::TxCfr(action) => Completion::TxCfr(self.tx_cfr(action)?),
            Action::BluetoothTxGain(action) => {
                Completion::BluetoothTxGain(self.bluetooth_tx_gain(action)?)
            }
            Action::PbusMemory(action) => Completion::PbusMemory(self.pbus_memory(action)?),
            Action::RxIq(action) => Completion::RxIq(self.rx_iq(action)?),
            Action::RxSaturation(action) => Completion::RxSaturation(self.rx_saturation(action)?),
            Action::RxGain(action) => Completion::RxGain(self.rx_gain(action)?),
            Action::Channel(action) => Completion::Channel(self.channel(action)?),
        })
    }
}
