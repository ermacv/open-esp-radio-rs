//! Conservative branch and direct-control-flow inventory.

use std::collections::{BTreeMap, BTreeSet};

use rv_asm::{Inst, Reg};

use super::{CoverageInventory, ExecutableImage};
use crate::{Result, artifact::andi_immediate};

impl ExecutableImage {
    /// Finds every conditional branch reachable through direct control flow.
    ///
    /// Both feasible successors of a conditional branch are explored. With no
    /// argument constraints both remain feasible; a concrete ABI fact prunes
    /// the complete unreachable descendant graph. Direct calls are followed
    /// as well as their return continuation, so branch coverage also includes
    /// statically linked children. An indirect edge is retained as unresolved
    /// only when propagated constants cannot determine its target.
    pub fn coverage_inventory(&self, symbol: &str) -> Result<CoverageInventory> {
        self.coverage_inventory_with_argument_constraints(symbol, &[None; 8])
    }

    pub fn coverage_inventory_with_arguments(
        &self,
        symbol: &str,
        arguments: Option<&[u32; 8]>,
    ) -> Result<CoverageInventory> {
        let constraints = arguments.map_or([None; 8], |arguments| {
            core::array::from_fn(|index| Some(arguments[index]))
        });
        self.coverage_inventory_with_argument_constraints(symbol, &constraints)
    }

    /// Inventories control flow reachable under partial ABI argument facts.
    ///
    /// `None` leaves one `a0..a7` input unconstrained. A concrete value prunes
    /// both conditional outcomes and all descendants of an infeasible edge,
    /// including panic paths and their non-returning fallthrough bytes.
    pub fn coverage_inventory_with_argument_constraints(
        &self,
        symbol: &str,
        arguments: &[Option<u32>; 8],
    ) -> Result<CoverageInventory> {
        self.coverage_inventory_with_constraints(symbol, arguments, &BTreeMap::new())
    }

