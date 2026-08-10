//! Hierarchical semantic projection of the complete baseband parent.

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BasebandChildPhase {
    TxDc,
    PowerDetector,
    TxCap,
    Temperature,
    TxPower,
    TxDcPwdet,
    Dcode,
    TxIq,
    TxCfr,
    BluetoothTxGain,
    PbusMemory,
    RxIq,
    RxSaturation,
    RxGain,
    Channel,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BasebandInitEvent {
    Mmio(open_esp_radio_esp32s31_phy::phy_bb::PhyBbMmioAction),
    Child(BasebandChildPhase),
    Complete { calibration_performed: bool },
}

fn child_phase(
    action: open_esp_radio_esp32s31_phy::phy_bb::PhyBbInitAction,
) -> Option<BasebandChildPhase> {
    use open_esp_radio_esp32s31_phy::phy_bb::PhyBbInitAction as Action;
    Some(match action {
        Action::Mmio(_) => return None,
        Action::TxDc(_) => BasebandChildPhase::TxDc,
        Action::Pwdet(_) => BasebandChildPhase::PowerDetector,
        Action::TxCap(_) => BasebandChildPhase::TxCap,
        Action::Temperature(_) => BasebandChildPhase::Temperature,
        Action::TxPower(_) => BasebandChildPhase::TxPower,
        Action::TxDcPwdet(_) => BasebandChildPhase::TxDcPwdet,
        Action::Dcode(_) => BasebandChildPhase::Dcode,
        Action::TxIq(_) => BasebandChildPhase::TxIq,
        Action::TxCfr(_) => BasebandChildPhase::TxCfr,
        Action::BluetoothTxGain(_) => BasebandChildPhase::BluetoothTxGain,
        Action::PbusMemory(_) => BasebandChildPhase::PbusMemory,
        Action::RxIq(_) => BasebandChildPhase::RxIq,
        Action::RxSaturation(_) => BasebandChildPhase::RxSaturation,
        Action::RxGain(_) => BasebandChildPhase::RxGain,
        Action::Channel(_) => BasebandChildPhase::Channel,
    })
}

pub fn rust_baseband_init_events(
    state: PhyState,
    channel_or_frequency: u16,
) -> Result<(Vec<BasebandInitEvent>, PhyState)> {
    use open_esp_radio_esp32s31_phy::phy_bb::{
        PhyBbInitAction, PhyBbInitLocalStep, PhyBbInitTransition,
    };

    let mut transition = PhyBbInitTransition::new_on_channel(state, channel_or_frequency);
    let mut completion_driver = DeterministicPhyCompletion::default();
    let mut events = Vec::new();
    for _ in 0..10_000_000 {
        match transition
            .step_local()
            .map_err(|error| format!("Rust baseband parent failed locally: {error:?}"))?
        {
            PhyBbInitLocalStep::StateAdvanced => continue,
            PhyBbInitLocalStep::External(action) => {
                match action {
                    PhyBbInitAction::Mmio(action) => events.push(BasebandInitEvent::Mmio(action)),
                    action => {
                        let phase = child_phase(action).expect("non-MMIO action has a child phase");
                        if events.last() != Some(&BasebandInitEvent::Child(phase)) {
                            events.push(BasebandInitEvent::Child(phase));
                        }
                    }
                }
                let completion = completion_driver.baseband(action)?;
                transition.advance_external(completion).map_err(|error| {
                    format!("Rust baseband parent rejected completion: {error:?}")
                })?;
            }
            PhyBbInitLocalStep::Complete(outcome) => {
                events.push(BasebandInitEvent::Complete {
                    calibration_performed: outcome.calibration_performed,
                });
                return Ok((events, transition.into_state()));
            }
            PhyBbInitLocalStep::Failed(failure) => {
                return Err(format!("Rust baseband parent failed: {failure:?}").into());
            }
        }
    }
    Err("Rust baseband parent exceeded its semantic step bound".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_baseband_path_uses_the_shared_completion_environment() {
        let mut state = PhyState::default();
        state.mark_baseband_calibration_complete();
        state.apply_rx_gain_init_outcome(
            open_esp_radio_esp32s31_phy::phy_rx_gain::PhyRxGainInitOutcome {
                dc: Some(
                    open_esp_radio_esp32s31_phy::phy_rx_gain_cal::PhyRxGainDcOutcome {
                        wifi_index_dc: [[0; 2]; 8],
                        wifi_dc_base: [0; 2],
                        shared_index_dc: [[0; 2]; 11],
                        rxbb_dc_adjustments: [[0; 2]; 6],
                    },
                ),
                generated_tables: true,
                wifi_last_index: 0x4e,
                shared_last_index: 0x4e,
            },
        );
        let (events, _) = rust_baseband_init_events(state, 11).unwrap();
        assert_eq!(
            events.last(),
            Some(&BasebandInitEvent::Complete {
                calibration_performed: false,
            })
        );
    }

    #[test]
    fn cold_baseband_path_uses_the_shared_completion_environment() {
        let (events, _) = rust_baseband_init_events(PhyState::default(), 11).unwrap();
        assert_eq!(
            events.last(),
            Some(&BasebandInitEvent::Complete {
                calibration_performed: true,
            })
        );
    }
}
