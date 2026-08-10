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
    Parent(BasebandParentPhase),
    Child(BasebandChildPhase),
    Complete { calibration_performed: bool },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BasebandParentPhase {
    EnableInitialization,
    SetMode(open_esp_radio_esp32s31_phy::phy_bb::PhyBbBasebandMode),
    RxTable,
    RfRxSaturation(open_esp_radio_esp32s31_phy::phy_bb::PhyRfRxSaturationPhase),
    RegisterInit,
    UpdateAgc,
    UpdatePostInit,
    EnableAgc,
    SetWifiEnabled(bool),
    ConfigureI2cTxRate,
    ConfigureTxPowerTracking(bool),
}

mod environment;
pub use environment::*;

fn parent_phase(
    action: open_esp_radio_esp32s31_phy::phy_bb::PhyBbMmioAction,
) -> BasebandParentPhase {
    use open_esp_radio_esp32s31_phy::phy_bb::PhyBbMmioAction as Action;
    match action {
        Action::EnableBasebandInitialization => BasebandParentPhase::EnableInitialization,
        Action::SetBasebandMode { mode } => BasebandParentPhase::SetMode(mode),
        Action::UpdateAgcRegisters => BasebandParentPhase::UpdateAgc,
        Action::UpdatePostInitRegisters => BasebandParentPhase::UpdatePostInit,
        Action::EnableAgc => BasebandParentPhase::EnableAgc,
        Action::SetWifiEnabled { enabled } => BasebandParentPhase::SetWifiEnabled(enabled),
        Action::ConfigureTxPowerTracking { enabled } => {
            BasebandParentPhase::ConfigureTxPowerTracking(enabled)
        }
        Action::ConfigureRfRxSaturation { phase } => BasebandParentPhase::RfRxSaturation(phase),
        Action::ConfigureI2cTxRate => BasebandParentPhase::ConfigureI2cTxRate,
        Action::ProgramGainMemory(_) | Action::ConfigureRxTable { .. } => {
            BasebandParentPhase::RxTable
        }
        Action::EnableIqCorrection
        | Action::SetWifiAgcSaturationGain { .. }
        | Action::ConfigureBasebandWatchdog
        | Action::EnableMacBaseband
        | Action::ConfigureNoiseFloorAuto
        | Action::ConfigureAntenna
        | Action::ConfigureBtFilter
        | Action::ConfigurePhyRegisters { .. } => BasebandParentPhase::RegisterInit,
    }
}

fn push_event(events: &mut Vec<BasebandInitEvent>, event: BasebandInitEvent) {
    if events.last() != Some(&event) {
        events.push(event);
    }
}

