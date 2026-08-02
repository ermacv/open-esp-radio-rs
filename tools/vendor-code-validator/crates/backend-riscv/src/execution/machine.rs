//! Concrete fail-closed RV32 interpreter.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use rv_asm::{Inst, Reg};

use super::{
    ExecutableImage, ExecutionEvent, ExecutionResult, ExecutionTimelineEvent, IndirectCall,
    MemoryAlias, MemoryChange, MemoryOwner, MemoryOwnership, MemoryRange, MmioValue, OrderedCall,
    RETURN_SENTINEL, STACK_POINTER, Scenario, execution_stack_contains,
};
use crate::{MmioRegisterMap, Result, artifact::andi_immediate};

pub(super) struct Machine<'a> {
    pub(super) image: &'a ExecutableImage,
    pub(super) svd: &'a MmioRegisterMap,
    pub(super) registers: [u32; 32],
    pub(super) pc: u32,
    pub(super) overlay: BTreeMap<u32, u8>,
    pub(super) initial_overlay: BTreeMap<u32, u8>,
    pub(super) observed_memory: Vec<MemoryRange>,
    pub(super) memory_aliases: Vec<MemoryAlias>,
    pub(super) persistent_memory: Vec<MemoryRange>,
    pub(super) memory_ownership: Vec<MemoryOwnership>,
    /// Explicit stable read values supplied by the scenario. Bus writes do
    /// not update this map: storage/W1C/FIFO semantics belong to an explicit
    /// peripheral model, not to the generic transaction recorder.
    pub(super) mmio_read_seeds: BTreeMap<u32, u32>,
    pub(super) mmio_reads: BTreeMap<u32, VecDeque<u32>>,
    pub(super) events: Vec<ExecutionEvent>,
    pub(super) timeline: Vec<ExecutionTimelineEvent>,
    pub(super) branches: BTreeSet<(u32, bool)>,
    pub(super) ordered_branches: Vec<(u32, bool)>,
    pub(super) calls: BTreeSet<String>,
    pub(super) ordered_calls: Vec<OrderedCall>,
    pub(super) indirect_calls: BTreeSet<IndirectCall>,
    pub(super) steps: u64,
    pub(super) max_steps: u64,
}

impl<'a> Machine<'a> {
    pub(super) fn new(
        image: &'a ExecutableImage,
        svd: &'a MmioRegisterMap,
        start: u32,
        scenario: Scenario,
    ) -> Self {
        let mut registers = [0_u32; 32];
        registers[usize::from(Reg::RA.0)] = RETURN_SENTINEL;
        registers[usize::from(Reg::SP.0)] = STACK_POINTER;
        if let Some(global_pointer) = image.global_pointer {
            registers[usize::from(Reg::GP.0)] = global_pointer;
        }
        for (index, value) in scenario.arguments.into_iter().take(8).enumerate() {
            registers[10 + index] = value;
        }
        let initial_overlay = scenario.memory_initial;
        Self {
            image,
            svd,
            registers,
            pc: start,
            overlay: initial_overlay.clone(),
            initial_overlay,
            observed_memory: scenario.observed_memory,
            memory_aliases: scenario.memory_aliases,
            persistent_memory: scenario.persistent_memory,
            memory_ownership: scenario.memory_ownership,
            mmio_read_seeds: scenario.mmio_initial,
            mmio_reads: scenario.mmio_reads,
            events: Vec::new(),
            timeline: Vec::new(),
            branches: BTreeSet::new(),
            ordered_branches: Vec::new(),
            calls: BTreeSet::new(),
            ordered_calls: Vec::new(),
            indirect_calls: BTreeSet::new(),
            steps: 0,
            max_steps: if scenario.max_steps == 0 {
                100_000
            } else {
                scenario.max_steps
            },
        }
    }

    pub(super) fn register(&self, register: Reg) -> u32 {
        self.registers[usize::from(register.0)]
    }

