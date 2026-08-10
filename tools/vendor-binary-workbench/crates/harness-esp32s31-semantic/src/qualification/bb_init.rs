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

const BASEBAND_INIT_STATE_FOOTPRINT: &[StateFootprintRange] = &[
    state_range(
        0x000,
        2,
        StateAccess::ReadWrite,
        execution::MemoryOwner::MmioDerived,
        "temperature-result",
    ),
    state_range(
        0x002,
        2,
        StateAccess::Read,
        execution::MemoryOwner::Cpu,
        "pbus-path-selectors",
    ),
    state_range(
        0x007,
        2,
        StateAccess::Read,
        execution::MemoryOwner::Cpu,
        "gain-publication-control",
    ),
    state_range(
        0x00e,
        7,
        StateAccess::Read,
        execution::MemoryOwner::Cpu,
        "calibration-controls",
    ),
    state_range(
        0x016,
        1,
        StateAccess::Write,
        execution::MemoryOwner::MmioDerived,
        "temperature-range",
    ),
    state_range(
        0x018,
        1,
        StateAccess::ReadWrite,
        execution::MemoryOwner::MmioDerived,
        "power-attenuation",
    ),
    state_range(
        0x01a,
        4,
        StateAccess::ReadWrite,
        execution::MemoryOwner::MmioDerived,
        "sar-reference-state",
    ),
    state_range(
        0x020,
        2,
        StateAccess::Read,
        execution::MemoryOwner::Cpu,
        "frequency-offset",
    ),
    state_range(
        0x026,
        1,
        StateAccess::Read,
        execution::MemoryOwner::Cpu,
        "channel-14-control",
    ),
    state_range(
        0x028,
        1,
        StateAccess::Read,
        execution::MemoryOwner::Cpu,
        "dot11p-control",
    ),
    state_range(
        0x02b,
        1,
        StateAccess::Read,
        execution::MemoryOwner::Cpu,
        "power-target-adjustment",
    ),
    state_range(
        0x030,
        24,
        StateAccess::Write,
        execution::MemoryOwner::MmioDerived,
        "txdc-calibration-results",
    ),
    state_range(
        0x04f,
        1,
        StateAccess::Read,
        execution::MemoryOwner::Cpu,
        "crystal-selector",
    ),
    state_range(
        0x0a4,
        28,
        StateAccess::ReadWrite,
        execution::MemoryOwner::Cpu,
        "calibration-flags-and-gain-seed",
    ),
    state_range(
        0x0c0,
        16,
        StateAccess::Write,
        execution::MemoryOwner::MmioDerived,
        "generated-gain-prefix",
    ),
    state_range(
        0x0d0,
        2,
        StateAccess::ReadWrite,
        execution::MemoryOwner::Cpu,
        "tx-gain-configuration",
    ),
    state_range(
        0x0d4,
        14,
        StateAccess::ReadWrite,
        execution::MemoryOwner::MmioDerived,
        "tx-gain-and-capacitance-state",
    ),
    state_range(
        0x0e6,
        2,
        StateAccess::ReadWrite,
        execution::MemoryOwner::MmioDerived,
        "tx-calibration-state",
    ),
    state_range(
        0x0f1,
        14,
        StateAccess::ReadWrite,
        execution::MemoryOwner::MmioDerived,
        "tx-gain-and-power-results",
    ),
    state_range(
        0x100,
        2,
        StateAccess::Write,
        execution::MemoryOwner::MmioDerived,
        "tx-power-result-tail",
    ),
    state_range(
        0x104,
        30,
        StateAccess::ReadWrite,
        execution::MemoryOwner::MmioDerived,
        "dco-and-channel-results",
    ),
    state_range(
        0x123,
        2,
        StateAccess::Read,
        execution::MemoryOwner::Cpu,
        "wifi-and-bluetooth-gain-base",
    ),
    state_range(
        0x14e,
        36,
        StateAccess::ReadWrite,
        execution::MemoryOwner::MmioDerived,
        "rxiq-calibration-results",
    ),
    state_range(
        0x190,
        2,
        StateAccess::Write,
        execution::MemoryOwner::Cpu,
        "rx-gain-table-prefix",
    ),
    state_range(
        0x196,
        1,
        StateAccess::Read,
        execution::MemoryOwner::Cpu,
        "rx-gain-control",
    ),
    state_range(
        0x198,
        4,
        StateAccess::Write,
        execution::MemoryOwner::Cpu,
        "rx-gain-table-header",
    ),
    state_range(
        0x1a1,
        2,
        StateAccess::Write,
        execution::MemoryOwner::Cpu,
        "rx-gain-table-indices",
    ),
    state_range(
        0x1a3,
        1,
        StateAccess::ReadWrite,
        execution::MemoryOwner::Cpu,
        "rx-gain-mode",
    ),
    state_range(
        0x1a4,
        6,
        StateAccess::Write,
        execution::MemoryOwner::MmioDerived,
        "rx-gain-calibration-header",
    ),
    state_range(
        0x1aa,
        1,
        StateAccess::ReadWrite,
        execution::MemoryOwner::Cpu,
        "tone-read-policy",
    ),
    state_range(
        0x1ac,
        2,
        StateAccess::ReadWrite,
        execution::MemoryOwner::MmioDerived,
        "calibration-scratch",
    ),
    state_range(
        0x1b4,
        64,
        StateAccess::ReadWrite,
        execution::MemoryOwner::MmioDerived,
        "rx-gain-calibration-tables",
    ),
    state_range(
        0x1f4,
        4,
        StateAccess::Write,
        execution::MemoryOwner::MmioDerived,
        "rx-gain-result-tail",
    ),
];

