//! Scenario input parsing, normalization, and coverage inventory helpers.

use std::collections::BTreeSet;

use crate::*;
use serde::Deserialize;

pub(crate) fn parse_assignment(value: &str, option: &str) -> Result<(u32, u32)> {
    let (address, value) = value
        .split_once('=')
        .ok_or_else(|| format!("{option} requires ADDRESS=VALUE"))
        .map_err(crate::Error::invalid)?;
    let address = parse_u32(address)
        .ok_or_else(|| format!("invalid {option} address"))
        .map_err(crate::Error::invalid)?;
    let value = parse_u32(value)
        .ok_or_else(|| format!("invalid {option} value"))
        .map_err(crate::Error::invalid)?;
    Ok((address, value))
}

pub(crate) fn parse_call_return(value: &str, option: &str) -> Result<(String, u32)> {
    let (symbol, value) = value
        .split_once('=')
        .ok_or_else(|| format!("{option} requires SYMBOL=VALUE"))
        .map_err(crate::Error::invalid)?;
    if symbol.is_empty() || symbol.chars().any(char::is_whitespace) {
        return Err(crate::Error::invalid(format!(
            "{option} requires one non-empty symbol"
        )));
    }
    let value = parse_u32(value)
        .ok_or_else(|| format!("invalid {option} value"))
        .map_err(crate::Error::invalid)?;
    Ok((symbol.to_owned(), value))
}

pub(crate) fn parse_symbol_word(value: &str, option: &str) -> Result<SymbolWord> {
    let (address, symbol) = value
        .split_once('=')
        .ok_or_else(|| format!("{option} requires ADDRESS=SYMBOL"))
        .map_err(crate::Error::invalid)?;
    let address = parse_u32(address)
        .ok_or_else(|| format!("invalid {option} address"))
        .map_err(crate::Error::invalid)?;
    if symbol.is_empty() {
        return Err(crate::Error::invalid(format!(
            "{option} requires a non-empty symbol"
        )));
    }
    Ok(SymbolWord {
        address,
        symbol: symbol.to_owned(),
    })
}

pub(crate) fn seed_ram_word(scenario: &mut execution::Scenario, address: u32, value: u32) {
    write_ram_word(scenario, address, value);
    scenario
        .observed_memory
        .push(crate::execution_model::MemoryRange {
            start: address,
            length: 4,
        });
}

pub(crate) fn write_ram_word(scenario: &mut execution::Scenario, address: u32, value: u32) {
    for (offset, byte) in value.to_le_bytes().into_iter().enumerate() {
        scenario
            .memory_initial
            .insert(address.wrapping_add(offset as u32), byte);
    }
}

