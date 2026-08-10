//! Bluetooth transmit-power calibration projection.

use super::*;

const BLUETOOTH_TX_POWER_STATE_FOOTPRINT: &[StateFootprintRange] = &[
    StateFootprintRange {
        offset: 0x003,
        length: 1,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "bt-tone-selector",
    },
    StateFootprintRange {
        offset: 0x00e,
        length: 3,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "power-offset-and-diagnostic-control",
    },
    StateFootprintRange {
        offset: 0x002,
        length: 1,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "shared-pbus-rx-path",
    },
    StateFootprintRange {
        offset: 0x012,
        length: 3,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "shared-and-bt-pbus-paths",
    },
    StateFootprintRange {
        offset: 0x018,
        length: 1,
        access: StateAccess::ReadWrite,
        owner: execution::MemoryOwner::MmioDerived,
        name: "bt-power-attenuation",
    },
    StateFootprintRange {
        offset: 0x01a,
        length: 4,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "sar-reference-codes",
    },
    StateFootprintRange {
        offset: 0x02b,
        length: 1,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "power-target-adjustment",
    },
    StateFootprintRange {
        offset: 0x04f,
        length: 1,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "crystal-selector",
    },
    StateFootprintRange {
        offset: 0x0a4,
        length: 4,
        access: StateAccess::ReadWrite,
        owner: execution::MemoryOwner::Cpu,
        name: "calibration-flags",
    },
    StateFootprintRange {
        offset: 0x0a8,
        length: 8,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "shared-work-mode-dco",
    },
    StateFootprintRange {
        offset: 0x0dc,
        length: 6,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "tx-capacitance",
    },
    StateFootprintRange {
        offset: 0x0f8,
        length: 10,
        access: StateAccess::ReadWrite,
        owner: execution::MemoryOwner::MmioDerived,
        name: "bt-power-result",
    },
    StateFootprintRange {
        offset: 0x104,
        length: 8,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "bt-dco-row-zero",
    },
    StateFootprintRange {
        offset: 0x1aa,
        length: 1,
        access: StateAccess::ReadWrite,
        owner: execution::MemoryOwner::Cpu,
        name: "tone-read-policy",
    },
];

