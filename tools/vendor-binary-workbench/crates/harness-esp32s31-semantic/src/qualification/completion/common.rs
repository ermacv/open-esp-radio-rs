//! Deterministic completion methods for this PHY subsystem.

use super::*;

impl DeterministicPhyCompletion {
    pub(super) fn temperature(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_temperature::PhyTemperatureAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_temperature::PhyTemperatureCompletion> {
        use open_esp_radio_esp32s31_phy::phy_temperature::{
            PhyTemperatureAction as Action, PhyTemperatureCompletion as Completion,
        };
        Ok(match action {
            Action::ReadMasked {
                address,
                high_bit,
                low_bit,
            } => Completion::MaskedRead {
                address,
                high_bit,
                low_bit,
                value: TEMPERATURE_DAC,
            },
            Action::SampleCode => Completion::CodeSampled {
                value: TEMPERATURE_CODE,
            },
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
            terminal => {
                return Err(format!(
                    "deterministic PHY environment received terminal temperature action {terminal:?}"
                )
                .into());
            }
        })
    }

    pub(super) fn tx_cfr(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_bb::PhyTxCfrAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_bb::PhyTxCfrCompletion> {
        use open_esp_radio_esp32s31_phy::phy_bb::{
            PhyTxCfrAction as Action, PhyTxCfrCompletion as Completion,
        };
        Ok(match action {
            Action::ReadStartIndex => Completion::StartIndexRead { base_index: 0 },
            Action::ProgramEntry(entry) => Completion::EntryProgrammed(entry),
            Action::Complete(_) => {
                return Err("deterministic PHY environment received terminal TX-CFR action".into());
            }
        })
    }

    pub(super) fn pbus_memory(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_pbus_memory::PhyPbusMemoryAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_pbus_memory::PhyPbusMemoryCompletion> {
        use open_esp_radio_esp32s31_phy::phy_pbus_memory::{
            PhyPbusMemoryAction as Action, PhyPbusMemoryCompletion as Completion,
        };
        Ok(match action {
            Action::Program(entry) => Completion::Programmed(entry),
            Action::Capture => Completion::Captured { values: [0; 6] },
            Action::Complete(_) => {
                return Err(
                    "deterministic PHY environment received terminal PBus-memory action".into(),
                );
            }
        })
    }

    pub(super) fn rx_saturation(
        &mut self,
        action: open_esp_radio_esp32s31_phy::phy_rx_saturation::PhyRxSaturationAction,
    ) -> Result<open_esp_radio_esp32s31_phy::phy_rx_saturation::PhyRxSaturationCompletion> {
        use open_esp_radio_esp32s31_phy::phy_rx_saturation::{
            PhyRxSaturationAction as Action, PhyRxSaturationCompletion as Completion,
        };
        Ok(match action {
            Action::ConfigureDebugMode => Completion::DebugModeConfigured,
            Action::ForcePbus(transaction) => Completion::PbusCompleted(transaction),
            Action::DelayMicros { micros } => Completion::DelayElapsed { micros },
            Action::SampleStatus { sample_index, .. } => Completion::StatusSampled {
                sample_index,
                active: false,
            },
            Action::ConfigureWorkMode => Completion::WorkModeConfigured,
            Action::Complete(_) => {
                return Err(
                    "deterministic PHY environment received terminal RX-saturation action".into(),
                );
            }
        })
    }
}
