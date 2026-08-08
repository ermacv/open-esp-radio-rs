//! Scenario input parsing, normalization, and coverage inventory helpers.

use std::collections::BTreeSet;

use crate::*;

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

pub(crate) fn parse_table_instance(
    value: &str,
    option: &str,
) -> Result<crate::execution_model::TableInstance> {
    let parts = value.split_whitespace().collect::<Vec<_>>();
    if !(3..=4).contains(&parts.len()) {
        return Err(crate::Error::invalid(format!(
            "{option} requires LAYOUT-ID BASE SIZE [POINTER-CELL]"
        )));
    }
    let layout_id = parts[0];
    if layout_id.is_empty() || layout_id.chars().any(char::is_whitespace) {
        return Err(crate::Error::invalid(format!(
            "{option} requires one non-empty layout id"
        )));
    }
    let base_address = parse_u32(parts[1])
        .ok_or_else(|| format!("invalid {option} base address"))
        .map_err(crate::Error::invalid)?;
    let layout_size = parse_u32(parts[2])
        .ok_or_else(|| format!("invalid {option} layout size"))
        .map_err(crate::Error::invalid)?;
    let pointer_cells = parts
        .get(3)
        .map(|value| {
            parse_u32(value)
                .ok_or_else(|| format!("invalid {option} pointer cell"))
                .map_err(crate::Error::invalid)
        })
        .transpose()?
        .into_iter()
        .collect();
    Ok(crate::execution_model::TableInstance {
        layout_id: layout_id.to_owned(),
        base_address,
        layout_size,
        pointer_cells,
        slots: Vec::new(),
    })
}

pub(crate) fn parse_table_slot(
    value: &str,
    option: &str,
) -> Result<(String, crate::execution_model::TableInstanceSlot)> {
    let parts = value.split_whitespace().collect::<Vec<_>>();
    if parts.len() != 3 {
        return Err(crate::Error::invalid(format!(
            "{option} requires LAYOUT-ID OFFSET SYMBOL"
        )));
    }
    let offset = parse_u32(parts[1])
        .ok_or_else(|| format!("invalid {option} slot offset"))
        .map_err(crate::Error::invalid)?;
    if parts[0].is_empty() || parts[2].is_empty() {
        return Err(crate::Error::invalid(format!(
            "{option} requires non-empty layout and symbol names"
        )));
    }
    Ok((
        parts[0].to_owned(),
        crate::execution_model::TableInstanceSlot {
            offset,
            target: if parts[2] == "null" {
                crate::execution_model::TableSlotTarget::Null
            } else {
                crate::execution_model::TableSlotTarget::Symbol(parts[2].to_owned())
            },
        },
    ))
}

pub(crate) fn add_table_slot(
    instances: &mut [crate::execution_model::TableInstance],
    layout_id: &str,
    slot: crate::execution_model::TableInstanceSlot,
    option: &str,
) -> Result<()> {
    let instance = instances
        .iter_mut()
        .find(|instance| instance.layout_id == layout_id)
        .ok_or_else(|| format!("{option} refers to undeclared table {layout_id}"))
        .map_err(crate::Error::invalid)?;
    instance.slots.push(slot);
    Ok(())
}

