//! Parser retained only for the older human-readable disassembly unit tests.

use std::collections::HashMap;

use crate::{
    DraftReferenceEvent, FunctionAnalysis, MemoryAccess, MmioMap, ObservableEvent, SymbolicValue,
    parse_fence_set, parse_u32,
};

#[cfg(test)]
fn parse_i64(value: &str) -> Option<i64> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix("-0x") {
        i64::from_str_radix(hex, 16).ok().map(|number| -number)
    } else if let Some(hex) = value.strip_prefix("0x") {
        i64::from_str_radix(hex, 16).ok()
    } else {
        value.parse().ok()
    }
}

#[cfg(test)]
fn split_operands(operands: &str) -> Vec<&str> {
    operands.split(',').map(str::trim).collect()
}

#[cfg(test)]
fn memory_operand(operand: &str) -> Option<(i64, &str)> {
    let open = operand.find('(')?;
    let close = operand.rfind(')')?;
    let offset = parse_i64(operand[..open].trim())?;
    Some((offset, operand[open + 1..close].trim()))
}

#[cfg(test)]
fn effective_address(values: &HashMap<String, SymbolicValue>, operand: &str) -> Option<u32> {
    let (offset, base) = memory_operand(operand)?;
    let SymbolicValue::Constant(base) = values.get(base)? else {
        return None;
    };
    Some(base.wrapping_add(offset as u32))
}

#[cfg(test)]
fn width_for(mnemonic: &str) -> Option<u8> {
    match mnemonic {
        "lb" | "lbu" | "sb" => Some(8),
        "lh" | "lhu" | "sh" => Some(16),
        "lw" | "sw" => Some(32),
        _ => None,
    }
}

#[cfg(test)]
fn disassembly_label(line: &str) -> Option<&str> {
    let (_, remainder) = line.split_once('<')?;
    let (name, suffix) = remainder.split_once('>')?;
    suffix.trim().eq(":").then_some(name)
}