pub fn vendor_bluetooth_tx_power_state_footprint(
    result: &execution::ExecutionResult,
    phy_param: u32,
) -> Result<StateFootprintStats> {
    validate_state_footprint(
        "esp32s31-bluetooth-tx-power",
        result,
        phy_param,
        VENDOR_PHY_PARAM_LEN,
        BLUETOOTH_TX_POWER_STATE_FOOTPRINT,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothTxPowerI2cEvent {
    Read {
        block: u8,
        register: u8,
    },
    ReadMasked {
        block: u8,
        register: u8,
        high_bit: u8,
        low_bit: u8,
    },
    Write {
        block: u8,
        register: u8,
        value: u8,
    },
    WriteMasked {
        block: u8,
        register: u8,
        high_bit: u8,
        low_bit: u8,
        value: u8,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothTxPowerProjection {
    pub point_corrections: [i8; 3],
    pub power_curve: [i8; 3],
    pub power_adjustment: i8,
    pub attenuation: u8,
    pub current_channel: u16,
    pub calibrated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothTxPowerEvent {
    I2c(BluetoothTxPowerI2cEvent),
    ConfigurePbusDebugMode,
    ReadPbus { selector: u8, path: u8 },
    ForcePbus { selector: u8, path: u8, value: u16 },
    SetTxCap { channel: u16 },
    SetChannel { frequency_index: u8 },
    ConfigureTone { selector: u16, attenuation: u8 },
    StopTone,
    ConfigureTxClock { enabled: bool },
    DelayMicros(u32),
    ConfigurePbusWorkMode,
    Complete(BluetoothTxPowerProjection),
}

pub fn vendor_bluetooth_tx_power_scenario(
    phy_param: u32,
    phy_functions_pointer: u32,
) -> execution::Scenario {
    let mut scenario = vendor_rf_init_scenario(phy_param, phy_functions_pointer);
    scenario.max_steps = 1_000_000;
    scenario.mmio_initial.insert(0x2010_080c, 7 << 14);
    scenario.mmio_initial.insert(0x2010_081c, 0);
    scenario.mmio_initial.insert(0x2010_0820, 0);
    scenario.mmio_initial.insert(0x2010_0824, 0);
    scenario.mmio_initial.insert(0x2010_0828, 0);
    scenario.mmio_initial.insert(0x2010_082c, 0);
    scenario.mmio_initial.insert(0x2010_0c04, 0);
    declare_state_ownership(&mut scenario, phy_param, BLUETOOTH_TX_POWER_STATE_FOOTPRINT);
    scenario
}

fn vendor_bluetooth_tx_power_projection(
    image: &execution::ExecutableImage,
    result: &execution::ExecutionResult,
    phy_param: u32,
) -> Result<BluetoothTxPowerProjection> {
    let byte = |offset: u32| -> Result<u8> {
        result
            .persistent_memory
            .get(&(phy_param + offset))
            .copied()
            .or_else(|| image.loaded_byte(phy_param + offset))
            .ok_or_else(|| {
                format!("vendor BT power state byte {offset:#05x} is unavailable").into()
            })
    };
    Ok(BluetoothTxPowerProjection {
        point_corrections: [byte(0x0f8)? as i8, byte(0x0f9)? as i8, byte(0x0fa)? as i8],
        power_curve: [byte(0x0fb)? as i8, byte(0x0fc)? as i8, byte(0x0fd)? as i8],
        power_adjustment: byte(0x0fe)? as i8,
        attenuation: byte(0x018)?,
        current_channel: u16::from_le_bytes([byte(0x100)?, byte(0x101)?]),
        calibrated: byte(0x0a5)? & 0x80 != 0,
    })
}

pub fn normalize_vendor_bluetooth_tx_power(
    image: &execution::ExecutableImage,
    result: &execution::ExecutionResult,
    phy_param: u32,
) -> Result<Vec<BluetoothTxPowerEvent>> {
    let mut events = Vec::new();
    for call in &result.ordered_calls {
        let arguments = call.arguments;
        let event = match call.symbol.as_str() {
            "phy_i2c_readReg" => Some(BluetoothTxPowerEvent::I2c(BluetoothTxPowerI2cEvent::Read {
                block: arguments[0] as u8,
                register: arguments[2] as u8,
            })),
            "phy_i2c_readReg_Mask" => Some(BluetoothTxPowerEvent::I2c(
                BluetoothTxPowerI2cEvent::ReadMasked {
                    block: arguments[0] as u8,
                    register: arguments[2] as u8,
                    high_bit: arguments[3] as u8,
                    low_bit: arguments[4] as u8,
                },
            )),
            "phy_i2c_writeReg" => Some(BluetoothTxPowerEvent::I2c(
                BluetoothTxPowerI2cEvent::Write {
                    block: arguments[0] as u8,
                    register: arguments[2] as u8,
                    value: arguments[3] as u8,
                },
            )),
            "phy_i2c_writeReg_Mask" => Some(BluetoothTxPowerEvent::I2c(
                BluetoothTxPowerI2cEvent::WriteMasked {
                    block: arguments[0] as u8,
                    register: arguments[2] as u8,
                    high_bit: arguments[3] as u8,
                    low_bit: arguments[4] as u8,
                    value: arguments[5] as u8,
                },
            )),
            "phy_pbus_debugmode" => Some(BluetoothTxPowerEvent::ConfigurePbusDebugMode),
            "phy_pbus_rd" => Some(BluetoothTxPowerEvent::ReadPbus {
                selector: arguments[0] as u8,
                path: arguments[1] as u8,
            }),
            "phy_pbus_force_test" => Some(BluetoothTxPowerEvent::ForcePbus {
                selector: arguments[0] as u8,
                path: arguments[1] as u8,
                value: arguments[2] as u16,
            }),
            "phy_set_txcap_reg" => Some(BluetoothTxPowerEvent::SetTxCap {
                channel: arguments[1] as u16,
            }),
            "phy_set_channel_rfpll_freq" => {
                let channel = arguments[0] as u16;
                let frequency = if channel == 14 {
                    2_484
                } else {
                    2_407 + channel * 5
                };
                Some(BluetoothTxPowerEvent::SetChannel {
                    frequency_index: frequency.wrapping_sub(2_400) as u8,
                })
            }
            "phy_start_tx_tone_step" => Some(BluetoothTxPowerEvent::ConfigureTone {
                selector: arguments[1] as u16,
                attenuation: arguments[2] as u8,
            }),
            "phy_stop_tx_tone" => Some(BluetoothTxPowerEvent::StopTone),
            "phy_set_txclk_en" => Some(BluetoothTxPowerEvent::ConfigureTxClock {
                enabled: arguments[0] != 0,
            }),
            "ets_delay_us" => Some(BluetoothTxPowerEvent::DelayMicros(arguments[0])),
            "phy_pbus_force_mode" if arguments[0] == 0 => {
                Some(BluetoothTxPowerEvent::ConfigurePbusWorkMode)
            }
            _ => None,
        };
        if let Some(event) = event {
            events.push(event);
        }
    }
    events.push(BluetoothTxPowerEvent::Complete(
        vendor_bluetooth_tx_power_projection(image, result, phy_param)?,
    ));
    Ok(events)
}

const fn field_mask(high_bit: u8, low_bit: u8) -> u8 {
    ((((1_u16 << (high_bit - low_bit + 1)) - 1) << low_bit) & 0xff) as u8
}

fn append_i2c_request_events(
    events: &mut Vec<BluetoothTxPowerEvent>,
    request: open_esp_radio_esp32s31_phy::phy_cold::PhyColdI2cRequest,
) -> open_esp_radio_esp32s31_phy::phy_cold::PhyColdI2cOutcome {
    use open_esp_radio_esp32s31_phy::phy_cold::{PhyColdI2cOutcome, PhyColdI2cRequest};
    let address = request.address();
    let block = address.block();
    let register = address.register();
    match request {
        PhyColdI2cRequest::ReadByte { .. } => {
            events.push(BluetoothTxPowerEvent::I2c(BluetoothTxPowerI2cEvent::Read {
                block,
                register,
            }));
            PhyColdI2cOutcome::Read {
                address,
                value: 0x5b,
            }
        }
        PhyColdI2cRequest::ReadMasked {
            high_bit, low_bit, ..
        } => {
            events.push(BluetoothTxPowerEvent::I2c(
                BluetoothTxPowerI2cEvent::ReadMasked {
                    block,
                    register,
                    high_bit,
                    low_bit,
                },
            ));
            PhyColdI2cOutcome::Read {
                address,
                value: (0x5b & field_mask(high_bit, low_bit)) >> low_bit,
            }
        }
        PhyColdI2cRequest::WriteByte { value, .. } => {
            events.push(BluetoothTxPowerEvent::I2c(
                BluetoothTxPowerI2cEvent::Write {
                    block,
                    register,
                    value,
                },
            ));
            PhyColdI2cOutcome::Written { address }
        }
        PhyColdI2cRequest::WriteMasked {
            high_bit,
            low_bit,
            value,
            ..
        } => {
            events.push(BluetoothTxPowerEvent::I2c(
                BluetoothTxPowerI2cEvent::WriteMasked {
                    block,
                    register,
                    high_bit,
                    low_bit,
                    value,
                },
            ));
            PhyColdI2cOutcome::Written { address }
        }
    }
}

fn append_rfpll_action(
    events: &mut Vec<BluetoothTxPowerEvent>,
    action: RfpllFrequencyAction,
) -> RfpllFrequencyCompletion {
    match action {
        RfpllFrequencyAction::StartChannelSwitch {
            frequency_index,
            crystal_selector,
        } => {
            events.push(BluetoothTxPowerEvent::SetChannel { frequency_index });
            RfpllFrequencyCompletion::ChannelSwitchStarted {
                frequency_index,
                crystal_selector,
            }
        }
        RfpllFrequencyAction::ClearChannelSwitch => RfpllFrequencyCompletion::ChannelSwitchCleared,
        RfpllFrequencyAction::ReadChannelReady { .. } => {
            RfpllFrequencyCompletion::ChannelReadyObserved { ready: true }
        }
        RfpllFrequencyAction::ConfigureNrx { frequency_mhz } => {
            RfpllFrequencyCompletion::NrxConfigured { frequency_mhz }
        }
        RfpllFrequencyAction::WriteMasked {
            address,
            high_bit,
            low_bit,
            value: _,
        } => RfpllFrequencyCompletion::MaskedWrite {
            address,
            high_bit,
            low_bit,
        },
        RfpllFrequencyAction::WriteByte { address, value: _ } => {
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
            value: (0x5b & field_mask(high_bit, low_bit)) >> low_bit,
        },
        RfpllFrequencyAction::ReadByte { address } => RfpllFrequencyCompletion::ByteRead {
            address,
            value: 0x5b,
        },
        RfpllFrequencyAction::DelayMicros(micros) => {
            events.push(BluetoothTxPowerEvent::DelayMicros(micros));
            RfpllFrequencyCompletion::DelayElapsed(micros)
        }
        terminal => panic!("semantic executor received terminal RFPLL action {terminal:?}"),
    }
}

fn tone_sar_completion(
    events: &mut Vec<BluetoothTxPowerEvent>,
    action: PhyToneSarAction,
) -> PhyToneSarCompletion {
    match action {
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
            events.push(BluetoothTxPowerEvent::DelayMicros(micros));
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
        terminal => panic!("semantic executor received terminal tone/SAR action {terminal:?}"),
    }
}

fn point_completion(
    events: &mut Vec<BluetoothTxPowerEvent>,
    action: PhyPowerControlPointAction,
) -> PhyPowerControlPointCompletion {
    match action {
        PhyPowerControlPointAction::ConfigureTone {
            identity,
            iteration,
            selector,
            attenuation,
        } => {
            events.push(BluetoothTxPowerEvent::ConfigureTone {
                selector,
                attenuation,
            });
            PhyPowerControlPointCompletion::ToneConfigured {
                identity,
                iteration,
                selector,
                attenuation,
            }
        }
        PhyPowerControlPointAction::ToneSar(action) => {
            PhyPowerControlPointCompletion::ToneSar(tone_sar_completion(events, action))
        }
        PhyPowerControlPointAction::StopTone { identity } => {
            events.push(BluetoothTxPowerEvent::StopTone);
            PhyPowerControlPointCompletion::ToneStopped { identity }
        }
        terminal => panic!("semantic executor received terminal power-point action {terminal:?}"),
    }
}

fn tx_power_completion(
    events: &mut Vec<BluetoothTxPowerEvent>,
    action: PhyTxPowerAction,
) -> PhyTxPowerCompletion {
    match action {
        PhyTxPowerAction::Rfpll(action) => {
            PhyTxPowerCompletion::Rfpll(append_rfpll_action(events, action))
        }
        PhyTxPowerAction::WriteI2c { address, value } => {
            events.push(BluetoothTxPowerEvent::SetTxCap { channel: 6 });
            PhyTxPowerCompletion::I2cWritten { address, value }
        }
        PhyTxPowerAction::Point(action) => {
            PhyTxPowerCompletion::Point(point_completion(events, action))
        }
        unsupported => {
            panic!("BT semantic executor received unsupported TX-power action {unsupported:?}")
        }
    }
}

pub fn rust_bluetooth_tx_power_events(
    mut state: PhyState,
) -> Result<(Vec<BluetoothTxPowerEvent>, PhyState)> {
    let mut transition: PhyBluetoothTxPowerTransition = state.bluetooth_tx_power_transition();
    let mut events = Vec::new();
    for _ in 0..200_000 {
        let action = transition.action();
        let completion = match action {
            PhyBluetoothTxPowerAction::I2c(request) => {
                PhyBluetoothTxPowerCompletion::I2c(append_i2c_request_events(&mut events, request))
            }
            PhyBluetoothTxPowerAction::Prepare(action) => {
                use open_esp_radio_esp32s31_phy::phy_tx_cal::{
                    PhyTxCalibrationEnvironmentAction as Action,
                    PhyTxCalibrationEnvironmentCompletion as Completion,
                };
                let completion = match action {
                    Action::ConfigurePbusDebugMode => {
                        events.push(BluetoothTxPowerEvent::ConfigurePbusDebugMode);
                        Completion::PbusDebugModeConfigured
                    }
                    Action::ForcePbus(transaction) => {
                        events.push(BluetoothTxPowerEvent::ForcePbus {
                            selector: transaction.selector(),
                            path: transaction.path(),
                            value: transaction.value(),
                        });
                        Completion::PbusCompleted(transaction)
                    }
                    Action::ConfigureTxClock { enabled } => {
                        events.push(BluetoothTxPowerEvent::ConfigureTxClock { enabled });
                        Completion::TxClockConfigured { enabled }
                    }
                    Action::ConfigurePowerDetector => Completion::PowerDetectorConfigured,
                    Action::ConfigureCalibrationMode => Completion::CalibrationModeConfigured,
                    unsupported => {
                        return Err(format!(
                            "BT prepare emitted unsupported action {unsupported:?}"
                        )
                        .into());
                    }
                };
                PhyBluetoothTxPowerCompletion::Prepare(completion)
            }
            PhyBluetoothTxPowerAction::ForcePbus(transaction) => {
                events.push(BluetoothTxPowerEvent::ForcePbus {
                    selector: transaction.selector(),
                    path: transaction.path(),
                    value: transaction.value(),
                });
                PhyBluetoothTxPowerCompletion::PbusCompleted(transaction)
            }
            PhyBluetoothTxPowerAction::ReadPbus { selector, path } => {
                events.push(BluetoothTxPowerEvent::ReadPbus { selector, path });
                PhyBluetoothTxPowerCompletion::PbusRead {
                    selector,
                    path,
                    value: 0,
                }
            }
            PhyBluetoothTxPowerAction::Calibration(action) => {
                PhyBluetoothTxPowerCompletion::Calibration(tx_power_completion(&mut events, action))
            }
            PhyBluetoothTxPowerAction::Cleanup(action) => {
                use open_esp_radio_esp32s31_phy::phy_tx_cal::{
                    PhyTxCalibrationEnvironmentAction as Action,
                    PhyTxCalibrationEnvironmentCompletion as Completion,
                };
                let completion = match action {
                    Action::StopTone => {
                        events.push(BluetoothTxPowerEvent::StopTone);
                        Completion::ToneStopped
                    }
                    Action::ConfigureTxClock { enabled } => {
                        events.push(BluetoothTxPowerEvent::ConfigureTxClock { enabled });
                        Completion::TxClockConfigured { enabled }
                    }
                    Action::ForcePbus(transaction) => {
                        events.push(BluetoothTxPowerEvent::ForcePbus {
                            selector: transaction.selector(),
                            path: transaction.path(),
                            value: transaction.value(),
                        });
                        Completion::PbusCompleted(transaction)
                    }
                    Action::ConfigurePbusWorkMode => {
                        events.push(BluetoothTxPowerEvent::ConfigurePbusWorkMode);
                        Completion::PbusWorkModeConfigured {
                            settle_required: false,
                        }
                    }
                    Action::DelayMicros { phase, micros } => {
                        events.push(BluetoothTxPowerEvent::DelayMicros(micros));
                        Completion::DelayElapsed { phase, micros }
                    }
                    Action::ConfigurePbusWorkModePulse => Completion::PbusWorkModePulseConfigured,
                    Action::ClearPbusWorkModePulse => Completion::PbusWorkModePulseCleared,
                    unsupported => {
                        return Err(format!(
                            "BT cleanup emitted unsupported action {unsupported:?}"
                        )
                        .into());
                    }
                };
                PhyBluetoothTxPowerCompletion::Cleanup(completion)
            }
            PhyBluetoothTxPowerAction::Complete(outcome) => {
                let calibration = outcome.calibration;
                state.apply_bluetooth_tx_power_outcome(outcome);
                events.push(BluetoothTxPowerEvent::Complete(
                    BluetoothTxPowerProjection {
                        point_corrections: calibration.point_corrections,
                        power_curve: calibration.power_curve,
                        power_adjustment: calibration.power_adjustment,
                        attenuation: calibration.final_attenuation,
                        current_channel: calibration.current_channel,
                        calibrated: state.bluetooth_tx_power_calibrated(),
                    },
                ));
                return Ok((events, state));
            }
            PhyBluetoothTxPowerAction::Failed(failure) => {
                return Err(format!("Rust BT TX-power transition failed: {failure:?}").into());
            }
        };
        transition.advance(completion).map_err(|error| {
            format!("Rust BT TX-power transition rejected completion: {error:?}")
        })?;
    }
    Err("Rust BT TX-power transition exceeded its semantic step bound".into())
}