const fn state_range(
    offset: u32,
    length: u32,
    access: StateAccess,
    owner: execution::MemoryOwner,
    name: &'static str,
) -> StateFootprintRange {
    StateFootprintRange {
        offset,
        length,
        access,
        owner,
        name,
    }
}

/// Deterministic environment for the complete vendor baseband parent.
///
/// Child contracts own their detailed peripheral semantics. This scenario
/// composes the already-reviewed TX calibration environment with the stable
/// temperature/channel observations consumed by the later children.
pub fn vendor_baseband_init_scenario(
    phy_param: u32,
    phy_functions_pointer: u32,
) -> execution::Scenario {
    let mut scenario = vendor_bluetooth_tx_gain_init_scenario(phy_param, phy_functions_pointer);
    scenario.arguments.clear();
    scenario.max_steps = 50_000_000;
    for (address, value) in [
        (0x2010_7030, 0),
        (0x2010_f818, 0),
        (0x2010_f820, 0),
        (0x2010_f800, u32::from(TEMPERATURE_DAC) << 16),
        (0x2081_8000, u32::from(TEMPERATURE_CODE)),
        (0x2010_001c, 0),
        (0x2010_0028, 1 << 8),
        (0x2010_7848, 0),
        (0x2010_4400, 0),
        (0x2010_9c18, 0),
        (0x2010_0874, 0),
        (0x2010_702c, 0),
        (0x2010_70a0, 0),
        (0x2010_f804, u32::from(TX_CAP_READ) << 16),
        (0x2010_0408, 0),
        (0x2010_0844, 0),
        (0x2010_0854, 0),
        (0x2010_0858, 0),
        (0x2010_085c, 0),
        (0x2010_0860, 0),
        (0x2010_0864, 0),
        (0x2010_0868, 0),
        (0x2010_7ce0, 0),
        (0x2010_7ce4, 0),
        (0x2010_703c, 0),
    ] {
        scenario.mmio_initial.insert(address, value);
    }
    // Remaining statically addressed parent/child registers are initialized
    // explicitly as quiescent RMW inputs. Existing readiness observations
    // above and in the composed child scenarios take precedence.
    for address in [
        0x2010_0434,
        0x2010_0424,
        0x2010_0438,
        0x2010_0808,
        0x2010_080c,
        0x2010_0818,
        0x2010_0848,
        0x2010_084c,
        0x2010_0850,
        0x2010_086c,
        0x2010_0870,
        0x2010_0874,
        0x2010_0890,
        0x2010_08bc,
        0x2010_08d0,
        0x2010_0c0c,
        0x2010_448c,
        0x2010_7018,
        0x2010_702c,
        0x2010_7030,
        0x2010_7044,
        0x2010_7048,
        0x2010_705c,
        0x2010_7064,
        0x2010_7094,
        0x2010_7104,
        0x2010_7114,
        0x2010_711c,
        0x2010_7120,
        0x2010_7124,
        0x2010_7128,
        0x2010_713c,
        0x2010_7400,
        0x2010_7428,
        0x2010_743c,
        0x2010_7454,
        0x2010_7458,
        0x2010_745c,
        0x2010_7460,
        0x2010_7808,
        0x2010_7848,
        0x2010_7890,
        0x2010_78a4,
        0x2010_78c8,
        0x2010_78dc,
        0x2010_78e4,
        0x2010_790c,
        0x2010_7980,
        0x2010_7a28,
        0x2010_7c00,
        0x2010_7c30,
        0x2010_7c3c,
        0x2010_7c40,
        0x2010_7c44,
        0x2010_7c50,
        0x2010_7c6c,
        0x2010_7ca8,
        0x2010_7cd0,
        0x2010_7d4c,
        0x2010_8004,
        0x2010_8010,
        0x2010_8018,
        0x2010_801c,
        0x2010_8020,
        0x2010_8028,
        0x2010_802c,
        0x2010_8070,
        0x2010_8078,
        0x2010_9c18,
        0x2070_1068,
    ] {
        scenario.mmio_initial.entry(address).or_insert(0);
    }
    scenario
}

pub fn vendor_baseband_init_state_footprint(
    result: &execution::ExecutionResult,
    phy_param: u32,
) -> Result<StateFootprintStats> {
    validate_state_footprint(
        "esp32s31-baseband-init",
        result,
        phy_param,
        VENDOR_PHY_PARAM_LEN,
        BASEBAND_INIT_STATE_FOOTPRINT,
    )
}

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
