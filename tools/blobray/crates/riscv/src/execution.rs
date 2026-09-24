//! Concrete RV32IMAC execution. Instruction fetch, data, and events use explicit ports.
use super::*;
pub struct RiscvExecutor;
impl Executor for RiscvExecutor {
    fn identity(&self) -> &'static str {
        "rv32imac/execution-3/rv-asm-0.2.1"
    }
    fn execute(
        &self,
        entry: u32,
        stack: u32,
        arguments: &[Option<u32>; 8],
        memory: &mut dyn ExecutionMemory,
        control: &mut dyn RunControl,
    ) -> Result<(ExecutionStop, u64)> {
        let mut regs = [None; 32];
        regs[0] = Some(0);
        regs[1] = Some(u32::MAX - 1);
        regs[2] = Some(stack);
        for (i, value) in arguments.iter().enumerate() {
            regs[i + 10] = *value;
        }
        let mut pc = entry;
        let mut steps = 0;
        macro_rules! stop {
            ($reason:expr) => {
                return Ok((
                    ExecutionStop::Incomplete {
                        pc,
                        reason: $reason,
                    },
                    steps,
                ))
            };
        }
        macro_rules! reg {
            ($r:expr) => {
                match regs[$r as usize] {
                    Some(v) => v,
                    None => stop!(ExecutionGap::UnknownRegister { register: $r }),
                }
            };
        }
        loop {
            let mut position = control.position();
            position.entry = Some(u64::from(pc));
            control.set_position(position);
            control.checkpoint(1)?;
            if pc == u32::MAX - 1 {
                return Ok((
                    ExecutionStop::Returned {
                        low: regs[10],
                        high: regs[11],
                    },
                    steps,
                ));
            }
            if pc & 1 != 0 {
                stop!(ExecutionGap::Memory {
                    address: pc,
                    access: MemoryAccess::Fetch
                });
            }
            let Some(lo) = memory.read(pc, 2, MemoryAccess::Fetch, control)? else {
                stop!(ExecutionGap::Memory {
                    address: pc,
                    access: MemoryAccess::Fetch
                });
            };
            let mut bytes = [0; 4];
            bytes[..2].copy_from_slice(&(lo as u16).to_le_bytes());
            let width = if lo & 3 == 3 {
                let Some(hi) = pc
                    .checked_add(2)
                    .map(|p| memory.read(p, 2, MemoryAccess::Fetch, control))
                    .transpose()?
                    .flatten()
                else {
                    stop!(ExecutionGap::Memory {
                        address: pc,
                        access: MemoryAccess::Fetch
                    });
                };
                bytes[2..].copy_from_slice(&(hi as u16).to_le_bytes());
                4
            } else {
                2
            };
            let Some((inst, _)) = decode_instruction(&bytes[..width]) else {
                stop!(ExecutionGap::UnsupportedInstruction);
            };
            steps += 1;
            let mut next = pc.wrapping_add(width as u32);
            match inst {
                Inst::LrW { order, dest, addr } => {
                    let address = reg!(addr.0);
                    let Some(value) = memory.load_reserved(address, ordering(order), control)?
                    else {
                        stop!(ExecutionGap::Memory {
                            address,
                            access: MemoryAccess::Atomic
                        });
                    };
                    regs[dest.0 as usize] = Some(value);
                }
                Inst::ScW {
                    order,
                    dest,
                    addr,
                    src,
                } => {
                    let (address, value) = (reg!(addr.0), reg!(src.0));
                    let Some(stored) =
                        memory.store_conditional(address, value, ordering(order), control)?
                    else {
                        stop!(ExecutionGap::Memory {
                            address,
                            access: MemoryAccess::Atomic
                        });
                    };
                    regs[dest.0 as usize] = Some(u32::from(!stored));
                }
                Inst::AmoW {
                    order,
                    op,
                    dest,
                    addr,
                    src,
                } => {
                    let (address, value) = (reg!(addr.0), reg!(src.0));
                    let Some(old) = memory.modify_word(
                        address,
                        ordering(order),
                        &mut |old| atomic(op, old, value),
                        control,
                    )?
                    else {
                        stop!(ExecutionGap::Memory {
                            address,
                            access: MemoryAccess::Atomic
                        });
                    };
                    regs[dest.0 as usize] = Some(old);
                }
                Inst::Jal { dest, offset } => {
                    regs[dest.0 as usize] = Some(next);
                    next = pc.wrapping_add_signed(offset.as_i32());
                }
                Inst::Jalr { dest, base, offset } => {
                    let target = reg!(base.0).wrapping_add_signed(offset.as_i32()) & !1;
                    regs[dest.0 as usize] = Some(next);
                    next = target;
                }
                Inst::Beq { offset, .. }
                | Inst::Bne { offset, .. }
                | Inst::Blt { offset, .. }
                | Inst::Bge { offset, .. }
                | Inst::Bltu { offset, .. }
                | Inst::Bgeu { offset, .. } => {
                    let Some((test, Operand::Register(a), Operand::Register(b))) =
                        RiscvDecoder.branch(&bytes[..width])
                    else {
                        stop!(ExecutionGap::UnsupportedInstruction);
                    };
                    let (a, b) = (reg!(a), reg!(b));
                    let taken = match test {
                        BranchTest::Eq => a == b,
                        BranchTest::Ne => a != b,
                        BranchTest::Lt => (a as i32) < b as i32,
                        BranchTest::Ge => (a as i32) >= b as i32,
                        BranchTest::Ltu => a < b,
                        BranchTest::Geu => a >= b,
                    };
                    if taken {
                        next = pc.wrapping_add_signed(offset.as_i32());
                    }
                }
                Inst::Fence { fence } => {
                    if fence.fm != 0 {
                        stop!(ExecutionGap::UnsupportedInstruction);
                    }
                    let bits = |s: rv_asm::FenceSet| {
                        u8::from(s.device_input) * 8
                            + u8::from(s.device_output) * 4
                            + u8::from(s.memory_read) * 2
                            + u8::from(s.memory_write)
                    };
                    memory.event(
                        ExecutionEvent::Fence {
                            predecessor: bits(fence.pred),
                            successor: bits(fence.succ),
                        },
                        control,
                    )?;
                }
                _ => match RiscvDecoder.lift(&bytes[..width]) {
                    SemanticOp::Integer {
                        op,
                        dest,
                        left,
                        right,
                    } => {
                        let operand = |o: Operand| -> Option<u32> {
                            match o {
                                Operand::Immediate(v) => Some(v),
                                Operand::Register(r) => regs[r as usize],
                            }
                        };
                        let (Some(a), Some(b)) = (operand(left), operand(right)) else {
                            let r=match [left,right].into_iter().find(|o|matches!(o,Operand::Register(r) if regs[*r as usize].is_none())).unwrap(){Operand::Register(r)=>r,_=>unreachable!()};
                            stop!(ExecutionGap::UnknownRegister { register: r });
                        };
                        regs[dest as usize] = Some(integer(op, a, b));
                    }
                    SemanticOp::Upper {
                        dest,
                        value,
                        pc_relative,
                    } => {
                        regs[dest as usize] = Some(if pc_relative {
                            pc.wrapping_add(value)
                        } else {
                            value
                        })
                    }
                    SemanticOp::Memory {
                        kind,
                        base,
                        displacement,
                        width,
                        dest,
                        source,
                        signed,
                        ..
                    } => {
                        if !matches!(kind, MemoryKind::Load | MemoryKind::Store) {
                            stop!(ExecutionGap::UnsupportedInstruction);
                        }
                        let address = reg!(base).wrapping_add_signed(displacement);
                        if kind == MemoryKind::Load {
                            let Some(mut value) =
                                memory.read(address, width, MemoryAccess::Read, control)?
                            else {
                                stop!(ExecutionGap::Memory {
                                    address,
                                    access: MemoryAccess::Read
                                });
                            };
                            if signed {
                                value = match width {
                                    1 => value as i8 as i32 as u32,
                                    2 => value as i16 as i32 as u32,
                                    _ => value,
                                };
                            }
                            regs[dest.unwrap() as usize] = Some(value);
                        } else if !memory.write(address, width, reg!(source.unwrap()), control)? {
                            stop!(ExecutionGap::Memory {
                                address,
                                access: MemoryAccess::Write
                            });
                        }
                    }
                    _ => stop!(ExecutionGap::UnsupportedInstruction),
                },
            }
            regs[0] = Some(0);
            pc = next;
        }
    }
}
fn integer(op: IntegerOp, a: u32, b: u32) -> u32 {
    use IntegerOp::*;
    match op {
        Add => a.wrapping_add(b),
        Sub => a.wrapping_sub(b),
        And => a & b,
        Or => a | b,
        Xor => a ^ b,
        Shl => a.wrapping_shl(b & 31),
        Shr => a.wrapping_shr(b & 31),
        Sar => ((a as i32) >> (b & 31)) as u32,
        Lt => u32::from((a as i32) < b as i32),
        Ltu => u32::from(a < b),
        Mul => a.wrapping_mul(b),
        Mulh => (((a as i32 as i64) * (b as i32 as i64)) >> 32) as u32,
        Mulhsu => (((a as i32 as i64) * i64::from(b)) >> 32) as u32,
        Mulhu => ((u64::from(a) * u64::from(b)) >> 32) as u32,
        Div => {
            if b == 0 {
                u32::MAX
            } else {
                (a as i32).wrapping_div(b as i32) as u32
            }
        }
        Divu => a.checked_div(b).unwrap_or(u32::MAX),
        Rem => {
            if b == 0 {
                a
            } else {
                (a as i32).wrapping_rem(b as i32) as u32
            }
        }
        Remu => {
            if b == 0 {
                a
            } else {
                a % b
            }
        }
    }
}

fn ordering(order: rv_asm::AmoOrdering) -> ExecutionOrdering {
    let (acquire, release) = order.aq_rl();
    ExecutionOrdering { acquire, release }
}
fn atomic(op: rv_asm::AmoOp, old: u32, value: u32) -> u32 {
    use rv_asm::AmoOp;
    match op {
        AmoOp::Swap => value,
        AmoOp::Add => old.wrapping_add(value),
        AmoOp::Xor => old ^ value,
        AmoOp::And => old & value,
        AmoOp::Or => old | value,
        AmoOp::Min => (old as i32).min(value as i32) as u32,
        AmoOp::Max => (old as i32).max(value as i32) as u32,
        AmoOp::Minu => old.min(value),
        AmoOp::Maxu => old.max(value),
    }
}