    pub(super) fn set_register(&mut self, register: Reg, value: u32) {
        if register != Reg::ZERO {
            self.registers[usize::from(register.0)] = value;
        }
    }

    pub(super) fn normal_byte(&self, address: u32) -> Result<u8> {
        if let Some(value) = self.overlay.get(&address).copied() {
            return Ok(value);
        }
        if self.memory_ownership.iter().any(|ownership| {
            ownership.range.contains(address) && ownership.owner.may_change_outside_cpu()
        }) {
            return Err(format!(
                "read from externally mutable RAM at {address:#010x} without a call-entry seed"
            )
            .into());
        }
        self.image
            .byte(address)
            .ok_or_else(|| format!("read from poison/unmapped memory at {address:#010x}").into())
    }

    pub(super) fn read(&mut self, address: u32, width: u8) -> Result<u32> {
        if self.svd.contains_mmio(address) {
            let sequenced = self
                .mmio_reads
                .get_mut(&address)
                .and_then(VecDeque::pop_front);
            let value = match sequenced {
                Some(value) => value & MmioValue::mask(width),
                None => self
                    .mmio_read_seeds
                    .get(&address)
                    .copied()
                    .map(|value| value & MmioValue::mask(width))
                    .ok_or_else(|| {
                        format!("MMIO read at {address:#010x} has no explicit seed or response")
                    })?,
            };
            self.record_event(ExecutionEvent::Read {
                width,
                address,
                register: self.svd.register_name(address),
                value,
            });
            return Ok(value);
        }
        let bytes = usize::from(width / 8);
        let mut value = 0_u32;
        for offset in 0..bytes {
            value |=
                u32::from(self.normal_byte(address.wrapping_add(offset as u32))?) << (offset * 8);
        }
        self.timeline.push(ExecutionTimelineEvent::RamRead {
            width,
            address,
            value,
        });
        Ok(value)
    }

    pub(super) fn normal_address_is_valid(&self, address: u32) -> bool {
        self.image.contains_memory(address)
            || execution_stack_contains(address)
            || self.initial_overlay.contains_key(&address)
            || self
                .observed_memory
                .iter()
                .any(|range| range.contains(address))
            || self.memory_aliases.iter().any(|alias| {
                address
                    .checked_sub(alias.start)
                    .is_some_and(|offset| offset < alias.length)
            })
    }

    pub(super) fn write(&mut self, address: u32, width: u8, value: u32) -> Result<()> {
        let bytes = usize::from(width / 8);
        if self.svd.contains_mmio(address) {
            self.record_event(ExecutionEvent::Write {
                width,
                address,
                register: self.svd.register_name(address),
                value: value & MmioValue::mask(width),
            });
            return Ok(());
        }
        for offset in 0..bytes {
            let byte_address = address.wrapping_add(offset as u32);
            if self.memory_ownership.iter().any(|ownership| {
                ownership.range.contains(byte_address) && ownership.owner == MemoryOwner::Immutable
            }) {
                return Err(format!(
                    "write to ownership-declared immutable RAM at {byte_address:#010x}"
                )
                .into());
            }
            if self.image.contains_memory(byte_address)
                && !self.image.contains_writable_memory(byte_address)
            {
                return Err(
                    format!("write to read-only ELF memory at {byte_address:#010x}").into(),
                );
            }
            if !self.normal_address_is_valid(byte_address) {
                return Err(format!("write to undeclared memory at {byte_address:#010x}").into());
            }
            self.overlay
                .insert(byte_address, (value >> (offset * 8)) as u8);
        }
        self.timeline.push(ExecutionTimelineEvent::RamWrite {
            width,
            address,
            value: value & MmioValue::mask(width),
        });
        Ok(())
    }

    pub(super) fn branch(&mut self, taken: bool, offset: i32, width: u32) {
        self.branches.insert((self.pc, taken));
        self.ordered_branches.push((self.pc, taken));
        self.timeline.push(ExecutionTimelineEvent::Branch {
            site: self.pc,
            taken,
        });
        self.pc = if taken {
            self.pc.wrapping_add(offset as u32)
        } else {
            self.pc.wrapping_add(width)
        };
    }