    /// Inventories control flow under ABI facts and reviewed stable memory
    /// words. The latter is primarily used for finite MMIO selector domains;
    /// ordered or stateful reads deliberately remain unknown.
    pub fn coverage_inventory_with_constraints(
        &self,
        symbol: &str,
        arguments: &[Option<u32>; 8],
        stable_words: &BTreeMap<u32, u32>,
    ) -> Result<CoverageInventory> {
        let start = self
            .symbol_address(symbol)
            .ok_or_else(|| format!("execution symbol {symbol} was not found"))?;
        const MAX_ABSTRACT_UPDATES: usize = 200_000;

        let mut initial = [None; 32];
        initial[usize::from(Reg::ZERO.0)] = Some(0);
        for (index, value) in arguments.iter().copied().enumerate() {
            initial[usize::from(Reg::A0.0) + index] = value;
        }
        let unknown_after_call = || {
            let mut registers = [None; 32];
            registers[usize::from(Reg::ZERO.0)] = Some(0);
            registers
        };
        let after_call = |before: [Option<u32>; 32]| {
            let mut registers = unknown_after_call();
            for register in [
                Reg::SP,
                Reg::GP,
                Reg::TP,
                Reg::S0,
                Reg::S1,
                Reg::S2,
                Reg::S3,
                Reg::S4,
                Reg::S5,
                Reg::S6,
                Reg::S7,
                Reg::S8,
                Reg::S9,
                Reg::S10,
                Reg::S11,
            ] {
                registers[usize::from(register.0)] = before[usize::from(register.0)];
            }
            registers
        };
        let mut pending = Vec::new();
        let mut queued = BTreeSet::new();
        let mut states = BTreeMap::<u32, [Option<u32>; 32]>::new();
        let mut updates = 0_usize;
        let mut inventory = CoverageInventory::default();

        // Constant propagation is a finite lattice at each instruction: an
        // unseen register fact becomes one concrete value and may widen once
        // to unknown. Joining at the CFG address avoids enumerating an
        // exponential cross-product of register tuples around loops while
        // remaining conservative for branch coverage.
        macro_rules! enqueue {
            ($address:expr, $incoming:expr $(,)?) => {{
                let address = $address;
                let incoming = $incoming;
                let changed = if let Some(existing) = states.get_mut(&address) {
                    let mut changed = false;
                    for (existing, incoming) in existing.iter_mut().zip(incoming) {
                        let joined = if *existing == incoming {
                            *existing
                        } else {
                            None
                        };
                        if *existing != joined {
                            *existing = joined;
                            changed = true;
                        }
                    }
                    changed
                } else {
                    states.insert(address, incoming);
                    true
                };
                if changed && queued.insert(address) {
                    pending.push(address);
                }
            }};
        }

        enqueue!(start, initial);

        while let Some(address) = pending.pop() {
            queued.remove(&address);
            updates += 1;
            if updates > MAX_ABSTRACT_UPDATES {
                return Err(format!(
                    "abstract branch analysis exceeded {MAX_ABSTRACT_UPDATES} state updates"
                )
                .into());
            }
            let mut registers = states[&address];
            if let Some(symbol) = self.symbol_at(address)
                && is_opaque_runtime_support(symbol)
            {
                continue;
            }
            if let Some(call) = self.relocated_call_at(address) {
                let link = self.relocated_call_link_register(address)?;
                if link != Reg::ZERO {
                    enqueue!(address.wrapping_add(8), after_call(registers));
                }
                if let Some(target) = call.target
                    && self.symbol_at(target) != Some("ets_delay_us")
                {
                    enqueue!(target, registers);
                } else if call.target.is_none() {
                    inventory
                        .unresolved_edges
                        .insert(address, format!("external-call {}", call.name));
                }
                continue;
            }
            let (instruction, width) = self.instruction(address)?;
            let next = address.wrapping_add(width);
            let get =
                |registers: &[Option<u32>; 32], register: Reg| registers[usize::from(register.0)];
            let set = |registers: &mut [Option<u32>; 32], register: Reg, value: Option<u32>| {
                if register != Reg::ZERO {
                    registers[usize::from(register.0)] = value;
                }
            };
            match instruction {
                Inst::Lui { uimm, dest } => {
                    set(&mut registers, dest, Some(uimm.as_u32()));
                    enqueue!(next, registers);
                }
                Inst::Auipc { uimm, dest } => {
                    set(
                        &mut registers,
                        dest,
                        Some(address.wrapping_add(uimm.as_u32())),
                    );
                    enqueue!(next, registers);
                }
                Inst::Addi { imm, dest, src1 } => {
                    let value = get(&registers, src1).map(|value| value.wrapping_add(imm.as_u32()));
                    set(&mut registers, dest, value);
                    enqueue!(next, registers);
                }
                Inst::Andi { imm, dest, src1 } => {
                    let immediate = andi_immediate(imm, width as u8);
                    let value = get(&registers, src1).map(|value| value & immediate);
                    set(&mut registers, dest, value);
                    enqueue!(next, registers);
                }
                Inst::Ori { imm, dest, src1 } => {
                    let value = get(&registers, src1).map(|value| value | imm.as_u32());
                    set(&mut registers, dest, value);
                    enqueue!(next, registers);
                }
                Inst::Xori { imm, dest, src1 } => {
                    let value = get(&registers, src1).map(|value| value ^ imm.as_u32());
                    set(&mut registers, dest, value);
                    enqueue!(next, registers);
                }
                Inst::Slli { imm, dest, src1 } => {
                    let value = get(&registers, src1).map(|value| value << (imm.as_u32() & 31));
                    set(&mut registers, dest, value);
                    enqueue!(next, registers);
                }
                Inst::Srli { imm, dest, src1 } => {
                    let value = get(&registers, src1).map(|value| value >> (imm.as_u32() & 31));
                    set(&mut registers, dest, value);
                    enqueue!(next, registers);
                }
                Inst::Srai { imm, dest, src1 } => {
                    let value = get(&registers, src1)
                        .map(|value| ((value as i32) >> (imm.as_u32() & 31)) as u32);
                    set(&mut registers, dest, value);
                    enqueue!(next, registers);
                }
                Inst::Slti { imm, dest, src1 } => {
                    let value =
                        get(&registers, src1).map(|value| u32::from((value as i32) < imm.as_i32()));
                    set(&mut registers, dest, value);
                    enqueue!(next, registers);
                }
                Inst::Sltiu { imm, dest, src1 } => {
                    let value = get(&registers, src1).map(|value| u32::from(value < imm.as_u32()));
                    set(&mut registers, dest, value);
                    enqueue!(next, registers);
                }
                Inst::Add { dest, src1, src2 }
                | Inst::Sub { dest, src1, src2 }
                | Inst::And { dest, src1, src2 }
                | Inst::Or { dest, src1, src2 }
                | Inst::Xor { dest, src1, src2 }
                | Inst::Sll { dest, src1, src2 }
                | Inst::Srl { dest, src1, src2 }
                | Inst::Sra { dest, src1, src2 }
                | Inst::Slt { dest, src1, src2 }
                | Inst::Sltu { dest, src1, src2 }
                | Inst::Mul { dest, src1, src2 }
                | Inst::Div { dest, src1, src2 }
                | Inst::Divu { dest, src1, src2 }
                | Inst::Rem { dest, src1, src2 }
                | Inst::Remu { dest, src1, src2 } => {
                    let left = get(&registers, src1);
                    let right = get(&registers, src2);
                    let value = left.zip(right).map(|(left, right)| match instruction {
                        Inst::Add { .. } => left.wrapping_add(right),
                        Inst::Sub { .. } => left.wrapping_sub(right),
                        Inst::And { .. } => left & right,
                        Inst::Or { .. } => left | right,
                        Inst::Xor { .. } => left ^ right,
                        Inst::Sll { .. } => left << (right & 31),
                        Inst::Srl { .. } => left >> (right & 31),
                        Inst::Sra { .. } => ((left as i32) >> (right & 31)) as u32,
                        Inst::Slt { .. } => u32::from((left as i32) < (right as i32)),
                        Inst::Sltu { .. } => u32::from(left < right),
                        Inst::Mul { .. } => left.wrapping_mul(right),
                        Inst::Div { .. } => {
                            let (left, right) = (left as i32, right as i32);
                            if right == 0 {
                                u32::MAX
                            } else if left == i32::MIN && right == -1 {
                                i32::MIN as u32
                            } else {
                                (left / right) as u32
                            }
                        }
                        Inst::Divu { .. } => left.checked_div(right).unwrap_or(u32::MAX),
                        Inst::Rem { .. } => {
                            let (left, right) = (left as i32, right as i32);
                            if right == 0 {
                                left as u32
                            } else if left == i32::MIN && right == -1 {
                                0
                            } else {
                                (left % right) as u32
                            }
                        }
                        Inst::Remu { .. } => {
                            if right == 0 {
                                left
                            } else {
                                left % right
                            }
                        }
                        _ => unreachable!(),
                    });
                    set(&mut registers, dest, value);
                    enqueue!(next, registers);
                }
                Inst::Mulh { dest, .. }
                | Inst::Mulhsu { dest, .. }
                | Inst::Mulhu { dest, .. }
                | Inst::Lb { dest, .. }
                | Inst::Lbu { dest, .. }
                | Inst::Lh { dest, .. }
                | Inst::Lhu { dest, .. } => {
                    set(&mut registers, dest, None);
                    enqueue!(next, registers);
                }
                Inst::Lw { offset, dest, base } => {
                    let value = get(&registers, base)
                        .map(|base| base.wrapping_add(offset.as_u32()))
                        .and_then(|address| {
                            stable_words
                                .get(&address)
                                .copied()
                                .or_else(|| self.immutable_word(address))
                        });
                    set(&mut registers, dest, value);
                    enqueue!(next, registers);
                }
                Inst::Sb { .. } | Inst::Sh { .. } | Inst::Sw { .. } | Inst::Fence { .. } => {
                    enqueue!(next, registers);
                }
                Inst::Beq { offset, src1, src2 }
                | Inst::Bne { offset, src1, src2 }
                | Inst::Blt { offset, src1, src2 }
                | Inst::Bge { offset, src1, src2 }
                | Inst::Bltu { offset, src1, src2 }
                | Inst::Bgeu { offset, src1, src2 } => {
                    inventory.branch_sites.insert(address);
                    let known = get(&registers, src1).zip(get(&registers, src2));
                    let taken = known.map(|(left, right)| match instruction {
                        Inst::Beq { .. } => left == right,
                        Inst::Bne { .. } => left != right,
                        Inst::Blt { .. } => (left as i32) < (right as i32),
                        Inst::Bge { .. } => (left as i32) >= (right as i32),
                        Inst::Bltu { .. } => left < right,
                        Inst::Bgeu { .. } => left >= right,
                        _ => unreachable!(),
                    });
                    for outcome in
                        taken.map_or([Some(false), Some(true)], |taken| [Some(taken), None])
                    {
                        let Some(outcome) = outcome else {
                            continue;
                        };
                        inventory.branch_outcomes.insert((address, outcome));
                        enqueue!(
                            if outcome {
                                address.wrapping_add(offset.as_u32())
                            } else {
                                next
                            },
                            registers,
                        );
                    }
                }
                Inst::Jal { offset, dest } => {
                    let target = address.wrapping_add(offset.as_u32());
                    if dest == Reg::ZERO {
                        enqueue!(target, registers);
                    } else {
                        set(&mut registers, dest, Some(next));
                        enqueue!(target, registers);
                        enqueue!(next, after_call(registers));
                    }
                }
                Inst::Jalr { offset, base, dest }
                    if dest == Reg::ZERO && base == Reg::RA && offset.as_u32() == 0 => {}
                Inst::Jalr { offset, base, dest } => {
                    if let Some(base) = get(&registers, base) {
                        let target = base.wrapping_add(offset.as_u32()) & !1;
                        if dest == Reg::ZERO {
                            enqueue!(target, registers);
                        } else {
                            set(&mut registers, dest, Some(next));
                            enqueue!(target, registers);
                            enqueue!(next, after_call(registers));
                        }
                    } else {
                        if dest != Reg::ZERO {
                            enqueue!(next, after_call(registers));
                        }
                        inventory
                            .unresolved_edges
                            .insert(address, instruction.to_string());
                    }
                }
                Inst::Ecall | Inst::Ebreak => {}
                _ => {
                    // An unsupported instruction may define any register.
                    // Forget all constants so later branches remain
                    // conservative, then continue through the fallthrough.
                    enqueue!(next, unknown_after_call());
                }
            }
        }
        Ok(inventory)
    }
}

/// Runtime support is executed concretely, but its implementation branches
/// are not source-level decisions of the function under verification.
/// Arithmetic results remain observable through the caller's later effects.
fn is_opaque_runtime_support(symbol: &str) -> bool {
    matches!(
        symbol,
        "ets_delay_us"
            | "__divdi3"
            | "__moddi3"
            | "__udivdi3"
            | "__umoddi3"
            | "__divmoddi4"
            | "__udivmoddi4"
    ) || symbol.contains("compiler_builtins")
}
