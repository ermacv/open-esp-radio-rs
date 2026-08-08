//! Machine-readable concrete equivalence profiles.

use std::{collections::BTreeSet, fs, path::Path, sync::Arc};

use crate::{
    MemoryObservation, NamedScenario, Result, add_memory_instance_binding, add_table_slot,
    execution, observe_memory, parse_assignment, parse_memory_instance, parse_symbol_observation,
    parse_symbol_word, parse_table_instance, parse_table_slot, parse_u32, seed_ram_word,
};

use super::dispositions::validate_source_id;

#[derive(Clone, Debug)]
pub struct Profile {
    pub name: String,
    pub vendor_source: String,
    pub vendor_symbol: String,
    pub rust_symbol: String,
    pub contract: ProfileContract,
    pub compare_return: bool,
    pub argument_ranges: Vec<ArgumentRange>,
    pub scenarios: Vec<NamedScenario>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ArgumentRange {
    pub index: usize,
    pub min: u32,
    pub max: u32,
}

impl Profile {
    /// Enumerates the explicitly admissible ABI argument domain for static
    /// reachability. Arguments without an `arg-range` remain unknown.
    pub fn coverage_argument_constraints(&self) -> Vec<[Option<u32>; 8]> {
        let mut constraints = vec![[None; 8]];
        for range in &self.argument_ranges {
            let mut expanded = Vec::new();
            for constraint in constraints {
                for value in range.min..=range.max {
                    let mut constraint = constraint;
                    constraint[range.index] = Some(value);
                    expanded.push(constraint);
                }
            }
            constraints = expanded;
        }
        constraints
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ProfileContract {
    #[default]
    Scenario,
    State,
}

impl ProfileContract {
    pub const fn evidence(self) -> &'static str {
        match self {
            Self::Scenario => "scenario",
            Self::State => "state",
        }
    }
}

#[derive(Default)]
struct ProfileBuilder {
    name: String,
    line: usize,
    vendor_source: Option<String>,
    vendor_symbol: Option<String>,
    rust_symbol: Option<String>,
    contract: ProfileContract,
    compare_return: bool,
    argument_ranges: Vec<ArgumentRange>,
    scenarios: Vec<NamedScenario>,
    current_scenario: Option<NamedScenario>,
}

impl ProfileBuilder {
    fn finish_scenario(&mut self) {
        if let Some(scenario) = self.current_scenario.take() {
            self.scenarios.push(scenario);
        }
    }

    fn finish(mut self) -> Result<Profile> {
        let line = self.line;
        (|| {
            self.finish_scenario();
            if self.scenarios.is_empty() {
                return Err(crate::Error::invalid(format!(
                    "profile {} has no cases",
                    self.name
                )));
            }
            self.argument_ranges.sort_by_key(|range| range.index);
            validate_argument_domain(&self.name, &self.argument_ranges, &self.scenarios)?;
            Ok(Profile {
                name: self.name,
                vendor_source: self
                    .vendor_source
                    .ok_or("profile has no vendor-source")
                    .map_err(crate::Error::invalid)?,
                vendor_symbol: self
                    .vendor_symbol
                    .ok_or("profile has no vendor-symbol")
                    .map_err(crate::Error::invalid)?,
                rust_symbol: self
                    .rust_symbol
                    .ok_or("profile has no rust-symbol")
                    .map_err(crate::Error::invalid)?,
                contract: self.contract,
                compare_return: self.compare_return,
                argument_ranges: self.argument_ranges,
                scenarios: self.scenarios,
            })
        })()
        .map_err(|error: crate::error::WorkbenchError| error.at_line(line))
    }

