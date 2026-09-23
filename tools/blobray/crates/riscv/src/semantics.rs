//! RV32 instruction lifting. Compressed forms are expanded by the pinned decoder.
use super::*;
impl FunctionSemantics for RiscvDecoder {
    fn branch(&self, bytes: &[u8]) -> Option<(BranchTest, Operand, Operand)> {
        let (inst, _) = decode_instruction(bytes)?;
        let (test, a, b) = match inst {
            Inst::Beq { src1, src2, .. } => (BranchTest::Eq, src1, src2),
            Inst::Bne { src1, src2, .. } => (BranchTest::Ne, src1, src2),
            Inst::Blt { src1, src2, .. } => (BranchTest::Lt, src1, src2),
            Inst::Bge { src1, src2, .. } => (BranchTest::Ge, src1, src2),
            Inst::Bltu { src1, src2, .. } => (BranchTest::Ltu, src1, src2),
            Inst::Bgeu { src1, src2, .. } => (BranchTest::Geu, src1, src2),
            _ => return None,
        };
        Some((test, Operand::Register(a.0), Operand::Register(b.0)))
    }

    fn semantic_identity(&self) -> &'static str {
        "rv32imac/values-4/rv-asm-0.2.1"
    }
    fn lift(&self, bytes: &[u8]) -> SemanticOp {
        let Some((inst, _)) = decode_instruction(bytes) else {
            return SemanticOp::Unsupported;
        };
        use Operand::{Immediate as Imm, Register as Reg};
        match inst {
            Inst::Lui { uimm, dest } => SemanticOp::Upper {
                dest: dest.0,
                value: uimm.as_u32(),
                pc_relative: false,
            },
            Inst::Auipc { uimm, dest } => SemanticOp::Upper {
                dest: dest.0,
                value: uimm.as_u32(),
                pc_relative: true,
            },
            Inst::Jal { dest, .. } | Inst::Jalr { dest, .. } => SemanticOp::Link { dest: dest.0 },
            Inst::Addi { imm, dest, src1 } => SemanticOp::Integer {
                op: IntegerOp::Add,
                dest: dest.0,
                left: Reg(src1.0),
                right: Imm(imm.as_u32()),
            },
            Inst::Slti { imm, dest, src1 } => SemanticOp::Integer {
                op: IntegerOp::Lt,
                dest: dest.0,
                left: Reg(src1.0),
                right: Imm(imm.as_u32()),
            },
            Inst::Sltiu { imm, dest, src1 } => SemanticOp::Integer {
                op: IntegerOp::Ltu,
                dest: dest.0,
                left: Reg(src1.0),
                right: Imm(imm.as_u32()),
            },
            Inst::Xori { imm, dest, src1 } => SemanticOp::Integer {
                op: IntegerOp::Xor,
                dest: dest.0,
                left: Reg(src1.0),
                right: Imm(imm.as_u32()),
            },
            Inst::Ori { imm, dest, src1 } => SemanticOp::Integer {
                op: IntegerOp::Or,
                dest: dest.0,
                left: Reg(src1.0),
                right: Imm(imm.as_u32()),
            },
            Inst::Andi { imm, dest, src1 } => SemanticOp::Integer {
                op: IntegerOp::And,
                dest: dest.0,
                left: Reg(src1.0),
                right: Imm(imm.as_u32()),
            },
            Inst::Slli { imm, dest, src1 } => SemanticOp::Integer {
                op: IntegerOp::Shl,
                dest: dest.0,
                left: Reg(src1.0),
                right: Imm(imm.as_u32()),
            },
            Inst::Srli { imm, dest, src1 } => SemanticOp::Integer {
                op: IntegerOp::Shr,
                dest: dest.0,
                left: Reg(src1.0),
                right: Imm(imm.as_u32()),
            },
            Inst::Srai { imm, dest, src1 } => SemanticOp::Integer {
                op: IntegerOp::Sar,
                dest: dest.0,
                left: Reg(src1.0),
                right: Imm(imm.as_u32()),
            },
            Inst::Add { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Add,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Sub { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Sub,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Slt { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Lt,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Sltu { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Ltu,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Xor { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Xor,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Or { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Or,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::And { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::And,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Sll { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Shl,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Srl { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Shr,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Sra { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Sar,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Mul { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Mul,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Mulh { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Mulh,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Mulhsu { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Mulhsu,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Mulhu { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Mulhu,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Div { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Div,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Divu { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Divu,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Rem { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Rem,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Remu { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Remu,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Lb { offset, dest, base } => SemanticOp::Memory {
                kind: MemoryKind::Load,
                base: base.0,
                displacement: offset.as_i32(),
                width: 1,
                dest: Some(dest.0),
                source: None,
                swap: false,
                signed: true,
            },
            Inst::Lbu { offset, dest, base } => SemanticOp::Memory {
                kind: MemoryKind::Load,
                base: base.0,
                displacement: offset.as_i32(),
                width: 1,
                dest: Some(dest.0),
                source: None,
                swap: false,
                signed: false,
            },
            Inst::Lh { offset, dest, base } => SemanticOp::Memory {
                kind: MemoryKind::Load,
                base: base.0,
                displacement: offset.as_i32(),
                width: 2,
                dest: Some(dest.0),
                source: None,
                swap: false,
                signed: true,
            },
            Inst::Lhu { offset, dest, base } => SemanticOp::Memory {
                kind: MemoryKind::Load,
                base: base.0,
                displacement: offset.as_i32(),
                width: 2,
                dest: Some(dest.0),
                source: None,
                swap: false,
                signed: false,
            },
            Inst::Lw { offset, dest, base } => SemanticOp::Memory {
                kind: MemoryKind::Load,
                base: base.0,
                displacement: offset.as_i32(),
                width: 4,
                dest: Some(dest.0),
                source: None,
                swap: false,
                signed: false,
            },
            Inst::Sb { offset, src, base } => SemanticOp::Memory {
                kind: MemoryKind::Store,
                base: base.0,
                displacement: offset.as_i32(),
                width: 1,
                dest: None,
                source: Some(src.0),
                swap: false,
                signed: false,
            },
            Inst::Sh { offset, src, base } => SemanticOp::Memory {
                kind: MemoryKind::Store,
                base: base.0,
                displacement: offset.as_i32(),
                width: 2,
                dest: None,
                source: Some(src.0),
                swap: false,
                signed: false,
            },
            Inst::Sw { offset, src, base } => SemanticOp::Memory {
                kind: MemoryKind::Store,
                base: base.0,
                displacement: offset.as_i32(),
                width: 4,
                dest: None,
                source: Some(src.0),
                swap: false,
                signed: false,
            },
            Inst::LrW { dest, addr, .. } => SemanticOp::Memory {
                kind: MemoryKind::LoadReserved,
                base: addr.0,
                displacement: 0,
                width: 4,
                dest: Some(dest.0),
                source: None,
                swap: false,
                signed: false,
            },
            Inst::ScW {
                dest, addr, src, ..
            } => SemanticOp::Memory {
                kind: MemoryKind::StoreConditional,
                base: addr.0,
                displacement: 0,
                width: 4,
                dest: Some(dest.0),
                source: Some(src.0),
                swap: false,
                signed: false,
            },
            Inst::AmoW {
                op,
                dest,
                addr,
                src,
                ..
            } => SemanticOp::Memory {
                kind: MemoryKind::Atomic,
                base: addr.0,
                displacement: 0,
                width: 4,
                dest: Some(dest.0),
                source: Some(src.0),
                swap: op == rv_asm::AmoOp::Swap,
                signed: false,
            },
            Inst::Beq { .. }
            | Inst::Bne { .. }
            | Inst::Blt { .. }
            | Inst::Bge { .. }
            | Inst::Bltu { .. }
            | Inst::Bgeu { .. }
            | Inst::Fence { .. } => SemanticOp::None,
            _ => SemanticOp::Unsupported,
        }
    }
    fn value_relocation(&self, r: &FunctionRelocation, operation: SemanticOp) -> ValueRelocation {
        let store = matches!(
            operation,
            SemanticOp::Memory {
                kind: MemoryKind::Store,
                ..
            }
        );
        if matches!(r.relocation_type, R_RISCV_LO12_S | R_RISCV_PCREL_LO12_S) && !store
            || matches!(r.relocation_type, R_RISCV_LO12_I | R_RISCV_PCREL_LO12_I)
                && !matches!(
                    operation,
                    SemanticOp::Integer {
                        op: IntegerOp::Add,
                        right: Operand::Immediate(_),
                        ..
                    } | SemanticOp::Memory {
                        kind: MemoryKind::Load,
                        ..
                    }
                )
        {
            return ValueRelocation::Unsupported;
        }
        match r.relocation_type {
            R_RISCV_NONE | R_RISCV_RELAX | R_RISCV_ALIGN | R_RISCV_BRANCH | R_RISCV_JAL
            | R_RISCV_RVC_BRANCH | R_RISCV_RVC_JUMP => ValueRelocation::Ignore,
            R_RISCV_CALL | R_RISCV_CALL_PLT => ValueRelocation::CallUpper,
            R_RISCV_HI20 => ValueRelocation::UpperAbsolute,
            R_RISCV_PCREL_HI20 => ValueRelocation::UpperPcRelative,
            R_RISCV_LO12_I | R_RISCV_LO12_S => ValueRelocation::LowerAbsolute,
            R_RISCV_PCREL_LO12_I | R_RISCV_PCREL_LO12_S => ValueRelocation::LowerPcRelative,
            _ => ValueRelocation::Unsupported,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compressed_and_full_instructions_lift_without_display_parsing() {
        assert_eq!(
            RiscvDecoder.lift(&[0x05, 0x05]),
            SemanticOp::Integer {
                op: IntegerOp::Add,
                dest: 10,
                left: Operand::Register(10),
                right: Operand::Immediate(1)
            }
        );
        assert!(matches!(
            RiscvDecoder.lift(&[0x02, 0x45]),
            SemanticOp::Memory {
                kind: MemoryKind::Load,
                base: 2,
                dest: Some(10),
                width: 4,
                displacement: 0,
                ..
            }
        ));
        assert!(matches!(
            RiscvDecoder.lift(&0x60000537u32.to_le_bytes()),
            SemanticOp::Upper {
                dest: 10,
                value: 0x60000000,
                pc_relative: false
            }
        ));
        assert_eq!(RiscvDecoder.lift(&[0xff; 4]), SemanticOp::Unsupported);
        assert_eq!(RiscvDecoder.lift(&[0x73, 0, 0, 0]), SemanticOp::Unsupported);
    }
}