pub fn normalize_vendor_baseband_init(
    vendor_artifact: &Path,
    result: &execution::ExecutionResult,
) -> Result<Vec<BasebandInitEvent>> {
    use open_esp_radio_esp32s31_phy::phy_bb::{PhyBbBasebandMode, PhyRfRxSaturationPhase};

    let parent = linked_symbol_range(vendor_artifact, "phy_bb_init")?;
    let direct_calls = result
        .ordered_calls
        .iter()
        .filter(|call| parent.contains(&call.site))
        .collect::<Vec<_>>();
    let calibration_performed = direct_calls
        .iter()
        .any(|call| call.symbol == "phy_txdc_cal_init");
    let mut events = vec![
        BasebandInitEvent::Parent(BasebandParentPhase::EnableInitialization),
        BasebandInitEvent::Parent(BasebandParentPhase::SetMode(PhyBbBasebandMode::Calibration)),
    ];
    for call in direct_calls {
        let event = match call.symbol.as_str() {
            "phy_txdc_cal_init" => BasebandInitEvent::Child(BasebandChildPhase::TxDc),
            "phy_pwdet_code_cal" => BasebandInitEvent::Child(BasebandChildPhase::PowerDetector),
            "phy_tx_cap_init" => BasebandInitEvent::Child(BasebandChildPhase::TxCap),
            "phy_tsens_temp_read" => BasebandInitEvent::Child(BasebandChildPhase::Temperature),
            "phy_tx_pwctrl_init" => BasebandInitEvent::Child(BasebandChildPhase::TxPower),
            "phy_txdc_cal_pwdet_init" => {
                if call.arguments[..3] != [1, 0, 0] {
                    return Err(format!(
                        "vendor baseband parent requested unexpected TXDC/PWDET tuple ({}, {}, {})",
                        call.arguments[0], call.arguments[1], call.arguments[2]
                    )
                    .into());
                }
                BasebandInitEvent::Child(BasebandChildPhase::TxDcPwdet)
            }
            "phy_dcode_cal_init" => BasebandInitEvent::Child(BasebandChildPhase::Dcode),
            "phy_txiq_cal_init" => BasebandInitEvent::Child(BasebandChildPhase::TxIq),
            "phy_set_tx_cfr_mem" => {
                if call.arguments[0] != 32 {
                    return Err(format!(
                        "vendor baseband parent requested {} TX-CFR entries",
                        call.arguments[0]
                    )
                    .into());
                }
                BasebandInitEvent::Child(BasebandChildPhase::TxCfr)
            }
            "phy_bt_tx_gain_init" => BasebandInitEvent::Child(BasebandChildPhase::BluetoothTxGain),
            "phy_set_pbus_mem" => BasebandInitEvent::Child(BasebandChildPhase::PbusMemory),
            "phy_rxiq_cal_init" => BasebandInitEvent::Child(BasebandChildPhase::RxIq),
            "phy_rx_table_init" => BasebandInitEvent::Parent(BasebandParentPhase::RxTable),
            "phy_rfrx_sat_rst" => BasebandInitEvent::Parent(BasebandParentPhase::RfRxSaturation(
                if call.arguments[0] == 0 {
                    PhyRfRxSaturationPhase::PrepareCheck
                } else if call.arguments[0] == 1 {
                    PhyRfRxSaturationPhase::Finalize
                } else {
                    return Err(format!(
                        "vendor baseband parent requested invalid RX-saturation phase {}",
                        call.arguments[0]
                    )
                    .into());
                },
            )),
            "phy_check_rx_sat" => BasebandInitEvent::Child(BasebandChildPhase::RxSaturation),
            "phy_set_rx_gain_table" => {
                if call.arguments[..2] != [0x985, 0] {
                    return Err(format!(
                        "vendor baseband parent requested unexpected RX-gain tuple ({:#x}, {})",
                        call.arguments[0], call.arguments[1]
                    )
                    .into());
                }
                BasebandInitEvent::Child(BasebandChildPhase::RxGain)
            }
            "phy_reg_init" => BasebandInitEvent::Parent(BasebandParentPhase::RegisterInit),
            "phy_bb_agc_reg_update" => BasebandInitEvent::Parent(BasebandParentPhase::UpdateAgc),
            "phy_reg_update_new" => BasebandInitEvent::Parent(BasebandParentPhase::UpdatePostInit),
            "phy_enable_agc" => BasebandInitEvent::Parent(BasebandParentPhase::EnableAgc),
            "phy_chip_set_chan" => {
                if call.arguments[..2] != [11, 0] {
                    return Err(format!(
                        "vendor baseband parent selected unexpected channel tuple ({}, {})",
                        call.arguments[0], call.arguments[1]
                    )
                    .into());
                }
                push_event(
                    &mut events,
                    BasebandInitEvent::Child(BasebandChildPhase::Channel),
                );
                BasebandInitEvent::Parent(BasebandParentPhase::SetMode(PhyBbBasebandMode::Idle))
            }
            "phy_wifi_enable_set" => BasebandInitEvent::Parent(
                BasebandParentPhase::SetWifiEnabled(call.arguments[0] != 0),
            ),
            "phy_i2c_txrate_init" => {
                BasebandInitEvent::Parent(BasebandParentPhase::ConfigureI2cTxRate)
            }
            "phy_bb_txpwr_track" => BasebandInitEvent::Parent(
                BasebandParentPhase::ConfigureTxPowerTracking(call.arguments[0] != 0),
            ),
            symbol => {
                return Err(format!(
                    "vendor baseband parent emitted unreviewed direct call {symbol} at {:#010x}",
                    call.site
                )
                .into());
            }
        };
        push_event(&mut events, event);
    }
    events.push(BasebandInitEvent::Complete {
        calibration_performed,
    });
    Ok(events)
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
                    PhyBbInitAction::Mmio(action) => {
                        let phase = BasebandInitEvent::Parent(parent_phase(action));
                        push_event(&mut events, phase);
                    }
                    action => {
                        let phase = child_phase(action).expect("non-MMIO action has a child phase");
                        push_event(&mut events, BasebandInitEvent::Child(phase));
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