    pub(super) fn record_call(&mut self, site: u32, symbol: String) {
        let arguments =
            core::array::from_fn(|index| self.registers[usize::from(Reg::A0.0) + index]);
        self.calls.insert(symbol.clone());
        let call = OrderedCall {
            site,
            symbol,
            arguments,
        };
        self.ordered_calls.push(call.clone());
        self.timeline.push(ExecutionTimelineEvent::Call(call));
    }

    pub(super) fn record_event(&mut self, event: ExecutionEvent) {
        self.events.push(event.clone());
        self.timeline
            .push(ExecutionTimelineEvent::Observable(event));
    }

    pub(super) fn memory_changes(&self) -> Result<Vec<MemoryChange>> {
        let mut observed_addresses: BTreeMap<u32, u32> = self
            .observed_memory
            .iter()
            .flat_map(|range| {
                (0..range.length).map(move |offset| {
                    let address = range.start.wrapping_add(offset);
                    (address, address)
                })
            })
            .collect();
        for alias in &self.memory_aliases {
            for offset in 0..alias.length {
                observed_addresses.insert(
                    alias.comparison_start.wrapping_add(offset),
                    alias.start.wrapping_add(offset),
                );
            }
        }
        let mut changes = Vec::new();
        for (comparison_address, address) in observed_addresses {
            let before = self
                .initial_overlay
                .get(&address)
                .copied()
                .or_else(|| self.image.byte(address))
                .ok_or_else(|| {
                    format!("observed memory at {address:#010x} has no explicit initial value")
                })?;
            let after = self
                .overlay
                .get(&address)
                .copied()
                .or_else(|| self.image.byte(address))
                .ok_or_else(|| format!("observed memory at {address:#010x} remains poison"))?;
            if before != after {
                changes.push(MemoryChange {
                    address: comparison_address,
                    before,
                    after,
                });
            }
        }
        Ok(changes)
    }

    pub(super) fn persistent_memory(&self) -> BTreeMap<u32, u8> {
        self.overlay
            .iter()
            .filter_map(|(address, value)| {
                let explicitly_persistent = self
                    .persistent_memory
                    .iter()
                    .any(|range| range.contains(*address));
                ((!execution_stack_contains(*address)
                    && self.image.contains_writable_memory(*address))
                    || explicitly_persistent)
                    .then_some((*address, *value))
            })
            .collect()
    }

