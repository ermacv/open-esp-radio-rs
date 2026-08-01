//! Concrete execution scenarios and vendor/Rust comparison.

use std::{collections::BTreeSet, path::Path};

use crate::*;

pub(crate) fn parse_assignment(value: &str, option: &str) -> Result<(u32, u32)> {
    let (address, value) = value
        .split_once('=')
        .ok_or_else(|| format!("{option} requires ADDRESS=VALUE"))?;
    let address = parse_u32(address).ok_or_else(|| format!("invalid {option} address"))?;
    let value = parse_u32(value).ok_or_else(|| format!("invalid {option} value"))?;
    Ok((address, value))
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ComparisonVerdict {
    Match,
    Mismatch,
    Incomplete,
}

impl ComparisonVerdict {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Match => "MATCH",
            Self::Mismatch => "MISMATCH",
            Self::Incomplete => "INCOMPLETE",
        }
    }
}

pub(crate) fn print_execution_event(side: &str, index: usize, event: &execution::ExecutionEvent) {
    match event {
        execution::ExecutionEvent::Read {
            width,
            address,
            register,
            value,
        } => println!(
            "TRACE-EVENT\t{side}\t{index}\tR\t{width}\t{address:#010x}\t{register}\tvalue={value:#010x}"
        ),
        execution::ExecutionEvent::Write {
            width,
            address,
            register,
            value,
        } => println!(
            "TRACE-EVENT\t{side}\t{index}\tW\t{width}\t{address:#010x}\t{register}\tvalue={value:#010x}",
        ),
        execution::ExecutionEvent::DelayMicros(micros) => {
            println!("TRACE-EVENT\t{side}\t{index}\tDELAY\tmicros={micros}");
        }
        execution::ExecutionEvent::Fence {
            fm,
            predecessor,
            successor,
        } => println!(
            "TRACE-EVENT\t{side}\t{index}\tFENCE\tfm={fm:#x}\tpred={predecessor:#x}\tsucc={successor:#x}"
        ),
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
            println!("COVERED-BRANCH\t{side}\t{location}\ttaken={taken}");
        } else {
            println!("UNCOVERED-BRANCH\t{side}\t{location}\ttaken={taken}");
            uncovered += 1;
        }
    }
    let sites: BTreeSet<_> = required.iter().map(|(site, _)| *site).collect();
    println!(
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

pub(crate) fn print_control_flow_coverage(
    side: &str,
    image: &execution::ExecutableImage,
    inventory: &execution::CoverageInventory,
    indirect_calls: &BTreeSet<execution::IndirectCall>,
) -> usize {
    let mut uncovered = 0;
    for (address, edge) in &inventory.unresolved_edges {
        let targets: Vec<_> = indirect_calls
            .iter()
            .filter_map(|call| (call.site == *address).then_some(call.symbol.as_str()))
            .collect();
        if targets.is_empty() {
            println!(
                "UNCOVERED-CONTROL-FLOW\t{side}\t{}\t{edge}",
                image.location(*address)
            );
            uncovered += 1;
        } else {
            println!(
                "COVERED-CONTROL-FLOW\t{side}\t{}\ttargets={}",
                image.location(*address),
                targets.join(",")
            );
        }
    }
    uncovered
}

#[derive(Clone, Copy)]
pub(crate) struct ExecutionInput<'a> {
    pub(crate) artifact: &'a Path,
    pub(crate) companion: Option<&'a Path>,
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

pub(crate) fn compare_execution_scenarios(
    svd: &MmioRegisterMap,
    vendor: ExecutionInput<'_>,
    rust: ExecutionInput<'_>,
    compare_return: bool,
    scenarios: &[NamedScenario],
) -> Result<ComparisonVerdict> {
    let vendor_digest = pinned_vendor_digest(vendor.artifact)?;
    println!(
        "ORACLE\t{}\tsha256={vendor_digest}",
        vendor.artifact.display()
    );
    if let Some(companion) = vendor.companion {
        let companion_digest = pinned_vendor_digest(companion)?;
        println!("ORACLE\t{}\tsha256={companion_digest}", companion.display());
    }
    let mut vendor_image = execution::ExecutableImage::load(vendor.artifact)?;
    if let Some(companion) = vendor.companion {
        vendor_image.add_companion(companion)?;
    }
    let mut rust_image = execution::ExecutableImage::load(rust.artifact)?;
    if let Some(companion) = rust.companion {
        rust_image.add_companion(companion)?;
    }
    let mut vendor_inventory = vendor_image.coverage_inventory(vendor.symbol)?;
    let mut rust_inventory = rust_image.coverage_inventory(rust.symbol)?;
    let mut vendor_covered = BTreeSet::new();
    let mut rust_covered = BTreeSet::new();
    let mut vendor_calls = BTreeSet::new();
    let mut rust_calls = BTreeSet::new();
    let mut vendor_indirect_calls = BTreeSet::new();
    let mut rust_indirect_calls = BTreeSet::new();
    let mut vendor_unmapped = BTreeSet::new();
    let mut rust_unmapped = BTreeSet::new();
    let mut matched_cases = 0_usize;
    let mut mismatched_cases = 0_usize;
    let mut incomplete_cases = 0_usize;

    for named in scenarios {
        let vendor_lengths: Vec<_> = named
            .vendor_observations
            .iter()
            .map(MemoryObservation::length)
            .collect();
        let rust_lengths: Vec<_> = named
            .rust_observations
            .iter()
            .map(MemoryObservation::length)
            .collect();
        if vendor_lengths != rust_lengths {
            return Err(format!(
                "scenario {} has different vendor/Rust observation layouts",
                named.name
            )
            .into());
        }
        let vendor_result = execution::execute(
            &vendor_image,
            svd,
            vendor.symbol,
            resolved_scenario(named, &vendor_image, true)?,
        );
        let rust_result = execution::execute(
            &rust_image,
            svd,
            rust.symbol,
            resolved_scenario(named, &rust_image, false)?,
        );
        let (vendor_result, rust_result) = match (vendor_result, rust_result) {
            (Ok(vendor_result), Ok(rust_result)) => (vendor_result, rust_result),
            (vendor_result, rust_result) => {
                incomplete_cases += 1;
                println!(
                    "CASE\t{}\tINCOMPLETE\tvendor={}\trust={}",
                    named.name,
                    vendor_result
                        .err()
                        .map_or_else(|| "complete".to_owned(), |error| error.to_string()),
                    rust_result
                        .err()
                        .map_or_else(|| "complete".to_owned(), |error| error.to_string()),
                );
                continue;
            }
        };
        vendor_covered.extend(vendor_result.branches.iter().copied());
        rust_covered.extend(rust_result.branches.iter().copied());
        vendor_calls.extend(vendor_result.calls.iter().cloned());
        rust_calls.extend(rust_result.calls.iter().cloned());
        vendor_indirect_calls.extend(vendor_result.indirect_calls.iter().cloned());
        rust_indirect_calls.extend(rust_result.indirect_calls.iter().cloned());
        vendor_unmapped.extend(
            vendor_result
                .events
                .iter()
                .filter_map(unmapped_execution_address),
        );
        rust_unmapped.extend(
            rust_result
                .events
                .iter()
                .filter_map(unmapped_execution_address),
        );

        let events_equal = vendor_result.events == rust_result.events;
        let memory_equal = vendor_result.memory_changes == rust_result.memory_changes;
        let returns_equal =
            !compare_return || vendor_result.return_value == rust_result.return_value;
        if events_equal && memory_equal && returns_equal {
            matched_cases += 1;
            println!(
                "CASE\t{}\tMATCH\tevents={}\tmemory-changes={}\treturn={}",
                named.name,
                vendor_result.events.len(),
                vendor_result.memory_changes.len(),
                if compare_return { "checked" } else { "ignored" }
            );
        } else {
            mismatched_cases += 1;
            println!(
                "CASE\t{}\tMISMATCH\tvendor-events={}\trust-events={}\tvendor-memory-changes={}\trust-memory-changes={}\tvendor-return={:#010x}\trust-return={:#010x}",
                named.name,
                vendor_result.events.len(),
                rust_result.events.len(),
                vendor_result.memory_changes.len(),
                rust_result.memory_changes.len(),
                vendor_result.return_value,
                rust_result.return_value,
            );
            for (index, event) in vendor_result.events.iter().enumerate() {
                print_execution_event("vendor", index, event);
            }
            for (index, event) in rust_result.events.iter().enumerate() {
                print_execution_event("rust", index, event);
            }
            for change in &vendor_result.memory_changes {
                println!(
                    "MEMORY-CHANGE\tvendor\t{:#010x}\tbefore={:#04x}\tafter={:#04x}",
                    change.address, change.before, change.after
                );
            }
            for change in &rust_result.memory_changes {
                println!(
                    "MEMORY-CHANGE\trust\t{:#010x}\tbefore={:#04x}\tafter={:#04x}",
                    change.address, change.before, change.after
                );
            }
        }
    }

    for call in vendor_calls {
        println!("COVERED-CALL\tvendor\t{call}");
    }
    for call in rust_calls {
        println!("COVERED-CALL\trust\t{call}");
    }
    extend_dynamic_inventory(&vendor_image, &mut vendor_inventory, &vendor_indirect_calls)?;
    extend_dynamic_inventory(&rust_image, &mut rust_inventory, &rust_indirect_calls)?;
    let vendor_uncovered = print_branch_coverage(
        "vendor",
        &vendor_image,
        &vendor_inventory.branch_outcomes,
        &vendor_covered,
    );
    let rust_uncovered = print_branch_coverage(
        "rust",
        &rust_image,
        &rust_inventory.branch_outcomes,
        &rust_covered,
    );
    let vendor_unresolved = print_control_flow_coverage(
        "vendor",
        &vendor_image,
        &vendor_inventory,
        &vendor_indirect_calls,
    );
    let rust_unresolved =
        print_control_flow_coverage("rust", &rust_image, &rust_inventory, &rust_indirect_calls);
    for address in &vendor_unmapped {
        println!("UNCOVERED-MMIO\tvendor\t{address:#010x}");
    }
    for address in &rust_unmapped {
        println!("UNCOVERED-MMIO\trust\t{address:#010x}");
    }
    let cases_match = matched_cases == scenarios.len();
    let coverage_complete = vendor_uncovered == 0
        && rust_uncovered == 0
        && vendor_unresolved == 0
        && rust_unresolved == 0
        && vendor_unmapped.is_empty()
        && rust_unmapped.is_empty();
    let verdict = if mismatched_cases != 0 {
        ComparisonVerdict::Mismatch
    } else if incomplete_cases != 0 || !coverage_complete || !cases_match {
        ComparisonVerdict::Incomplete
    } else {
        ComparisonVerdict::Match
    };
    println!(
        "SUMMARY\tcases={}\tmatched={matched_cases}\tmismatched={mismatched_cases}\tincomplete={incomplete_cases}\tvendor-uncovered-branch-outcomes={vendor_uncovered}\trust-uncovered-branch-outcomes={rust_uncovered}\tvendor-unresolved-control-flow={}\trust-unresolved-control-flow={}\tvendor-unmapped-mmio={}\trust-unmapped-mmio={}",
        scenarios.len(),
        vendor_unresolved,
        rust_unresolved,
        vendor_unmapped.len(),
        rust_unmapped.len(),
    );
    println!("VERDICT\t{}", verdict.label());
    Ok(verdict)
}
