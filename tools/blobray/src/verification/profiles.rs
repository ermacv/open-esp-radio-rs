//! Machine-readable concrete equivalence profiles.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    sync::Arc,
};

use open_radio_vendor_semantics::VerificationClaim;
use serde::Deserialize;

use crate::{
    MemoryObservation, NamedScenario, Result, RuntimeMemoryInstance, SymbolWord, observe_memory,
    seed_ram_word,
};

use super::dispositions::validate_source_id;

mod validation;

#[derive(Clone, Debug)]
pub struct Profile {
    pub name: String,
    pub vendor_source: String,
    pub vendor_symbol: String,
    pub rust_symbol: String,
    pub claim: VerificationClaim,
    pub precondition: Option<String>,
    pub contract: ProfileContract,
    pub compare_return: bool,
    pub case_execution: CaseExecution,
    pub transaction_comparison: TransactionComparison,
    pub call_equivalences: Vec<CallEquivalence>,
    pub argument_ranges: Vec<ArgumentRange>,
    pub argument_values: Vec<ArgumentValues>,
    pub mmio_domains: Vec<MmioDomain>,
    pub vendor_setup: Vec<VendorSetupPhase>,
    pub scenarios: Vec<NamedScenario>,
}

#[derive(Clone, Debug)]
pub struct VendorSetupPhase {
    pub name: String,
    pub symbol: String,
    pub scenario: crate::execution::Scenario,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaseExecution {
    /// Every declared case starts from an independent execution session.
    Independent,
    /// Cases execute in declaration order with persistent software RAM and
    /// external FIFO state. MMIO responses remain explicit per case.
    Stateful,
}

impl CaseExecution {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Independent => "independent",
            Self::Stateful => "stateful",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransactionComparison {
    /// Compare only explicitly projected final memory and optional return
    /// value. Vendor-only platform/context observations remain recorded but
    /// cannot be mistaken for whole transaction equivalence.
    StateOnly,
    /// Compare projected final state plus explicitly reviewed semantic call
    /// pairs. Physical addresses and unlisted implementation calls remain
    /// provenance rather than equality inputs.
    StateAndReviewedCalls,
    /// Compare the ordered externally observable MMIO/delay/fence stream.
    Observables,
    /// Compare the ordered observable stream through the effect contract
    /// attached to the reviewed production binding.
    ///
    /// This is deliberately not a profile-local normalizer. The disposition
    /// remains the sole owner of every required, omitted, replaced, or added
    /// effect, and an absent rule fails closed.
    ObservablesUnderEffectContract,
    /// Also compare named call boundaries at their exact position in the
    /// observable stream. Call-site addresses are provenance, not equality.
    ObservablesAndCalls,
    /// Compare only explicitly reviewed vendor/Rust semantic call pairs.
    /// Unlisted implementation-detail calls remain recorded but do not enter
    /// equivalence.
    ObservablesAndReviewedCalls,
    /// Include branch decisions and ordinary RAM transactions. This requires
    /// a reviewed ABI/layout projection and is intentionally opt-in.
    Full,
}

impl TransactionComparison {
    pub const fn compares_observables(self) -> bool {
        !self.state_domain()
    }

    pub const fn includes_calls(self) -> bool {
        matches!(
            self,
            Self::StateAndReviewedCalls
                | Self::ObservablesAndCalls
                | Self::ObservablesAndReviewedCalls
                | Self::Full
        )
    }

    pub const fn includes_internal_state(self) -> bool {
        matches!(self, Self::Full)
    }

    pub const fn reviewed_calls_only(self) -> bool {
        matches!(
            self,
            Self::StateAndReviewedCalls | Self::ObservablesAndReviewedCalls
        )
    }