    pub(super) fn step(&mut self) -> Result<bool> {
        if self.steps == self.max_steps {
            let context = self
                .ordered_calls
                .iter()
                .rev()
                .take(6)
                .map(|call| {
                    format!(
                        "{}({:#x},{:#x},{:#x},{:#x})",
                        call.symbol,
                        call.arguments[0],
                        call.arguments[1],
                        call.arguments[2],
                        call.arguments[3]
                    )
                })
                .collect::<Vec<_>>()
                .join(" <- ");
            return Err(format!(
                "execution exceeded {} steps at pc={:#010x}; recent calls: {}",
                self.max_steps,
                self.pc,
                if context.is_empty() {
                    "<entry>"
                } else {
                    &context
                }
            )
            .into());
        }
        self.steps += 1;

        if let Some(symbol) = self.image.symbol_at(self.pc)
            && symbol == "ets_delay_us"
        {
            self.record_event(ExecutionEvent::DelayMicros(self.register(Reg::A0)));
            let return_address = self.register(Reg::RA);
            if return_address == RETURN_SENTINEL {
                return Ok(false);
            }
            self.pc = return_address;
            return Ok(true);
        }
        if let Some(call) = self.image.relocated_call_at(self.pc).cloned() {
            let link = self.image.relocated_call_link_register(self.pc)?;
            let continuation = self.pc.wrapping_add(8);
            if link != Reg::ZERO {
                self.set_register(link, continuation);
            }
            if let Some(target) = call.target {
                self.record_call(self.pc, call.name);
                self.pc = target;
            } else {
                return Err(format!(
                    "execution reached unresolved external call {} at {:#010x}",
                    call.name, self.pc
                )
                .into());
            }
            return Ok(true);
        }

        let (instruction, width) = self.image.instruction(self.pc)?;
        let next = self.pc.wrapping_add(width);
        match instruction {
            Inst::Lui { uimm, dest } => self.set_register(dest, uimm.as_u32()),
            Inst::Auipc { uimm, dest } => {
                self.set_register(dest, self.pc.wrapping_add(uimm.as_u32()));
            }
            Inst::Addi { imm, dest, src1 } => {
                self.set_register(dest, self.register(src1).wrapping_add(imm.as_u32()));
            }
            Inst::Andi { imm, dest, src1 } => {
                self.set_register(dest, self.register(src1) & andi_immediate(imm, width as u8));
            }
            Inst::Ori { imm, dest, src1 } => {
                self.set_register(dest, self.register(src1) | imm.as_u32());
            }
            Inst::Xori { imm, dest, src1 } => {
                self.set_register(dest, self.register(src1) ^ imm.as_u32());
            }
            Inst::Slli { imm, dest, src1 } => {
                self.set_register(dest, self.register(src1) << (imm.as_u32() & 31));
            }
            Inst::Srli { imm, dest, src1 } => {
                self.set_register(dest, self.register(src1) >> (imm.as_u32() & 31));
            }
            Inst::Srai { imm, dest, src1 } => self.set_register(
                dest,
                ((self.register(src1) as i32) >> (imm.as_u32() & 31)) as u32,
            ),
            Inst::Slti { imm, dest, src1 } => {
                self.set_register(dest, u32::from((self.register(src1) as i32) < imm.as_i32()));
            }
            Inst::Sltiu { imm, dest, src1 } => {
                self.set_register(dest, u32::from(self.register(src1) < imm.as_u32()));
            }
            Inst::Add { dest, src1, src2 } => {
                self.set_register(dest, self.register(src1).wrapping_add(self.register(src2)))
            }
            Inst::Sub { dest, src1, src2 } => {
                self.set_register(dest, self.register(src1).wrapping_sub(self.register(src2)))
            }
            Inst::And { dest, src1, src2 } => {
                self.set_register(dest, self.register(src1) & self.register(src2));
            }
            Inst::Or { dest, src1, src2 } => {
                self.set_register(dest, self.register(src1) | self.register(src2));
            }
            Inst::Xor { dest, src1, src2 } => {
                self.set_register(dest, self.register(src1) ^ self.register(src2));
            }
            Inst::Sll { dest, src1, src2 } => {
                self.set_register(dest, self.register(src1) << (self.register(src2) & 31));
            }
            Inst::Srl { dest, src1, src2 } => {
                self.set_register(dest, self.register(src1) >> (self.register(src2) & 31));
            }
            Inst::Sra { dest, src1, src2 } => self.set_register(
                dest,
                ((self.register(src1) as i32) >> (self.register(src2) & 31)) as u32,
            ),
            Inst::Slt { dest, src1, src2 } => self.set_register(
                dest,
                u32::from((self.register(src1) as i32) < (self.register(src2) as i32)),
            ),
            Inst::Sltu { dest, src1, src2 } => {
                self.set_register(dest, u32::from(self.register(src1) < self.register(src2)));
            }
            Inst::Mul { dest, src1, src2 } => {
                self.set_register(dest, self.register(src1).wrapping_mul(self.register(src2)))
            }
            Inst::Mulh { dest, src1, src2 } => self.set_register(
                dest,
                (((self.register(src1) as i32 as i64) * (self.register(src2) as i32 as i64)) >> 32)
                    as u32,
            ),
            Inst::Mulhsu { dest, src1, src2 } => self.set_register(
                dest,
                (((self.register(src1) as i32 as i64) * i64::from(self.register(src2))) >> 32)
                    as u32,
            ),
            Inst::Mulhu { dest, src1, src2 } => self.set_register(
                dest,
                ((u64::from(self.register(src1)) * u64::from(self.register(src2))) >> 32) as u32,
            ),
            Inst::Div { dest, src1, src2 } => {
                let left = self.register(src1) as i32;
                let right = self.register(src2) as i32;
                self.set_register(
                    dest,
                    if right == 0 {
                        u32::MAX
                    } else if left == i32::MIN && right == -1 {
                        i32::MIN as u32
                    } else {
                        (left / right) as u32
                    },
                );
            }
            Inst::Divu { dest, src1, src2 } => {
                let right = self.register(src2);
                self.set_register(
                    dest,
                    self.register(src1).checked_div(right).unwrap_or(u32::MAX),
                );
            }
            Inst::Rem { dest, src1, src2 } => {
                let left = self.register(src1) as i32;
                let right = self.register(src2) as i32;
                self.set_register(
                    dest,
                    if right == 0 {
                        left as u32
                    } else if left == i32::MIN && right == -1 {
                        0
                    } else {
                        (left % right) as u32
                    },
                );
            }
            Inst::Remu { dest, src1, src2 } => {
                let right = self.register(src2);
                self.set_register(
                    dest,
                    if right == 0 {
                        self.register(src1)
                    } else {
                        self.register(src1) % right
                    },
                );
            }
            Inst::Lb { offset, dest, base } => {
                let address = self.register(base).wrapping_add(offset.as_u32());
                let value = (self.read(address, 8)? as u8 as i8 as i32) as u32;
                self.set_register(dest, value);
            }
            Inst::Lbu { offset, dest, base } => {
                let address = self.register(base).wrapping_add(offset.as_u32());
                let value = self.read(address, 8)? & 0xff;
                self.set_register(dest, value);
            }
            Inst::Lh { offset, dest, base } => {
                let address = self.register(base).wrapping_add(offset.as_u32());
                let value = (self.read(address, 16)? as u16 as i16 as i32) as u32;
                self.set_register(dest, value);
            }
            Inst::Lhu { offset, dest, base } => {
                let address = self.register(base).wrapping_add(offset.as_u32());
                let value = self.read(address, 16)? & 0xffff;
                self.set_register(dest, value);
            }
            Inst::Lw { offset, dest, base } => {
                let address = self.register(base).wrapping_add(offset.as_u32());
                let value = self.read(address, 32)?;
                self.set_register(dest, value);
            }
            Inst::Sb { offset, src, base } => {
                self.write(
                    self.register(base).wrapping_add(offset.as_u32()),
                    8,
                    self.register(src),
                )?;
            }
            Inst::Sh { offset, src, base } => {
                self.write(
                    self.register(base).wrapping_add(offset.as_u32()),
                    16,
                    self.register(src),
                )?;
            }
            Inst::Sw { offset, src, base } => {
                self.write(
                    self.register(base).wrapping_add(offset.as_u32()),
                    32,
                    self.register(src),
                )?;
            }
            Inst::Beq { offset, src1, src2 } => {
                self.branch(
                    self.register(src1) == self.register(src2),
                    offset.as_i32(),
                    width,
                );
                return Ok(true);
            }
            Inst::Bne { offset, src1, src2 } => {
                self.branch(
                    self.register(src1) != self.register(src2),
                    offset.as_i32(),
                    width,
                );
                return Ok(true);
            }
            Inst::Blt { offset, src1, src2 } => {
                self.branch(
                    (self.register(src1) as i32) < (self.register(src2) as i32),
                    offset.as_i32(),
                    width,
                );
                return Ok(true);
            }
            Inst::Bge { offset, src1, src2 } => {
                self.branch(
                    (self.register(src1) as i32) >= (self.register(src2) as i32),
                    offset.as_i32(),
                    width,
                );
                return Ok(true);
            }
            Inst::Bltu { offset, src1, src2 } => {
                self.branch(
                    self.register(src1) < self.register(src2),
                    offset.as_i32(),
                    width,
                );
                return Ok(true);
            }
            Inst::Bgeu { offset, src1, src2 } => {
                self.branch(
                    self.register(src1) >= self.register(src2),
                    offset.as_i32(),
                    width,
                );
                return Ok(true);
            }
            Inst::Jal { offset, dest } => {
                let target = self.pc.wrapping_add(offset.as_u32());
                self.set_register(dest, next);
                let leaves_call_trampoline =
                    self.image.call_trampoline_addresses.contains(&self.pc);
                if !leaves_call_trampoline && let Some(symbol) = self.image.symbol_at(target) {
                    self.record_call(self.pc, symbol.to_owned());
                }
                self.pc = target;
                return Ok(true);
            }
            Inst::Jalr { offset, base, dest } => {
                let target = self.register(base).wrapping_add(offset.as_u32()) & !1;
                self.set_register(dest, next);
                if target == RETURN_SENTINEL {
                    return Ok(false);
                }
                if let Some(symbol) = self.image.symbol_at(target) {
                    let is_return = dest == Reg::ZERO && base == Reg::RA && offset.as_u32() == 0;
                    if !is_return {
                        self.record_call(self.pc, symbol.to_owned());
                        self.indirect_calls.insert(IndirectCall {
                            site: self.pc,
                            symbol: symbol.to_owned(),
                            arguments: core::array::from_fn(|index| {
                                self.registers[usize::from(Reg::A0.0) + index]
                            }),
                        });
                    }
                }
                self.pc = target;
                return Ok(true);
            }
            Inst::Fence { fence } => {
                let encode = |set: rv_asm::FenceSet| {
                    u8::from(set.device_input) << 3
                        | u8::from(set.device_output) << 2
                        | u8::from(set.memory_read) << 1
                        | u8::from(set.memory_write)
                };
                self.record_event(ExecutionEvent::Fence {
                    fm: fence.fm,
                    predecessor: encode(fence.pred),
                    successor: encode(fence.succ),
                });
            }
            _ => {
                return Err(
                    format!("unsupported instruction at {:#x}: {instruction}", self.pc).into(),
                );
            }
        }
        self.pc = next;
        self.registers[0] = 0;
        Ok(true)
    }
}

