//! Semantic normalization for Rust architectural replacements.
//!
//! These models deliberately live in the validator, not in production PHY
//! code. They relate a pinned vendor call/MMIO timeline to the public actions
//! of a Rust state machine without requiring identical stack layout, polling
//! loops, or instruction structure.

use open_esp_radio_esp32s31_phy::{
    phy_bluetooth::{
        PhyBluetoothTxDcPwdetTransition, PhyBluetoothTxDcTransition, PhyBluetoothTxPowerAction,
        PhyBluetoothTxPowerCompletion, PhyBluetoothTxPowerTransition,
    },
    phy_channel::{
        PhyChipChannelAction, PhyChipChannelCompletion, PhyChipChannelRequest,
        PhyChipChannelTransition, PhyWifiTxGainImage, calculate_wifi_tx_gain,
    },
    phy_cold::{
        PhyColdExternalBinding, PhyColdI2cAction, PhyColdLocalStep, PhyColdObservationRequest,
        PhyColdObservationResult, PhyColdPbusAction, PhyColdPbusHardwareResult, PhyColdState,
        PhyRfColdInit,
    },
    phy_dc_iq::{PhyDcIqAccumulatorSnapshot, PhyDcIqReadinessSnapshot},
    phy_i2c::{PhyRfInitPrefixAction, PhyRfInitPrefixOutcome},
    phy_rfpll::{RfpllFrequencyAction, RfpllFrequencyCompletion},
    phy_signal_power::PhySignalPowerAccumulatorSnapshot,
    phy_temperature::{PhyTemperatureAction, PhyTemperatureCompletion},
    phy_tx_cal::{PhyToneSarAction, PhyToneSarCompletion},
    phy_tx_power::{
        PhyPowerControlPointAction, PhyPowerControlPointCompletion, PhyTxPowerAction,
        PhyTxPowerCompletion,
    },
    phy_txdc::{PhyTxDcAction, PhyTxDcCompletion, PhyTxDcParameters},
    phy_txdc_pwdet::{
        PhyTxDcPwdetAction, PhyTxDcPwdetCompletion, PhyTxDcPwdetSearchAction,
        PhyTxDcPwdetSearchCompletion,
    },
};

use crate::{Result, emulator, seed_ram_word};

const ROM_PHY_FUNCTION_TABLE: u32 = 0x2f07_f944;
const ROM_PHY_FUNCTION_TABLE_POINTER: u32 = 0x2f07_fc3c;
const ROM_PHY_PARAM_POINTER: u32 = 0x2f07_fc40;
const ROM_PHY_FUNCTIONS: [u32; 13] = [
    0x2f82_9f18,
    0x2f82_9f1a,
    0x2f82_9f84,
    0x2f82_9fc0,
    0x2f82_44fe,
    0x2f82_78b0,
    0x2f82_5dc8,
    0x2f82_5ecc,
    0x2f82_5f7c,
    0x2f82_711c,
    0x2f82_7392,
    0x2f82_66da,
    0x2f82_88de,
];

const TEMPERATURE_DAC: u8 = 5;
const TEMPERATURE_CODE: u8 = 0;
const TX_CAP_READ: u8 = 0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateAccess {
    Read,
    Write,
    ReadWrite,
}

impl StateAccess {
    const fn permits(self, requested: Self) -> bool {
        matches!(
            (self, requested),
            (Self::ReadWrite, _) | (Self::Read, Self::Read) | (Self::Write, Self::Write)
        )
    }
}

/// One reviewed part of a vendor state image used by a semantic contract.
///
/// Ranges are deliberately expressed relative to the vendor image. Their
/// names describe why the bytes are allowed to participate in the contract;
/// an access that matches no range fails qualification instead of silently
/// disappearing from the canonical projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StateFootprintRange {
    pub offset: u32,
    pub length: u32,
    pub access: StateAccess,
    pub owner: emulator::MemoryOwner,
    pub name: &'static str,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StateFootprintStats {
    pub read_bytes: usize,
    pub written_bytes: usize,
    pub classified_ranges: usize,
}

fn validate_state_footprint(
    contract: &str,
    result: &emulator::ExecutionResult,
    state_base: u32,
    state_length: u32,
    ranges: &[StateFootprintRange],
) -> Result<StateFootprintStats> {
    let state_end = state_base
        .checked_add(state_length)
        .ok_or("semantic state range overflows RV32")?;
    let mut reads = std::collections::BTreeSet::new();
    let mut writes = std::collections::BTreeSet::new();
    let mut unknown_reads = std::collections::BTreeSet::new();
    let mut unknown_writes = std::collections::BTreeSet::new();

    let mut classify = |address: u32, width: u8, access: StateAccess| {
        for byte in 0..u32::from(width / 8) {
            let address = address.wrapping_add(byte);
            if !(state_base..state_end).contains(&address) {
                continue;
            }
            let offset = address - state_base;
            let permitted = ranges.iter().any(|range| {
                offset
                    .checked_sub(range.offset)
                    .is_some_and(|relative| relative < range.length)
                    && range.access.permits(access)
            });
            match access {
                StateAccess::Read => {
                    reads.insert(offset);
                    if !permitted {
                        unknown_reads.insert(offset);
                    }
                }
                StateAccess::Write => {
                    writes.insert(offset);
                    if !permitted {
                        unknown_writes.insert(offset);
                    }
                }
                StateAccess::ReadWrite => unreachable!("timeline access has one direction"),
            }
        }
    };
    for event in &result.timeline {
        match event {
            emulator::ExecutionTimelineEvent::RamRead { width, address, .. } => {
                classify(*address, *width, StateAccess::Read);
            }
            emulator::ExecutionTimelineEvent::RamWrite { width, address, .. } => {
                classify(*address, *width, StateAccess::Write);
            }
            _ => {}
        }
    }
    if !unknown_reads.is_empty() || !unknown_writes.is_empty() {
        let offsets = |values: &std::collections::BTreeSet<u32>| {
            values
                .iter()
                .map(|offset| format!("{offset:#05x}"))
                .collect::<Vec<_>>()
                .join(",")
        };
        return Err(format!(
            "{contract} accessed unclassified state: reads=[{}] writes=[{}]",
            offsets(&unknown_reads),
            offsets(&unknown_writes)
        )
        .into());
    }
    Ok(StateFootprintStats {
        read_bytes: reads.len(),
        written_bytes: writes.len(),
        classified_ranges: ranges.len(),
    })
}

const RF_INIT_STATE_FOOTPRINT: &[StateFootprintRange] = &[
    StateFootprintRange {
        offset: 0x002,
        length: 1,
        access: StateAccess::Read,
        owner: emulator::MemoryOwner::Cpu,
        name: "pbus-rx-path",
    },
    StateFootprintRange {
        offset: 0x016,
        length: 1,
        access: StateAccess::Read,
        owner: emulator::MemoryOwner::Cpu,
        name: "rf-init-control-016",
    },
    StateFootprintRange {
        offset: 0x04a,
        length: 1,
        access: StateAccess::Write,
        owner: emulator::MemoryOwner::MmioDerived,
        name: "bbpll-register-snapshot",
    },
    StateFootprintRange {
        offset: 0x04f,
        length: 1,
        access: StateAccess::Read,
        owner: emulator::MemoryOwner::Cpu,
        name: "xtal-frequency-code",
    },
    StateFootprintRange {
        offset: 0x0a4,
        length: 4,
        access: StateAccess::ReadWrite,
        owner: emulator::MemoryOwner::Cpu,
        name: "calibration-completion-flags",
    },
    StateFootprintRange {
        offset: 0x0e8,
        length: 9,
        access: StateAccess::ReadWrite,
        owner: emulator::MemoryOwner::MmioDerived,
        name: "filter-dcap-state",
    },
    StateFootprintRange {
        offset: 0x18e,
        length: 1,
        access: StateAccess::ReadWrite,
        owner: emulator::MemoryOwner::MmioDerived,
        name: "rfpll-parameter-18e",
    },
    StateFootprintRange {
        offset: 0x193,
        length: 1,
        access: StateAccess::Read,
        owner: emulator::MemoryOwner::Cpu,
        name: "channel-frequency-override",
    },
    StateFootprintRange {
        offset: 0x19e,
        length: 3,
        access: StateAccess::ReadWrite,
        owner: emulator::MemoryOwner::MmioDerived,
        name: "xtal-duty-result",
    },
    StateFootprintRange {
        offset: 0x1ac,
        length: 2,
        access: StateAccess::Write,
        owner: emulator::MemoryOwner::MmioDerived,
        name: "rf-init-calibration-scratch",
    },
    StateFootprintRange {
        offset: 0x1af,
        length: 1,
        access: StateAccess::Read,
        owner: emulator::MemoryOwner::Cpu,
        name: "front-end-control",
    },
];