pub(crate) fn parse_symbol_observation(value: &str, option: &str) -> Result<MemoryObservation> {
    let (target, length) = value
        .split_once('=')
        .ok_or_else(|| format!("{option} requires SYMBOL[+OFFSET]=LENGTH"))
        .map_err(crate::Error::invalid)?;
    let length = parse_u32(length)
        .ok_or_else(|| format!("invalid {option} length"))
        .map_err(crate::Error::invalid)?;
    if length == 0 {
        return Err(crate::Error::invalid(format!(
            "{option} length must be non-zero"
        )));
    }
    let (symbol, offset) = target
        .split_once('+')
        .map_or((target, 0), |(symbol, offset)| {
            (symbol, parse_u32(offset).unwrap_or(u32::MAX))
        });
    if symbol.is_empty() || offset == u32::MAX {
        return Err(crate::Error::invalid(format!(
            "invalid {option} symbol or offset"
        )));
    }
    Ok(MemoryObservation::Symbol {
        symbol: symbol.to_owned(),
        offset,
        length,
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

#[derive(Clone, Debug)]
pub(crate) struct SymbolWord {
    pub(crate) address: u32,
    pub(crate) symbol: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RuntimeMemoryObjectBinding {
    Argument { index: usize },
    Global { symbol: String },
    DereferencedGlobal { symbol: String, pointer_offset: u32 },
    Absolute { address_space: String, address: u32 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RuntimeMemoryInstance {
    pub(crate) id: String,
    pub(crate) base_address: u32,
    pub(crate) length: u32,
    pub(crate) bindings: Vec<RuntimeMemoryObjectBinding>,
}

pub(crate) fn parse_memory_instance(value: &str, option: &str) -> Result<RuntimeMemoryInstance> {
    let parts = value.split_whitespace().collect::<Vec<_>>();
    let [id, base, length] = parts.as_slice() else {
        return Err(crate::Error::invalid(format!(
            "{option} requires INSTANCE-ID BASE LENGTH"
        )));
    };
    if id.is_empty() {
        return Err(crate::Error::invalid(format!(
            "{option} requires a non-empty instance id"
        )));
    }
    let base_address = parse_u32(base)
        .ok_or_else(|| format!("invalid {option} base address"))
        .map_err(crate::Error::invalid)?;
    let length = parse_u32(length)
        .filter(|length| *length != 0)
        .ok_or_else(|| format!("invalid {option} length"))
        .map_err(crate::Error::invalid)?;
    base_address
        .checked_add(length)
        .ok_or_else(|| format!("{option} range overflows the 32-bit address space"))
        .map_err(crate::Error::invalid)?;
    Ok(RuntimeMemoryInstance {
        id: (*id).to_owned(),
        base_address,
        length,
        bindings: Vec::new(),
    })
}

pub(crate) fn add_memory_instance_binding(
    instances: &mut [RuntimeMemoryInstance],
    value: &str,
    option: &str,
) -> Result<()> {
    let parts = value.split_whitespace().collect::<Vec<_>>();
    let Some((id, binding)) = parts.split_first() else {
        return Err(crate::Error::invalid(format!(
            "{option} requires INSTANCE-ID KIND ..."
        )));
    };
    let instance = instances
        .iter_mut()
        .find(|instance| instance.id == *id)
        .ok_or_else(|| format!("{option} refers to undeclared memory instance {id}"))
        .map_err(crate::Error::invalid)?;
    let binding = match binding {
        ["argument", index] => RuntimeMemoryObjectBinding::Argument {
            index: parse_u32(index)
                .and_then(|index| usize::try_from(index).ok())
                .filter(|index| *index < 8)
                .ok_or_else(|| format!("invalid {option} argument index"))
                .map_err(crate::Error::invalid)?,
        },
        ["global", symbol] if !symbol.is_empty() => RuntimeMemoryObjectBinding::Global {
            symbol: (*symbol).to_owned(),
        },
        ["dereferenced-global", symbol, pointer_offset] if !symbol.is_empty() => {
            RuntimeMemoryObjectBinding::DereferencedGlobal {
                symbol: (*symbol).to_owned(),
                pointer_offset: parse_u32(pointer_offset)
                    .ok_or_else(|| format!("invalid {option} pointer offset"))
                    .map_err(crate::Error::invalid)?,
            }
        }
        ["absolute", address_space, address] if !address_space.is_empty() => {
            RuntimeMemoryObjectBinding::Absolute {
                address_space: (*address_space).to_owned(),
                address: parse_u32(address)
                    .ok_or_else(|| format!("invalid {option} absolute address"))
                    .map_err(crate::Error::invalid)?,
            }
        }
        _ => {
            return Err(crate::Error::invalid(format!(
                "{option} requires INSTANCE-ID followed by argument INDEX, global SYMBOL, dereferenced-global SYMBOL POINTER-OFFSET, or absolute ADDRESS-SPACE ADDRESS"
            )));
        }
    };
    if instance.bindings.contains(&binding) {
        return Err(crate::Error::invalid(format!(
            "{option} repeats a binding for memory instance {id}"
        )));
    }
    instance.bindings.push(binding);
    Ok(())
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
    pub(crate) vendor_table_instances: Vec<crate::execution_model::TableInstance>,
    pub(crate) rust_table_instances: Vec<crate::execution_model::TableInstance>,
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
