//! Bluetooth TX DC calibration projection.

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothTxDcEvent {
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
    ConfigureTxClock {
        enabled: bool,
    },
    ConfigureTone {
        enabled: bool,
        selector: u16,
        step: u8,
    },
    DelayMicros(u32),
    ConfigurePbusWorkMode,
    Complete {
        dco: [[u16; 4]; 3],
        calibrated: bool,
    },
}

pub fn vendor_bluetooth_txdc_scenario(
    phy_param: u32,
    phy_functions_pointer: u32,
) -> execution::Scenario {
    let mut scenario = vendor_rf_init_scenario(phy_param, phy_functions_pointer);
    scenario.max_steps = 200_000;
    // ROM phy_txdc_cal polls bit 22 and then samples comparator bits 29/28.
    // Ready with both comparators low drives one deterministic complete path.
    scenario.mmio_initial.insert(0x2010_0418, 1 << 22);
    scenario.mmio_initial.insert(0x2010_0c04, 0);
    scenario
}

fn bluetooth_txdc_projection(
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
                    "vendor BT TXDC state byte {:#010x} is outside persistent ELF memory",
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
    if byte(0x0a5)? & 0x10 == 0 {
        return Err("vendor phy_bt_txdc_cal_new did not publish its completion flag".into());
    }
    Ok(dco)
}

pub fn normalize_vendor_bluetooth_txdc(
    image: &execution::ExecutableImage,
    result: &execution::ExecutionResult,
    phy_param: u32,
) -> Result<Vec<BluetoothTxDcEvent>> {
    let mut events = Vec::new();
    for call in &result.ordered_calls {
        let event = match call.symbol.as_str() {
            "phy_pbus_debugmode" => Some(BluetoothTxDcEvent::ConfigurePbusDebugMode),
            "phy_pbus_rd" => Some(BluetoothTxDcEvent::ReadPbus {
                selector: call.arguments[0] as u8,
                path: call.arguments[1] as u8,
            }),
            "phy_pbus_force_test" => Some(BluetoothTxDcEvent::ForcePbus {
                selector: call.arguments[0] as u8,
                path: call.arguments[1] as u8,
                value: call.arguments[2] as u16,
            }),
            "phy_set_txclk_en" => Some(BluetoothTxDcEvent::ConfigureTxClock {
                enabled: call.arguments[0] != 0,
            }),
            "phy_start_tx_tone_step" => Some(BluetoothTxDcEvent::ConfigureTone {
                enabled: call.arguments[0] != 0,
                selector: call.arguments[1] as u16,
                step: call.arguments[2] as u8,
            }),
            "ets_delay_us" => Some(BluetoothTxDcEvent::DelayMicros(call.arguments[0])),
            "phy_pbus_force_mode" => {
                match call.arguments[0] {
                    // The enclosing phy_pbus_debugmode call already owns this
                    // semantic event; do not count its leaf twice.
                    1 => None,
                    0 => Some(BluetoothTxDcEvent::ConfigurePbusWorkMode),
                    mode => {
                        return Err(format!(
                            "vendor BT TXDC requested unexpected PBus force mode {mode}"
                        )
                        .into());
                    }
                }
            }
            _ => None,
        };
        if let Some(event) = event {
            events.push(event);
        }
    }
    events.push(BluetoothTxDcEvent::Complete {
        dco: bluetooth_txdc_projection(image, result, phy_param)?,
        calibrated: true,
    });
    Ok(events)
}

pub fn rust_bluetooth_txdc_events(
    mut state: PhyState,
) -> Result<(Vec<BluetoothTxDcEvent>, PhyState)> {
    let mut transition: PhyBluetoothTxDcTransition = state.bluetooth_tx_dc_transition();
    let mut events = Vec::new();
    for _ in 0..20_000 {
        let action = transition.action();
        let completion = match action {
            PhyTxDcAction::ConfigurePbusDebugMode => {
                events.push(BluetoothTxDcEvent::ConfigurePbusDebugMode);
                PhyTxDcCompletion::PbusDebugModeConfigured
            }
            PhyTxDcAction::ReadPbus { selector, path } => {
                events.push(BluetoothTxDcEvent::ReadPbus { selector, path });
                PhyTxDcCompletion::PbusRead {
                    selector,
                    path,
                    value: 0,
                }
            }
            PhyTxDcAction::ForcePbus(transaction) => {
                events.push(BluetoothTxDcEvent::ForcePbus {
                    selector: transaction.selector(),
                    path: transaction.path(),
                    value: transaction.value(),
                });
                PhyTxDcCompletion::PbusCompleted(transaction)
            }
            PhyTxDcAction::ConfigureTxClock => {
                events.push(BluetoothTxDcEvent::ConfigureTxClock { enabled: true });
                PhyTxDcCompletion::TxClockConfigured
            }
            PhyTxDcAction::ConfigureTone {
                enabled,
                selector,
                step,
            } => {
                events.push(BluetoothTxDcEvent::ConfigureTone {
                    enabled,
                    selector,
                    step,
                });
                PhyTxDcCompletion::ToneConfigured {
                    enabled,
                    selector,
                    step,
                }
            }
            PhyTxDcAction::DelayMicros { phase, micros } => {
                events.push(BluetoothTxDcEvent::DelayMicros(micros));
                PhyTxDcCompletion::DelayElapsed { phase, micros }
            }
            PhyTxDcAction::TriggerMeasurement {
                gain_index,
                iteration,
            } => PhyTxDcCompletion::MeasurementTriggered {
                gain_index,
                iteration,
            },
            PhyTxDcAction::PollReady {
                gain_index,
                iteration,
            } => PhyTxDcCompletion::ReadySampled {
                gain_index,
                iteration,
                ready: true,
            },
            PhyTxDcAction::ReadComparators {
                gain_index,
                iteration,
            } => PhyTxDcCompletion::ComparatorsRead {
                gain_index,
                iteration,
                comparator_high: [false, false],
            },
            PhyTxDcAction::ClearMeasurement => PhyTxDcCompletion::MeasurementCleared,
            PhyTxDcAction::ConfigurePbusWorkMode => {
                events.push(BluetoothTxDcEvent::ConfigurePbusWorkMode);
                PhyTxDcCompletion::PbusWorkModeConfigured {
                    settle_required: false,
                }
            }
            PhyTxDcAction::ConfigurePbusWorkModePulse => {
                PhyTxDcCompletion::PbusWorkModePulseConfigured
            }
            PhyTxDcAction::ClearPbusWorkModePulse => PhyTxDcCompletion::PbusWorkModePulseCleared,
            PhyTxDcAction::Complete(outcome) => {
                state.apply_bluetooth_tx_dc_outcome(outcome);
                let dco = [outcome.dco[0], outcome.dco[1], outcome.dco[2]];
                events.push(BluetoothTxDcEvent::Complete {
                    dco,
                    calibrated: state.bluetooth_tx_dc_calibrated(),
                });
                return Ok((events, state));
            }
            PhyTxDcAction::Failed(failure) => {
                return Err(format!("Rust BT TXDC transition failed: {failure:?}").into());
            }
        };
        transition
            .advance(completion)
            .map_err(|error| format!("Rust BT TXDC rejected completion: {error:?}"))?;
    }
    Err("Rust BT TXDC transition exceeded its semantic step bound".into())
}