const CHANNEL_STATE_FOOTPRINT: &[StateFootprintRange] = &[
    StateFootprintRange {
        offset: 0x000,
        length: 2,
        access: StateAccess::ReadWrite,
        owner: emulator::MemoryOwner::MmioDerived,
        name: "temperature-result",
    },
    StateFootprintRange {
        offset: 0x007,
        length: 2,
        access: StateAccess::Read,
        owner: emulator::MemoryOwner::Cpu,
        name: "tx-gain-publication-control",
    },
    StateFootprintRange {
        offset: 0x016,
        length: 1,
        access: StateAccess::Write,
        owner: emulator::MemoryOwner::MmioDerived,
        name: "temperature-sensor-range",
    },
    StateFootprintRange {
        offset: 0x020,
        length: 2,
        access: StateAccess::Read,
        owner: emulator::MemoryOwner::Cpu,
        name: "frequency-offset",
    },
    StateFootprintRange {
        offset: 0x026,
        length: 1,
        access: StateAccess::Read,
        owner: emulator::MemoryOwner::Cpu,
        name: "channel-14-mic-control",
    },
    StateFootprintRange {
        offset: 0x028,
        length: 2,
        access: StateAccess::Read,
        owner: emulator::MemoryOwner::Cpu,
        name: "dot11p-configuration",
    },
    StateFootprintRange {
        offset: 0x04f,
        length: 1,
        access: StateAccess::Read,
        owner: emulator::MemoryOwner::Cpu,
        name: "crystal-selector",
    },
    StateFootprintRange {
        offset: 0x0a8,
        length: 24,
        access: StateAccess::Read,
        owner: emulator::MemoryOwner::Cpu,
        name: "tx-gain-seed",
    },
    StateFootprintRange {
        offset: 0x0d0,
        length: 2,
        access: StateAccess::Read,
        owner: emulator::MemoryOwner::Cpu,
        name: "tx-gain-configuration",
    },
    StateFootprintRange {
        offset: 0x0dc,
        length: 6,
        access: StateAccess::Read,
        owner: emulator::MemoryOwner::Cpu,
        name: "tx-capacitance",
    },
    StateFootprintRange {
        offset: 0x0f1,
        length: 7,
        access: StateAccess::Read,
        owner: emulator::MemoryOwner::Cpu,
        name: "tx-gain-curve-and-correction",
    },
    StateFootprintRange {
        offset: 0x11c,
        length: 4,
        access: StateAccess::ReadWrite,
        owner: emulator::MemoryOwner::Cpu,
        name: "channel-result",
    },
    StateFootprintRange {
        offset: 0x123,
        length: 1,
        access: StateAccess::Read,
        owner: emulator::MemoryOwner::Cpu,
        name: "tx-gain-base",
    },
];

fn declare_state_ownership(
    scenario: &mut emulator::Scenario,
    state_base: u32,
    ranges: &[StateFootprintRange],
) {
    scenario
        .memory_ownership
        .extend(ranges.iter().map(|range| emulator::MemoryOwnership {
            range: emulator::MemoryRange {
                start: state_base + range.offset,
                length: range.length,
            },
            owner: range.owner,
        }));
}

pub fn vendor_rf_init_state_footprint(
    result: &emulator::ExecutionResult,
    phy_param: u32,
) -> Result<StateFootprintStats> {
    validate_state_footprint(
        "esp32s31-rf-init",
        result,
        phy_param,
        open_esp_radio_esp32s31_phy::phy_cold::PHY_COLD_PARAMETER_LEN as u32,
        RF_INIT_STATE_FOOTPRINT,
    )
}

pub fn vendor_channel_state_footprint(
    result: &emulator::ExecutionResult,
    phy_param: u32,
) -> Result<StateFootprintStats> {
    validate_state_footprint(
        "esp32s31-channel",
        result,
        phy_param,
        open_esp_radio_esp32s31_phy::phy_cold::PHY_COLD_PARAMETER_LEN as u32,
        CHANNEL_STATE_FOOTPRINT,
    )
}

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
) -> emulator::Scenario {
    let mut scenario = vendor_rf_init_scenario(phy_param, phy_functions_pointer);
    scenario.max_steps = 200_000;
    // ROM phy_txdc_cal polls bit 22 and then samples comparator bits 29/28.
    // Ready with both comparators low drives one deterministic complete path.
    scenario.mmio_initial.insert(0x2010_0418, 1 << 22);
    scenario.mmio_initial.insert(0x2010_0c04, 0);
    scenario
}

