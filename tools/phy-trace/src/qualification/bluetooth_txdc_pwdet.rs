//! Bluetooth TX DC and power-detector calibration projection.

use super::*;

const BLUETOOTH_TXDC_PWDET_STATE_FOOTPRINT: &[StateFootprintRange] = &[
    StateFootprintRange {
        offset: 0x010,
        length: 2,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "diagnostic-control",
    },
    StateFootprintRange {
        offset: 0x014,
        length: 1,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "bt-tx-path",
    },
    StateFootprintRange {
        offset: 0x104,
        length: 24,
        access: StateAccess::ReadWrite,
        owner: execution::MemoryOwner::MmioDerived,
        name: "bt-pwdet-dco-rows",
    },
    StateFootprintRange {
        offset: 0x1aa,
        length: 1,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "tone-read-policy",
    },
];

pub fn vendor_bluetooth_txdc_pwdet_scenario(
    phy_param: u32,
    phy_functions_pointer: u32,
) -> execution::Scenario {
    let mut scenario = vendor_rf_init_scenario(phy_param, phy_functions_pointer);
    scenario.arguments = vec![1, 0, 1];
    scenario.max_steps = 5_000_000;
    // `phy_pwdet_tone_start` observes the three-bit ready code, then
    // `phy_read_sar_dout` consumes all four result words. A zero result drives
    // one deterministic, finite search path for each of the three BT rows.
    scenario.mmio_initial.insert(0x2010_080c, 7 << 14);
    scenario.mmio_initial.insert(0x2010_0814, 0);
    for address in [0x2010_081c, 0x2010_0820, 0x2010_0824, 0x2010_0828] {
        scenario.mmio_initial.insert(address, 0);
    }
    scenario.mmio_initial.insert(0x2010_0c04, 0);
    declare_state_ownership(
        &mut scenario,
        phy_param,
        BLUETOOTH_TXDC_PWDET_STATE_FOOTPRINT,
    );
    scenario
}

