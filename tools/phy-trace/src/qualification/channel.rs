//! Wi-Fi channel-transition projection.

use super::*;

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
) -> Result<execution::Scenario> {
    let mut scenario = execution::Scenario {
        arguments: vec![u32::from(channel_or_frequency), u32::from(cbw)],
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
    scenario.observed_memory.push(execution::MemoryRange {
        start: phy_param + 0x11c,
        length: 4,
    });
    Ok(scenario)
}

fn vendor_tx_gain_image(
    vendor_image: &execution::ExecutableImage,
    result: &execution::ExecutionResult,
    phy_param: u32,
    call: &execution::OrderedCall,
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
        if let execution::ExecutionTimelineEvent::RamWrite {
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
    image: &execution::ExecutableImage,
    result: &execution::ExecutionResult,
    phy_param: u32,
    channel_or_frequency: u16,
) -> Result<Vec<ChannelEvent>> {
    vendor_channel_state_footprint(result, phy_param)?;
    let mut events = Vec::new();
    let mut ready_samples = 0_u32;
    let mut publish_tx_cap_pending = false;

    for (timeline_index, timeline) in result.timeline.iter().enumerate() {
        match timeline {
            execution::ExecutionTimelineEvent::Observable(
                execution::ExecutionEvent::DelayMicros(micros),
            ) => {
                if *micros == 10 {
                    events.push(ChannelEvent::ClearFrequencySwitch);
                }
                events.push(ChannelEvent::DelayMicros(*micros));
            }
            execution::ExecutionTimelineEvent::Observable(execution::ExecutionEvent::Read {
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
            execution::ExecutionTimelineEvent::Call(call) => match call.symbol.as_str() {
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
