//! Machine-readable concrete equivalence profiles.

use std::{collections::BTreeSet, fs, path::Path, sync::Arc};

use serde::Deserialize;

use crate::{
    MemoryObservation, NamedScenario, Result, RuntimeMemoryInstance, SymbolWord, observe_memory,
    seed_ram_word,
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

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileDocument {
    schema: u32,
    profiles: Vec<ProfileInput>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct ProfileInput {
    name: String,
    vendor_source: String,
    vendor_symbol: String,
    rust_symbol: String,
    #[serde(default)]
    contract: ProfileContract,
    #[serde(default)]
    compare_return: bool,
    #[serde(default)]
    argument_ranges: Vec<ArgumentRangeInput>,
    #[serde(rename = "cases")]
    scenarios: Vec<ScenarioInput>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArgumentRangeInput {
    index: usize,
    min: u32,
    max: u32,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct WordInput {
    address: u32,
    value: u32,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct RangeInput {
    address: u32,
    length: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct ScenarioInput {
    name: String,
    #[serde(default)]
    arguments: Vec<u32>,
    #[serde(default)]
    mmio_initial: Vec<WordInput>,
    #[serde(default)]
    mmio_reads: Vec<WordInput>,
    #[serde(default)]
    device_models: Vec<crate::execution_model::DeviceModelSpec>,
    #[serde(default)]
    ram: Vec<WordInput>,
    #[serde(default)]
    vendor_ram: Vec<WordInput>,
    #[serde(default)]
    rust_ram: Vec<WordInput>,
    #[serde(default)]
    vendor_ram_symbols: Vec<SymbolWord>,
    #[serde(default)]
    rust_ram_symbols: Vec<SymbolWord>,
    #[serde(default)]
    vendor_tables: Vec<crate::execution_model::TableInstance>,
    #[serde(default)]
    rust_tables: Vec<crate::execution_model::TableInstance>,
    #[serde(default)]
    vendor_memory_instances: Vec<RuntimeMemoryInstance>,
    #[serde(default)]
    rust_memory_instances: Vec<RuntimeMemoryInstance>,
    #[serde(default)]
    vendor_observations: Vec<MemoryObservation>,
    #[serde(default)]
    rust_observations: Vec<MemoryObservation>,
    #[serde(default)]
    observe: Vec<RangeInput>,
    #[serde(default)]
    max_steps: Option<u64>,
}

impl ProfileInput {
    fn finish(self) -> Result<Profile> {
        validate_source_id(&self.vendor_source, 1)?;
        if self.scenarios.is_empty() {
            return Err(crate::Error::invalid(format!(
                "profile {} has no cases",
                self.name
            )));
        }
        let mut argument_ranges = self
            .argument_ranges
            .into_iter()
            .map(|range| ArgumentRange {
                index: range.index,
                min: range.min,
                max: range.max,
            })
            .collect::<Vec<_>>();
        argument_ranges.sort_by_key(|range| range.index);
        let scenarios = self
            .scenarios
            .into_iter()
            .map(ScenarioInput::finish)
            .collect::<Result<Vec<_>>>()?;
        validate_argument_domain(&self.name, &argument_ranges, &scenarios)?;
        Ok(Profile {
            name: self.name,
            vendor_source: self.vendor_source,
            vendor_symbol: self.vendor_symbol,
            rust_symbol: self.rust_symbol,
            contract: self.contract,
            compare_return: self.compare_return,
            argument_ranges,
            scenarios,
        })
    }
}

impl ScenarioInput {
    fn finish(self) -> Result<NamedScenario> {
        let mut output = NamedScenario::new(self.name);
        output.scenario.arguments = self.arguments;
        for word in self.mmio_initial {
            output
                .scenario
                .mmio_initial
                .insert(word.address, word.value);
        }
        for word in self.mmio_reads {
            output
                .scenario
                .mmio_reads
                .entry(word.address)
                .or_default()
                .push_back(word.value);
        }
        output.scenario.device_models = self
            .device_models
            .into_iter()
            .map(|model| Arc::new(model) as Arc<dyn crate::execution_model::DeviceModel>)
            .collect();
        for word in self.ram {
            seed_ram_word(&mut output.scenario, word.address, word.value);
        }
        output.vendor_ram_words = self
            .vendor_ram
            .into_iter()
            .map(|word| (word.address, word.value))
            .collect();
        output.rust_ram_words = self
            .rust_ram
            .into_iter()
            .map(|word| (word.address, word.value))
            .collect();
        output.vendor_symbol_words = self.vendor_ram_symbols;
        output.rust_symbol_words = self.rust_ram_symbols;
        output.vendor_table_instances = self.vendor_tables;
        output.rust_table_instances = self.rust_tables;
        output.vendor_memory_instances = self.vendor_memory_instances;
        output.rust_memory_instances = self.rust_memory_instances;
        output.vendor_observations = self.vendor_observations;
        output.rust_observations = self.rust_observations;
        for observation in self.observe {
            observe_memory(
                &mut output.scenario,
                observation.address,
                observation.length,
            )?;
        }
        if let Some(max_steps) = self.max_steps {
            output.scenario.max_steps = max_steps;
        }
        validate_scenario(&output)?;
        Ok(output)
    }
}

fn validate_scenario(scenario: &NamedScenario) -> Result<()> {
    for instance in scenario
        .vendor_memory_instances
        .iter()
        .chain(&scenario.rust_memory_instances)
    {
        if instance.id.is_empty() || instance.length == 0 {
            return Err(crate::Error::invalid(
                "memory instances require a non-empty id and non-zero length",
            ));
        }
        instance
            .base_address
            .checked_add(instance.length)
            .ok_or("memory instance range overflows the 32-bit address space")
            .map_err(crate::Error::invalid)?;
    }
    for observation in scenario
        .vendor_observations
        .iter()
        .chain(&scenario.rust_observations)
    {
        if observation.length() == 0 {
            return Err(crate::Error::invalid(
                "memory observation length must be non-zero",
            ));
        }
    }
    Ok(())
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

#[tracing::instrument(name = "load_verification_profiles", fields(path = %path.display()))]
pub fn load(path: &Path) -> Result<Vec<Profile>> {
    let input = fs::read_to_string(path)?;
    let document: ProfileDocument = toml_edit::de::from_str(&input).map_err(|error| {
        crate::error::WorkbenchError::manifest_source(
            "verification profile TOML",
            path,
            &input,
            &error,
            error.span(),
        )
    })?;
    finish(document).map_err(|error| {
        crate::error::WorkbenchError::manifest_document(
            "verification profile TOML",
            path,
            &input,
            error,
        )
    })
}

#[cfg(test)]
fn parse(input: &str) -> Result<Vec<Profile>> {
    let document: ProfileDocument = toml_edit::de::from_str(input)
        .map_err(|error| crate::Error::invalid(format!("invalid profile TOML: {error}")))?;
    finish(document)
}

fn finish(document: ProfileDocument) -> Result<Vec<Profile>> {
    if document.schema != 1 {
        return Err(crate::Error::invalid(
            "verification profile TOML requires schema = 1",
        ));
    }
    if document.profiles.is_empty() {
        return Err(crate::Error::invalid(
            "verification profile TOML contains no profiles",
        ));
    }
    document
        .profiles
        .into_iter()
        .map(ProfileInput::finish)
        .collect()
}

#[cfg(test)]
#[path = "../harnesses/esp32s31/profiles_tests.rs"]
mod tests;