pub(crate) fn observe_memory(
    scenario: &mut execution::Scenario,
    address: u32,
    length: u32,
) -> Result<()> {
    if length == 0 {
        return Err(crate::Error::invalid("--observe length must be non-zero"));
    }
    scenario
        .observed_memory
        .push(crate::execution_model::MemoryRange {
            start: address,
            length,
        });
    Ok(())
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SymbolWord {
    pub(crate) address: u32,
    pub(crate) symbol: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum RuntimeMemoryObjectBinding {
    Argument { index: usize },
    Global { symbol: String },
    DereferencedGlobal { symbol: String, pointer_offset: u32 },
    Absolute { address_space: String, address: u32 },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct RuntimeMemoryInstance {
    pub(crate) id: String,
    pub(crate) base_address: u32,
    pub(crate) length: u32,
    pub(crate) bindings: Vec<RuntimeMemoryObjectBinding>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum MemoryObservation {
    Absolute {
        address: u32,
        length: u32,
    },
    Symbol {
        symbol: String,
        offset: u32,
        length: u32,
    },
}

impl MemoryObservation {
    pub(crate) const fn length(&self) -> u32 {
        match self {
            Self::Absolute { length, .. } | Self::Symbol { length, .. } => *length,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct NamedScenario {
    pub(crate) name: String,
    pub(crate) scenario: execution::Scenario,
    pub(crate) vendor_symbol_words: Vec<SymbolWord>,
    pub(crate) rust_symbol_words: Vec<SymbolWord>,
    pub(crate) vendor_ram_words: Vec<(u32, u32)>,
    pub(crate) rust_ram_words: Vec<(u32, u32)>,
    pub(crate) vendor_table_instances: Vec<crate::execution_model::TableInstance>,
    pub(crate) rust_table_instances: Vec<crate::execution_model::TableInstance>,
    pub(crate) vendor_call_responses: Vec<(String, execution::ModeledCallResponse)>,
    pub(crate) rust_call_responses: Vec<(String, execution::ModeledCallResponse)>,
    pub(crate) vendor_memory_instances: Vec<RuntimeMemoryInstance>,
    pub(crate) rust_memory_instances: Vec<RuntimeMemoryInstance>,
    pub(crate) vendor_observations: Vec<MemoryObservation>,
    pub(crate) rust_observations: Vec<MemoryObservation>,
}

impl NamedScenario {
    pub(crate) fn new(name: String) -> Self {
        Self {
            name,
            scenario: execution::Scenario::default(),
            vendor_symbol_words: Vec::new(),
            rust_symbol_words: Vec::new(),
            vendor_ram_words: Vec::new(),
            rust_ram_words: Vec::new(),
            vendor_table_instances: Vec::new(),
            rust_table_instances: Vec::new(),
            vendor_call_responses: Vec::new(),
            rust_call_responses: Vec::new(),
            vendor_memory_instances: Vec::new(),
            rust_memory_instances: Vec::new(),
            vendor_observations: Vec::new(),
            rust_observations: Vec::new(),
        }
    }
}

pub(crate) fn unnamed_execution_address(event: &execution::ExecutionEvent) -> Option<u32> {
    match event {
        execution::ExecutionEvent::Read {
            address, register, ..
        }
        | execution::ExecutionEvent::Write {
            address, register, ..
        } if register.is_none() => Some(*address),
        _ => None,
    }
}

pub(crate) fn extend_dynamic_inventory(
    image: &execution::ExecutableImage,
    inventory: &mut execution::CoverageInventory,
    indirect_calls: &BTreeSet<execution::IndirectCall>,
) -> Result<()> {
    for call in indirect_calls {
        let dynamic =
            image.coverage_inventory_with_arguments(&call.symbol, Some(&call.arguments))?;
        inventory.branch_sites.extend(dynamic.branch_sites);
        inventory.branch_outcomes.extend(dynamic.branch_outcomes);
        inventory.unresolved_edges.extend(dynamic.unresolved_edges);
    }
    Ok(())
}

pub(crate) fn static_inventory_for_argument_domain(
    image: &execution::ExecutableImage,
    symbol: &str,
    argument_domain: &[[Option<u32>; 8]],
) -> Result<execution::CoverageInventory> {
    if argument_domain.is_empty() {
        return Err(crate::Error::invalid(
            "static argument domain must not be empty",
        ));
    }
    let mut aggregate = execution::CoverageInventory::default();
    for constraints in argument_domain {
        let inventory = image.coverage_inventory_with_argument_constraints(symbol, constraints)?;
        aggregate.branch_sites.extend(inventory.branch_sites);
        aggregate.branch_outcomes.extend(inventory.branch_outcomes);
        aggregate
            .unresolved_edges
            .extend(inventory.unresolved_edges);
    }
    Ok(aggregate)
}

#[derive(Clone, Copy)]
pub(crate) struct ExecutionInput<'a> {
    pub(crate) artifact: &'a std::path::Path,
    pub(crate) companion: Option<&'a std::path::Path>,
    pub(crate) symbol: &'a str,
}

pub(crate) fn resolved_scenario(
    named: &NamedScenario,
    image: &execution::ExecutableImage,
    vendor: bool,
) -> Result<execution::Scenario> {
    let mut scenario = named.scenario.clone();
    let words = if vendor {
        &named.vendor_symbol_words
    } else {
        &named.rust_symbol_words
    };
    let ram_words = if vendor {
        &named.vendor_ram_words
    } else {
        &named.rust_ram_words
    };
    for (address, value) in ram_words {
        write_ram_word(&mut scenario, *address, *value);
    }
    scenario.table_instances.extend(if vendor {
        named.vendor_table_instances.clone()
    } else {
        named.rust_table_instances.clone()
    });
    for (symbol, response) in if vendor {
        &named.vendor_call_responses
    } else {
        &named.rust_call_responses
    } {
        scenario
            .call_responses
            .entry(symbol.clone())
            .or_default()
            .push_back(response.clone());
    }
    materialize_memory_instances(
        &mut scenario,
        image,
        if vendor {
            &named.vendor_memory_instances
        } else {
            &named.rust_memory_instances
        },
        &named.name,
        if vendor { "vendor" } else { "Rust" },
    )?;
    for word in words {
        let value = image
            .symbol_address(&word.symbol)
            .ok_or_else(|| {
                format!(
                    "scenario {} refers to missing {} symbol {}",
                    named.name,
                    if vendor { "vendor" } else { "Rust" },
                    word.symbol
                )
            })
            .map_err(crate::Error::invalid)?;
        seed_ram_word(&mut scenario, word.address, value);
    }
    let observations = if vendor {
        &named.vendor_observations
    } else {
        &named.rust_observations
    };
    let mut comparison_start = 0_u32;
    for observation in observations {
        let (start, length) = match observation {
            MemoryObservation::Absolute { address, length } => (*address, *length),
            MemoryObservation::Symbol {
                symbol,
                offset,
                length,
            } => {
                let address = image
                    .symbol_address(symbol)
                    .ok_or_else(|| {
                        format!(
                            "scenario {} refers to missing {} observation symbol {}",
                            named.name,
                            if vendor { "vendor" } else { "Rust" },
                            symbol
                        )
                    })
                    .map_err(crate::Error::invalid)?;
                (address.wrapping_add(*offset), *length)
            }
        };
        scenario.memory_aliases.push(execution::MemoryAlias {
            start,
            length,
            comparison_start,
        });
        comparison_start = comparison_start
            .checked_add(length)
            .ok_or("normalized observation length overflow")
            .map_err(crate::Error::invalid)?;
    }
    Ok(scenario)
}

fn materialize_memory_instances(
    scenario: &mut execution::Scenario,
    image: &execution::ExecutableImage,
    instances: &[RuntimeMemoryInstance],
    scenario_name: &str,
    side: &str,
) -> Result<()> {
    let mut ids = BTreeSet::new();
    for instance in instances {
        if !ids.insert(&instance.id) {
            return Err(crate::Error::invalid(format!(
                "scenario {scenario_name} repeats {side} memory instance {}",
                instance.id
            )));
        }
        if instance.bindings.len() < 2 {
            return Err(crate::Error::invalid(format!(
                "scenario {scenario_name} {side} memory instance {} needs at least two bindings",
                instance.id
            )));
        }
        for binding in &instance.bindings {
            match binding {
                RuntimeMemoryObjectBinding::Argument { index } => {
                    if let Some(value) = scenario.arguments.get(*index)
                        && *value != instance.base_address
                    {
                        return Err(crate::Error::invalid(format!(
                            "scenario {scenario_name} {side} memory instance {} conflicts with argument {index}: {value:#010x} != {:#010x}",
                            instance.id, instance.base_address
                        )));
                    }
                    scenario
                        .arguments
                        .resize((*index + 1).max(scenario.arguments.len()), 0);
                    scenario.arguments[*index] = instance.base_address;
                }
                RuntimeMemoryObjectBinding::Global { symbol } => {
                    let address = image.symbol_address(symbol).ok_or_else(|| {
                        crate::Error::invalid(format!(
                            "scenario {scenario_name} {side} memory instance {} refers to missing global {symbol}",
                            instance.id
                        ))
                    })?;
                    if address != instance.base_address {
                        return Err(crate::Error::invalid(format!(
                            "scenario {scenario_name} {side} global {symbol} is at {address:#010x}, not runtime instance base {:#010x}",
                            instance.base_address
                        )));
                    }
                }
                RuntimeMemoryObjectBinding::DereferencedGlobal {
                    symbol,
                    pointer_offset,
                } => {
                    let address = image
                        .symbol_address(symbol)
                        .and_then(|address| address.checked_add(*pointer_offset))
                        .ok_or_else(|| {
                            crate::Error::invalid(format!(
                                "scenario {scenario_name} {side} memory instance {} cannot resolve pointer global {symbol}+{pointer_offset:#x}",
                                instance.id
                            ))
                        })?;
                    write_ram_word(scenario, address, instance.base_address);
                }
                RuntimeMemoryObjectBinding::Absolute {
                    address_space,
                    address,
                } => {
                    if *address != instance.base_address {
                        return Err(crate::Error::invalid(format!(
                            "scenario {scenario_name} {side} absolute object {address_space}:{address:#010x} does not equal runtime instance base {:#010x}",
                            instance.base_address
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}
