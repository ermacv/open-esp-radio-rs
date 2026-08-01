//! Cold RF initialization projection.

use super::*;

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
    SymbolicValue(u32),
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

pub(super) const fn rf_phase(phase: RfInitPhase, parameters: RfInitPhaseParameters) -> RfInitEvent {
    RfInitEvent::Phase { phase, parameters }
}

pub fn vendor_rf_init_scenario(phy_param: u32, phy_functions_pointer: u32) -> execution::Scenario {
    let mut scenario = execution::Scenario {
        max_steps: 500_000,
        ..execution::Scenario::default()
    };
    seed_ram_word(&mut scenario, phy_functions_pointer, ROM_PHY_FUNCTION_TABLE);
    seed_ram_word(
        &mut scenario,
        ROM_PHY_FUNCTION_TABLE_POINTER,
        ROM_PHY_FUNCTION_TABLE,
    );
    seed_ram_word(&mut scenario, ROM_PHY_PARAM_POINTER, phy_param);
    for (offset, target) in
        entry_contract::function_targets(entry_contract::FunctionTable::Esp32s31PhyCold)
    {
        let entry_contract::FunctionTarget::Address(value) = target else {
            unreachable!("the cold ROM PHY table contains only absolute ROM targets")
        };
        seed_ram_word(&mut scenario, ROM_PHY_FUNCTION_TABLE + offset, value);
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

pub(super) fn vendor_rf_init_phase(call: &execution::OrderedCall) -> Result<RfInitPhase> {
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

fn timeline_first_ram_byte(result: &execution::ExecutionResult, wanted: u32) -> Option<u8> {
    result.timeline.iter().find_map(|event| {
        let execution::ExecutionTimelineEvent::RamRead {
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
    call: &execution::OrderedCall,
    phase: RfInitPhase,
    rc_already_complete: bool,
) -> RfInitPhaseParameters {
    match phase {
        RfInitPhase::ConfigureBbpllCalibration => {
            RfInitPhaseParameters::Enabled(call.arguments[0] != 0)
        }
        RfInitPhase::PostOpenI2cDelay | RfInitPhase::ConfigureI2cClockSelection => {
            RfInitPhaseParameters::SymbolicValue(call.arguments[0])
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
    image: &execution::ExecutableImage,
    result: &execution::ExecutionResult,
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
    image: &execution::ExecutableImage,
    result: &execution::ExecutionResult,
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
            RfInitPhaseParameters::SymbolicValue(micros),
        ),
        PhyRfInitPrefixAction::PbusClear(_) => plain(RfInitPhase::ClearPbus),
        PhyRfInitPrefixAction::ConfigureI2cClockSelection { selection } => (
            RfInitPhase::ConfigureI2cClockSelection,
            RfInitPhaseParameters::SymbolicValue(selection),
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
