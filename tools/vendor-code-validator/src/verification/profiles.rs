//! Machine-readable concrete equivalence profiles.

use std::{fs, path::Path};

use crate::{
    MemoryObservation, NamedScenario, Result, execution, observe_memory, parse_assignment,
    parse_symbol_observation, parse_symbol_word, parse_u32, seed_ram_word,
};

#[derive(Clone, Debug)]
pub struct Profile {
    pub name: String,
    pub vendor_source: String,
    pub vendor_symbol: String,
    pub rust_symbol: String,
    pub contract: ProfileContract,
    pub compare_return: bool,
    pub scenarios: Vec<NamedScenario>,
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
    vendor_source: Option<String>,
    vendor_symbol: Option<String>,
    rust_symbol: Option<String>,
    contract: ProfileContract,
    compare_return: bool,
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
        self.finish_scenario();
        if self.scenarios.is_empty() {
            return Err(format!("profile {} has no cases", self.name).into());
        }
        Ok(Profile {
            name: self.name,
            vendor_source: self.vendor_source.ok_or("profile has no vendor-source")?,
            vendor_symbol: self.vendor_symbol.ok_or("profile has no vendor-symbol")?,
            rust_symbol: self.rust_symbol.ok_or("profile has no rust-symbol")?,
            contract: self.contract,
            compare_return: self.compare_return,
            scenarios: self.scenarios,
        })
    }

    fn scenario(&mut self, line: usize) -> Result<&mut execution::Scenario> {
        self.current_scenario
            .as_mut()
            .map(|scenario| &mut scenario.scenario)
            .ok_or_else(|| format!("profile directive before case at line {line}").into())
    }
}

fn split_directive(line: &str, line_number: usize) -> Result<(&str, &str)> {
    line.split_once(char::is_whitespace)
        .map(|(directive, value)| (directive, value.trim()))
        .filter(|(_, value)| !value.is_empty())
        .ok_or_else(|| format!("profile directive needs a value at line {line_number}").into())
}

pub fn load(path: &Path) -> Result<Vec<Profile>> {
    let input = fs::read_to_string(path)?;
    let mut profiles = Vec::new();
    let mut current: Option<ProfileBuilder> = None;

    for (index, raw_line) in input.lines().enumerate() {
        let line_number = index + 1;
        let line = raw_line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        let (directive, value) = split_directive(line, line_number)?;
        if directive == "profile" {
            if let Some(profile) = current.take() {
                profiles.push(profile.finish()?);
            }
            current = Some(ProfileBuilder {
                name: value.to_owned(),
                ..ProfileBuilder::default()
            });
            continue;
        }
        let profile = current
            .as_mut()
            .ok_or_else(|| format!("directive before profile at line {line_number}"))?;
        match directive {
            "vendor-source" => {
                if !matches!(value, "rom" | "archive") {
                    return Err(
                        format!("invalid vendor-source {value:?} at line {line_number}").into(),
                    );
                }
                profile.vendor_source = Some(value.to_owned());
            }
            "vendor-symbol" => profile.vendor_symbol = Some(value.to_owned()),
            "rust-symbol" => profile.rust_symbol = Some(value.to_owned()),
            "contract" => {
                profile.contract = match value {
                    "scenario" => ProfileContract::Scenario,
                    "state" => ProfileContract::State,
                    _ => {
                        return Err(
                            format!("invalid contract {value:?} at line {line_number}").into()
                        );
                    }
                };
            }
            "compare-return" => {
                profile.compare_return = value
                    .parse()
                    .map_err(|_| format!("invalid boolean at line {line_number}"))?;
            }
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
                return Err(
                    format!("unknown profile directive {directive} at line {line_number}").into(),
                );
            }
        }
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
