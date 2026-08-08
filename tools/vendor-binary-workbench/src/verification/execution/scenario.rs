//! Scenario input parsing, normalization, and coverage inventory helpers.

use std::collections::BTreeSet;

use crate::*;

pub(crate) fn parse_assignment(value: &str, option: &str) -> Result<(u32, u32)> {
    let (address, value) = value
        .split_once('=')
        .ok_or_else(|| format!("{option} requires ADDRESS=VALUE"))?;
    let address = parse_u32(address).ok_or_else(|| format!("invalid {option} address"))?;
    let value = parse_u32(value).ok_or_else(|| format!("invalid {option} value"))?;
    Ok((address, value))
}

pub(crate) fn parse_call_return(value: &str, option: &str) -> Result<(String, u32)> {
    let (symbol, value) = value
        .split_once('=')
        .ok_or_else(|| format!("{option} requires SYMBOL=VALUE"))?;
    if symbol.is_empty() || symbol.chars().any(char::is_whitespace) {
        return Err(format!("{option} requires one non-empty symbol").into());
    }
    let value = parse_u32(value).ok_or_else(|| format!("invalid {option} value"))?;
    Ok((symbol.to_owned(), value))
}

pub(crate) fn parse_symbol_word(value: &str, option: &str) -> Result<SymbolWord> {
    let (address, symbol) = value
        .split_once('=')
        .ok_or_else(|| format!("{option} requires ADDRESS=SYMBOL"))?;
    let address = parse_u32(address).ok_or_else(|| format!("invalid {option} address"))?;
    if symbol.is_empty() {
        return Err(format!("{option} requires a non-empty symbol").into());
    }
    Ok(SymbolWord {
        address,
        symbol: symbol.to_owned(),
    })
}

pub(crate) fn parse_symbol_observation(value: &str, option: &str) -> Result<MemoryObservation> {
    let (target, length) = value
        .split_once('=')
        .ok_or_else(|| format!("{option} requires SYMBOL[+OFFSET]=LENGTH"))?;
    let length = parse_u32(length).ok_or_else(|| format!("invalid {option} length"))?;
    if length == 0 {
        return Err(format!("{option} length must be non-zero").into());
    }
    let (symbol, offset) = target
        .split_once('+')
        .map_or((target, 0), |(symbol, offset)| {
            (symbol, parse_u32(offset).unwrap_or(u32::MAX))
        });
    if symbol.is_empty() || offset == u32::MAX {
        return Err(format!("invalid {option} symbol or offset").into());
    }
    Ok(MemoryObservation::Symbol {
        symbol: symbol.to_owned(),
        offset,
        length,
    })
}

pub(crate) fn seed_ram_word(scenario: &mut execution::Scenario, address: u32, value: u32) {
    write_ram_word(scenario, address, value);
    scenario.observed_memory.push(execution::MemoryRange {
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
        return Err("--observe length must be non-zero".into());
    }
    scenario.observed_memory.push(execution::MemoryRange {
        start: address,
        length,
    });
    Ok(())
}

#[derive(Clone, Debug)]
pub(crate) struct SymbolWord {
    pub(crate) address: u32,
    pub(crate) symbol: String,
}

#[derive(Clone, Debug)]
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
            vendor_observations: Vec::new(),
            rust_observations: Vec::new(),
        }
    }
}

pub(crate) fn unmapped_execution_address(event: &execution::ExecutionEvent) -> Option<u32> {
    match event {
        execution::ExecutionEvent::Read {
            address, register, ..
        }
        | execution::ExecutionEvent::Write {
            address, register, ..
        } if register == "UNMAPPED" => Some(*address),
        _ => None,
    }
}

pub(crate) fn print_branch_coverage(
    side: &str,
    image: &execution::ExecutableImage,
    required: &BTreeSet<(u32, bool)>,
    covered: &BTreeSet<(u32, bool)>,
) -> usize {
    let mut uncovered = 0;
    for (site, taken) in required {
        let location = image.location(*site);
        if covered.contains(&(*site, *taken)) {
            outputln!("COVERED-BRANCH\t{side}\t{location}\ttaken={taken}");
        } else {
            outputln!("UNCOVERED-BRANCH\t{side}\t{location}\ttaken={taken}");
            uncovered += 1;
        }
    }
    let sites: BTreeSet<_> = required.iter().map(|(site, _)| *site).collect();
    outputln!(
        "SUMMARY-BRANCHES\t{side}\tsites={}\toutcomes={}\tcovered={}\tuncovered={uncovered}",
        sites.len(),
        required.len(),
        required.len() - uncovered,
    );
    uncovered
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
        return Err("static argument domain must not be empty".into());
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
    for word in words {
        let value = image.symbol_address(&word.symbol).ok_or_else(|| {
            format!(
                "scenario {} refers to missing {} symbol {}",
                named.name,
                if vendor { "vendor" } else { "Rust" },
                word.symbol
            )
        })?;
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
                let address = image.symbol_address(symbol).ok_or_else(|| {
                    format!(
                        "scenario {} refers to missing {} observation symbol {}",
                        named.name,
                        if vendor { "vendor" } else { "Rust" },
                        symbol
                    )
                })?;
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
            .ok_or("normalized observation length overflow")?;
    }
    Ok(scenario)
}