    pub const fn state_domain(self) -> bool {
        matches!(self, Self::StateOnly | Self::StateAndReviewedCalls)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallEquivalence {
    pub operation: String,
    pub vendor_symbol: String,
    pub rust_symbol: String,
    pub argument_comparison: CallArgumentComparison,
    pub argument_indices: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum CallArgumentComparison {
    Exact,
    Ignore,
    Selected,
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
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct ProfileDocument {
    schema: u32,
    case_execution: CaseExecution,
    transaction_comparison: TransactionComparison,
    profiles: Vec<ProfileInput>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct ProfileInput {
    name: String,
    vendor_source: String,
    vendor_symbol: String,
    rust_symbol: String,
    claim: VerificationClaim,
    precondition: Option<String>,
    #[serde(default)]
    contract: ProfileContract,
    #[serde(default)]
    compare_return: bool,
    case_execution: Option<CaseExecution>,
    transaction_comparison: Option<TransactionComparison>,
    #[serde(default)]
    call_equivalences: Vec<CallEquivalenceInput>,
    #[serde(default)]
    argument_ranges: Vec<ArgumentRangeInput>,
    #[serde(default)]
    argument_values: Vec<ArgumentValuesInput>,
    #[serde(default)]
    mmio_domains: Vec<MmioDomainInput>,
    #[serde(default)]
    vendor_setup: Vec<VendorSetupInput>,
    #[serde(rename = "cases")]
    scenarios: Vec<ScenarioInput>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct VendorSetupInput {
    name: String,
    symbol: String,
    #[serde(default)]
    arguments: Vec<u32>,
    #[serde(default)]
    goal: crate::execution_model::ExecutionGoal,
    max_steps: Option<u64>,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct CallEquivalenceInput {
    operation: String,
    vendor_symbol: String,
    rust_symbol: String,
    argument_comparison: CallArgumentComparison,
    #[serde(default)]
    argument_indices: Vec<u8>,
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
    vendor_mmio_reads: Vec<WordInput>,
    #[serde(default)]
    rust_mmio_reads: Vec<WordInput>,
    #[serde(default)]
    vendor_stack_fill: Option<u8>,
    #[serde(default)]
    rust_stack_fill: Option<u8>,
    #[serde(default)]
    device_models: Vec<crate::execution_model::DeviceModelSpec>,
    #[serde(default)]
    ram: Vec<WordInput>,
    #[serde(default)]
    vendor_ram: Vec<WordInput>,
    #[serde(default)]
    rust_ram: Vec<WordInput>,
    #[serde(default)]
    persistent_memory: Vec<RangeInput>,
    #[serde(default)]
    vendor_persistent_memory: Vec<RangeInput>,
    #[serde(default)]
    rust_persistent_memory: Vec<RangeInput>,
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
    fn finish(
        self,
        document_transaction_comparison: TransactionComparison,
        document_case_execution: CaseExecution,
    ) -> Result<Profile> {
        let case_execution = self.case_execution.unwrap_or(document_case_execution);
        let transaction_comparison = self
            .transaction_comparison
            .unwrap_or(document_transaction_comparison);
        if transaction_comparison.state_domain() && self.contract != ProfileContract::State {
            return Err(crate::Error::invalid(format!(
                "profile {} uses state-only comparison without contract = \"state\"",
                self.name
            )));
        }
        let call_equivalences = self
            .call_equivalences
            .into_iter()
            .map(|pair| CallEquivalence {
                operation: pair.operation,
                vendor_symbol: pair.vendor_symbol,
                rust_symbol: pair.rust_symbol,
                argument_comparison: pair.argument_comparison,
                argument_indices: pair.argument_indices,
            })
            .collect::<Vec<_>>();
        validate_call_equivalences(&self.name, transaction_comparison, &call_equivalences)?;
        validate_source_id(&self.vendor_source, 1)?;
        if self.scenarios.is_empty() {
            return Err(crate::Error::invalid(format!(
                "profile {} has no cases",
                self.name
            )));
        }
        if case_execution == CaseExecution::Stateful && self.scenarios.len() < 2 {
            return Err(crate::Error::invalid(format!(
                "stateful profile {} needs at least two ordered cases",
                self.name
            )));
        }
        if !self.vendor_setup.is_empty() && case_execution != CaseExecution::Stateful {
            return Err(crate::Error::invalid(format!(
                "profile {} declares vendor setup phases without case-execution = \"stateful\"",
                self.name
            )));
        }
        let mut setup_names = BTreeSet::new();
        let vendor_setup = self
            .vendor_setup
            .into_iter()
            .map(|setup| {
                if setup.name.trim().is_empty()
                    || setup.symbol.trim().is_empty()
                    || !setup_names.insert(setup.name.clone())
                {
                    return Err(crate::Error::invalid(format!(
                        "profile {} has an empty or duplicate vendor setup phase",
                        self.name
                    )));
                }
                if setup.arguments.len() > 8 || setup.max_steps == Some(0) {
                    return Err(crate::Error::invalid(format!(
                        "profile {} vendor setup phase {} has an invalid RV32 scenario",
                        self.name, setup.name
                    )));
                }
                let mut scenario = crate::execution::Scenario {
                    arguments: setup.arguments,
                    goal: setup.goal,
                    ..crate::execution::Scenario::default()
                };
                if let Some(max_steps) = setup.max_steps {
                    scenario.max_steps = max_steps;
                }
                Ok(VendorSetupPhase {
                    name: setup.name,
                    symbol: setup.symbol,
                    scenario,
                })
            })
            .collect::<Result<Vec<_>>>()?;
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
            case_execution,
            transaction_comparison,
            call_equivalences,
            argument_ranges,
            argument_values,
            mmio_domains,
            vendor_setup,
            scenarios,
        })
    }
}

fn validate_call_equivalences(
    profile: &str,
    comparison: TransactionComparison,
    pairs: &[CallEquivalence],
) -> Result<()> {
    if comparison.reviewed_calls_only() && pairs.is_empty() {
        return Err(crate::Error::invalid(format!(
            "profile {profile} compares reviewed calls but declares no call-equivalences"
        )));
    }
    if !comparison.reviewed_calls_only() && !pairs.is_empty() {
        return Err(crate::Error::invalid(format!(
            "profile {profile} declares call-equivalences without observables-and-reviewed-calls"
        )));
    }
    let mut operations = BTreeSet::new();
    let mut vendor = BTreeSet::new();
    let mut rust = BTreeSet::new();
    for pair in pairs {
        if pair.operation.trim().is_empty()
            || pair.vendor_symbol.trim().is_empty()
            || pair.rust_symbol.trim().is_empty()
        {
            return Err(crate::Error::invalid(format!(
                "profile {profile} has an empty call-equivalence identity"
            )));
        }
        if !operations.insert(&pair.operation)
            || !vendor.insert(&pair.vendor_symbol)
            || !rust.insert(&pair.rust_symbol)
        {
            return Err(crate::Error::invalid(format!(
                "profile {profile} has an ambiguous call-equivalence mapping"
            )));
        }
        match pair.argument_comparison {
            CallArgumentComparison::Selected
                if pair.argument_indices.is_empty()
                    || pair.argument_indices.iter().any(|index| *index >= 8) =>
            {
                return Err(crate::Error::invalid(format!(
                    "profile {profile} selected call argument comparison requires unique indices in 0..8"
                )));
            }
            CallArgumentComparison::Exact | CallArgumentComparison::Ignore
                if !pair.argument_indices.is_empty() =>
            {
                return Err(crate::Error::invalid(format!(
                    "profile {profile} call argument indices require selected comparison"
                )));
            }
            _ => {}
        }
        if pair.argument_indices.iter().collect::<BTreeSet<_>>().len()
            != pair.argument_indices.len()
        {
            return Err(crate::Error::invalid(format!(
                "profile {profile} selected call argument indices must be unique"
            )));
        }
    }
    Ok(())
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
        output.vendor_mmio_reads = self
            .vendor_mmio_reads
            .into_iter()
            .map(|word| (word.address, word.value))
            .collect();
        output.rust_mmio_reads = self
            .rust_mmio_reads
            .into_iter()
            .map(|word| (word.address, word.value))
            .collect();
        output.vendor_stack_fill = self.vendor_stack_fill;
        output.rust_stack_fill = self.rust_stack_fill;
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
        output.scenario.persistent_memory = self
            .persistent_memory
            .into_iter()
            .map(|range| crate::execution_model::MemoryRange {
                start: range.address,
                length: range.length,
            })
            .collect();
        output.vendor_persistent_memory = self
            .vendor_persistent_memory
            .into_iter()
            .map(|range| crate::execution_model::MemoryRange {
                start: range.address,
                length: range.length,
            })
            .collect();
        output.rust_persistent_memory = self
            .rust_persistent_memory
            .into_iter()
            .map(|range| crate::execution_model::MemoryRange {
                start: range.address,
                length: range.length,
            })
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
        crate::error::BlobrayError::manifest_source(
            "verification profile TOML",
            path,
            &input,
            &error,
            error.span(),
        )
    })?;
    finish(document).map_err(|error| {
        crate::error::BlobrayError::manifest_document(
            "verification profile TOML",
            path,
            &input,
            error,
        )
    })
}

fn finish(document: ProfileDocument) -> Result<Vec<Profile>> {
    if document.schema != 5 {
        return Err(crate::Error::invalid(
            "verification profile TOML requires schema = 5",
        ));
    }
    if document.profiles.is_empty() {
        return Err(crate::Error::invalid(
            "verification profile TOML contains no profiles",
        ));
    }
    let transaction_comparison = document.transaction_comparison;
    let case_execution = document.case_execution;
    document
        .profiles
        .into_iter()
        .map(|profile| profile.finish(transaction_comparison, case_execution))
        .collect()
}

#[cfg(test)]
mod schema_tests {
    use super::*;

    fn minimal_profile(case_execution: &str, cases: &str) -> String {
        format!(
            "schema = 5\ncase-execution = {case_execution:?}\ntransaction-comparison = \"observables\"\n\n[[profiles]]\nname = \"ordered\"\nvendor-source = \"libpp\"\nvendor-symbol = \"vendor_step\"\nrust-symbol = \"rust_step\"\nclaim = \"whole-function-equivalence\"\n{cases}"
        )
    }

    #[test]
    fn schema_five_is_a_hard_cutover_with_explicit_execution_and_transaction_policy() {
        let old: ProfileDocument = toml_edit::de::from_str(
            "schema = 4\ncase-execution = \"independent\"\ntransaction-comparison = \"observables\"\nprofiles = []\n",
        )
        .unwrap();
        assert!(
            finish(old)
                .unwrap_err()
                .to_string()
                .contains("requires schema = 5")
        );

        let missing_transaction = match toml_edit::de::from_str::<ProfileDocument>(
            "schema = 5\ncase-execution = \"independent\"\nprofiles = []\n",
        ) {
            Ok(_) => panic!("missing transaction policy was accepted"),
            Err(error) => error.to_string(),
        };
        assert!(missing_transaction.contains("transaction-comparison"));
        let missing_execution = match toml_edit::de::from_str::<ProfileDocument>(
            "schema = 5\ntransaction-comparison = \"observables\"\nprofiles = []\n",
        ) {
            Ok(_) => panic!("missing case execution policy was accepted"),
            Err(error) => error.to_string(),
        };
        assert!(missing_execution.contains("case-execution"));
    }

    #[test]
    fn reviewed_effect_contract_comparison_is_an_explicit_profile_mode() {
        let document = minimal_profile("independent", "[[profiles.cases]]\nname = \"first\"\n")
            .replace(
                "transaction-comparison = \"observables\"",
                "transaction-comparison = \"observables-under-effect-contract\"",
            );
        let profiles = finish(toml_edit::de::from_str(&document).unwrap()).unwrap();

        assert_eq!(
            profiles[0].transaction_comparison,
            TransactionComparison::ObservablesUnderEffectContract
        );
    }

    #[test]
    fn state_only_comparison_requires_an_explicit_state_contract() {
        let scenario = "[[profiles.cases]]\nname = \"first\"\n";
        for comparison in ["state-only", "state-and-reviewed-calls"] {
            let mut invalid = minimal_profile("independent", scenario).replace(
                "transaction-comparison = \"observables\"",
                &format!("transaction-comparison = {comparison:?}"),
            );
            if comparison == "state-and-reviewed-calls" {
                invalid.push_str("\n[[profiles.call-equivalences]]\noperation = \"edge\"\nvendor-symbol = \"vendor_edge\"\nrust-symbol = \"rust_edge\"\nargument-comparison = \"ignore\"\n");
            }
            assert!(finish(toml_edit::de::from_str(&invalid).unwrap()).is_err());

            let valid = invalid.replace(
                "claim = \"whole-function-equivalence\"",
                "claim = \"whole-function-equivalence\"\ncontract = \"state\"",
            );
            assert!(finish(toml_edit::de::from_str(&valid).unwrap()).is_ok());
        }
    }

    #[test]
    fn selected_call_arguments_are_explicit_and_abi_bounded() {
        let pair = |comparison, indices| CallEquivalence {
            operation: "semantic.operation".to_owned(),
            vendor_symbol: "vendor_leaf".to_owned(),
            rust_symbol: "rust_leaf".to_owned(),
            argument_comparison: comparison,
            argument_indices: indices,
        };
        assert!(
            validate_call_equivalences(
                "selected",
                TransactionComparison::ObservablesAndReviewedCalls,
                &[pair(CallArgumentComparison::Selected, vec![0, 7])],
            )
            .is_ok()
        );
        for invalid in [
            pair(CallArgumentComparison::Selected, Vec::new()),
            pair(CallArgumentComparison::Selected, vec![8]),
            pair(CallArgumentComparison::Selected, vec![0, 0]),
            pair(CallArgumentComparison::Exact, vec![0]),
        ] {
            assert!(
                validate_call_equivalences(
                    "invalid",
                    TransactionComparison::ObservablesAndReviewedCalls,
                    &[invalid],
                )
                .is_err()
            );
        }
    }

    #[test]
    fn stateful_profiles_require_two_ordered_cases() {
        let one_case = minimal_profile("stateful", "\n[[profiles.cases]]\nname = \"first\"\n");
        let document: ProfileDocument = toml_edit::de::from_str(&one_case).unwrap();
        assert!(
            finish(document)
                .unwrap_err()
                .to_string()
                .contains("needs at least two ordered cases")
        );

        let two_cases = minimal_profile(
            "stateful",
            "\n[[profiles.cases]]\nname = \"first\"\n\n[[profiles.cases]]\nname = \"second\"\n",
        );
        let profiles = finish(toml_edit::de::from_str(&two_cases).unwrap()).unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].case_execution, CaseExecution::Stateful);
        assert_eq!(profiles[0].scenarios[0].name, "first");
        assert_eq!(profiles[0].scenarios[1].name, "second");
    }

    #[test]
    fn profile_can_override_document_case_execution() {
        let input = minimal_profile(
            "independent",
            "case-execution = \"stateful\"\n\n[[profiles.cases]]\nname = \"first\"\n\n[[profiles.cases]]\nname = \"second\"\n",
        );
        let profiles = finish(toml_edit::de::from_str(&input).unwrap()).unwrap();
        assert_eq!(profiles[0].case_execution, CaseExecution::Stateful);
    }

    #[test]
    fn persistent_memory_ranges_fail_closed() {
        for range in [
            "{ address = 4096, length = 0 }",
            "{ address = 4294967294, length = 4 }",
        ] {
            let input = minimal_profile(
                "independent",
                &format!("\n[[profiles.cases]]\nname = \"only\"\npersistent-memory = [{range}]\n"),
            );
            let document: ProfileDocument = toml_edit::de::from_str(&input).unwrap();
            assert!(finish(document).is_err(), "accepted {range}");
        }
    }

    #[test]
    fn cases_preserve_independent_vendor_and_rust_stack_fills() {
        let input = minimal_profile(
            "independent",
            "\n[[profiles.cases]]\nname = \"only\"\nvendor-stack-fill = 90\nrust-stack-fill = 165\n",
        );
        let profiles = finish(toml_edit::de::from_str(&input).unwrap()).unwrap();
        assert_eq!(profiles[0].scenarios[0].vendor_stack_fill, Some(0x5a));
        assert_eq!(profiles[0].scenarios[0].rust_stack_fill, Some(0xa5));
    }
}