    fn scenario(&mut self, line: usize) -> Result<&mut execution::Scenario> {
        self.current_scenario
            .as_mut()
            .map(|scenario| &mut scenario.scenario)
            .ok_or_else(|| {
                crate::Error::invalid(format!("profile directive before case at line {line}"))
            })
    }
}

fn parse_argument_range(value: &str, line: usize) -> Result<ArgumentRange> {
    let fields = value.split_whitespace().collect::<Vec<_>>();
    let [index, min, max] = fields.as_slice() else {
        return Err(crate::Error::invalid(format!(
            "arg-range requires INDEX MIN MAX at line {line}"
        )));
    };
    let index = parse_u32(index)
        .and_then(|index| usize::try_from(index).ok())
        .ok_or_else(|| format!("invalid arg-range index at line {line}"))
        .map_err(crate::Error::invalid)?;
    let min = parse_u32(min)
        .ok_or_else(|| format!("invalid arg-range minimum at line {line}"))
        .map_err(crate::Error::invalid)?;
    let max = parse_u32(max)
        .ok_or_else(|| format!("invalid arg-range maximum at line {line}"))
        .map_err(crate::Error::invalid)?;
    Ok(ArgumentRange { index, min, max })
}

fn parse_device_model(value: &str, line: usize) -> Result<crate::execution_model::DeviceModelSpec> {
    let fields = value.split_whitespace().collect::<Vec<_>>();
    let number = |value: &str, field: &str| {
        parse_u32(value)
            .ok_or_else(|| format!("invalid device-model {field} at line {line}"))
            .map_err(crate::Error::invalid)
    };
    let width = |value: &str| {
        u8::try_from(number(value, "width")?).map_err(|_| {
            crate::Error::invalid(format!("invalid device-model width at line {line}"))
        })
    };
    let list = |value: &str, field: &str| -> Result<Vec<u32>> {
        if value == "-" {
            return Ok(Vec::new());
        }
        value.split(',').map(|value| number(value, field)).collect()
    };
    Ok(match fields.as_slice() {
        ["constant-read", id, address, bits, value] => {
            crate::execution_model::DeviceModelSpec::ConstantRead {
                id: (*id).to_owned(),
                address: number(address, "address")?,
                width: width(bits)?,
                value: number(value, "value")?,
            }
        }
        ["sequence-read", id, address, bits, values] => {
            crate::execution_model::DeviceModelSpec::SequenceRead {
                id: (*id).to_owned(),
                address: number(address, "address")?,
                width: width(bits)?,
                values: list(values, "sequence value")?,
            }
        }
        ["w1c", id, address, bits, initial, clear, read_clear] => {
            crate::execution_model::DeviceModelSpec::W1c {
                id: (*id).to_owned(),
                address: number(address, "address")?,
                width: width(bits)?,
                initial_value: number(initial, "initial value")?,
                clear_mask: number(clear, "clear mask")?,
                read_clear_mask: number(read_clear, "read-clear mask")?,
            }
        }
        ["read-to-clear", id, address, bits, initial, clear] => {
            crate::execution_model::DeviceModelSpec::ReadToClear {
                id: (*id).to_owned(),
                address: number(address, "address")?,
                width: width(bits)?,
                initial_value: number(initial, "initial value")?,
                clear_mask: number(clear, "clear mask")?,
            }
        }
        ["self-clearing", id, address, bits, initial, store, command] => {
            crate::execution_model::DeviceModelSpec::SelfClearing {
                id: (*id).to_owned(),
                address: number(address, "address")?,
                width: width(bits)?,
                initial_value: number(initial, "initial value")?,
                store_mask: number(store, "store mask")?,
                command_mask: number(command, "command mask")?,
            }
        }
        ["fifo", id, address, bits, reads, writes] => {
            crate::execution_model::DeviceModelSpec::Fifo {
                id: (*id).to_owned(),
                address: number(address, "address")?,
                width: width(bits)?,
                read_values: list(reads, "FIFO read value")?,
                expected_writes: list(writes, "FIFO expected write")?,
            }
        }
        [
            "indexed-bank",
            id,
            index_address,
            data_address,
            bits,
            values,
        ] => crate::execution_model::DeviceModelSpec::IndexedBank {
            id: (*id).to_owned(),
            index_address: number(index_address, "index address")?,
            data_address: number(data_address, "data address")?,
            width: width(bits)?,
            initial_values: list(values, "bank value")?,
        },
        _ => {
            return Err(crate::Error::invalid(format!(
                "device-model requires a supported KIND and its arguments at line {line}"
            )));
        }
    })
}

fn validate_argument_domain(
    profile: &str,
    ranges: &[ArgumentRange],
    scenarios: &[NamedScenario],
) -> Result<()> {
    const MAX_DOMAIN_CASES: u64 = 4_096;

    for pair in ranges.windows(2) {
        if pair[0].index == pair[1].index {
            return Err(crate::Error::invalid(format!(
                "profile {profile} repeats arg-range {}",
                pair[0].index
            )));
        }
    }
    let mut domain_size = 1_u64;
    for range in ranges {
        if range.index >= 8 {
            return Err(crate::Error::invalid(format!(
                "profile {profile} arg-range index exceeds RV32 ABI a0..a7"
            )));
        }
        if range.min > range.max {
            return Err(crate::Error::invalid(format!(
                "profile {profile} arg-range {} has minimum above maximum",
                range.index
            )));
        }
        domain_size = domain_size
            .checked_mul(u64::from(range.max) - u64::from(range.min) + 1)
            .ok_or_else(|| format!("profile {profile} argument domain overflows"))
            .map_err(crate::Error::invalid)?;
        if domain_size > MAX_DOMAIN_CASES {
            return Err(crate::Error::invalid(format!(
                "profile {profile} argument domain has {domain_size} cases; maximum is {MAX_DOMAIN_CASES}"
            )));
        }
    }

    if ranges.is_empty() {
        return Ok(());
    }

    let mut covered = BTreeSet::new();
    for scenario in scenarios {
        let mut projection = [None; 8];
        for range in ranges {
            let value = scenario
                .scenario
                .arguments
                .get(range.index)
                .copied()
                .ok_or_else(|| {
                    format!(
                        "profile {profile} case {} does not provide constrained argument {}",
                        scenario.name, range.index
                    )
                })
                .map_err(crate::Error::invalid)?;
            if !(range.min..=range.max).contains(&value) {
                return Err(crate::Error::invalid(format!(
                    "profile {profile} case {} argument {} value {value:#x} is outside {:#x}..={:#x}",
                    scenario.name, range.index, range.min, range.max
                )));
            }
            projection[range.index] = Some(value);
        }
        covered.insert(projection);
    }

    let profile_stub = Profile {
        name: String::new(),
        vendor_source: String::new(),
        vendor_symbol: String::new(),
        rust_symbol: String::new(),
        contract: ProfileContract::Scenario,
        compare_return: false,
        argument_ranges: ranges.to_vec(),
        scenarios: Vec::new(),
    };
    for expected in profile_stub.coverage_argument_constraints() {
        if !covered.contains(&expected) {
            let values = ranges
                .iter()
                .map(|range| format!("a{}={:#x}", range.index, expected[range.index].unwrap()))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(crate::Error::invalid(format!(
                "profile {profile} has no case for admissible argument combination {values}"
            )));
        }
    }
    Ok(())
}

fn split_directive(line: &str, line_number: usize) -> Result<(&str, &str)> {
    line.split_once(char::is_whitespace)
        .map(|(directive, value)| (directive, value.trim()))
        .filter(|(_, value)| !value.is_empty())
        .ok_or_else(|| {
            crate::Error::invalid(format!(
                "profile directive needs a value at line {line_number}"
            ))
        })
}

#[tracing::instrument(name = "load_verification_profiles", fields(path = %path.display()))]
pub fn load(path: &Path) -> Result<Vec<Profile>> {
    let input = fs::read_to_string(path)?;
    parse(&input).map_err(|error| {
        crate::error::WorkbenchError::manifest_document(
            "verification profile manifest",
            path,
            &input,
            error,
        )
    })
}

fn parse(input: &str) -> Result<Vec<Profile>> {
    let mut profiles = Vec::new();
    let mut current: Option<ProfileBuilder> = None;

    for (index, raw_line) in input.lines().enumerate() {
        let line_number = index + 1;
        let line = raw_line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        (|| -> Result<()> {
            let (directive, value) = split_directive(line, line_number)?;
            if directive == "profile" {
                if let Some(profile) = current.take() {
                    profiles.push(profile.finish()?);
                }
                current = Some(ProfileBuilder {
                    name: value.to_owned(),
                    line: line_number,
                    ..ProfileBuilder::default()
                });
                return Ok(());
            }
            let profile = current
                .as_mut()
                .ok_or_else(|| format!("directive before profile at line {line_number}"))
                .map_err(crate::Error::invalid)?;
            match directive {
                "vendor-source" => {
                    validate_source_id(value, line_number)?;
                    profile.vendor_source = Some(value.to_owned());
                }
                "vendor-symbol" => profile.vendor_symbol = Some(value.to_owned()),
                "rust-symbol" => profile.rust_symbol = Some(value.to_owned()),
                "contract" => {
                    profile.contract = match value {
                        "scenario" => ProfileContract::Scenario,
                        "state" => ProfileContract::State,
                        _ => {
                            return Err(crate::Error::invalid(format!(
                                "invalid contract {value:?} at line {line_number}"
                            )));
                        }
                    };
                }
                "compare-return" => {
                    profile.compare_return = value
                        .parse()
                        .map_err(|_| format!("invalid boolean at line {line_number}"))
                        .map_err(crate::Error::invalid)?;
                }
                "arg-range" => profile
                    .argument_ranges
                    .push(parse_argument_range(value, line_number)?),
                "case" => {
                    profile.finish_scenario();
                    profile.current_scenario = Some(NamedScenario::new(value.to_owned()));
                }
                "arg" => profile.scenario(line_number)?.arguments.push(
                    parse_u32(value)
                        .ok_or_else(|| format!("invalid arg at line {line_number}"))
                        .map_err(crate::Error::invalid)?,
                ),
                "mmio" => {
                    let (address, value) = parse_assignment(value, "mmio")?;
                    profile
                        .scenario(line_number)?
                        .mmio_initial
                        .insert(address, value);
                }
                "read" => {
                    let (address, value) = parse_assignment(value, "read")?;
                    profile
                        .scenario(line_number)?
                        .mmio_reads
                        .entry(address)
                        .or_default()
                        .push_back(value);
                }
                "device-model" => {
                    let model = parse_device_model(value, line_number)?;
                    profile
                        .scenario(line_number)?
                        .device_models
                        .push(Arc::new(model));
                }
                "ram" => {
                    let (address, value) = parse_assignment(value, "ram")?;
                    seed_ram_word(profile.scenario(line_number)?, address, value);
                }
                "vendor-ram" | "rust-ram" => {
                    let word = parse_assignment(value, directive)?;
                    let scenario = profile
                        .current_scenario
                        .as_mut()
                        .ok_or_else(|| {
                            format!("profile directive before case at line {line_number}")
                        })
                        .map_err(crate::Error::invalid)?;
                    if directive == "vendor-ram" {
                        scenario.vendor_ram_words.push(word);
                    } else {
                        scenario.rust_ram_words.push(word);
                    }
                }
                "vendor-ram-symbol" => profile
                    .current_scenario
                    .as_mut()
                    .ok_or_else(|| format!("profile directive before case at line {line_number}"))
                    .map_err(crate::Error::invalid)?
                    .vendor_symbol_words
                    .push(parse_symbol_word(value, "vendor-ram-symbol")?),
                "rust-ram-symbol" => profile
                    .current_scenario
                    .as_mut()
                    .ok_or_else(|| format!("profile directive before case at line {line_number}"))
                    .map_err(crate::Error::invalid)?
                    .rust_symbol_words
                    .push(parse_symbol_word(value, "rust-ram-symbol")?),
                "vendor-table" | "rust-table" => {
                    let instance = parse_table_instance(value, directive)?;
                    let scenario = profile
                        .current_scenario
                        .as_mut()
                        .ok_or_else(|| {
                            format!("profile directive before case at line {line_number}")
                        })
                        .map_err(crate::Error::invalid)?;
                    if directive == "vendor-table" {
                        scenario.vendor_table_instances.push(instance);
                    } else {
                        scenario.rust_table_instances.push(instance);
                    }
                }
                "vendor-table-slot" | "rust-table-slot" => {
                    let (layout_id, slot) = parse_table_slot(value, directive)?;
                    let scenario = profile
                        .current_scenario
                        .as_mut()
                        .ok_or_else(|| {
                            format!("profile directive before case at line {line_number}")
                        })
                        .map_err(crate::Error::invalid)?;
                    let instances = if directive == "vendor-table-slot" {
                        &mut scenario.vendor_table_instances
                    } else {
                        &mut scenario.rust_table_instances
                    };
                    add_table_slot(instances, &layout_id, slot, directive)?;
                }
                "vendor-memory-instance" | "rust-memory-instance" => {
                    let instance = parse_memory_instance(value, directive)?;
                    let scenario = profile
                        .current_scenario
                        .as_mut()
                        .ok_or_else(|| {
                            format!("profile directive before case at line {line_number}")
                        })
                        .map_err(crate::Error::invalid)?;
                    let instances = if directive == "vendor-memory-instance" {
                        &mut scenario.vendor_memory_instances
                    } else {
                        &mut scenario.rust_memory_instances
                    };
                    instances.push(instance);
                }
                "vendor-memory-binding" | "rust-memory-binding" => {
                    let scenario = profile
                        .current_scenario
                        .as_mut()
                        .ok_or_else(|| {
                            format!("profile directive before case at line {line_number}")
                        })
                        .map_err(crate::Error::invalid)?;
                    let instances = if directive == "vendor-memory-binding" {
                        &mut scenario.vendor_memory_instances
                    } else {
                        &mut scenario.rust_memory_instances
                    };
                    add_memory_instance_binding(instances, value, directive)?;
                }
                "vendor-observe" | "rust-observe" => {
                    let (address, length) = parse_assignment(value, directive)?;
                    if length == 0 {
                        return Err(crate::Error::invalid(format!(
                            "{directive} length must be non-zero"
                        )));
                    }
                    let observation = MemoryObservation::Absolute { address, length };
                    let scenario = profile
                        .current_scenario
                        .as_mut()
                        .ok_or_else(|| {
                            format!("profile directive before case at line {line_number}")
                        })
                        .map_err(crate::Error::invalid)?;
                    if directive == "vendor-observe" {
                        scenario.vendor_observations.push(observation);
                    } else {
                        scenario.rust_observations.push(observation);
                    }
                }
                "vendor-observe-symbol" | "rust-observe-symbol" => {
                    let observation = parse_symbol_observation(value, directive)?;
                    let scenario = profile
                        .current_scenario
                        .as_mut()
                        .ok_or_else(|| {
                            format!("profile directive before case at line {line_number}")
                        })
                        .map_err(crate::Error::invalid)?;
                    if directive == "vendor-observe-symbol" {
                        scenario.vendor_observations.push(observation);
                    } else {
                        scenario.rust_observations.push(observation);
                    }
                }
                "observe" => {
                    let (address, length) = parse_assignment(value, "observe")?;
                    observe_memory(profile.scenario(line_number)?, address, length)?;
                }
                "max-steps" => {
                    profile.scenario(line_number)?.max_steps = value
                        .parse()
                        .map_err(|_| format!("invalid max-steps at line {line_number}"))
                        .map_err(crate::Error::invalid)?;
                }
                _ => {
                    return Err(crate::Error::invalid(format!(
                        "unknown profile directive {directive} at line {line_number}"
                    )));
                }
            }
            Ok(())
        })()
        .map_err(|error| error.at_line(line_number))?;
    }
    if let Some(profile) = current {
        profiles.push(profile.finish()?);
    }
    if profiles.is_empty() {
        return Err(crate::Error::invalid("profile file contains no profiles"));
    }
    Ok(profiles)
}

#[cfg(test)]
#[path = "../harnesses/esp32s31/profiles_tests.rs"]
mod tests;
