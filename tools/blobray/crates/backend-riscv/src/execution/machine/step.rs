//! One-step RV32 instruction and call dispatch.

use rv_asm::{AmoOp, AmoOrdering, Inst, Reg};

use super::super::{
    AtomicOperation, AtomicOrdering, ExecutionEvent, ExecutionTimelineEvent, IndirectCall,
    RETURN_SENTINEL,
};
use super::Machine;
use crate::{Result, artifact::andi_immediate};

pub(in crate::execution) fn atomic_word_result(operation: AmoOp, current: u32, source: u32) -> u32 {
    match operation {
        AmoOp::Swap => source,
        AmoOp::Add => current.wrapping_add(source),
        AmoOp::Xor => current ^ source,
        AmoOp::And => current & source,
        AmoOp::Or => current | source,
        AmoOp::Min => {
            if (current as i32) < (source as i32) {
                current
            } else {
                source
            }
        }
        AmoOp::Max => {
            if (current as i32) > (source as i32) {
                current
            } else {
                source
            }
        }
        AmoOp::Minu => current.min(source),
        AmoOp::Maxu => current.max(source),
    }
}

const fn atomic_ordering(ordering: AmoOrdering) -> AtomicOrdering {
    match ordering {
        AmoOrdering::Relaxed => AtomicOrdering::Relaxed,
        AmoOrdering::Acquire => AtomicOrdering::Acquire,
        AmoOrdering::Release => AtomicOrdering::Release,
        AmoOrdering::SeqCst => AtomicOrdering::AcquireRelease,
    }
}

const fn atomic_operation(operation: AmoOp) -> AtomicOperation {
    match operation {
        AmoOp::Swap => AtomicOperation::Swap,
        AmoOp::Add => AtomicOperation::Add,
        AmoOp::Xor => AtomicOperation::Xor,
        AmoOp::And => AtomicOperation::And,
        AmoOp::Or => AtomicOperation::Or,
        AmoOp::Min => AtomicOperation::Min,
        AmoOp::Max => AtomicOperation::Max,
        AmoOp::Minu => AtomicOperation::MinUnsigned,
        AmoOp::Maxu => AtomicOperation::MaxUnsigned,
    }
}

