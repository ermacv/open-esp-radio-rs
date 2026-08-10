//! Hierarchical qualification of the cold `register_chipv7_phy` parent.

use super::*;

const REGISTER_CALIBRATION_IMAGE: u32 = 0x1001_0000;
const REGISTER_CALIBRATION_IMAGE_LEN: u32 = 0x20c;
const REGISTER_CALIBRATION_FOOTPRINT: &[StateFootprintRange] = &[register_state_range(
    0,
    REGISTER_CALIBRATION_IMAGE_LEN,
    StateAccess::ReadWrite,
    "caller-owned-calibration-image",
)];

const REGISTER_STATE_FOOTPRINT: &[StateFootprintRange] = &[
    register_state_range(0x000, 0x1fc, StateAccess::Read, "serialized-phy-state"),
    register_state_range(0x000, 2, StateAccess::Write, "temperature-result"),
    register_state_range(0x004, 2, StateAccess::Write, "registration-defaults"),
    register_state_range(0x016, 1, StateAccess::Write, "temperature-range"),
    register_state_range(0x018, 1, StateAccess::Write, "power-attenuation"),
    register_state_range(0x01a, 4, StateAccess::Write, "sar-reference-state"),
    register_state_range(0x025, 1, StateAccess::Write, "registered-flag"),
    register_state_range(0x030, 0x1b, StateAccess::Write, "txdc-and-rf-results"),
    register_state_range(0x04e, 0x14, StateAccess::Write, "rf-parameter-defaults"),
    register_state_range(0x064, 0x34, StateAccess::Write, "calibration-defaults"),
    register_state_range(0x0a4, 0x2e, StateAccess::Write, "flags-and-gain-tables"),
    register_state_range(0x0d4, 0x0e, StateAccess::Write, "tx-capacitance-state"),
    register_state_range(0x0e6, 5, StateAccess::Write, "rf-calibration-state"),
    register_state_range(0x0ed, 0x12, StateAccess::Write, "tx-gain-results"),
    register_state_range(0x100, 2, StateAccess::Write, "tx-power-result-tail"),
    register_state_range(0x104, 0x1e, StateAccess::Write, "dco-and-channel-results"),
    register_state_range(0x12e, 4, StateAccess::Write, "registration-control"),
    register_state_range(0x14e, 0x24, StateAccess::Write, "rxiq-results"),
    register_state_range(0x18e, 1, StateAccess::Write, "rfpll-parameter"),
    register_state_range(0x190, 2, StateAccess::Write, "rx-gain-prefix"),
    register_state_range(0x198, 4, StateAccess::Write, "rx-gain-header"),
    register_state_range(0x19e, 0x0b, StateAccess::Write, "xtal-and-rx-gain-state"),
    register_state_range(0x1aa, 1, StateAccess::Write, "tone-read-policy"),
    register_state_range(0x1ac, 2, StateAccess::Write, "calibration-scratch"),
    register_state_range(0x1b4, 0x48, StateAccess::Write, "rx-gain-tables"),
];

