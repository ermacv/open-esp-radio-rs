//! ISA-only function decoder and relocation interpretation; no I/O authority.
mod execution;
use blobray_domain::*;
pub use execution::RiscvExecutor;
use object::elf::*;
use rv_asm::{Inst, IsCompressed, Xlen};
pub struct RiscvDecoder;
impl FunctionDecoder for RiscvDecoder {
    fn identity(&self) -> &'static str {
        "rv32imac/rv-asm-0.2.1/policy-1"
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
                    for hi in all {
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
            target: ReferenceTarget {
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
            },
        }
    }
    #[test]
    fn pcrel_lo_uses_label_identity_not_adjacency_or_symbol_name() {
        let lo = relocation(24, R_RISCV_PCREL_LO12_I, 0);
        let mut hi = relocation(0, R_RISCV_PCREL_HI20, 128);
        hi.target.name = b"actual_data".to_vec();
        hi.addend = Some(4);
        let other = relocation(20, R_RISCV_PCREL_HI20, 256);
        let all = [lo.clone(), other, hi.clone()];
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
    Some((inst, width))
}

mod semantics;
