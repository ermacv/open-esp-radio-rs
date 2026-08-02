//! Final-ELF policy for statically resolved direct control-flow targets.

use std::{collections::BTreeSet, path::Path};

use rv_asm::{Inst, IsCompressed, Reg, Xlen};

use crate::{Result, artifact};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForbiddenTargetRange {
    pub name: String,
    pub start: u32,
    pub end: u32,
}

impl ForbiddenTargetRange {
    pub fn contains(&self, address: u32) -> bool {
        address >= self.start && address < self.end
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ForbiddenDirectTarget {
    pub section: String,
    pub site: u32,
    pub target: u32,
    pub range: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DirectTargetAudit {
    pub executable_sections: usize,
    pub executable_bytes: usize,
    pub decoded_instructions: usize,
    pub unsupported_instructions: usize,
    pub forbidden_targets: Vec<ForbiddenDirectTarget>,
}

fn reset_values(values: &mut [Option<u32>; 32]) {
    values.fill(None);
    values[usize::from(Reg::ZERO.0)] = Some(0);
}

fn value(values: &[Option<u32>; 32], register: Reg) -> Option<u32> {
    values[usize::from(register.0)]
}

fn set_value(values: &mut [Option<u32>; 32], register: Reg, new_value: Option<u32>) {
    if register != Reg::ZERO {
        values[usize::from(register.0)] = new_value;
    }
    values[usize::from(Reg::ZERO.0)] = Some(0);
}

fn set_unary_value(
    values: &mut [Option<u32>; 32],
    destination: Reg,
    source: Reg,
    operation: impl FnOnce(u32) -> u32,
) {
    let new_value = value(values, source).map(operation);
    set_value(values, destination, new_value);
}

fn set_binary_value(
    values: &mut [Option<u32>; 32],
    destination: Reg,
    left: Reg,
    right: Reg,
    operation: impl FnOnce(u32, u32) -> u32,
) {
    let new_value = value(values, left)
        .zip(value(values, right))
        .map(|(left, right)| operation(left, right));
    set_value(values, destination, new_value);
}

fn record_target(
    findings: &mut BTreeSet<ForbiddenDirectTarget>,
    ranges: &[ForbiddenTargetRange],
    section: &str,
    site: u32,
    target: u32,
) {
    for range in ranges.iter().filter(|range| range.contains(target)) {
        findings.insert(ForbiddenDirectTarget {
            section: section.to_owned(),
            site,
            target,
            range: range.name.clone(),
        });
    }
}

fn audit_section(
    section: &artifact::ExecutableSection,
    ranges: &[ForbiddenTargetRange],
    findings: &mut BTreeSet<ForbiddenDirectTarget>,
) -> Result<(usize, usize)> {
    let section_address = u32::try_from(section.address)
        .map_err(|_| format!("section {} exceeds RV32 address space", section.name))?;
    let mut values = [None; 32];
    reset_values(&mut values);
    let mut decoded = 0;
    let mut unsupported = 0;
    let mut offset = 0_usize;

    while offset < section.bytes.len() {
        let remaining = &section.bytes[offset..];
        if remaining.len() < 2 {
            return Err(
                format!("truncated instruction in {} at +{offset:#x}", section.name).into(),
            );
        }
        let width = if Inst::first_byte_is_compressed(remaining[0]) {
            2
        } else {
            4
        };
        let Some(instruction_bytes) = remaining.get(..width) else {
            return Err(
                format!("truncated instruction in {} at +{offset:#x}", section.name).into(),
            );
        };
        let mut word = [0_u8; 4];
        word[..width].copy_from_slice(instruction_bytes);
        let pc = section_address.wrapping_add(offset as u32);
        let instruction = match Inst::decode(u32::from_le_bytes(word), Xlen::Rv32) {
            Ok((instruction, decoded_width)) => {
                let actual_width = if decoded_width == IsCompressed::Yes {
                    2
                } else {
                    4
                };
                if actual_width != width {
                    return Err(format!(
                        "decoder width disagreement in {} at {pc:#x}",
                        section.name
                    )
                    .into());
                }
                decoded += 1;
                instruction
            }
            Err(_) => {
                // rv-asm intentionally covers the integer ISA used by the
                // validator, while a HIL may also contain floating-point
                // instructions or alignment fill. Neither can encode an
                // integer JAL/JALR. Forget all facts and continue at the
                // architecturally encoded instruction width.
                unsupported += 1;
                reset_values(&mut values);
                offset += width;
                continue;
            }
        };

        match instruction {
            Inst::Lui { uimm, dest } => set_value(&mut values, dest, Some(uimm.as_u32())),
            Inst::Auipc { uimm, dest } => {
                set_value(&mut values, dest, Some(pc.wrapping_add(uimm.as_u32())));
            }
            Inst::Addi { imm, dest, src1 } => {
                set_unary_value(&mut values, dest, src1, |base| {
                    base.wrapping_add(imm.as_u32())
                });
            }
            Inst::Ori { imm, dest, src1 } => {
                set_unary_value(&mut values, dest, src1, |base| base | imm.as_u32());
            }
            Inst::Xori { imm, dest, src1 } => {
                set_unary_value(&mut values, dest, src1, |base| base ^ imm.as_u32());
            }
            Inst::Andi { imm, dest, src1 } => {
                set_unary_value(&mut values, dest, src1, |base| {
                    base & artifact::andi_immediate(imm, width as u8)
                });
            }
            Inst::Slli { imm, dest, src1 } => {
                set_unary_value(&mut values, dest, src1, |base| {
                    base.wrapping_shl(imm.as_u32() & 31)
                });
            }
            Inst::Srli { imm, dest, src1 } => {
                set_unary_value(&mut values, dest, src1, |base| {
                    base.wrapping_shr(imm.as_u32() & 31)
                });
            }
            Inst::Srai { imm, dest, src1 } => {
                set_unary_value(&mut values, dest, src1, |base| {
                    ((base as i32) >> (imm.as_u32() & 31)) as u32
                });
            }
            Inst::Add { dest, src1, src2 } => {
                set_binary_value(&mut values, dest, src1, src2, u32::wrapping_add)
            }
            Inst::Sub { dest, src1, src2 } => {
                set_binary_value(&mut values, dest, src1, src2, u32::wrapping_sub)
            }
            Inst::Or { dest, src1, src2 } => {
                set_binary_value(&mut values, dest, src1, src2, |left, right| left | right)
            }
            Inst::And { dest, src1, src2 } => {
                set_binary_value(&mut values, dest, src1, src2, |left, right| left & right)
            }
            Inst::Xor { dest, src1, src2 } => {
                set_binary_value(&mut values, dest, src1, src2, |left, right| left ^ right)
            }
            Inst::Jal { offset: jump, .. } => {
                record_target(
                    findings,
                    ranges,
                    &section.name,
                    pc,
                    pc.wrapping_add(jump.as_u32()),
                );
                reset_values(&mut values);
            }
            Inst::Jalr {
                offset: jump, base, ..
            } => {
                if let Some(base) = value(&values, base) {
                    record_target(
                        findings,
                        ranges,
                        &section.name,
                        pc,
                        base.wrapping_add(jump.as_u32()) & !1,
                    );
                }
                reset_values(&mut values);
            }
            Inst::Beq { .. }
            | Inst::Bne { .. }
            | Inst::Blt { .. }
            | Inst::Bge { .. }
            | Inst::Bltu { .. }
            | Inst::Bgeu { .. } => reset_values(&mut values),
            Inst::Sb { .. }
            | Inst::Sh { .. }
            | Inst::Sw { .. }
            | Inst::Sd { .. }
            | Inst::Fence { .. }
            | Inst::Ecall
            | Inst::Ebreak => {}
            _ => reset_values(&mut values),
        }
        offset += width;
    }
    Ok((decoded, unsupported))
}

pub fn audit_direct_targets(
    artifact_path: &Path,
    ranges: &[ForbiddenTargetRange],
) -> Result<DirectTargetAudit> {
    if ranges.is_empty() {
        return Err("direct-target audit requires at least one forbidden range".into());
    }
    let sections = artifact::load_executable_sections(artifact_path)?;
    let mut audit = DirectTargetAudit {
        executable_sections: sections.len(),
        executable_bytes: sections.iter().map(|section| section.bytes.len()).sum(),
        ..DirectTargetAudit::default()
    };
    let mut findings = BTreeSet::new();
    for section in &sections {
        let (decoded, unsupported) = audit_section(section, ranges, &mut findings)?;
        audit.decoded_instructions += decoded;
        audit.unsupported_instructions += unsupported;
    }
    audit.forbidden_targets = findings.into_iter().collect();
    Ok(audit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_words(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_le_bytes()).collect()
    }

    fn lui(dest: u8, value: u32) -> u32 {
        (value & 0xffff_f000) | (u32::from(dest) << 7) | 0x37
    }

    fn addi(dest: u8, src: u8, immediate: i32) -> u32 {
        ((immediate as u32 & 0xfff) << 20) | (u32::from(src) << 15) | (u32::from(dest) << 7) | 0x13
    }

    fn jalr(dest: u8, base: u8, immediate: i32) -> u32 {
        ((immediate as u32 & 0xfff) << 20) | (u32::from(base) << 15) | (u32::from(dest) << 7) | 0x67
    }

    fn audit_words(words: &[u32]) -> DirectTargetAudit {
        let section = artifact::ExecutableSection {
            name: ".text".to_owned(),
            address: 0x5000_0000,
            bytes: encode_words(words),
        };
        let ranges = [ForbiddenTargetRange {
            name: "radio-rom".to_owned(),
            start: 0x2f82_0000,
            end: 0x2f84_0000,
        }];
        let mut findings = BTreeSet::new();
        let (decoded, unsupported) = audit_section(&section, &ranges, &mut findings).unwrap();
        DirectTargetAudit {
            executable_sections: 1,
            executable_bytes: section.bytes.len(),
            decoded_instructions: decoded,
            unsupported_instructions: unsupported,
            forbidden_targets: findings.into_iter().collect(),
        }
    }

    #[test]
    fn rejects_lui_addi_jalr_into_forbidden_rom() {
        let audit = audit_words(&[lui(5, 0x2f82_7000), addi(5, 5, -8), jalr(1, 5, 0)]);
        assert_eq!(audit.forbidden_targets.len(), 1);
        assert_eq!(audit.forbidden_targets[0].target, 0x2f82_6ff8);
    }

    #[test]
    fn allows_a_direct_call_to_system_rom_outside_forbidden_range() {
        let audit = audit_words(&[lui(5, 0x2f80_0000), addi(5, 5, 0x24), jalr(1, 5, 0)]);
        assert!(audit.forbidden_targets.is_empty());
    }

    #[test]
    fn rejects_jalr_offset_into_forbidden_rom() {
        let audit = audit_words(&[lui(5, 0x2f82_0000), jalr(1, 5, 0x40)]);
        assert_eq!(audit.forbidden_targets[0].target, 0x2f82_0040);
    }
}