fn bluetooth_txdc_projection(
    image: &emulator::ExecutableImage,
    result: &emulator::ExecutionResult,
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
    image: &emulator::ExecutableImage,
    result: &emulator::ExecutionResult,
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
    mut state: PhyColdState,
) -> Result<(Vec<BluetoothTxDcEvent>, PhyColdState)> {
    let parameters = PhyTxDcParameters {
        pbus_rx_path_value: state.parameter_image()[0x002],
    };
    let mut transition =
        PhyBluetoothTxDcTransition::new(parameters, state.parameter_image()[0x014]);
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

const BLUETOOTH_TXDC_PWDET_STATE_FOOTPRINT: &[StateFootprintRange] = &[
    StateFootprintRange {
        offset: 0x010,
        length: 2,
        access: StateAccess::Read,
        owner: emulator::MemoryOwner::Cpu,
        name: "diagnostic-control",
    },
    StateFootprintRange {
        offset: 0x014,
        length: 1,
        access: StateAccess::Read,
        owner: emulator::MemoryOwner::Cpu,
        name: "bt-tx-path",
    },
    StateFootprintRange {
        offset: 0x104,
        length: 24,
        access: StateAccess::ReadWrite,
        owner: emulator::MemoryOwner::MmioDerived,
        name: "bt-pwdet-dco-rows",
    },
    StateFootprintRange {
        offset: 0x1aa,
        length: 1,
        access: StateAccess::Read,
        owner: emulator::MemoryOwner::Cpu,
        name: "tone-read-policy",
    },
];

pub fn vendor_bluetooth_txdc_pwdet_scenario(
    phy_param: u32,
    phy_functions_pointer: u32,
) -> emulator::Scenario {
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
    result: &emulator::ExecutionResult,
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
    image: &emulator::ExecutableImage,
    result: &emulator::ExecutionResult,
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
    image: &emulator::ExecutableImage,
    result: &emulator::ExecutionResult,
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

const BLUETOOTH_TX_POWER_STATE_FOOTPRINT: &[StateFootprintRange] = &[
    StateFootprintRange {
        offset: 0x003,
        length: 1,
        access: StateAccess::Read,
        owner: emulator::MemoryOwner::Cpu,
        name: "bt-tone-selector",
    },
    StateFootprintRange {
        offset: 0x00e,
        length: 3,
        access: StateAccess::Read,
        owner: emulator::MemoryOwner::Cpu,
        name: "power-offset-and-diagnostic-control",
    },
    StateFootprintRange {
        offset: 0x002,
        length: 1,
        access: StateAccess::Read,
        owner: emulator::MemoryOwner::Cpu,
        name: "shared-pbus-rx-path",
    },
    StateFootprintRange {
        offset: 0x012,
        length: 3,
        access: StateAccess::Read,
        owner: emulator::MemoryOwner::Cpu,
        name: "shared-and-bt-pbus-paths",
    },
    StateFootprintRange {
        offset: 0x018,
        length: 1,
        access: StateAccess::ReadWrite,
        owner: emulator::MemoryOwner::MmioDerived,
        name: "bt-power-attenuation",
    },
    StateFootprintRange {
        offset: 0x01a,
        length: 4,
        access: StateAccess::Read,
        owner: emulator::MemoryOwner::Cpu,
        name: "sar-reference-codes",
    },
    StateFootprintRange {
        offset: 0x02b,
        length: 1,
        access: StateAccess::Read,
        owner: emulator::MemoryOwner::Cpu,
        name: "power-target-adjustment",
    },
    StateFootprintRange {
        offset: 0x04f,
        length: 1,
        access: StateAccess::Read,
        owner: emulator::MemoryOwner::Cpu,
        name: "crystal-selector",
    },
    StateFootprintRange {
        offset: 0x0a4,
        length: 4,
        access: StateAccess::ReadWrite,
        owner: emulator::MemoryOwner::Cpu,
        name: "calibration-flags",
    },
    StateFootprintRange {
        offset: 0x0a8,
        length: 8,
        access: StateAccess::Read,
        owner: emulator::MemoryOwner::Cpu,
        name: "shared-work-mode-dco",
    },
    StateFootprintRange {
        offset: 0x0dc,
        length: 6,
        access: StateAccess::Read,
        owner: emulator::MemoryOwner::Cpu,
        name: "tx-capacitance",
    },
    StateFootprintRange {
        offset: 0x0f8,
        length: 10,
        access: StateAccess::ReadWrite,
        owner: emulator::MemoryOwner::MmioDerived,
        name: "bt-power-result",
    },
    StateFootprintRange {
        offset: 0x104,
        length: 8,
        access: StateAccess::Read,
        owner: emulator::MemoryOwner::Cpu,
        name: "bt-dco-row-zero",
    },
    StateFootprintRange {
        offset: 0x1aa,
        length: 1,
        access: StateAccess::ReadWrite,
        owner: emulator::MemoryOwner::Cpu,
        name: "tone-read-policy",
    },
];

pub fn vendor_bluetooth_tx_power_state_footprint(
    result: &emulator::ExecutionResult,
    phy_param: u32,
) -> Result<StateFootprintStats> {
    validate_state_footprint(
        "esp32s31-bluetooth-tx-power",
        result,
        phy_param,
        open_esp_radio_esp32s31_phy::phy_cold::PHY_COLD_PARAMETER_LEN as u32,
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
) -> emulator::Scenario {
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
    image: &emulator::ExecutableImage,
    result: &emulator::ExecutionResult,
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
    image: &emulator::ExecutableImage,
    result: &emulator::ExecutionResult,
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
    mut state: PhyColdState,
) -> Result<(Vec<BluetoothTxPowerEvent>, PhyColdState)> {
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
                state.apply_bluetooth_tx_power_outcome(outcome);
                let bytes = state.parameter_image();
                events.push(BluetoothTxPowerEvent::Complete(
                    BluetoothTxPowerProjection {
                        point_corrections: [
                            bytes[0x0f8] as i8,
                            bytes[0x0f9] as i8,
                            bytes[0x0fa] as i8,
                        ],
                        power_curve: [bytes[0x0fb] as i8, bytes[0x0fc] as i8, bytes[0x0fd] as i8],
                        power_adjustment: bytes[0x0fe] as i8,
                        attenuation: bytes[0x018],
                        current_channel: u16::from_le_bytes([bytes[0x100], bytes[0x101]]),
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RfInitPhase {
    ConfigureFeBbClock,
    ConfigureBbpllCalibration,
    EnableBias,
    OpenI2c,
    PostOpenI2cDelay,
    ClearPbus,
    ConfigureI2cClockSelection,
    ConfigureI2cBbpll,
    ConfigureAdcRate,
    ConfigureI2cMasterRegisters,
    ConfigurePowerDetectorRegisters,
    ConfigureFrontEndRegisters,
    ConfigureTemperatureSensorRead,
    ConfigureTxPowerControlBackground,
    ConfigureRcCalibration,
    InitializeRcCalibration,
    ConfigureFilterDcap,
    ReadParameter18e,
    InitializeI2c,
    CalibrateRfpllChargePump,
    ConfigureI2cMasterCommandMemory,
    ReadSar2InitializationState,
    InitializeSar2,
    CalibrateXtalDuty,
    UpdateFrontEndRegisters,
    InitializeChannelFrequency,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RfInitStateProjection {
    pub rc_calibration_complete: bool,
    pub bbpll_register_snapshot: u8,
    pub parameter_18e: u8,
    pub xtal_initial_duty: u8,
    pub xtal_low_candidate: u8,
    pub xtal_high_candidate: u8,
    pub frequency_table_initialized: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RfInitPhaseParameters {
    None,
    Enabled(bool),
    Value(u32),
    I2cRead {
        block: u8,
        host: u8,
        register: u8,
    },
    I2cReadMasked {
        block: u8,
        host: u8,
        register: u8,
        high_bit: u8,
        low_bit: u8,
    },
    RcCalibrationPrestate {
        already_complete: bool,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RfInitEvent {
    Phase {
        phase: RfInitPhase,
        parameters: RfInitPhaseParameters,
    },
    Complete(RfInitStateProjection),
}

const fn rf_phase(phase: RfInitPhase, parameters: RfInitPhaseParameters) -> RfInitEvent {
    RfInitEvent::Phase { phase, parameters }
}

pub fn vendor_rf_init_scenario(phy_param: u32, phy_functions_pointer: u32) -> emulator::Scenario {
    let mut scenario = emulator::Scenario {
        max_steps: 500_000,
        ..emulator::Scenario::default()
    };
    seed_ram_word(&mut scenario, phy_functions_pointer, ROM_PHY_FUNCTION_TABLE);
    seed_ram_word(
        &mut scenario,
        ROM_PHY_FUNCTION_TABLE_POINTER,
        ROM_PHY_FUNCTION_TABLE,
    );
    seed_ram_word(&mut scenario, ROM_PHY_PARAM_POINTER, phy_param);
    for (index, value) in ROM_PHY_FUNCTIONS.into_iter().enumerate() {
        seed_ram_word(
            &mut scenario,
            ROM_PHY_FUNCTION_TABLE + index as u32 * 4,
            value,
        );
    }
    declare_state_ownership(&mut scenario, phy_param, RF_INIT_STATE_FOOTPRINT);

    // One explicit deterministic hardware environment. Host-one PHY-I2C
    // reads expose the stable SDM byte, the frequency switch is ready, and
    // the IQ estimator completes immediately. Every remaining sampled
    // register starts at a declared zero rather than poison.
    for (address, value) in [
        (0x2010_0800, 0),
        (0x2010_7030, 0),
        (0x2010_f818, 0),
        (0x2010_f820, 0),
        (0x2010_f800, 0),
        (0x2081_8000, 0),
        (0x2010_001c, 0),
        (0x2010_0028, 1 << 8),
        (0x2010_7848, 0),
        (0x2010_4400, 0),
        (0x2010_9c18, 0),
        (0x2010_0874, 0),
        (0x2010_702c, 0),
        (0x2010_70a0, 0),
        (0x2010_f804, 0x5b << 16),
        (0x2010_0408, 0),
        (0x2010_0844, 0),
        (0x2010_7ce0, 0),
        (0x2010_7ce4, 0),
        (0x2010_703c, 0),
        (0x2070_401c, 0),
        (0x2070_4184, 0),
        (0x2070_40f0, 0),
        (0x2070_4208, 0),
        (0x2010_d800, 0),
        (0x2010_088c, 0),
        (0x2010_0884, 0),
        (0x2010_0890, 0),
        (0x2010_f824, 0),
        (0x2010_f828, 0),
        (0x2010_f82c, 0),
        (0x2010_0448, 0),
        (0x2010_0808, 0),
        (0x2070_1068, 0),
        (0x2010_0894, 0),
        (0x2010_0c08, 0),
        (0x2010_0444, 0),
        (0x2010_040c, 0),
        (0x2010_0438, 0),
        (0x2010_0c0c, 0),
        (0x2010_086c, 0),
        (0x2010_0c20, 0),
        (0x2081_8018, 0),
        (0x2071_0030, 0),
        (0x2010_080c, 0),
        (0x2010_0434, 0),
        (0x2010_0428, 0),
        (0x2010_041c, 0),
        (0x2010_0420, 0),
        (0x2010_0410, 0),
        (0x2010_044c, 0),
        (0x2010_0450, 0),
        (0x2010_047c, 1 << 16),
        (0x2010_08d0, 0),
        (0x2010_0464, 0),
        (0x2010_0468, 0),
        (0x2010_046c, 0),
        (0x2010_0454, 0),
        (0x2010_0460, 0),
        (0x2010_0458, 0),
        (0x2010_045c, 0),
        (0x2010_0024, 0),
        (0x2010_0030, 0),
    ] {
        scenario.mmio_initial.insert(address, value);
    }
    scenario
}

fn vendor_rf_init_phase(call: &emulator::OrderedCall) -> Result<RfInitPhase> {
    let expect = |argument: usize, value: u32| -> Result<()> {
        if call.arguments[argument] == value {
            Ok(())
        } else {
            Err(format!(
                "vendor {} argument {argument} is {:#x}, expected {value:#x}",
                call.symbol, call.arguments[argument]
            )
            .into())
        }
    };
    let phase = match call.symbol.as_str() {
        "phy_open_fe_bb_clk" => RfInitPhase::ConfigureFeBbClock,
        "phy_bbpll_cal" => {
            expect(0, 1)?;
            RfInitPhase::ConfigureBbpllCalibration
        }
        "phy_bias_reg_set" => {
            expect(0, 1)?;
            RfInitPhase::EnableBias
        }
        "phy_open_i2c_xpd_new" => {
            expect(0, 1)?;
            RfInitPhase::OpenI2c
        }
        "ets_delay_us" => {
            expect(0, 10)?;
            RfInitPhase::PostOpenI2cDelay
        }
        "phy_pbus_clear_reg" => RfInitPhase::ClearPbus,
        "phy_i2c_clk_sel" => {
            expect(0, 8)?;
            RfInitPhase::ConfigureI2cClockSelection
        }
        "phy_i2c_bbpll_set" => {
            expect(0, 1)?;
            RfInitPhase::ConfigureI2cBbpll
        }
        "phy_adc_rate_set" => {
            expect(0, 1)?;
            RfInitPhase::ConfigureAdcRate
        }
        "phy_i2cmst_reg_init" => RfInitPhase::ConfigureI2cMasterRegisters,
        "phy_pwdet_reg_init" => RfInitPhase::ConfigurePowerDetectorRegisters,
        "phy_fe_reg_init" => RfInitPhase::ConfigureFrontEndRegisters,
        "phy_tsens_read_init" => {
            expect(0, 1)?;
            RfInitPhase::ConfigureTemperatureSensorRead
        }
        "phy_tx_pwctrl_bg_init" => RfInitPhase::ConfigureTxPowerControlBackground,
        "phy_i2c_rc_cal_set" => {
            expect(0, 3)?;
            expect(1, 1)?;
            expect(2, 9)?;
            RfInitPhase::ConfigureRcCalibration
        }
        "phy_rc_cal_init" => RfInitPhase::InitializeRcCalibration,
        "phy_filter_dcap_set" => RfInitPhase::ConfigureFilterDcap,
        "phy_i2c_readReg" => {
            expect(0, 0x62)?;
            expect(1, 1)?;
            expect(2, 0x0f)?;
            RfInitPhase::ReadParameter18e
        }
        "phy_i2c_init1" => RfInitPhase::InitializeI2c,
        "phy_rfpll_chgp_cal" => RfInitPhase::CalibrateRfpllChargePump,
        "phy_i2c_master_cmd_mem_init" => RfInitPhase::ConfigureI2cMasterCommandMemory,
        "phy_i2c_readReg_Mask" => {
            expect(0, 0x69)?;
            expect(1, 0)?;
            expect(2, 4)?;
            expect(3, 3)?;
            expect(4, 0)?;
            RfInitPhase::ReadSar2InitializationState
        }
        "phy_i2c_sar2_init_code" => {
            expect(0, 0x578)?;
            RfInitPhase::InitializeSar2
        }
        "phy_xtal_duty_cal_init" => {
            expect(0, 0)?;
            RfInitPhase::CalibrateXtalDuty
        }
        "phy_fe_reg_update" => RfInitPhase::UpdateFrontEndRegisters,
        "phy_set_chan_freq_hw_init" => {
            expect(0, 2)?;
            expect(1, 4)?;
            RfInitPhase::InitializeChannelFrequency
        }
        symbol => {
            return Err(format!(
                "unrecognized direct phy_rf_init child {symbol} at {:#010x}",
                call.site
            )
            .into());
        }
    };
    Ok(phase)
}

fn timeline_first_ram_byte(result: &emulator::ExecutionResult, wanted: u32) -> Option<u8> {
    result.timeline.iter().find_map(|event| {
        let emulator::ExecutionTimelineEvent::RamRead {
            width,
            address,
            value,
        } = event
        else {
            return None;
        };
        let byte = wanted.checked_sub(*address)?;
        if byte < u32::from(*width / 8) {
            Some((value >> (byte * 8)) as u8)
        } else {
            None
        }
    })
}

fn vendor_rf_init_parameters(
    call: &emulator::OrderedCall,
    phase: RfInitPhase,
    rc_already_complete: bool,
) -> RfInitPhaseParameters {
    match phase {
        RfInitPhase::ConfigureBbpllCalibration => {
            RfInitPhaseParameters::Enabled(call.arguments[0] != 0)
        }
        RfInitPhase::PostOpenI2cDelay | RfInitPhase::ConfigureI2cClockSelection => {
            RfInitPhaseParameters::Value(call.arguments[0])
        }
        RfInitPhase::ReadParameter18e => RfInitPhaseParameters::I2cRead {
            block: call.arguments[0] as u8,
            host: call.arguments[1] as u8,
            register: call.arguments[2] as u8,
        },
        RfInitPhase::ReadSar2InitializationState => RfInitPhaseParameters::I2cReadMasked {
            block: call.arguments[0] as u8,
            host: call.arguments[1] as u8,
            register: call.arguments[2] as u8,
            high_bit: call.arguments[3] as u8,
            low_bit: call.arguments[4] as u8,
        },
        RfInitPhase::InitializeRcCalibration => RfInitPhaseParameters::RcCalibrationPrestate {
            already_complete: rc_already_complete,
        },
        _ => RfInitPhaseParameters::None,
    }
}

fn vendor_rf_init_projection(
    image: &emulator::ExecutableImage,
    result: &emulator::ExecutionResult,
    phy_param: u32,
) -> Result<RfInitStateProjection> {
    let byte = |offset: u32| -> Result<u8> {
        result
            .persistent_memory
            .get(&(phy_param + offset))
            .copied()
            .or_else(|| image.loaded_byte(phy_param + offset))
            .ok_or_else(|| {
                format!(
                    "vendor RF-init state byte {:#010x} is outside persistent ELF memory",
                    phy_param + offset
                )
                .into()
            })
    };
    Ok(RfInitStateProjection {
        rc_calibration_complete: byte(0xa6)? & 0x80 != 0,
        bbpll_register_snapshot: byte(0x4a)?,
        parameter_18e: byte(0x18e)?,
        xtal_initial_duty: byte(0x19e)?,
        xtal_low_candidate: byte(0x19f)?,
        xtal_high_candidate: byte(0x1a0)?,
        frequency_table_initialized: byte(0xa4)? & 0x20 != 0,
    })
}

pub fn normalize_vendor_rf_init(
    image: &emulator::ExecutableImage,
    result: &emulator::ExecutionResult,
    phy_param: u32,
) -> Result<Vec<RfInitEvent>> {
    vendor_rf_init_state_footprint(result, phy_param)?;
    if result.return_value != 0 {
        return Err(format!("vendor phy_rf_init returned {:#010x}", result.return_value).into());
    }
    let extent = image
        .symbol_extent("phy_rf_init")
        .ok_or("vendor artifact has no bounded phy_rf_init text symbol")?;
    let rc_already_complete = timeline_first_ram_byte(result, phy_param + 0xa6)
        .ok_or("vendor phy_rf_init never inspected its RC-calibration state")?
        & 0x80
        != 0;
    let mut events = result
        .ordered_calls
        .iter()
        .filter(|call| extent.contains(&call.site))
        .map(|call| {
            let phase = vendor_rf_init_phase(call)?;
            Ok(rf_phase(
                phase,
                vendor_rf_init_parameters(call, phase, rc_already_complete),
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    events.push(RfInitEvent::Complete(vendor_rf_init_projection(
        image, result, phy_param,
    )?));
    Ok(events)
}

fn rust_rf_init_phase(
    action: PhyRfInitPrefixAction,
    rc_already_complete: bool,
) -> Option<RfInitEvent> {
    let plain = |phase| (phase, RfInitPhaseParameters::None);
    let (phase, parameters) = match action {
        PhyRfInitPrefixAction::ConfigureFeBbClock => plain(RfInitPhase::ConfigureFeBbClock),
        PhyRfInitPrefixAction::ConfigureBbpllCalibration { enabled } => (
            RfInitPhase::ConfigureBbpllCalibration,
            RfInitPhaseParameters::Enabled(enabled),
        ),
        PhyRfInitPrefixAction::Bias(_) => plain(RfInitPhase::EnableBias),
        PhyRfInitPrefixAction::OpenI2cXpd(_) => plain(RfInitPhase::OpenI2c),
        PhyRfInitPrefixAction::DelayMicros(micros) => (
            RfInitPhase::PostOpenI2cDelay,
            RfInitPhaseParameters::Value(micros),
        ),
        PhyRfInitPrefixAction::PbusClear(_) => plain(RfInitPhase::ClearPbus),
        PhyRfInitPrefixAction::ConfigureI2cClockSelection { selection } => (
            RfInitPhase::ConfigureI2cClockSelection,
            RfInitPhaseParameters::Value(selection),
        ),
        PhyRfInitPrefixAction::I2cBbpll(_) => plain(RfInitPhase::ConfigureI2cBbpll),
        PhyRfInitPrefixAction::AdcRate(_) => plain(RfInitPhase::ConfigureAdcRate),
        PhyRfInitPrefixAction::ConfigureI2cMasterRegisters => {
            plain(RfInitPhase::ConfigureI2cMasterRegisters)
        }
        PhyRfInitPrefixAction::ConfigurePowerDetectorRegisters => {
            plain(RfInitPhase::ConfigurePowerDetectorRegisters)
        }
        PhyRfInitPrefixAction::ConfigureFrontEndRegisters => {
            plain(RfInitPhase::ConfigureFrontEndRegisters)
        }
        PhyRfInitPrefixAction::ConfigureTemperatureSensorRead => {
            plain(RfInitPhase::ConfigureTemperatureSensorRead)
        }
        PhyRfInitPrefixAction::ConfigureTxPowerControlBackground => {
            plain(RfInitPhase::ConfigureTxPowerControlBackground)
        }
        PhyRfInitPrefixAction::RcCalibrationSet(_) => plain(RfInitPhase::ConfigureRcCalibration),
        PhyRfInitPrefixAction::InspectRcCalibrationState
        | PhyRfInitPrefixAction::RcCalibration(_) => (
            RfInitPhase::InitializeRcCalibration,
            RfInitPhaseParameters::RcCalibrationPrestate {
                already_complete: rc_already_complete,
            },
        ),
        PhyRfInitPrefixAction::CaptureFilterDcapParameters => return None,
        PhyRfInitPrefixAction::FilterDcap(_) => plain(RfInitPhase::ConfigureFilterDcap),
        PhyRfInitPrefixAction::ReadParameter18e { address } => (
            RfInitPhase::ReadParameter18e,
            RfInitPhaseParameters::I2cRead {
                block: address.block(),
                host: address.host(),
                register: address.register(),
            },
        ),
        PhyRfInitPrefixAction::I2cInit1(_) => plain(RfInitPhase::InitializeI2c),
        PhyRfInitPrefixAction::RfpllChargePump(_) => plain(RfInitPhase::CalibrateRfpllChargePump),
        PhyRfInitPrefixAction::ConfigureI2cMasterCommandMemory { .. } => {
            plain(RfInitPhase::ConfigureI2cMasterCommandMemory)
        }
        PhyRfInitPrefixAction::ReadMasked69 {
            address,
            high_bit,
            low_bit,
        } => (
            RfInitPhase::ReadSar2InitializationState,
            RfInitPhaseParameters::I2cReadMasked {
                block: address.block(),
                host: address.host(),
                register: address.register(),
                high_bit,
                low_bit,
            },
        ),
        PhyRfInitPrefixAction::Sar2Init(_) => plain(RfInitPhase::InitializeSar2),
        PhyRfInitPrefixAction::CaptureXtalDutyParameters => return None,
        PhyRfInitPrefixAction::XtalDuty(_) => plain(RfInitPhase::CalibrateXtalDuty),
        PhyRfInitPrefixAction::ConfigureFrontEndRegisterUpdate => {
            plain(RfInitPhase::UpdateFrontEndRegisters)
        }
        PhyRfInitPrefixAction::CaptureChannelFrequencyControl => return None,
        PhyRfInitPrefixAction::ChannelFrequency(_) => {
            plain(RfInitPhase::InitializeChannelFrequency)
        }
        PhyRfInitPrefixAction::Complete(_) => return None,
    };
    Some(rf_phase(phase, parameters))
}

fn rust_rf_init_projection(
    state: &PhyColdState,
    outcome: PhyRfInitPrefixOutcome,
) -> Result<RfInitStateProjection> {
    let PhyRfInitPrefixOutcome::ChannelFrequencyInitialized {
        bbpll_register_snapshot,
        parameter,
        xtal_duty,
        channel_frequency,
        ..
    } = outcome
    else {
        return Err(format!("Rust RF init did not complete successfully: {outcome:?}").into());
    };
    Ok(RfInitStateProjection {
        rc_calibration_complete: state.rc_calibration_complete(),
        bbpll_register_snapshot,
        parameter_18e: parameter.parameter_18e(),
        xtal_initial_duty: xtal_duty.initial_duty,
        xtal_low_candidate: xtal_duty.low_frequency.best_candidate,
        xtal_high_candidate: xtal_duty.high_frequency.best_candidate,
        frequency_table_initialized: channel_frequency.table_is_initialized,
    })
}

fn complete_i2c_binding(
    mut binding: open_esp_radio_esp32s31_phy::phy_cold::PhyColdI2cBinding,
) -> Result<open_esp_radio_esp32s31_phy::phy_i2c::PhyRfInitPrefixCompletion> {
    for _ in 0..6 {
        match binding.action() {
            PhyColdI2cAction::StartRead { .. } => binding
                .read_started()
                .map_err(|error| format!("cannot start semantic PHY-I2C read: {error:?}"))?,
            PhyColdI2cAction::AwaitReadCompletionEdge { address } => {
                let value = if address.host() == 1 { 0x5b } else { 0 };
                binding
                    .observe_read_result(Ok(value))
                    .map_err(|error| format!("cannot complete semantic PHY-I2C read: {error:?}"))?;
            }
            PhyColdI2cAction::StartWrite { .. } => binding
                .write_started()
                .map_err(|error| format!("cannot start semantic PHY-I2C write: {error:?}"))?,
            PhyColdI2cAction::AwaitWriteCompletionEdge { .. } => {
                binding.observe_write_result(Ok(())).map_err(|error| {
                    format!("cannot complete semantic PHY-I2C write: {error:?}")
                })?;
            }
            PhyColdI2cAction::Complete(_) => {
                return binding.into_completion().map_err(|error| {
                    format!("cannot lower semantic PHY-I2C result: {error:?}").into()
                });
            }
        }
    }
    Err("semantic PHY-I2C binding exceeded its finite edge limit".into())
}

fn complete_rf_init_external(
    binding: PhyColdExternalBinding,
) -> Result<open_esp_radio_esp32s31_phy::phy_i2c::PhyRfInitPrefixCompletion> {
    match binding {
        PhyColdExternalBinding::I2c(binding) => complete_i2c_binding(binding),
        PhyColdExternalBinding::Mmio(binding) => binding
            .into_completion()
            .map_err(|error| format!("cannot lower semantic MMIO result: {error:?}").into()),
        PhyColdExternalBinding::Timer(binding) => binding
            .into_elapsed_completion()
            .map_err(|error| format!("cannot lower semantic timer result: {error:?}").into()),
        PhyColdExternalBinding::Pbus(mut binding) => {
            match binding.action() {
                PhyColdPbusAction::Start(_) => binding.started().map_err(|error| {
                    format!("cannot start semantic PBus transaction: {error:?}")
                })?,
                action => {
                    return Err(format!(
                        "semantic PBus binding did not start at Start: {action:?}"
                    )
                    .into());
                }
            }
            binding
                .observe_result(PhyColdPbusHardwareResult::Completed)
                .map_err(|error| format!("cannot complete semantic PBus transaction: {error:?}"))?;
            binding
                .into_completion()
                .map_err(|error| format!("cannot lower semantic PBus result: {error:?}").into())
        }
        PhyColdExternalBinding::Observation(binding) => {
            let result = match binding.request() {
                PhyColdObservationRequest::ConfigureOpenI2cPowerAndPulse => {
                    PhyColdObservationResult::OpenI2cPowerAndPulse {
                        started_at_cycle: 0,
                    }
                }
                PhyColdObservationRequest::CheckOpenI2cSdmDeadline { .. } => {
                    PhyColdObservationResult::OpenI2cSdmDeadline { expired: false }
                }
                PhyColdObservationRequest::ConfigurePbusWorkMode => {
                    PhyColdObservationResult::PbusWorkMode {
                        settle_required: false,
                    }
                }
                PhyColdObservationRequest::MaskRxDcoControl => {
                    PhyColdObservationResult::RxDcoControlMasked { saved_field: 0 }
                }
                PhyColdObservationRequest::ReadRxDcoPbus { selector, path } => {
                    PhyColdObservationResult::RxDcoPbusRead {
                        selector,
                        path,
                        value: 0,
                    }
                }
                PhyColdObservationRequest::ObserveDcIqReadiness { request, .. } => {
                    PhyColdObservationResult::DcIqReadiness {
                        request,
                        snapshot: PhyDcIqReadinessSnapshot {
                            ready: true,
                            activity: false,
                        },
                    }
                }
                PhyColdObservationRequest::ReadDcIqAccumulators(request) => {
                    PhyColdObservationResult::DcIqAccumulators {
                        request,
                        snapshot: PhyDcIqAccumulatorSnapshot {
                            i: 0,
                            q: 0,
                            power: 0,
                        },
                    }
                }
                PhyColdObservationRequest::ObserveSignalPowerReadiness { request, .. } => {
                    PhyColdObservationResult::SignalPowerReadiness {
                        request,
                        snapshot: PhyDcIqReadinessSnapshot {
                            ready: true,
                            activity: false,
                        },
                    }
                }
                PhyColdObservationRequest::ReadSignalPowerAccumulators(request) => {
                    PhyColdObservationResult::SignalPowerAccumulators {
                        request,
                        snapshot: PhySignalPowerAccumulatorSnapshot {
                            sum_i: 0,
                            difference_i: 0,
                            difference_q: 0,
                            sum_q: 0,
                        },
                    }
                }
            };
            binding.into_completion(result).map_err(|error| {
                format!("cannot lower semantic observation result: {error:?}").into()
            })
        }
    }
}

pub fn rust_rf_init_events(state: PhyColdState) -> Result<(Vec<RfInitEvent>, PhyColdState)> {
    let rc_already_complete = state.rc_calibration_complete();
    let mut init = PhyRfColdInit::new(state);
    let mut events = Vec::new();
    let mut last_phase_event = None;
    for _ in 0..100_000 {
        let action = init.action();
        if let Some(event) = rust_rf_init_phase(action, rc_already_complete)
            && last_phase_event != Some(event)
        {
            events.push(event);
            last_phase_event = Some(event);
        }
        match init
            .step_local()
            .map_err(|error| format!("Rust RF init rejected local action: {error:?}"))?
        {
            PhyColdLocalStep::StateAdvanced => {}
            PhyColdLocalStep::External(action) => {
                let binding = PhyColdExternalBinding::lower(action).map_err(|error| {
                    format!("cannot lower Rust RF-init action {action:?}: {error:?}")
                })?;
                let completion = complete_rf_init_external(binding)?;
                init.advance_external(completion).map_err(|error| {
                    format!("Rust RF init rejected external completion: {error:?}")
                })?;
            }
            PhyColdLocalStep::Complete(outcome) => {
                events.push(RfInitEvent::Complete(rust_rf_init_projection(
                    init.state(),
                    outcome,
                )?));
                return Ok((events, init.into_state()));
            }
        }
    }
    Err("Rust RF init exceeded semantic action limit".into())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChannelEvent {
    SetAgc(bool),
    SetBbpllCalibration(bool),
    TemperatureSample,
    StartFrequencySwitch {
        frequency_index: u8,
        crystal_selector: u8,
    },
    DelayMicros(u32),
    ClearFrequencySwitch,
    FrequencyReady {
        samples: u32,
    },
    ConfigureNrx(u16),
    ConfigureBssCbw(u8),
    ConfigureRxCompensation,
    WriteI2c {
        block: u8,
        register: u8,
        value: u8,
    },
    CalculateTxGain {
        channel: u16,
    },
    PublishTxGain(PhyWifiTxGainImage),
    ReadI2c {
        block: u8,
        register: u8,
    },
    PublishTxCapCommandMemory(u8),
    ConfigureChannelCbw(u8),
    ClearDcMemory,
    Complete {
        channel: u16,
        frequency_mhz: u16,
        cbw: u8,
        init_complete: bool,
    },
}

pub fn vendor_channel_scenario(
    channel_or_frequency: u16,
    cbw: u8,
    phy_param: u32,
    phy_functions_pointer: u32,
) -> Result<emulator::Scenario> {
    let mut scenario = emulator::Scenario {
        arguments: vec![u32::from(channel_or_frequency), u32::from(cbw)],
        ..emulator::Scenario::default()
    };

    seed_ram_word(&mut scenario, phy_functions_pointer, ROM_PHY_FUNCTION_TABLE);
    seed_ram_word(
        &mut scenario,
        ROM_PHY_FUNCTION_TABLE_POINTER,
        ROM_PHY_FUNCTION_TABLE,
    );
    seed_ram_word(&mut scenario, ROM_PHY_PARAM_POINTER, phy_param);
    for (index, value) in ROM_PHY_FUNCTIONS.into_iter().enumerate() {
        seed_ram_word(
            &mut scenario,
            ROM_PHY_FUNCTION_TABLE + index as u32 * 4,
            value,
        );
    }
    declare_state_ownership(&mut scenario, phy_param, CHANNEL_STATE_FOOTPRINT);

    // Stable response values are explicit environment inputs. In particular,
    // DAC 5 is a valid temperature state; using zero would reproduce the
    // vendor ROM's out-of-bounds reset-DAC table path instead of a valid
    // abstract state shared with Rust.
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
        (0x2010_7ce0, 0),
        (0x2010_7ce4, 0),
        (0x2010_703c, 0),
    ] {
        scenario.mmio_initial.insert(address, value);
    }
    scenario.observed_memory.push(emulator::MemoryRange {
        start: phy_param + 0x11c,
        length: 4,
    });
    Ok(scenario)
}

fn vendor_tx_gain_image(
    vendor_image: &emulator::ExecutableImage,
    result: &emulator::ExecutionResult,
    phy_param: u32,
    call: &emulator::OrderedCall,
    timeline_index: usize,
) -> Result<PhyWifiTxGainImage> {
    if call.arguments[0] != 0 || call.arguments[1] != 32 {
        return Err(format!(
            "unexpected vendor TX-gain publication mode/count: {}/{}",
            call.arguments[0], call.arguments[1]
        )
        .into());
    }
    if call.arguments[6] != phy_param + 0xd0 {
        return Err(format!(
            "vendor TX-gain config pointer is {:#010x}, expected phy_param+0xd0",
            call.arguments[6]
        )
        .into());
    }
    let mut ram = result.initial_memory.clone();
    for event in &result.timeline[..timeline_index] {
        if let emulator::ExecutionTimelineEvent::RamWrite {
            width,
            address,
            value,
        } = event
        {
            for offset in 0..usize::from(*width / 8) {
                ram.insert(address + offset as u32, (value >> (offset * 8)) as u8);
            }
        }
    }
    let read_word = |address: u32| -> Result<u32> {
        let byte = |offset: u32| -> Result<u8> {
            Ok(ram.get(&(address + offset)).copied().ok_or_else(|| {
                format!(
                    "vendor TX-gain byte {:#010x} was not written",
                    address + offset
                )
            })?)
        };
        Ok(u32::from_le_bytes([byte(0)?, byte(1)?, byte(2)?, byte(3)?]))
    };
    let mut gain_image = PhyWifiTxGainImage {
        seed: [0; 6],
        output_32: [0; 8],
        output_64: [0; 16],
        output_72: [0; 16],
        config: 0,
    };
    for (index, word) in gain_image.seed.iter_mut().enumerate() {
        *word = read_word(call.arguments[5] + index as u32 * 4)?;
    }
    for (index, word) in gain_image.output_32.iter_mut().enumerate() {
        *word = read_word(call.arguments[4] + index as u32 * 4)?;
    }
    for (index, word) in gain_image.output_64.iter_mut().enumerate() {
        *word = read_word(call.arguments[3] + index as u32 * 4)?;
    }
    for (index, word) in gain_image.output_72.iter_mut().enumerate() {
        *word = read_word(call.arguments[2] + index as u32 * 4)?;
    }
    gain_image.config = u16::from_le_bytes([
        ram.get(&(phy_param + 0xd0))
            .copied()
            .or_else(|| vendor_image.loaded_byte(phy_param + 0xd0))
            .ok_or("vendor TX-gain config is outside call-time memory")?,
        ram.get(&(phy_param + 0xd1))
            .copied()
            .or_else(|| vendor_image.loaded_byte(phy_param + 0xd1))
            .ok_or("vendor TX-gain config is outside call-time memory")?,
    ]);
    Ok(gain_image)
}

pub fn normalize_vendor_channel(
    image: &emulator::ExecutableImage,
    result: &emulator::ExecutionResult,
    phy_param: u32,
    channel_or_frequency: u16,
) -> Result<Vec<ChannelEvent>> {
    vendor_channel_state_footprint(result, phy_param)?;
    let mut events = Vec::new();
    let mut ready_samples = 0_u32;
    let mut publish_tx_cap_pending = false;

    for (timeline_index, timeline) in result.timeline.iter().enumerate() {
        match timeline {
            emulator::ExecutionTimelineEvent::Observable(
                emulator::ExecutionEvent::DelayMicros(micros),
            ) => {
                if *micros == 10 {
                    events.push(ChannelEvent::ClearFrequencySwitch);
                }
                events.push(ChannelEvent::DelayMicros(*micros));
            }
            emulator::ExecutionTimelineEvent::Observable(emulator::ExecutionEvent::Read {
                address: 0x2010_0028,
                value,
                ..
            }) => {
                if value & (1 << 8) == 0 {
                    return Err("vendor frequency-ready scenario did not become ready".into());
                }
                events.push(ChannelEvent::FrequencyReady {
                    samples: ready_samples,
                });
                ready_samples += 1;
            }
            emulator::ExecutionTimelineEvent::Call(call) => match call.symbol.as_str() {
                "phy_disable_agc" => events.push(ChannelEvent::SetAgc(false)),
                "phy_enable_agc" => events.push(ChannelEvent::SetAgc(true)),
                "phy_bbpll_cal" => {
                    events.push(ChannelEvent::SetBbpllCalibration(call.arguments[0] != 0))
                }
                "phy_tsens_temp_read" => events.push(ChannelEvent::TemperatureSample),
                "phy_freq_chan_en_sw" => events.push(ChannelEvent::StartFrequencySwitch {
                    frequency_index: call.arguments[0] as u8,
                    crystal_selector: call.arguments[1] as u8,
                }),
                "phy_nrx_freq_set" => {
                    events.push(ChannelEvent::ConfigureNrx(call.arguments[0] as u16));
                }
                "phy_bb_bss_cbw40" => {
                    events.push(ChannelEvent::ConfigureBssCbw(call.arguments[0] as u8));
                }
                "phy_set_rx_comp_" => events.push(ChannelEvent::ConfigureRxCompensation),
                "phy_chip_i2c_writeReg"
                    if call.arguments[0] as u8 == 0x6b && call.arguments[2] as u8 == 0x02 =>
                {
                    events.push(ChannelEvent::WriteI2c {
                        block: 0x6b,
                        register: 0x02,
                        value: call.arguments[3] as u8,
                    });
                }
                "phy_wifi_get_tx_gain" => events.push(ChannelEvent::CalculateTxGain {
                    channel: call.arguments[0] as u16,
                }),
                "phy_set_tx_gain_mem_new" => events.push(ChannelEvent::PublishTxGain(
                    vendor_tx_gain_image(image, result, phy_param, call, timeline_index)?,
                )),
                "phy_i2c_master_mem_txcap" => publish_tx_cap_pending = true,
                "phy_chip_i2c_readReg"
                    if publish_tx_cap_pending
                        && call.arguments[0] as u8 == 0x6b
                        && call.arguments[2] as u8 == 0x02 =>
                {
                    events.push(ChannelEvent::ReadI2c {
                        block: 0x6b,
                        register: 0x02,
                    });
                }
                "phy_bb_cbw_chan_cfg" => {
                    if !publish_tx_cap_pending {
                        return Err("vendor channel-CBW call preceded TX-cap publication".into());
                    }
                    events.push(ChannelEvent::PublishTxCapCommandMemory(TX_CAP_READ));
                    publish_tx_cap_pending = false;
                    events.push(ChannelEvent::ConfigureChannelCbw(call.arguments[0] as u8));
                }
                "phy_dc_mem_clr" => events.push(ChannelEvent::ClearDcMemory),
                _ => {}
            },
            _ => {}
        }
    }
    if publish_tx_cap_pending {
        return Err("vendor TX-cap publication did not reach channel-CBW configuration".into());
    }
    if result.return_value != 0 {
        return Err(format!(
            "vendor channel transition returned {:#010x}",
            result.return_value
        )
        .into());
    }

    let final_byte = |offset: u32| -> Result<u8> {
        Ok(result
            .persistent_memory
            .get(&(phy_param + offset))
            .copied()
            .or_else(|| image.loaded_byte(phy_param + offset))
            .ok_or_else(|| {
                format!(
                    "vendor channel state byte {:#010x} is outside the loaded ELF image",
                    phy_param + offset
                )
            })?)
    };
    let channel = u16::from_le_bytes([final_byte(0x11c)?, final_byte(0x11d)?]);
    let init_complete = final_byte(0x11e)? != 0;
    let cbw = final_byte(0x11f)?;
    let frequency_mhz = if channel_or_frequency > 2_411 {
        channel_or_frequency
    } else if channel == 14 {
        2_484
    } else {
        2_407 + channel * 5
    };
    events.push(ChannelEvent::Complete {
        channel,
        frequency_mhz,
        cbw,
        init_complete,
    });
    Ok(events)
}

#[cfg(test)]
pub fn rust_channel_events(channel_or_frequency: u16, cbw: u8) -> Result<Vec<ChannelEvent>> {
    rust_channel_events_with_state(PhyColdState::new(), channel_or_frequency, cbw)
        .map(|(events, _)| events)
}

pub fn rust_channel_events_with_state(
    mut state: PhyColdState,
    channel_or_frequency: u16,
    cbw: u8,
) -> Result<(Vec<ChannelEvent>, PhyColdState)> {
    let mut transition = PhyChipChannelTransition::new(PhyChipChannelRequest {
        channel_or_frequency,
        cbw,
        parameters: state.channel_parameters(),
    });
    let mut events = Vec::new();
    let mut temperature_started = false;

    for _ in 0..128 {
        let action = transition.action();
        let completion = match action {
            PhyChipChannelAction::SetAgc { enabled } => {
                events.push(ChannelEvent::SetAgc(enabled));
                PhyChipChannelCompletion::AgcSet { enabled }
            }
            PhyChipChannelAction::SetBbpllCalibration { enabled } => {
                events.push(ChannelEvent::SetBbpllCalibration(enabled));
                PhyChipChannelCompletion::BbpllCalibrationSet { enabled }
            }
            PhyChipChannelAction::Temperature(action) => {
                if !temperature_started {
                    events.push(ChannelEvent::TemperatureSample);
                    temperature_started = true;
                }
                let completion = match action {
                    PhyTemperatureAction::ReadMasked {
                        address,
                        high_bit,
                        low_bit,
                    } => PhyTemperatureCompletion::MaskedRead {
                        address,
                        high_bit,
                        low_bit,
                        value: TEMPERATURE_DAC,
                    },
                    PhyTemperatureAction::SampleCode => PhyTemperatureCompletion::CodeSampled {
                        value: TEMPERATURE_CODE,
                    },
                    PhyTemperatureAction::WriteMasked {
                        address,
                        high_bit,
                        low_bit,
                        value,
                    } => PhyTemperatureCompletion::MaskedWrite {
                        address,
                        high_bit,
                        low_bit,
                        value,
                    },
                    terminal => {
                        return Err(
                            format!("unexpected terminal temperature action {terminal:?}").into(),
                        );
                    }
                };
                PhyChipChannelCompletion::Temperature(completion)
            }
            PhyChipChannelAction::StartFrequencySwitch {
                frequency_index,
                crystal_selector,
            } => {
                events.push(ChannelEvent::StartFrequencySwitch {
                    frequency_index,
                    crystal_selector,
                });
                PhyChipChannelCompletion::FrequencySwitchStarted {
                    frequency_index,
                    crystal_selector,
                }
            }
            PhyChipChannelAction::DelayMicros { phase, micros } => {
                events.push(ChannelEvent::DelayMicros(micros));
                PhyChipChannelCompletion::DelayElapsed { phase, micros }
            }
            PhyChipChannelAction::ClearFrequencySwitch => {
                events.push(ChannelEvent::ClearFrequencySwitch);
                PhyChipChannelCompletion::FrequencySwitchCleared
            }
            PhyChipChannelAction::AwaitFrequencyReadyEdge { samples } => {
                events.push(ChannelEvent::FrequencyReady { samples });
                PhyChipChannelCompletion::FrequencyReadyObserved { ready: true }
            }
            PhyChipChannelAction::ConfigureNrx { frequency_mhz } => {
                events.push(ChannelEvent::ConfigureNrx(frequency_mhz));
                PhyChipChannelCompletion::NrxConfigured { frequency_mhz }
            }
            PhyChipChannelAction::ConfigureBssCbw { cbw } => {
                events.push(ChannelEvent::ConfigureBssCbw(cbw));
                PhyChipChannelCompletion::BssCbwConfigured { cbw }
            }
            PhyChipChannelAction::ConfigureRxCompensation => {
                events.push(ChannelEvent::ConfigureRxCompensation);
                PhyChipChannelCompletion::RxCompensationConfigured
            }
            PhyChipChannelAction::WriteI2c {
                phase,
                address,
                value,
            } => {
                events.push(ChannelEvent::WriteI2c {
                    block: address.block(),
                    register: address.register(),
                    value,
                });
                PhyChipChannelCompletion::I2cWriteCompleted {
                    phase,
                    address,
                    value,
                }
            }
            PhyChipChannelAction::CalculateTxGain(request) => {
                events.push(ChannelEvent::CalculateTxGain {
                    channel: request.channel,
                });
                PhyChipChannelCompletion::TxGainCalculated {
                    request,
                    image: calculate_wifi_tx_gain(request),
                }
            }
            PhyChipChannelAction::PublishTxGain(image) => {
                events.push(ChannelEvent::PublishTxGain(image));
                PhyChipChannelCompletion::TxGainPublished
            }
            PhyChipChannelAction::ReadI2c { phase, address } => {
                events.push(ChannelEvent::ReadI2c {
                    block: address.block(),
                    register: address.register(),
                });
                PhyChipChannelCompletion::I2cReadCompleted {
                    phase,
                    address,
                    value: TX_CAP_READ,
                }
            }
            PhyChipChannelAction::PublishTxCapCommandMemory { value } => {
                events.push(ChannelEvent::PublishTxCapCommandMemory(value));
                PhyChipChannelCompletion::TxCapCommandMemoryPublished { value }
            }
            PhyChipChannelAction::ConfigureChannelCbw { cbw } => {
                events.push(ChannelEvent::ConfigureChannelCbw(cbw));
                PhyChipChannelCompletion::ChannelCbwConfigured { cbw }
            }
            PhyChipChannelAction::ClearDcMemory => {
                events.push(ChannelEvent::ClearDcMemory);
                PhyChipChannelCompletion::DcMemoryCleared
            }
            PhyChipChannelAction::Complete(outcome) => {
                events.push(ChannelEvent::Complete {
                    channel: outcome.channel,
                    frequency_mhz: outcome.frequency_mhz,
                    cbw: outcome.cbw,
                    init_complete: outcome.init_complete,
                });
                state.apply_channel_outcome(outcome);
                return Ok((events, state));
            }
            PhyChipChannelAction::Failed(failure) => {
                return Err(format!("Rust channel transition failed: {failure:?}").into());
            }
        };
        transition
            .advance(completion)
            .map_err(|error| format!("Rust channel transition rejected completion: {error:?}"))?;
    }
    Err("Rust channel transition exceeded semantic action limit".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn execution_result_with_timeline(
        timeline: Vec<emulator::ExecutionTimelineEvent>,
    ) -> emulator::ExecutionResult {
        emulator::ExecutionResult {
            events: Vec::new(),
            timeline,
            return_value: 0,
            steps: 0,
            branches: std::collections::BTreeSet::new(),
            ordered_branches: Vec::new(),
            calls: std::collections::BTreeSet::new(),
            ordered_calls: Vec::new(),
            indirect_calls: std::collections::BTreeSet::new(),
            memory_changes: Vec::new(),
            initial_memory: std::collections::BTreeMap::new(),
            persistent_memory: std::collections::BTreeMap::new(),
        }
    }

    #[test]
    fn rust_channel_model_exposes_complete_action_order() {
        let events = rust_channel_events(11, 0).unwrap();
        assert_eq!(events.first(), Some(&ChannelEvent::SetAgc(false)));
        assert!(events.contains(&ChannelEvent::FrequencyReady { samples: 0 }));
        assert_eq!(
            events.last(),
            Some(&ChannelEvent::Complete {
                channel: 11,
                frequency_mhz: 2_462,
                cbw: 0,
                init_complete: false,
            })
        );
    }

    #[test]
    fn rust_rf_init_model_preserves_typed_state_across_a_second_run() {
        let (first, state) = rust_rf_init_events(PhyColdState::new()).unwrap();
        let (second, _) = rust_rf_init_events(state).unwrap();

        assert_eq!(
            first.first(),
            Some(&rf_phase(
                RfInitPhase::ConfigureFeBbClock,
                RfInitPhaseParameters::None,
            ))
        );
        assert_eq!(first.last(), second.last());
        assert!(matches!(first.last(), Some(RfInitEvent::Complete(_))));
        assert!(first.contains(&rf_phase(
            RfInitPhase::InitializeRcCalibration,
            RfInitPhaseParameters::RcCalibrationPrestate {
                already_complete: false,
            },
        )));
        assert!(second.contains(&rf_phase(
            RfInitPhase::InitializeRcCalibration,
            RfInitPhaseParameters::RcCalibrationPrestate {
                already_complete: true,
            },
        )));
        assert!(first.contains(&rf_phase(
            RfInitPhase::ConfigureBbpllCalibration,
            RfInitPhaseParameters::Enabled(true),
        )));
        assert!(first.contains(&rf_phase(
            RfInitPhase::PostOpenI2cDelay,
            RfInitPhaseParameters::Value(10),
        )));
        assert!(first.contains(&rf_phase(
            RfInitPhase::ConfigureI2cClockSelection,
            RfInitPhaseParameters::Value(8),
        )));
    }

    #[test]
    fn state_footprints_reject_unknown_offsets_and_access_directions() {
        let state_base = 0x1000;
        let unknown =
            execution_result_with_timeline(vec![emulator::ExecutionTimelineEvent::RamRead {
                width: 8,
                address: state_base + 0x123,
                value: 0,
            }]);
        let error = vendor_rf_init_state_footprint(&unknown, state_base).unwrap_err();
        assert!(error.to_string().contains("reads=[0x123]"));

        let wrong_direction =
            execution_result_with_timeline(vec![emulator::ExecutionTimelineEvent::RamWrite {
                width: 8,
                address: state_base + 0x007,
                value: 0,
            }]);
        let error = vendor_channel_state_footprint(&wrong_direction, state_base).unwrap_err();
        assert!(error.to_string().contains("writes=[0x007]"));
    }

    #[test]
    fn vendor_rf_phase_rejects_mutated_direct_call_arguments() {
        let call = emulator::OrderedCall {
            site: 0x1000,
            symbol: "ets_delay_us".to_owned(),
            arguments: [11, 0, 0, 0, 0, 0, 0, 0],
        };
        let error = vendor_rf_init_phase(&call).unwrap_err();
        assert!(error.to_string().contains("expected 0xa"));
    }
}
