//! Machine-readable concrete equivalence profiles.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    sync::Arc,
};

use open_radio_vendor_semantics::DriverAdapterClaim;
use serde::Deserialize;

use crate::{
    MemoryObservation, NamedScenario, Result, RuntimeMemoryInstance, SymbolWord, observe_memory,
    seed_ram_word,
};

use super::dispositions::validate_source_id;

mod validation;

#[cfg(test)]
use validation::validate_argument_domain;

#[derive(Clone, Debug)]
pub struct Profile {
    pub name: String,
    pub vendor_source: String,
    pub vendor_symbol: String,
    pub rust_symbol: String,
    pub claim: DriverAdapterClaim,
    pub precondition: Option<String>,
    pub contract: ProfileContract,
    pub compare_return: bool,
    pub argument_ranges: Vec<ArgumentRange>,
    pub argument_values: Vec<ArgumentValues>,
    pub mmio_domains: Vec<MmioDomain>,
    pub scenarios: Vec<NamedScenario>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ArgumentRange {
    pub index: usize,
    pub min: u32,
    pub max: u32,
}

/// A reviewed, finite and potentially sparse ABI argument domain.
///
/// This is distinct from [`ArgumentRange`]: selectors such as jump-table
/// cases 6 and 8 must not silently admit case 7 merely to describe one
/// verification profile.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ArgumentValues {
    pub index: usize,
    pub values: Vec<u32>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MmioDomain {
    pub address: u32,
    pub values: Vec<u32>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ProfileCoverageConstraint {
    pub arguments: [Option<u32>; 8],
    pub stable_words: BTreeMap<u32, u32>,
}

impl Profile {
    /// Enumerates the explicitly admissible ABI argument domain for static
    /// reachability. Arguments without an `argument-range` or
    /// `argument-values` entry remain unknown.
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
        for domain in &self.argument_values {
            let mut expanded = Vec::new();
            for constraint in constraints {
                for value in &domain.values {
                    let mut constraint = constraint;
                    constraint[domain.index] = Some(*value);
                    expanded.push(constraint);
                }
            }
            constraints = expanded;
        }
        constraints
    }

    /// Enumerate the reviewed finite input domain used by static coverage.
    /// MMIO words listed here must also be present in concrete cases, so a
    /// hardware selector invariant cannot silently hide an untested path.
    pub fn coverage_constraints(&self) -> Vec<ProfileCoverageConstraint> {
        let mut constraints = self
            .coverage_argument_constraints()
            .into_iter()
            .map(|arguments| ProfileCoverageConstraint {
                arguments,
                stable_words: BTreeMap::new(),
            })
            .collect::<Vec<_>>();
        for domain in &self.mmio_domains {
            let mut expanded = Vec::new();
            for constraint in constraints {
                for value in &domain.values {
                    let mut constraint = constraint.clone();
                    constraint.stable_words.insert(domain.address, *value);
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
    claim: DriverAdapterClaim,
    precondition: Option<String>,
    #[serde(default)]
    contract: ProfileContract,
    #[serde(default)]
    compare_return: bool,
    #[serde(default)]
    argument_ranges: Vec<ArgumentRangeInput>,
    #[serde(default)]
    argument_values: Vec<ArgumentValuesInput>,
    #[serde(default)]
    mmio_domains: Vec<MmioDomainInput>,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArgumentValuesInput {
    index: usize,
    values: Vec<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MmioDomainInput {
    address: u32,
    values: Vec<u32>,
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

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct CallResponseInput {
    symbol: String,
    #[serde(default)]
    return_words: Vec<u32>,
    #[serde(default)]
    outputs: Vec<CallOutputInput>,
    allocation: Option<CallAllocationInput>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct CallAllocationInput {
    address: u32,
    size_argument: u8,
    capacity: u32,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "kebab-case"
)]
enum CallOutputInput {
    PrivateStack {
        pointer_argument: u8,
        width: u8,
        value: u32,
    },
}

impl CallResponseInput {
    fn finish(
        self,
        scenario: &str,
        side: &str,
    ) -> Result<(String, crate::execution::ModeledCallResponse)> {
        if self.symbol.is_empty() || self.symbol.chars().any(char::is_whitespace) {
            return Err(crate::Error::invalid(format!(
                "scenario {scenario} {side} call response requires one non-empty symbol"
            )));
        }
        if self.return_words.len() > 2 {
            return Err(crate::Error::invalid(format!(
                "scenario {scenario} {side} call {} declares more than RV32 a0/a1 return words",
                self.symbol
            )));
        }
        if self.allocation.is_some() && !self.return_words.is_empty() {
            return Err(crate::Error::invalid(format!(
                "scenario {scenario} {side} call {} allocation cannot also declare return words",
                self.symbol
            )));
        }
        let mut return_words = [None; 2];
        for (index, value) in self.return_words.into_iter().enumerate() {
            return_words[index] = Some(value);
        }
        let mut output_arguments = BTreeSet::new();
        let mut outputs = Vec::with_capacity(self.outputs.len());
        for output in self.outputs {
            let CallOutputInput::PrivateStack {
                pointer_argument,
                width,
                value,
            } = output;
            if pointer_argument >= 8 {
                return Err(crate::Error::invalid(format!(
                    "scenario {scenario} {side} call {} output argument a{pointer_argument} exceeds RV32 a0..a7",
                    self.symbol
                )));
            }
            if !output_arguments.insert(pointer_argument) {
                return Err(crate::Error::invalid(format!(
                    "scenario {scenario} {side} call {} repeats output argument a{pointer_argument}",
                    self.symbol
                )));
            }
            if !matches!(width, 8 | 16 | 32)
                || value
                    & !match width {
                        8 => 0xff,
                        16 => 0xffff,
                        _ => u32::MAX,
                    }
                    != 0
            {
                return Err(crate::Error::invalid(format!(
                    "scenario {scenario} {side} call {} has invalid {width}-bit output value {value:#010x}",
                    self.symbol
                )));
            }
            outputs.push(crate::execution::ModeledCallOutput::PrivateStack {
                pointer_argument,
                width,
                value,
            });
        }
        Ok((
            self.symbol,
            crate::execution::ModeledCallResponse {
                return_words,
                outputs,
                allocation: self
                    .allocation
                    .map(|allocation| crate::execution::ModeledAllocation {
                        address: allocation.address,
                        size_argument: allocation.size_argument,
                        capacity: allocation.capacity,
                    }),
            },
        ))
    }
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
    vendor_fifo_services: Vec<crate::execution_model::FifoServiceInstance>,
    #[serde(default)]
    rust_fifo_services: Vec<crate::execution_model::FifoServiceInstance>,
    #[serde(default)]
    vendor_fifo_bindings: Vec<crate::execution_model::FifoServiceBinding>,
    #[serde(default)]
    rust_fifo_bindings: Vec<crate::execution_model::FifoServiceBinding>,
    #[serde(default)]
    vendor_goal: crate::execution_model::ExecutionGoal,
    #[serde(default)]
    rust_goal: crate::execution_model::ExecutionGoal,
    #[serde(default)]
    vendor_calls: Vec<CallResponseInput>,
    #[serde(default)]
    rust_calls: Vec<CallResponseInput>,
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
        let mut argument_values = self
            .argument_values
            .into_iter()
            .map(|domain| ArgumentValues {
                index: domain.index,
                values: domain.values,
            })
            .collect::<Vec<_>>();
        argument_values.sort_by_key(|domain| domain.index);
        let scenarios = self
            .scenarios
            .into_iter()
            .map(ScenarioInput::finish)
            .collect::<Result<Vec<_>>>()?;
        if self.compare_return
            && scenarios.iter().any(|scenario| {
                !matches!(
                    scenario.vendor_goal,
                    crate::execution_model::ExecutionGoal::Return
                ) || !matches!(
                    scenario.rust_goal,
                    crate::execution_model::ExecutionGoal::Return
                )
            })
        {
            return Err(crate::Error::invalid(format!(
                "profile {} cannot compare return values when a scenario stops at a non-return execution goal",
                self.name
            )));
        }
        validation::validate_argument_domain(
            &self.name,
            &argument_ranges,
            &argument_values,
            &scenarios,
        )?;
        let mut mmio_domains = self
            .mmio_domains
            .into_iter()
            .map(|domain| MmioDomain {
                address: domain.address,
                values: domain.values,
            })
            .collect::<Vec<_>>();
        mmio_domains.sort_by_key(|domain| domain.address);
        validation::validate_coverage_domain(
            &self.name,
            &argument_ranges,
            &argument_values,
            &mmio_domains,
            &scenarios,
        )?;
        validation::validate_claim(
            &self.name,
            self.claim,
            self.precondition.as_deref(),
            &argument_ranges,
            &argument_values,
            &mmio_domains,
        )?;
        Ok(Profile {
            name: self.name,
            vendor_source: self.vendor_source,
            vendor_symbol: self.vendor_symbol,
            rust_symbol: self.rust_symbol,
            claim: self.claim,
            precondition: self.precondition,
            contract: self.contract,
            compare_return: self.compare_return,
            argument_ranges,
            argument_values,
            mmio_domains,
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
        output.vendor_fifo_services = self.vendor_fifo_services;
        output.rust_fifo_services = self.rust_fifo_services;
        output.vendor_fifo_bindings = self.vendor_fifo_bindings;
        output.rust_fifo_bindings = self.rust_fifo_bindings;
        output.vendor_goal = self.vendor_goal;
        output.rust_goal = self.rust_goal;
        output.vendor_call_responses = self
            .vendor_calls
            .into_iter()
            .map(|response| response.finish(&output.name, "vendor"))
            .collect::<Result<_>>()?;
        output.rust_call_responses = self
            .rust_calls
            .into_iter()
            .map(|response| response.finish(&output.name, "Rust"))
            .collect::<Result<_>>()?;
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
        validation::validate_scenario(&output)?;
        Ok(output)
    }
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
    if document.schema != 3 {
        return Err(crate::Error::invalid(
            "verification profile TOML requires schema = 3",
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