impl Machine<'_> {
    fn finish_external_leaf(&mut self) -> bool {
        let return_address = self.register(Reg::RA);
        if return_address == RETURN_SENTINEL {
            false
        } else {
            self.pc = return_address;
            true
        }
    }

    fn dispatch_builtin_memory_call(&mut self, symbol: &str) -> Result<Option<bool>> {
        const MAX_BUILTIN_MEMORY_BYTES: u32 = 1 << 20;

        let destination = self.register(Reg::A0);
        let source_or_value = self.register(Reg::A1);
        let length = self.register(Reg::A2);
        if !matches!(symbol, "memcpy" | "memmove" | "memset") {
            return Ok(None);
        }
        if length > MAX_BUILTIN_MEMORY_BYTES {
            return Err(format!(
                "builtin {symbol} length {length:#x} exceeds the executor limit {MAX_BUILTIN_MEMORY_BYTES:#x}"
            )
            .into());
        }

        match symbol {
            "memcpy" | "memmove" => {
                // Snapshot first so memmove overlap has its specified behavior.
                // The same implementation is valid for memcpy's non-overlap
                // contract and keeps the executor independent of host memory.
                let bytes = (0..length)
                    .map(|offset| self.normal_byte(source_or_value.wrapping_add(offset)))
                    .collect::<Result<Vec<_>>>()?;
                for (offset, byte) in bytes.into_iter().enumerate() {
                    self.write(destination.wrapping_add(offset as u32), 8, u32::from(byte))?;
                }
            }
            "memset" => {
                for offset in 0..length {
                    self.write(destination.wrapping_add(offset), 8, source_or_value & 0xff)?;
                }
            }
            _ => unreachable!(),
        }
        self.set_register(Reg::A0, destination);
        Ok(Some(self.finish_external_leaf()))
    }

    fn dispatch_builtin_arithmetic_call(&mut self, symbol: &str) -> Result<Option<bool>> {
        if !matches!(symbol, "__divdi3" | "__moddi3" | "__udivdi3" | "__umoddi3") {
            return Ok(None);
        }

        // RV32 passes a 64-bit dividend in a1:a0 and divisor in a3:a2.
        // Compiler runtime helpers return the 64-bit result in a1:a0.
        let dividend =
            u64::from(self.register(Reg::A0)) | (u64::from(self.register(Reg::A1)) << 32);
        let divisor = u64::from(self.register(Reg::A2)) | (u64::from(self.register(Reg::A3)) << 32);
        if divisor == 0 {
            return Err(format!(
                "builtin {symbol} cannot execute with a zero divisor; the result is undefined by the helper ABI"
            )
            .into());
        }

        let result = match symbol {
            "__divdi3" => (dividend as i64).wrapping_div(divisor as i64) as u64,
            "__moddi3" => (dividend as i64).wrapping_rem(divisor as i64) as u64,
            "__udivdi3" => dividend / divisor,
            "__umoddi3" => dividend % divisor,
            _ => unreachable!(),
        };
        self.set_register(Reg::A0, result as u32);
        self.set_register(Reg::A1, (result >> 32) as u32);
        Ok(Some(self.finish_external_leaf()))
    }

    pub(in crate::execution) fn step(&mut self) -> Result<bool> {
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
        self.executed_pcs.insert(self.pc);

        if let Some(symbol) = self.call_symbol_at(self.pc).map(str::to_owned) {
            if let Some(running) = self.dispatch_builtin_memory_call(&symbol)? {
                return Ok(running);
            }
            if let Some(running) = self.dispatch_builtin_arithmetic_call(&symbol)? {
                return Ok(running);
            }
            if symbol == "ets_delay_us" {
                self.record_event(ExecutionEvent::DelayMicros(self.register(Reg::A0)));
                return Ok(self.finish_external_leaf());
            }

            if self.image.call_trampoline_addresses.contains(&self.pc) {
                if self.fifo_bindings.contains_key(&symbol) {
                    self.apply_fifo_service_call(&symbol, self.pc)?;
                    let return_address = self.register(Reg::RA);
                    if return_address == RETURN_SENTINEL {
                        return Ok(false);
                    }
                    self.pc = return_address;
                    return Ok(true);
                }
                if let Some(response) = self.modeled_call_response(&symbol, self.pc)? {
                    self.apply_modeled_call_response(&symbol, self.pc, response)?;
                    let return_address = self.register(Reg::RA);
                    if return_address == RETURN_SENTINEL {
                        return Ok(false);
                    }
                    self.pc = return_address;
                    return Ok(true);
                }
                if let Some(target) = self.image.symbol_address(&symbol)
                    && target != self.pc
                {
                    self.pc = target;
                    return Ok(true);
                }
                return Err(format!(
                    "unresolved call trampoline {symbol} at {:#010x}; provide the target image or an explicit call model",
                    self.pc
                )
                .into());
            }
        }
        if let Some(call) = self.image.relocated_call_at(self.pc).cloned() {
            let link = self.image.relocated_call_link_register(self.pc)?;
            let continuation = self.pc.wrapping_add(8);
            let name = call.name.clone();
            if self.fifo_bindings.contains_key(&name) {
                self.record_call(self.pc, name.clone());
                self.apply_fifo_service_call(&name, self.pc)?;
                if link == Reg::ZERO {
                    let return_address = self.register(Reg::RA);
                    if return_address == RETURN_SENTINEL {
                        return Ok(false);
                    }
                    self.pc = return_address;
                } else {
                    self.set_register(link, continuation);
                    self.pc = continuation;
                }
                return Ok(true);
            }
            if let Some(response) = self.modeled_call_response(&name, self.pc)? {
                self.record_call(self.pc, name.clone());
                self.apply_modeled_call_response(&name, self.pc, response)?;
                if link == Reg::ZERO {
                    let return_address = self.register(Reg::RA);
                    if return_address == RETURN_SENTINEL {
                        return Ok(false);
                    }
                    self.pc = return_address;
                } else {
                    self.set_register(link, continuation);
                    self.pc = continuation;
                }
                return Ok(true);
            }
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
            Inst::LrW { order, dest, addr } => {
                let address = self.register(addr);
                self.validate_atomic_word_address(address)?;
                let value = self.read(address, 32)?;
                self.word_reservation = Some(address);
                self.set_register(dest, value);
                self.timeline.push(ExecutionTimelineEvent::Atomic {
                    operation: AtomicOperation::LoadReserved,
                    ordering: atomic_ordering(order),
                    address,
                    succeeded: None,
                });
            }
            Inst::ScW {
                order,
                dest,
                addr,
                src,
            } => {
                let address = self.register(addr);
                self.validate_atomic_word_address(address)?;
                let succeeds = self.word_reservation == Some(address);
                self.word_reservation = None;
                if succeeds {
                    self.write(address, 32, self.register(src))?;
                }
                self.set_register(dest, u32::from(!succeeds));
                self.timeline.push(ExecutionTimelineEvent::Atomic {
                    operation: AtomicOperation::StoreConditional,
                    ordering: atomic_ordering(order),
                    address,
                    succeeded: Some(succeeds),
                });
            }
            Inst::AmoW {
                order,
                op,
                dest,
                addr,
                src,
            } => {
                let address = self.register(addr);
                self.validate_atomic_word_address(address)?;
                let source = self.register(src);
                let previous = self.read(address, 32)?;
                self.write(address, 32, atomic_word_result(op, previous, source))?;
                self.set_register(dest, previous);
                self.timeline.push(ExecutionTimelineEvent::Atomic {
                    operation: atomic_operation(op),
                    ordering: atomic_ordering(order),
                    address,
                    succeeded: None,
                });
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
                if let Some(symbol) = self.call_symbol_at(target).map(str::to_owned)
                    && self.fifo_bindings.contains_key(&symbol)
                {
                    self.record_call(self.pc, symbol.clone());
                    self.apply_fifo_service_call(&symbol, self.pc)?;
                    if dest == Reg::ZERO {
                        let return_address = self.register(Reg::RA);
                        if return_address == RETURN_SENTINEL {
                            return Ok(false);
                        }
                        self.pc = return_address;
                    } else {
                        self.set_register(dest, next);
                        self.pc = next;
                    }
                    return Ok(true);
                }
                if let Some(symbol) = self.image.symbol_at(target)
                    && let Some(response) = self.modeled_call_response(symbol, self.pc)?
                {
                    self.record_call(self.pc, symbol.to_owned());
                    self.apply_modeled_call_response(symbol, self.pc, response)?;
                    if dest == Reg::ZERO {
                        let return_address = self.register(Reg::RA);
                        if return_address == RETURN_SENTINEL {
                            return Ok(false);
                        }
                        self.pc = return_address;
                    } else {
                        self.set_register(dest, next);
                        self.pc = next;
                    }
                    return Ok(true);
                }
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
                if let Some(symbol) = self.call_symbol_at(target).map(str::to_owned)
                    && self.fifo_bindings.contains_key(&symbol)
                {
                    self.record_call(self.pc, symbol.clone());
                    self.indirect_calls.insert(IndirectCall {
                        site: self.pc,
                        symbol: symbol.clone(),
                        arguments: core::array::from_fn(|index| {
                            self.registers[usize::from(Reg::A0.0) + index]
                        }),
                    });
                    self.record_indirect_table_call(self.pc, target, &symbol);
                    self.apply_fifo_service_call(&symbol, self.pc)?;
                    if dest == Reg::ZERO {
                        let return_address = self.register(Reg::RA);
                        if return_address == RETURN_SENTINEL {
                            return Ok(false);
                        }
                        self.pc = return_address;
                    } else {
                        self.set_register(dest, next);
                        self.pc = next;
                    }
                    return Ok(true);
                }
                if let Some(symbol) = self.call_symbol_at(target).map(str::to_owned)
                    && let Some(response) = self.modeled_call_response(&symbol, self.pc)?
                {
                    self.record_call(self.pc, symbol.clone());
                    self.indirect_calls.insert(IndirectCall {
                        site: self.pc,
                        symbol: symbol.clone(),
                        arguments: core::array::from_fn(|index| {
                            self.registers[usize::from(Reg::A0.0) + index]
                        }),
                    });
                    self.record_indirect_table_call(self.pc, target, &symbol);
                    self.apply_modeled_call_response(&symbol, self.pc, response)?;
                    if dest == Reg::ZERO {
                        let return_address = self.register(Reg::RA);
                        if return_address == RETURN_SENTINEL {
                            return Ok(false);
                        }
                        self.pc = return_address;
                    } else {
                        self.set_register(dest, next);
                        self.pc = next;
                    }
                    return Ok(true);
                }
                self.set_register(dest, next);
                if target == RETURN_SENTINEL {
                    return Ok(false);
                }
                if let Some(symbol) = self.call_symbol_at(target).map(str::to_owned) {
                    let is_return = dest == Reg::ZERO && base == Reg::RA && offset.as_u32() == 0;
                    if !is_return {
                        self.record_call(self.pc, symbol.clone());
                        self.indirect_calls.insert(IndirectCall {
                            site: self.pc,
                            symbol: symbol.clone(),
                            arguments: core::array::from_fn(|index| {
                                self.registers[usize::from(Reg::A0.0) + index]
                            }),
                        });
                        self.record_indirect_table_call(self.pc, target, &symbol);
                    }
                }
                if self.is_modeled_call_target(target) {
                    let symbol = self
                        .call_symbol_at(target)
                        .unwrap_or("<unknown-modeled-target>");
                    return Err(format!(
                        "modeled external call target {symbol} at {target:#010x} was reached without an executable model; FIFO bindings={:?}, call responses={:?}",
                        self.fifo_bindings.keys().collect::<Vec<_>>(),
                        self.call_responses.keys().collect::<Vec<_>>()
                    )
                    .into());
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

    fn validate_atomic_word_address(&self, address: u32) -> Result<()> {
        if address & 3 != 0 {
            return Err(format!(
                "misaligned atomic word access at {address:#010x} from pc={:#010x}",
                self.pc
            )
            .into());
        }
        if self.svd.intersects_mmio(address, 32) {
            return Err(format!(
                "atomic word access to MMIO at {address:#010x} requires an explicit peripheral model"
            )
            .into());
        }
        Ok(())
    }
}