#[cfg(test)]
pub(crate) fn trace_disassembly(
    symbol: &str,
    disassembly: &str,
    svd: &MmioMap,
) -> FunctionAnalysis {
    let mut values: HashMap<String, SymbolicValue> = HashMap::new();
    for (index, register) in ["a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7"]
        .into_iter()
        .enumerate()
    {
        values.insert(register.to_owned(), SymbolicValue::input(index as u8));
    }
    let mut events = Vec::new();
    let mut reference_events = Vec::new();
    let mut blockers = Vec::new();
    let mut reference_blockers = Vec::new();
    let mut return_value = SymbolicValue::Unknown;
    let mut next_mmio_read_token = 0_u32;
    let mut in_symbol = false;

    for line in disassembly.lines() {
        let trimmed = line.trim();
        if let Some(label) = disassembly_label(trimmed) {
            if label == symbol {
                in_symbol = true;
            } else if in_symbol && !label.starts_with('.') {
                break;
            }
            continue;
        }
        if !in_symbol {
            continue;
        }
        let Some((pc_text, instruction)) = trimmed.split_once(':') else {
            continue;
        };
        if u64::from_str_radix(pc_text.trim(), 16).is_err() {
            continue;
        }
        let instruction = instruction.trim();
        if instruction.is_empty() {
            continue;
        }
        let (mnemonic, operands) = instruction
            .split_once(char::is_whitespace)
            .map(|(mnemonic, operands)| (mnemonic, operands.trim()))
            .unwrap_or((instruction, ""));
        let operands = split_operands(operands);

        if mnemonic.starts_with('b') && mnemonic != "bseti" && mnemonic != "bclri" {
            blockers.push(format!(
                "control-flow instruction at 0x{}: {instruction}",
                pc_text.trim()
            ));
        }
        if matches!(
            mnemonic,
            "j" | "jr" | "jal" | "jalr" | "call" | "tail" | "c.j" | "c.jr" | "c.jal" | "c.jalr"
        ) {
            blockers.push(format!(
                "call/jump instruction at 0x{}: {instruction}",
                pc_text.trim()
            ));
        }

        match mnemonic {
            "lui" if operands.len() == 2 => {
                let value = parse_i64(operands[1])
                    .map(|value| SymbolicValue::Constant((value as u32) << 12))
                    .unwrap_or(SymbolicValue::Unknown);
                values.insert(operands[0].to_owned(), value);
            }
            "li" if operands.len() == 2 => {
                let value = parse_i64(operands[1])
                    .map(|value| SymbolicValue::Constant(value as u32))
                    .unwrap_or(SymbolicValue::Unknown);
                values.insert(operands[0].to_owned(), value);
            }
            "mv" if operands.len() == 2 => {
                let value = values
                    .get(operands[1])
                    .cloned()
                    .unwrap_or(SymbolicValue::Unknown);
                values.insert(operands[0].to_owned(), value);
            }
            "addi" if operands.len() == 3 => {
                let value = match (values.get(operands[1]).cloned(), parse_i64(operands[2])) {
                    (Some(source), Some(offset)) => source.add_constant(offset as u32),
                    _ => SymbolicValue::Unknown,
                };
                values.insert(operands[0].to_owned(), value);
            }
            "and" | "or" if operands.len() == 3 => {
                let left = values
                    .get(operands[1])
                    .cloned()
                    .unwrap_or(SymbolicValue::Unknown);
                let right = values
                    .get(operands[2])
                    .cloned()
                    .unwrap_or(SymbolicValue::Unknown);
                let value = match mnemonic {
                    "and" => left.symbolic_bitand(right),
                    "or" => left.symbolic_bitor(right),
                    _ => unreachable!(),
                };
                values.insert(operands[0].to_owned(), value);
            }
            "andi" | "ori" | "xori" if operands.len() == 3 => {
                let source = values
                    .get(operands[1])
                    .cloned()
                    .unwrap_or(SymbolicValue::Unknown);
                let value = match parse_i64(operands[2]) {
                    Some(constant) if mnemonic == "andi" => source.and(constant as u32),
                    Some(constant) if mnemonic == "ori" => source.or(constant as u32),
                    Some(constant) => source.xor(constant as u32),
                    None => SymbolicValue::Unknown,
                };
                values.insert(operands[0].to_owned(), value);
            }
            "slli" | "srli" if operands.len() == 3 => {
                let source = values
                    .get(operands[1])
                    .cloned()
                    .unwrap_or(SymbolicValue::Unknown);
                let value = match parse_u32(operands[2]).filter(|amount| *amount < 32) {
                    Some(amount) if mnemonic == "slli" => source.shift_left(amount),
                    Some(amount) => source.shift_right(amount),
                    None => SymbolicValue::Unknown,
                };
                values.insert(operands[0].to_owned(), value);
            }
            "not" if operands.len() == 2 => {
                let source = values
                    .get(operands[1])
                    .cloned()
                    .unwrap_or(SymbolicValue::Unknown);
                values.insert(operands[0].to_owned(), source.symbolic_not());
            }
            "seqz" if operands.len() == 2 => {
                let source = values
                    .get(operands[1])
                    .cloned()
                    .unwrap_or(SymbolicValue::Unknown);
                values.insert(operands[0].to_owned(), source.seqz());
            }
            "bseti" | "bclri" if operands.len() == 3 => {
                let source = values
                    .get(operands[1])
                    .cloned()
                    .unwrap_or(SymbolicValue::Unknown);
                let value = match parse_u32(operands[2]).filter(|bit| *bit < 32) {
                    Some(bit) if mnemonic == "bseti" => source.or(1 << bit),
                    Some(bit) => source.and(!(1 << bit)),
                    None => SymbolicValue::Unknown,
                };
                values.insert(operands[0].to_owned(), value);
            }
            "lb" | "lbu" | "lh" | "lhu" | "lw" if operands.len() == 2 => {
                let address = effective_address(&values, operands[1]);
                if let Some(address) = address.filter(|address| svd.contains_mmio(*address)) {
                    let width = width_for(mnemonic).unwrap();
                    let read_token = next_mmio_read_token;
                    next_mmio_read_token += 1;
                    let event = ObservableEvent::Memory {
                        access: MemoryAccess::Read,
                        width,
                        address,
                        register: svd.display_register_name(address),
                        value: None,
                    };
                    events.push(event.clone());
                    reference_events.push(DraftReferenceEvent::Observable(event));
                    values.insert(
                        operands[0].to_owned(),
                        if width == 32 {
                            SymbolicValue::RegisterImage {
                                read_token,
                                address,
                                and_mask: u32::MAX,
                                or_mask: 0,
                            }
                        } else {
                            SymbolicValue::Unknown
                        },
                    );
                } else {
                    reference_blockers.push(format!(
                        "unmodeled-memory-load at 0x{}: {instruction}",
                        pc_text.trim()
                    ));
                    values.insert(operands[0].to_owned(), SymbolicValue::Unknown);
                }
            }
            "sb" | "sh" | "sw" if operands.len() == 2 => {
                if let Some(address) = effective_address(&values, operands[1])
                    .filter(|address| svd.contains_mmio(*address))
                {
                    let value = values
                        .get(operands[0])
                        .cloned()
                        .unwrap_or(SymbolicValue::Unknown);
                    if !value.is_resolved() {
                        blockers.push(format!(
                            "unresolved MMIO write value at 0x{}: {instruction}",
                            pc_text.trim()
                        ));
                    }
                    let event = ObservableEvent::Memory {
                        access: MemoryAccess::Write,
                        width: width_for(mnemonic).unwrap(),
                        address,
                        register: svd.display_register_name(address),
                        value: Some(value),
                    };
                    events.push(event.clone());
                    reference_events.push(DraftReferenceEvent::Observable(event));
                } else {
                    reference_blockers.push(format!(
                        "unmodeled-memory-store at 0x{}: {instruction}",
                        pc_text.trim()
                    ));
                }
            }
            "ret" => {
                return_value = values.get("a0").cloned().unwrap_or(SymbolicValue::Unknown);
            }
            "fence" if operands.len() == 2 => {
                match (parse_fence_set(operands[0]), parse_fence_set(operands[1])) {
                    (Some(predecessor), Some(successor)) => {
                        let event = ObservableEvent::Fence {
                            fm: 0,
                            predecessor,
                            successor,
                        };
                        events.push(event.clone());
                        reference_events.push(DraftReferenceEvent::Observable(event));
                    }
                    _ => blockers.push(format!(
                        "unsupported fence at 0x{}: {instruction}",
                        pc_text.trim()
                    )),
                }
            }
            "nop" => {}
            "fence.i" => blockers.push(format!(
                "unsupported instruction-cache fence at 0x{}: {instruction}",
                pc_text.trim()
            )),
            _ => {
                if let Some(destination) = operands.first()
                    && is_register(destination)
                    && !matches!(mnemonic, "sw" | "sh" | "sb")
                {
                    values.insert((*destination).to_owned(), SymbolicValue::Unknown);
                }
            }
        }
    }
    if !in_symbol {
        blockers.push("symbol was not present in decoded instruction stream".to_owned());
    }
    FunctionAnalysis {
        symbol: symbol.to_owned(),
        events,
        located_events: Vec::new(),
        reference_events,
        reference_dependencies: Vec::new(),
        blockers,
        reference_blockers,
        return_value,
        reference_flow: None,
        unresolved_branch: None,
    }
}

#[cfg(test)]
fn is_register(value: &str) -> bool {
    matches!(
        value,
        "zero"
            | "ra"
            | "sp"
            | "gp"
            | "tp"
            | "t0"
            | "t1"
            | "t2"
            | "s0"
            | "fp"
            | "s1"
            | "a0"
            | "a1"
            | "a2"
            | "a3"
            | "a4"
            | "a5"
            | "a6"
            | "a7"
            | "s2"
            | "s3"
            | "s4"
            | "s5"
            | "s6"
            | "s7"
            | "s8"
            | "s9"
            | "s10"
            | "s11"
            | "t3"
            | "t4"
            | "t5"
            | "t6"
    )
}
