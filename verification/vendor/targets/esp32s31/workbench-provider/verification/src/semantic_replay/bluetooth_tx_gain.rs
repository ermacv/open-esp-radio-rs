//! Hierarchical semantic replay of the Bluetooth TX-gain initialization parent.
//!
//! The detailed RFPLL, TX-DC, TX-power and PWDET effects are compared by
//! their own contracts. This contract proves that the vendor parent and the
//! production Rust parent select those children in the same order, preserve
//! their resulting state and publish the final gain image.

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothTxGainPowerState {
    pub power_curve: [i8; 3],
    pub power_adjustment: i8,
    pub attenuation: u8,
    pub calibrated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothTxGainInitEvent {
    ConfigureRfpll,
    ConfigureTxCap,
    CalibrateTxDc,
    CalibrateTxPower,
    CalibrateTxDcPwdet,
    PublishGain,
    Complete {
        tx_dc_calibrated: bool,
        dco: [[u16; 4]; 3],
        tx_power: BluetoothTxGainPowerState,
    },
}

const BLUETOOTH_TX_GAIN_PARENT_STATE_FOOTPRINT: &[StateFootprintRange] = &[
    StateFootprintRange {
        offset: 0x008,
        length: 1,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "tx-gain-attenuation",
    },
    StateFootprintRange {
        offset: 0x0d0,
        length: 2,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "tx-gain-configuration",
    },
    StateFootprintRange {
        offset: 0x124,
        length: 1,
        access: StateAccess::Read,
        owner: execution::MemoryOwner::Cpu,
        name: "bluetooth-tx-gain-base",
    },
];

fn bluetooth_tx_gain_state_footprint() -> Vec<StateFootprintRange> {
    let mut ranges = Vec::new();
    ranges.extend_from_slice(RF_INIT_STATE_FOOTPRINT);
    ranges.extend_from_slice(BLUETOOTH_TX_POWER_STATE_FOOTPRINT);
    ranges.extend_from_slice(BLUETOOTH_TXDC_PWDET_STATE_FOOTPRINT);
    ranges.extend_from_slice(BLUETOOTH_TX_GAIN_PARENT_STATE_FOOTPRINT);
    ranges
}

pub fn vendor_bluetooth_tx_gain_init_scenario(
    phy_param: u32,
    phy_functions_pointer: u32,
) -> execution::Scenario {
    let mut scenario = vendor_bluetooth_txdc_pwdet_scenario(phy_param, phy_functions_pointer);
    scenario.arguments.clear();
    scenario.max_steps = 10_000_000;
    // Deterministic completion inputs required by the TXDC and TX-power
    // children in addition to the PWDET inputs installed above.
    scenario.mmio_initial.insert(0x2010_0418, 1 << 22);
    scenario.mmio_initial.insert(0x2010_082c, 0);
    // The PWDET scenario already owns the RF-init and three DCO rows. Add
    // only the remaining TX-power state: its read-only row-zero projection
    // overlaps the MMIO-derived PWDET output and must not weaken that owner.
    for range in BLUETOOTH_TX_POWER_STATE_FOOTPRINT
        .iter()
        .filter(|range| range.offset != 0x104)
    {
        declare_state_ownership(&mut scenario, phy_param, std::slice::from_ref(range));
    }
    declare_state_ownership(
        &mut scenario,
        phy_param,
        BLUETOOTH_TX_GAIN_PARENT_STATE_FOOTPRINT,
    );
    scenario
}

pub fn vendor_bluetooth_tx_gain_init_state_footprint(
    result: &execution::ExecutionResult,
    phy_param: u32,
) -> Result<StateFootprintStats> {
    let footprint = bluetooth_tx_gain_state_footprint();
    validate_state_footprint(
        "esp32s31-bluetooth-tx-gain-init",
        result,
        phy_param,
        VENDOR_PHY_PARAM_LEN,
        &footprint,
    )
}

pub(super) fn linked_symbol_range(
    vendor_artifact: &Path,
    symbol: &str,
) -> Result<std::ops::Range<u32>> {
    let definitions = crate::artifact::load_code_symbols(
        vendor_artifact,
        symbol,
        crate::artifact::CodeSymbolSelection::Exported,
    )?;
    let matches = definitions
        .iter()
        .filter(|definition| definition.name == symbol && definition.addresses_resolved)
        .collect::<Vec<_>>();
    let [definition] = matches.as_slice() else {
        return Err(format!(
            "expected one resolved linked definition for {symbol}, found {}",
            matches.len()
        )
        .into());
    };
    let start = u32::try_from(definition.address)
        .map_err(|_| format!("linked definition {symbol} exceeds RV32"))?;
    let length = u32::try_from(definition.bytes.len())
        .map_err(|_| format!("linked definition {symbol} is too large"))?;
    let end = start
        .checked_add(length)
        .ok_or_else(|| format!("linked definition {symbol} range overflows RV32"))?;
    Ok(start..end)
}

pub fn normalize_vendor_bluetooth_tx_gain_init(
    vendor_artifact: &Path,
    image: &execution::ExecutableImage,
    result: &execution::ExecutionResult,
    phy_param: u32,
) -> Result<Vec<BluetoothTxGainInitEvent>> {
    let parent = linked_symbol_range(vendor_artifact, "phy_bt_tx_gain_init")?;
    let direct_calls = result
        .ordered_calls
        .iter()
        .filter(|call| parent.contains(&call.site))
        .collect::<Vec<_>>();
    let expected = [
        "phy_set_channel_rfpll_freq",
        "phy_set_txcap_reg",
        "phy_bt_txdc_cal_new",
        "phy_bt_tx_pwctrl_init",
        "phy_txdc_cal_pwdet_init",
        "phy_bt_set_tx_gain_new",
    ];
    if direct_calls
        .iter()
        .map(|call| call.symbol.as_str())
        .ne(expected)
    {
        return Err(format!(
            "vendor BT gain parent direct-call order changed: {:?}",
            direct_calls
                .iter()
                .map(|call| call.symbol.as_str())
                .collect::<Vec<_>>()
        )
        .into());
    }
    for call in &direct_calls {
        let arguments = call.arguments;
        match call.symbol.as_str() {
            "phy_set_channel_rfpll_freq" => {
                if arguments[0] != 0x985 || arguments[2] != 0 {
                    return Err(format!(
                        "vendor BT gain parent requested unexpected RFPLL tuple ({:#x}, {:#x}, {:#x})",
                        arguments[0], arguments[1], arguments[2]
                    )
                    .into());
                }
            }
            "phy_set_txcap_reg" => {
                if arguments[1] != 6 {
                    return Err(format!(
                        "vendor BT gain parent requested TX-cap channel {}",
                        arguments[1]
                    )
                    .into());
                }
            }
            "phy_bt_txdc_cal_new" | "phy_bt_tx_pwctrl_init" => {}
            "phy_txdc_cal_pwdet_init" => {
                if arguments[..3] != [1, 0, 1] {
                    return Err(format!(
                        "vendor BT gain parent requested unexpected PWDET tuple ({}, {}, {})",
                        arguments[0], arguments[1], arguments[2]
                    )
                    .into());
                }
            }
            "phy_bt_set_tx_gain_new" => {
                if arguments[0] != 0 {
                    return Err(format!(
                        "vendor BT gain parent requested unexpected publication mode {}",
                        arguments[0]
                    )
                    .into());
                }
            }
            symbol => {
                return Err(format!(
                    "vendor BT gain parent emitted unreviewed direct call {symbol} at {:#010x}",
                    call.site
                )
                .into());
            }
        }
    }
    let child_is_active = |symbol: &str| -> Result<bool> {
        let range = linked_symbol_range(vendor_artifact, symbol)?;
        Ok(result
            .ordered_calls
            .iter()
            .any(|call| range.contains(&call.site)))
    };
    let mut events = vec![
        BluetoothTxGainInitEvent::ConfigureRfpll,
        BluetoothTxGainInitEvent::ConfigureTxCap,
    ];
    if child_is_active("phy_bt_txdc_cal_new")? {
        events.push(BluetoothTxGainInitEvent::CalibrateTxDc);
    }
    if child_is_active("phy_bt_tx_pwctrl_init")? {
        events.push(BluetoothTxGainInitEvent::CalibrateTxPower);
    }
    events.push(BluetoothTxGainInitEvent::CalibrateTxDcPwdet);
    events.push(BluetoothTxGainInitEvent::PublishGain);
    let dco = bluetooth_txdc_projection(image, result, phy_param)?;
    let tx_power = vendor_bluetooth_tx_power_projection(image, result, phy_param)?;
    events.push(BluetoothTxGainInitEvent::Complete {
        tx_dc_calibrated: true,
        dco,
        tx_power: BluetoothTxGainPowerState {
            power_curve: tx_power.power_curve,
            power_adjustment: tx_power.power_adjustment,
            attenuation: tx_power.attenuation,
            calibrated: tx_power.calibrated,
        },
    });
    Ok(events)
}

fn push_phase(events: &mut Vec<BluetoothTxGainInitEvent>, phase: BluetoothTxGainInitEvent) {
    if events.last() != Some(&phase) {
        events.push(phase);
    }
}

pub fn rust_bluetooth_tx_gain_init_events(
    mut state: PhyState,
) -> Result<(Vec<BluetoothTxGainInitEvent>, PhyState)> {
    let mut transition = state.bluetooth_tx_gain_init_transition();
    let mut events = Vec::new();
    let mut completion_driver = DeterministicPhyCompletion::default();
    for _ in 0..5_000_000 {
        match transition
            .step_local()
            .map_err(|error| format!("Rust BT gain parent failed locally: {error:?}"))?
        {
            PhyBluetoothTxGainInitLocalStep::StateAdvanced => continue,
            PhyBluetoothTxGainInitLocalStep::External(action) => {
                match action {
                    PhyBluetoothTxGainInitAction::Rfpll(_) => {
                        push_phase(&mut events, BluetoothTxGainInitEvent::ConfigureRfpll);
                    }
                    PhyBluetoothTxGainInitAction::TxCap(_) => {
                        push_phase(&mut events, BluetoothTxGainInitEvent::ConfigureTxCap);
                    }
                    PhyBluetoothTxGainInitAction::TxDc(_) => {
                        push_phase(&mut events, BluetoothTxGainInitEvent::CalibrateTxDc);
                    }
                    PhyBluetoothTxGainInitAction::TxPower(_) => {
                        push_phase(&mut events, BluetoothTxGainInitEvent::CalibrateTxPower);
                    }
                    PhyBluetoothTxGainInitAction::TxDcPwdet(_) => {
                        push_phase(&mut events, BluetoothTxGainInitEvent::CalibrateTxDcPwdet);
                    }
                    PhyBluetoothTxGainInitAction::Publish(_) => {
                        push_phase(&mut events, BluetoothTxGainInitEvent::PublishGain);
                    }
                }
                let completion = completion_driver.bluetooth_tx_gain(action)?;
                transition.advance_external(completion).map_err(|error| {
                    format!("Rust BT gain parent rejected completion: {error:?}")
                })?;
            }
            PhyBluetoothTxGainInitLocalStep::Complete(outcome) => {
                state.apply_bluetooth_tx_gain_init_outcome(outcome);
                let gain = state.bluetooth_tx_gain_parameters();
                let tx_power = state.bluetooth_tx_power_parameters();
                events.push(BluetoothTxGainInitEvent::Complete {
                    tx_dc_calibrated: outcome.tx_dc_calibrated,
                    dco: outcome.dco,
                    tx_power: BluetoothTxGainPowerState {
                        power_curve: gain.calibration_curve.map(|value| value as i8),
                        power_adjustment: gain.correction,
                        attenuation: tx_power.calibration.initial_attenuation,
                        calibrated: state.bluetooth_tx_power_calibrated(),
                    },
                });
                return Ok((events, state));
            }
            PhyBluetoothTxGainInitLocalStep::Failed(failure) => {
                return Err(format!("Rust BT gain parent failed: {failure:?}").into());
            }
        }
    }
    Err("Rust BT gain parent exceeded its semantic step bound".into())
}
