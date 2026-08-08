//! Machine-readable concrete equivalence profiles.

use std::{collections::BTreeSet, fs, path::Path};

use crate::{
    MemoryObservation, NamedScenario, Result, execution, observe_memory, parse_assignment,
    parse_symbol_observation, parse_symbol_word, parse_u32, seed_ram_word,
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
                return Err(format!("profile {} has no cases", self.name).into());
            }
            self.argument_ranges.sort_by_key(|range| range.index);
            validate_argument_domain(&self.name, &self.argument_ranges, &self.scenarios)?;
            Ok(Profile {
                name: self.name,
                vendor_source: self.vendor_source.ok_or("profile has no vendor-source")?,
                vendor_symbol: self.vendor_symbol.ok_or("profile has no vendor-symbol")?,
                rust_symbol: self.rust_symbol.ok_or("profile has no rust-symbol")?,
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
            .ok_or_else(|| format!("profile directive before case at line {line}").into())
    }
}

fn parse_argument_range(value: &str, line: usize) -> Result<ArgumentRange> {
    let fields = value.split_whitespace().collect::<Vec<_>>();
    let [index, min, max] = fields.as_slice() else {
        return Err(format!("arg-range requires INDEX MIN MAX at line {line}").into());
    };
    let index = parse_u32(index)
        .and_then(|index| usize::try_from(index).ok())
        .ok_or_else(|| format!("invalid arg-range index at line {line}"))?;
    let min = parse_u32(min).ok_or_else(|| format!("invalid arg-range minimum at line {line}"))?;
    let max = parse_u32(max).ok_or_else(|| format!("invalid arg-range maximum at line {line}"))?;
    Ok(ArgumentRange { index, min, max })
}

fn validate_argument_domain(
    profile: &str,
    ranges: &[ArgumentRange],
    scenarios: &[NamedScenario],
) -> Result<()> {
    const MAX_DOMAIN_CASES: u64 = 4_096;

    for pair in ranges.windows(2) {
        if pair[0].index == pair[1].index {
            return Err(format!("profile {profile} repeats arg-range {}", pair[0].index).into());
        }
    }
    let mut domain_size = 1_u64;
    for range in ranges {
        if range.index >= 8 {
            return Err(
                format!("profile {profile} arg-range index exceeds RV32 ABI a0..a7").into(),
            );
        }
        if range.min > range.max {
            return Err(format!(
                "profile {profile} arg-range {} has minimum above maximum",
                range.index
            )
            .into());
        }
        domain_size = domain_size
            .checked_mul(u64::from(range.max) - u64::from(range.min) + 1)
            .ok_or_else(|| format!("profile {profile} argument domain overflows"))?;
        if domain_size > MAX_DOMAIN_CASES {
            return Err(format!(
                "profile {profile} argument domain has {domain_size} cases; maximum is {MAX_DOMAIN_CASES}"
            )
            .into());
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
                })?;
            if !(range.min..=range.max).contains(&value) {
                return Err(format!(
                    "profile {profile} case {} argument {} value {value:#x} is outside {:#x}..={:#x}",
                    scenario.name, range.index, range.min, range.max
                )
                .into());
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
            return Err(format!(
                "profile {profile} has no case for admissible argument combination {values}"
            )
            .into());
        }
    }
    Ok(())
}

fn split_directive(line: &str, line_number: usize) -> Result<(&str, &str)> {
    line.split_once(char::is_whitespace)
        .map(|(directive, value)| (directive, value.trim()))
        .filter(|(_, value)| !value.is_empty())
        .ok_or_else(|| format!("profile directive needs a value at line {line_number}").into())
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
                .ok_or_else(|| format!("directive before profile at line {line_number}"))?;
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
                            return Err(format!(
                                "invalid contract {value:?} at line {line_number}"
                            )
                            .into());
                        }
                    };
                }
                "compare-return" => {
                    profile.compare_return = value
                        .parse()
                        .map_err(|_| format!("invalid boolean at line {line_number}"))?;
                }
                "arg-range" => profile
                    .argument_ranges
                    .push(parse_argument_range(value, line_number)?),
                "case" => {
                    profile.finish_scenario();
                    profile.current_scenario = Some(NamedScenario::new(value.to_owned()));
                }
                "arg" => profile.scenario(line_number)?.arguments.push(
                    parse_u32(value).ok_or_else(|| format!("invalid arg at line {line_number}"))?,
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
                "ram" => {
                    let (address, value) = parse_assignment(value, "ram")?;
                    seed_ram_word(profile.scenario(line_number)?, address, value);
                }
                "vendor-ram" | "rust-ram" => {
                    let word = parse_assignment(value, directive)?;
                    let scenario = profile.current_scenario.as_mut().ok_or_else(|| {
                        format!("profile directive before case at line {line_number}")
                    })?;
                    if directive == "vendor-ram" {
                        scenario.vendor_ram_words.push(word);
                    } else {
                        scenario.rust_ram_words.push(word);
                    }
                }
                "vendor-ram-symbol" => profile
                    .current_scenario
                    .as_mut()
                    .ok_or_else(|| format!("profile directive before case at line {line_number}"))?
                    .vendor_symbol_words
                    .push(parse_symbol_word(value, "vendor-ram-symbol")?),
                "rust-ram-symbol" => profile
                    .current_scenario
                    .as_mut()
                    .ok_or_else(|| format!("profile directive before case at line {line_number}"))?
                    .rust_symbol_words
                    .push(parse_symbol_word(value, "rust-ram-symbol")?),
                "vendor-observe" | "rust-observe" => {
                    let (address, length) = parse_assignment(value, directive)?;
                    if length == 0 {
                        return Err(format!("{directive} length must be non-zero").into());
                    }
                    let observation = MemoryObservation::Absolute { address, length };
                    let scenario = profile.current_scenario.as_mut().ok_or_else(|| {
                        format!("profile directive before case at line {line_number}")
                    })?;
                    if directive == "vendor-observe" {
                        scenario.vendor_observations.push(observation);
                    } else {
                        scenario.rust_observations.push(observation);
                    }
                }
                "vendor-observe-symbol" | "rust-observe-symbol" => {
                    let observation = parse_symbol_observation(value, directive)?;
                    let scenario = profile.current_scenario.as_mut().ok_or_else(|| {
                        format!("profile directive before case at line {line_number}")
                    })?;
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
                        .map_err(|_| format!("invalid max-steps at line {line_number}"))?;
                }
                _ => {
                    return Err(format!(
                        "unknown profile directive {directive} at line {line_number}"
                    )
                    .into());
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
        return Err("profile file contains no profiles".into());
    }
    Ok(profiles)
}

#[cfg(test)]
#[path = "../harnesses/esp32s31/profiles_tests.rs"]
mod tests;