pub fn execute(
    image: &ExecutableImage,
    svd: &MmioRegisterMap,
    symbol: &str,
    scenario: Scenario,
) -> Result<ExecutionResult> {
    if scenario.arguments.len() > 8 {
        return Err(format!(
            "{} arguments were provided, but stack arguments are not implemented; maximum is 8",
            scenario.arguments.len()
        )
        .into());
    }
    let start = image
        .symbol_address(symbol)
        .ok_or_else(|| format!("execution symbol {symbol} was not found"))?;
    let mut machine = Machine::new(image, svd, start, scenario);
    while machine.step()? {}
    let unconsumed: Vec<_> = machine
        .mmio_reads
        .iter()
        .filter_map(|(address, values)| (!values.is_empty()).then_some((*address, values.len())))
        .collect();
    if !unconsumed.is_empty() {
        return Err(format!("unconsumed MMIO read responses: {unconsumed:?}").into());
    }
    let return_value = machine.register(Reg::A0);
    let memory_changes = machine.memory_changes()?;
    let persistent_memory = machine.persistent_memory();
    let initial_memory = machine.initial_overlay.clone();
    Ok(ExecutionResult {
        events: machine.events,
        timeline: machine.timeline,
        return_value,
        steps: machine.steps,
        branches: machine.branches,
        ordered_branches: machine.ordered_branches,
        calls: machine.calls,
        ordered_calls: machine.ordered_calls,
        indirect_calls: machine.indirect_calls,
        memory_changes,
        initial_memory,
        persistent_memory,
    })
}