const fn register_state_range(
    offset: u32,
    length: u32,
    access: StateAccess,
    name: &'static str,
) -> StateFootprintRange {
    StateFootprintRange {
        offset,
        length,
        access,
        owner: execution::MemoryOwner::Cpu,
        name,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegisterInitChildPhase {
    Rf,
    Baseband,
    Temperature,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegisterInitEvent {
    Parent(RegisterInitParentPhase),
    Child(RegisterInitChildPhase),
    Complete { full_calibration_performed: bool },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegisterInitParentPhase {
    PrepareColdStart,
    ConfigureForceTxRx(bool),
    ResetFrequencyModule,
    SetHardwareFrequencyControl(bool),
    ResetI2cMaster(u8),
    ConfigureXtal40Mhz,
    SetCalibrationClock(bool),
    SetBbpllCalibration(bool),
    ReadFinalI2c,
}

pub fn vendor_register_init_scenario(
    phy_param: u32,
    phy_functions_pointer: u32,
) -> execution::Scenario {
    let mut scenario = vendor_baseband_init_scenario(phy_param, phy_functions_pointer);
    scenario.arguments = vec![0, REGISTER_CALIBRATION_IMAGE, 0];
    scenario.max_steps = 2_000_000;
    scenario
        .call_returns
        .entry("rtc_clk_xtal_freq_get".to_owned())
        .or_default()
        .push_back(40);
    scenario.mmio_initial.insert(0x2010_f028, 0);
    scenario.mmio_initial.insert(0x2010_f804, 0x5b << 16);
    scenario.mmio_initial.insert(0x2071_5050, 0);
    scenario.mmio_initial.insert(0x2071_5054, 0);
    let calibration_image = crate::execution_model::MemoryRange {
        start: REGISTER_CALIBRATION_IMAGE,
        length: REGISTER_CALIBRATION_IMAGE_LEN,
    };
    scenario.persistent_memory.push(calibration_image);
    scenario.observed_memory.push(calibration_image);
    for address in REGISTER_CALIBRATION_IMAGE..REGISTER_CALIBRATION_IMAGE + calibration_image.length
    {
        scenario.memory_initial.insert(address, 0);
    }
    scenario
}

pub fn vendor_register_init_state_footprint(
    result: &execution::ExecutionResult,
    phy_param: u32,
) -> Result<StateFootprintStats> {
    let state = validate_state_footprint(
        "esp32s31-register-init",
        result,
        phy_param,
        VENDOR_PHY_PARAM_LEN,
        REGISTER_STATE_FOOTPRINT,
    )?;
    let calibration = validate_state_footprint(
        "esp32s31-register-calibration-image",
        result,
        REGISTER_CALIBRATION_IMAGE,
        REGISTER_CALIBRATION_IMAGE_LEN,
        REGISTER_CALIBRATION_FOOTPRINT,
    )?;
    Ok(StateFootprintStats {
        read_bytes: state.read_bytes + calibration.read_bytes,
        written_bytes: state.written_bytes + calibration.written_bytes,
        classified_ranges: state.classified_ranges + calibration.classified_ranges,
    })
}

pub fn normalize_vendor_register_init(
    vendor_artifact: &Path,
    result: &execution::ExecutionResult,
) -> Result<Vec<RegisterInitEvent>> {
    let parent = linked_symbol_range(vendor_artifact, "register_chipv7_phy")?;
    let calls = result
        .ordered_calls
        .iter()
        .filter(|call| parent.contains(&call.site))
        .collect::<Vec<_>>();
    let expected = [
        "memset",
        "phy_get_romfunc_addr",
        "phy_force_txrx_off",
        "phy_freq_module_resetn",
        "phy_dis_hw_set_freq",
        "phy_i2c_master_reset",
        "register_chipv7_phy_init_param",
        "phy_get_xtal_freq",
        "phy_get_rf_cal_version",
        "phy_rf_init",
        "phy_bb_init",
        "phy_get_temp_init",
        "phy_rf_cal_data_backup_new",
        "phy_get_rf_cal_version",
        "phy_rfcal_data_check_new",
        "phy_bbpll_cal",
        "phy_i2c_readReg",
        "phy_en_hw_set_freq",
        "phy_force_txrx_off",
    ];
    let observed = calls
        .iter()
        .map(|call| call.symbol.as_str())
        .collect::<Vec<_>>();
    if observed != expected {
        return Err(format!(
            "vendor register parent call topology changed: observed={observed:?} expected={expected:?}"
        )
        .into());
    }
    let expect_argument = |index: usize, argument: usize, value: u32| -> Result<()> {
        if calls[index].arguments[argument] == value {
            Ok(())
        } else {
            Err(format!(
                "vendor {} argument {argument} is {:#x}, expected {value:#x}",
                calls[index].symbol, calls[index].arguments[argument]
            )
            .into())
        }
    };
    expect_argument(2, 0, 1)?;
    expect_argument(5, 0, 2)?;
    expect_argument(11, 0, 1)?;
    expect_argument(11, 1, 1)?;
    expect_argument(15, 0, 0)?;
    expect_argument(16, 0, 0x63)?;
    expect_argument(16, 1, 1)?;
    expect_argument(16, 2, 0)?;
    expect_argument(18, 0, 0)?;
    if result.return_value != 0 {
        return Err(format!(
            "vendor register parent returned {:#010x}, expected success",
            result.return_value
        )
        .into());
    }
    Ok(vec![
        RegisterInitEvent::Parent(RegisterInitParentPhase::PrepareColdStart),
        RegisterInitEvent::Parent(RegisterInitParentPhase::ConfigureForceTxRx(true)),
        RegisterInitEvent::Parent(RegisterInitParentPhase::ResetFrequencyModule),
        RegisterInitEvent::Parent(RegisterInitParentPhase::SetHardwareFrequencyControl(false)),
        RegisterInitEvent::Parent(RegisterInitParentPhase::ConfigureXtal40Mhz),
        RegisterInitEvent::Parent(RegisterInitParentPhase::SetCalibrationClock(true)),
        RegisterInitEvent::Child(RegisterInitChildPhase::Rf),
        RegisterInitEvent::Child(RegisterInitChildPhase::Baseband),
        RegisterInitEvent::Parent(RegisterInitParentPhase::SetCalibrationClock(false)),
        RegisterInitEvent::Child(RegisterInitChildPhase::Temperature),
        RegisterInitEvent::Parent(RegisterInitParentPhase::SetBbpllCalibration(false)),
        RegisterInitEvent::Parent(RegisterInitParentPhase::ReadFinalI2c),
        RegisterInitEvent::Parent(RegisterInitParentPhase::SetHardwareFrequencyControl(true)),
        RegisterInitEvent::Parent(RegisterInitParentPhase::ConfigureForceTxRx(false)),
        RegisterInitEvent::Complete {
            full_calibration_performed: true,
        },
    ])
}

fn parent_phase(
    action: open_esp_radio_esp32s31_phy::phy_register::PhyRegisterMmioAction,
) -> RegisterInitParentPhase {
    use open_esp_radio_esp32s31_phy::phy_register::PhyRegisterMmioAction as Action;
    match action {
        Action::PrepareColdStart => RegisterInitParentPhase::PrepareColdStart,
        Action::ConfigureForceTxRx { enabled, .. } => {
            RegisterInitParentPhase::ConfigureForceTxRx(enabled)
        }
        Action::ResetFrequencyModule => RegisterInitParentPhase::ResetFrequencyModule,
        Action::SetHardwareFrequencyControl { enabled } => {
            RegisterInitParentPhase::SetHardwareFrequencyControl(enabled)
        }
        Action::PulseI2cMasterReset { index } => RegisterInitParentPhase::ResetI2cMaster(index),
        Action::ConfigureXtal40Mhz => RegisterInitParentPhase::ConfigureXtal40Mhz,
        Action::SetCalibrationClock { enabled } => {
            RegisterInitParentPhase::SetCalibrationClock(enabled)
        }
        Action::SetBbpllCalibration { enabled } => {
            RegisterInitParentPhase::SetBbpllCalibration(enabled)
        }
    }
}

fn push_event(events: &mut Vec<RegisterInitEvent>, event: RegisterInitEvent) {
    if events.last() != Some(&event) {
        events.push(event);
    }
}

pub fn rust_register_init_events() -> Result<Vec<RegisterInitEvent>> {
    use open_esp_radio_esp32s31_phy::phy_register::{
        PhyRegisterAction as Action, PhyRegisterLocalStep,
    };

    let mut transition =
        open_esp_radio_esp32s31_phy::PhyRegisterTransition::with_production_config();
    let mut completion_driver = DeterministicPhyCompletion::default();
    let mut events = Vec::new();
    for _ in 0..20_000_000 {
        match transition
            .step_local()
            .map_err(|error| format!("Rust register parent failed locally: {error:?}"))?
        {
            PhyRegisterLocalStep::StateAdvanced => {}
            PhyRegisterLocalStep::External(action) => {
                match action {
                    Action::Mmio(action) => {
                        push_event(&mut events, RegisterInitEvent::Parent(parent_phase(action)));
                    }
                    Action::Rf(_) => push_event(
                        &mut events,
                        RegisterInitEvent::Child(RegisterInitChildPhase::Rf),
                    ),
                    Action::Baseband(_) => push_event(
                        &mut events,
                        RegisterInitEvent::Child(RegisterInitChildPhase::Baseband),
                    ),
                    Action::Temperature(_) => push_event(
                        &mut events,
                        RegisterInitEvent::Child(RegisterInitChildPhase::Temperature),
                    ),
                    Action::ReadFinalI2c { .. } => push_event(
                        &mut events,
                        RegisterInitEvent::Parent(RegisterInitParentPhase::ReadFinalI2c),
                    ),
                    Action::DelayMicros { .. } | Action::SampleI2cMasterReset { .. } => {}
                }
                let completion = completion_driver.register(action)?;
                transition.advance_external(completion).map_err(|error| {
                    format!("Rust register parent rejected completion: {error:?}")
                })?;
            }
            PhyRegisterLocalStep::Complete(outcome) => {
                events.push(RegisterInitEvent::Complete {
                    full_calibration_performed: outcome.full_calibration_performed,
                });
                return Ok(events);
            }
            PhyRegisterLocalStep::Failed(failure) => {
                return Err(format!("Rust register parent failed: {failure:?}").into());
            }
        }
    }
    Err("Rust register parent exceeded its semantic step bound".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_register_parent_completes_in_the_shared_environment() {
        let events = rust_register_init_events().unwrap();
        assert_eq!(
            events.last(),
            Some(&RegisterInitEvent::Complete {
                full_calibration_performed: true,
            })
        );
    }
}