pub fn vendor_bluetooth_txdc_pwdet_state_footprint(
    result: &execution::ExecutionResult,
    phy_param: u32,
) -> Result<StateFootprintStats> {
    validate_state_footprint(
        "esp32s31-bluetooth-txdc-pwdet",
        result,
        phy_param,
        open_esp_radio_esp32s31_phy::phy_cold::PHY_COLD_PARAMETER_LEN as u32,
        BLUETOOTH_TXDC_PWDET_STATE_FOOTPRINT,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothTxDcPwdetEvent {
    ConfigureTxClock {
        enabled: bool,
    },
    ConfigurePowerDetector,
    ConfigurePbusDebugMode,
    ReadPbus {
        selector: u8,
        path: u8,
    },
    ForcePbus {
        selector: u8,
        path: u8,
        value: u16,
    },
    ConfigureTone {
        enabled: bool,
        selector: u16,
        attenuation: u8,
    },
    DelayMicros(u32),
    ConfigurePbusWorkMode,
    Complete {
        dco: [[u16; 4]; 3],
    },
}

fn bluetooth_txdc_pwdet_projection(
    image: &execution::ExecutableImage,
    result: &execution::ExecutionResult,
    phy_param: u32,
) -> Result<[[u16; 4]; 3]> {
    let byte = |offset: u32| -> Result<u8> {
        result
            .persistent_memory
            .get(&(phy_param + offset))
            .copied()
            .or_else(|| image.loaded_byte(phy_param + offset))
            .ok_or_else(|| {
                format!(
                    "vendor BT TXDC PWDET state byte {:#010x} is outside persistent ELF memory",
                    phy_param + offset
                )
                .into()
            })
    };
    let mut dco = [[0_u16; 4]; 3];
    for (row, values) in dco.iter_mut().enumerate() {
        for (column, value) in values.iter_mut().enumerate() {
            let offset = 0x104 + row as u32 * 8 + column as u32 * 2;
            *value = u16::from_le_bytes([byte(offset)?, byte(offset + 1)?]);
        }
    }
    Ok(dco)
}

pub fn normalize_vendor_bluetooth_txdc_pwdet(
    image: &execution::ExecutableImage,
    result: &execution::ExecutionResult,
    phy_param: u32,
) -> Result<Vec<BluetoothTxDcPwdetEvent>> {
    let mut events = Vec::new();
    for call in &result.ordered_calls {
        let arguments = call.arguments;
        let event = match call.symbol.as_str() {
            "phy_set_txclk_en" => Some(BluetoothTxDcPwdetEvent::ConfigureTxClock {
                enabled: arguments[0] != 0,
            }),
            "phy_en_pwdet" => Some(BluetoothTxDcPwdetEvent::ConfigurePowerDetector),
            "phy_pbus_debugmode" => Some(BluetoothTxDcPwdetEvent::ConfigurePbusDebugMode),
            "phy_pbus_rd" => Some(BluetoothTxDcPwdetEvent::ReadPbus {
                selector: arguments[0] as u8,
                path: arguments[1] as u8,
            }),
            "phy_pbus_force_test" => Some(BluetoothTxDcPwdetEvent::ForcePbus {
                selector: arguments[0] as u8,
                path: arguments[1] as u8,
                value: arguments[2] as u16,
            }),
            "phy_start_tx_tone_step" | "phy_start_tx_tone_step_new" => {
                Some(BluetoothTxDcPwdetEvent::ConfigureTone {
                    enabled: arguments[0] != 0,
                    selector: arguments[1] as u16,
                    attenuation: arguments[2] as u8,
                })
            }
            "ets_delay_us" => Some(BluetoothTxDcPwdetEvent::DelayMicros(arguments[0])),
            "phy_pbus_force_mode" if arguments[0] == 0 => {
                Some(BluetoothTxDcPwdetEvent::ConfigurePbusWorkMode)
            }
            _ => None,
        };
        if let Some(event) = event {
            events.push(event);
        }
    }
    events.push(BluetoothTxDcPwdetEvent::Complete {
        dco: bluetooth_txdc_pwdet_projection(image, result, phy_param)?,
    });
    Ok(events)
}

fn txdc_pwdet_search_completion(
    events: &mut Vec<BluetoothTxDcPwdetEvent>,
    action: PhyTxDcPwdetSearchAction,
) -> PhyTxDcPwdetSearchCompletion {
    match action {
        PhyTxDcPwdetSearchAction::ForcePbus(transaction) => {
            events.push(BluetoothTxDcPwdetEvent::ForcePbus {
                selector: transaction.selector(),
                path: transaction.path(),
                value: transaction.value(),
            });
            PhyTxDcPwdetSearchCompletion::PbusCompleted(transaction)
        }
        PhyTxDcPwdetSearchAction::DelayMicros {
            identity,
            component,
            measurement,
            micros,
        } => {
            events.push(BluetoothTxDcPwdetEvent::DelayMicros(micros));
            PhyTxDcPwdetSearchCompletion::DelayElapsed {
                identity,
                component,
                measurement,
                micros,
            }
        }
        PhyTxDcPwdetSearchAction::ToneSar(action) => {
            let completion = match action {
                PhyToneSarAction::ArmTone {
                    measurement,
                    sample,
                } => PhyToneSarCompletion::ToneArmed {
                    measurement,
                    sample,
                },
                PhyToneSarAction::DelayMicros {
                    measurement,
                    sample,
                    phase,
                    micros,
                } => {
                    events.push(BluetoothTxDcPwdetEvent::DelayMicros(micros));
                    PhyToneSarCompletion::DelayElapsed {
                        measurement,
                        sample,
                        phase,
                        micros,
                    }
                }
                PhyToneSarAction::TriggerSar {
                    measurement,
                    sample,
                } => PhyToneSarCompletion::SarTriggered {
                    measurement,
                    sample,
                },
                PhyToneSarAction::PollReady {
                    measurement,
                    sample,
                } => PhyToneSarCompletion::ReadySampled {
                    measurement,
                    sample,
                    ready: true,
                },
                PhyToneSarAction::ClearTone {
                    measurement,
                    sample,
                } => PhyToneSarCompletion::ToneCleared {
                    measurement,
                    sample,
                },
                PhyToneSarAction::ReadSar {
                    measurement,
                    sample,
                } => PhyToneSarCompletion::SarRead {
                    measurement,
                    sample,
                    value: 0,
                },
                terminal => panic!("unexpected terminal tone/SAR action {terminal:?}"),
            };
            PhyTxDcPwdetSearchCompletion::ToneSar(completion)
        }
        terminal => panic!("unexpected terminal TXDC PWDET search action {terminal:?}"),
    }
}

pub fn rust_bluetooth_txdc_pwdet_events(
    mut state: PhyColdState,
) -> Result<(Vec<BluetoothTxDcPwdetEvent>, PhyColdState)> {
    let mut transition: PhyBluetoothTxDcPwdetTransition = state.bluetooth_tx_dc_pwdet_transition();
    let mut events = Vec::new();
    for _ in 0..2_000_000 {
        let action = transition.action();
        let completion = match action {
            PhyTxDcPwdetAction::CaptureRegisters => PhyTxDcPwdetCompletion::RegistersCaptured {
                power_table_low: 0,
                power_control_field: 0,
            },
            PhyTxDcPwdetAction::ConfigureTxClock { enabled } => {
                events.push(BluetoothTxDcPwdetEvent::ConfigureTxClock { enabled });
                PhyTxDcPwdetCompletion::TxClockConfigured { enabled }
            }
            PhyTxDcPwdetAction::ConfigurePowerDetector => {
                events.push(BluetoothTxDcPwdetEvent::ConfigurePowerDetector);
                PhyTxDcPwdetCompletion::PowerDetectorConfigured
            }
            PhyTxDcPwdetAction::ConfigurePbusDebugMode => {
                events.push(BluetoothTxDcPwdetEvent::ConfigurePbusDebugMode);
                PhyTxDcPwdetCompletion::PbusDebugModeConfigured
            }
            PhyTxDcPwdetAction::ReadPbus { selector, path } => {
                events.push(BluetoothTxDcPwdetEvent::ReadPbus { selector, path });
                PhyTxDcPwdetCompletion::PbusRead {
                    selector,
                    path,
                    value: 0,
                }
            }
            PhyTxDcPwdetAction::ForcePbus(transaction) => {
                events.push(BluetoothTxDcPwdetEvent::ForcePbus {
                    selector: transaction.selector(),
                    path: transaction.path(),
                    value: transaction.value(),
                });
                PhyTxDcPwdetCompletion::PbusCompleted(transaction)
            }
            PhyTxDcPwdetAction::ConfigureTone {
                enabled,
                selector,
                attenuation,
            } => {
                events.push(BluetoothTxDcPwdetEvent::ConfigureTone {
                    enabled,
                    selector,
                    attenuation,
                });
                PhyTxDcPwdetCompletion::ToneConfigured {
                    enabled,
                    selector,
                    attenuation,
                }
            }
            PhyTxDcPwdetAction::DelayMicros { phase, micros } => {
                events.push(BluetoothTxDcPwdetEvent::DelayMicros(micros));
                PhyTxDcPwdetCompletion::DelayElapsed { phase, micros }
            }
            PhyTxDcPwdetAction::ConfigureSarCalibration => {
                PhyTxDcPwdetCompletion::SarCalibrationConfigured
            }
            PhyTxDcPwdetAction::Search(action) => {
                PhyTxDcPwdetCompletion::Search(txdc_pwdet_search_completion(&mut events, action))
            }
            PhyTxDcPwdetAction::ConfigurePbusWorkMode => {
                events.push(BluetoothTxDcPwdetEvent::ConfigurePbusWorkMode);
                PhyTxDcPwdetCompletion::PbusWorkModeConfigured {
                    settle_required: false,
                }
            }
            PhyTxDcPwdetAction::ConfigurePbusWorkModePulse => {
                PhyTxDcPwdetCompletion::PbusWorkModePulseConfigured
            }
            PhyTxDcPwdetAction::ClearPbusWorkModePulse => {
                PhyTxDcPwdetCompletion::PbusWorkModePulseCleared
            }
            PhyTxDcPwdetAction::RestoreRegisters {
                power_table_low,
                power_control_field,
            } => PhyTxDcPwdetCompletion::RegistersRestored {
                power_table_low,
                power_control_field,
            },
            PhyTxDcPwdetAction::Complete(outcome) => {
                state.apply_bluetooth_tx_dc_pwdet_outcome(outcome);
                events.push(BluetoothTxDcPwdetEvent::Complete { dco: outcome.dco });
                return Ok((events, state));
            }
            PhyTxDcPwdetAction::Failed(failure) => {
                return Err(format!("Rust BT TXDC PWDET transition failed: {failure:?}").into());
            }
        };
        transition
            .advance(completion)
            .map_err(|error| format!("Rust BT TXDC PWDET rejected completion: {error:?}"))?;
    }
    Err("Rust BT TXDC PWDET transition exceeded its semantic step bound".into())
}
