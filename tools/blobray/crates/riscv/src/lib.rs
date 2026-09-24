//! ISA-only function decoder and relocation interpretation; no I/O authority.
mod execution;
use blobray_domain::*;
pub use execution::RiscvExecutor;
use object::elf::*;
use rv_asm::{Inst, IsCompressed, Xlen};
pub struct RiscvDecoder;
impl PointerDecoder for RiscvDecoder {
    fn pointer_identity(&self) -> Option<&'static str> {
        Some("rv32-absolute-rela/1")
    }
    fn pointer_relocation(&self, r: &FunctionRelocation) -> PointerRelocation {
        match r.relocation_type {
            object::elf::R_RISCV_NONE => PointerRelocation::None,
            object::elf::R_RISCV_32 => PointerRelocation::Absolute32,
            object::elf::R_RISCV_64 => PointerRelocation::Unsupported { width: Some(8) },
            _ => PointerRelocation::Unsupported { width: None },
        }
    }
}
impl FunctionDecoder for RiscvDecoder {
    fn identity(&self) -> &'static str {
        "rv32imac/rv-asm-0.2.1/policy-2"
    }
    fn unsupported_flow(&self, bytes: &[u8]) -> UnsupportedFlow {
        // ISA structure only, not CSR/privileged execution support. Zicsr:
        // https://docs.riscv.org/reference/isa/unpriv/zicsr.html
        // Trap returns transfer through architectural CSR state, not a static target.
        if let Ok(raw) = <[u8; 4]>::try_from(bytes) {
            let word = u32::from_le_bytes(raw);
            if matches!(word, 0x30200073 | 0x10200073) {
                return UnsupportedFlow::Indirect;
            }
            if word == 0x10500073
                || ((word & 0x7f) == 0x73 && matches!((word >> 12) & 7, 1 | 2 | 3 | 5 | 6 | 7))
            {
                return UnsupportedFlow::NonControl;
            }
            if matches!(
                word & 0x7f,
                0x03 | 0x07
                    | 0x0f
                    | 0x13
                    | 0x17
                    | 0x23
                    | 0x27
                    | 0x2f
                    | 0x33
                    | 0x37
                    | 0x43
                    | 0x47
                    | 0x4b
                    | 0x4f
                    | 0x53
            ) {
                return UnsupportedFlow::NonControl;
            }
        } else if let Ok(raw) = <[u8; 2]>::try_from(bytes) {
            let half = u16::from_le_bytes(raw);
            if half == 0 || matches!((half & 3, half >> 13), (0, _) | (2, 0..=3) | (2, 5..=7)) {
                return UnsupportedFlow::NonControl;
            }
        }
        UnsupportedFlow::Unknown
    }
    fn decode(&self, bytes: &[u8]) -> Option<DecodedOp> {
        let (inst, width) = decode_instruction(bytes)?;
        let flow = match inst {
            Inst::Jal { offset, dest } => InstructionFlow::Jump {
                displacement: offset.as_i32(),
                link: matches!(dest.0, 1 | 5),
            },
            Inst::Jalr { offset, base, dest } => InstructionFlow::Indirect {
                base: base.0,
                offset: offset.as_i32(),
                link: matches!(dest.0, 1 | 5),
            },
            Inst::Beq { offset, .. }
            | Inst::Bne { offset, .. }
            | Inst::Blt { offset, .. }
            | Inst::Bge { offset, .. }
            | Inst::Bltu { offset, .. }
            | Inst::Bgeu { offset, .. } => InstructionFlow::Branch {
                displacement: offset.as_i32(),
            },
            Inst::Ecall | Inst::Ebreak => InstructionFlow::Stop,
            _ => InstructionFlow::Next,
        };
        Some(DecodedOp {
            length: width as u8,
            text: inst.to_string(),
            flow,
        })
    }
    fn reference(
        &self,
        r: &FunctionRelocation,
        all: &[FunctionRelocation],
        section: u32,
        control: &mut dyn RunControl,
    ) -> Result<NormalizedReference> {
        control.checkpoint(1)?;
        let mut target = r.target.clone();
        let mut addend = r.addend;
        let mut paired = None;
        let kind = match r.relocation_type {
            R_RISCV_CALL | R_RISCV_CALL_PLT => ReferenceKind::Call,
            R_RISCV_BRANCH | R_RISCV_JAL | R_RISCV_RVC_BRANCH | R_RISCV_RVC_JUMP => {
                ReferenceKind::Branch
            }
            R_RISCV_HI20 | R_RISCV_LO12_I | R_RISCV_LO12_S | R_RISCV_PCREL_HI20 | R_RISCV_32 => {
                ReferenceKind::Address
            }
            R_RISCV_PCREL_LO12_I | R_RISCV_PCREL_LO12_S => {
                let mut found = None;
                let mut count = 0;
                if r.target.section == Some(section) && r.addend == Some(0) {
                    let start = all.partition_point(|hi| hi.offset < r.target.offset);
                    for hi in all[start..]
                        .iter()
                        .take_while(|hi| hi.offset == r.target.offset)
                    {
                        control.checkpoint(1)?;
                        if hi.offset == r.target.offset && hi.relocation_type == R_RISCV_PCREL_HI20
                        {
                            found = Some(hi);
                            count += 1;
                        }
                    }
                }
                if count == 1 {
                    let hi = found.unwrap();
                    target = hi.target.clone();
                    addend = hi.addend;
                    paired = Some((hi.section, hi.index));
                    ReferenceKind::Address
                } else {
                    ReferenceKind::Unknown
                }
            }
            R_RISCV_NONE | R_RISCV_RELAX | R_RISCV_ALIGN => ReferenceKind::Metadata,
            _ => ReferenceKind::Unknown,
        };
        let known = kind != ReferenceKind::Unknown
            && (kind == ReferenceKind::Metadata
                || (addend.is_some() && target.definition != SymbolDefinition::Null));
        Ok(NormalizedReference {
            kind,
            target,
            addend,
            paired,
            known,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn relocation(offset: u64, kind: u32, target_offset: u64) -> FunctionRelocation {
        FunctionRelocation {
            section: 9,
            index: offset,
            offset,
            relocation_type: kind,
            addend: Some(0),
            target: std::sync::Arc::new(ReferenceTarget {
                binding: 1,
                definition: SymbolDefinition::Section,
                symbol: SymbolId {
                    object: ObjectId {
                        artifact: ArtifactId::of_bytes(b"fixture"),
                        location: ObjectLocation::Standalone,
                    },
                    table: SymbolTableKind::Static,
                    table_section: 8,
                    index: offset + 1,
                },
                name: b"symbol".to_vec(),
                section: Some(1),
                offset: target_offset,
                symbol_type: 0,
            }),
        }
    }
    #[test]
    fn pcrel_lo_uses_label_identity_not_adjacency_or_symbol_name() {
        let lo = relocation(24, R_RISCV_PCREL_LO12_I, 0);
        let mut hi = relocation(0, R_RISCV_PCREL_HI20, 128);
        std::sync::Arc::make_mut(&mut hi.target).name = b"actual_data".to_vec();
        hi.addend = Some(4);
        let other = relocation(20, R_RISCV_PCREL_HI20, 256);
        let all = [hi.clone(), other, lo.clone()];
        let r = RiscvDecoder
            .reference(&lo, &all, 1, &mut || Ok(()))
            .unwrap();
        assert!(r.known);
        assert_eq!(r.target, hi.target);
        assert_eq!(r.addend, Some(4));
        assert_eq!(r.paired, Some((9, 0)));
        let r = RiscvDecoder
            .reference(&lo, &[hi.clone(), hi], 1, &mut || Ok(()))
            .unwrap();
        assert!(!r.known);
    }
    #[test]
    fn compressed_andi_uses_signed_six_bit_immediates_in_display_and_lifting() {
        // Independently encode every C.ANDI register and immediate, and compare
        // with the ordinary ANDI encoding and the ISA's signed numeric value.
        for register in 8u16..16 {
            for signed in -32i32..32 {
                let bits = (signed as u16) & 63;
                let half = 0x8801 | ((register - 8) << 7) | ((bits & 31) << 2) | ((bits & 32) << 7);
                let word = ((signed as u32 & 0xfff) << 20)
                    | (u32::from(register) << 15)
                    | 0x7013
                    | (u32::from(register) << 7);
                let expected = SemanticOp::Integer {
                    op: IntegerOp::And,
                    dest: register as u8,
                    left: Operand::Register(register as u8),
                    right: Operand::Immediate(signed as u32),
                };
                assert_eq!(RiscvDecoder.lift(&half.to_le_bytes()), expected);
                assert_eq!(RiscvDecoder.lift(&word.to_le_bytes()), expected);
                let compressed = RiscvDecoder.decode(&half.to_le_bytes()).unwrap();
                let full = RiscvDecoder.decode(&word.to_le_bytes()).unwrap();
                assert_eq!(compressed.text, full.text);
                assert_eq!(compressed.length, 2);
                assert_eq!(full.length, 4);
            }
        }
        // Ordinary ANDI has twelve immediate bits; it must not be truncated.
        for signed in [-2048i32, -33, 32, 63, 2047] {
            let word = ((signed as u32 & 0xfff) << 20) | 0x57513;
            assert!(matches!(RiscvDecoder.lift(&word.to_le_bytes()),
                SemanticOp::Integer { right: Operand::Immediate(value), .. }
                if value == signed as u32));
        }
    }
    #[test]
    fn decoding_rejects_truncated_long_and_unsupported_encodings() {
        assert_eq!(RiscvDecoder.decode(&[1, 0]).unwrap().length, 2);
        assert!(RiscvDecoder.decode(&[0x13, 0, 0]).is_none());
        assert!(matches!(
            RiscvDecoder.decode(&[0x6f, 1, 0x40, 0]).unwrap().flow,
            InstructionFlow::Jump { link: false, .. }
        ));
        assert!(RiscvDecoder.decode(&[0xff, 0xff, 0xff, 0xff]).is_none());
        assert!(matches!(
            RiscvDecoder.decode(&[0x67, 0x80, 0, 0]).unwrap().flow,
            InstructionFlow::Indirect {
                base: 1,
                link: false,
                ..
            }
        ));
    }
}

fn decode_instruction(bytes: &[u8]) -> Option<(Inst, usize)> {
    if bytes.len() < 2 {
        return None;
    }
    let half = u16::from_le_bytes([bytes[0], bytes[1]]);
    let width = if half & 3 != 3 {
        2
    } else if half & 0x1f != 0x1f {
        4
    } else {
        return None;
    };
    if bytes.len() < width {
        return None;
    }
    let mut code = [0; 4];
    code[..width].copy_from_slice(&bytes[..width]);
    let (inst, compressed) = Inst::decode(u32::from_le_bytes(code), Xlen::Rv32).ok()?;
    if (compressed == IsCompressed::Yes) != (width == 2) {
        return None;
    }
    // rv-asm 0.2.1 zero-extends C.ANDI's six-bit immediate. Normalize at
    // this shared boundary so display, abstract lifting and concrete execution
    // all implement the ISA's signed immediate (C extension, integer ALU).
    let inst = match (width, inst) {
        (2, Inst::Andi { imm, dest, src1 }) => Inst::Andi {
            imm: rv_asm::Imm::new_i32(((imm.as_u32() << 26) as i32) >> 26),
            dest,
            src1,
        },
        (_, inst) => inst,
    };
    Some((inst, width))
}

mod semantics;
