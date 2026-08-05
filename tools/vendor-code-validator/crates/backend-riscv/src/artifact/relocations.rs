//! RISC-V relocation normalization, including PC-relative HI/LO pairing.

use object::{
    Object, ObjectSection, ObjectSymbol, RelocationFlags, RelocationTarget, SectionIndex,
};

use super::{RelocationKind, riscv_relocation_kind};
use crate::Result;

#[derive(Clone, Debug)]
pub(super) struct SectionRelocation {
    pub(super) address: u64,
    pub(super) kind: RelocationKind,
    pub(super) symbol: String,
    addend: i64,
}

impl SectionRelocation {
    pub(super) const fn addend(&self) -> i64 {
        self.addend
    }
}

#[derive(Clone, Debug)]
struct RawRelocation {
    address: u64,
    r_type: u32,
    symbol: String,
    target_address: u64,
    target_section: Option<SectionIndex>,
    addend: i64,
}

fn normalize(
    raw: &RawRelocation,
    section_index: SectionIndex,
    all: &[RawRelocation],
) -> Result<Option<SectionRelocation>> {
    if !matches!(
        raw.r_type,
        object::elf::R_RISCV_PCREL_LO12_I | object::elf::R_RISCV_PCREL_LO12_S
    ) {
        let Some(kind) = riscv_relocation_kind(raw.r_type) else {
            return Ok(None);
        };
        if raw.symbol.is_empty() {
            return Err(format!(
                "RISC-V relocation {kind:?} at {:#x} has an unnamed target",
                raw.address
            )
            .into());
        }
        return Ok(Some(SectionRelocation {
            address: raw.address,
            kind,
            symbol: raw.symbol.clone(),
            addend: raw.addend,
        }));
    }

    if raw.addend != 0 {
        return Err(format!(
            "RISC-V PCREL_LO12 relocation at {:#x} has non-zero addend {:+#x}",
            raw.address, raw.addend
        )
        .into());
    }
    if raw.target_section != Some(section_index) {
        return Err(format!(
            "RISC-V PCREL_LO12 relocation at {:#x} does not target a label in the same section",
            raw.address
        )
        .into());
    }
    let high = all
        .iter()
        .find(|candidate| {
            candidate.address == raw.target_address
                && matches!(
                    candidate.r_type,
                    object::elf::R_RISCV_PCREL_HI20 | object::elf::R_RISCV_GOT_HI20
                )
        })
        .ok_or_else(|| {
            format!(
                "RISC-V PCREL_LO12 relocation at {:#x} has no HI20 relocation at label {:#x}",
                raw.address, raw.target_address
            )
        })?;
    if high.symbol.is_empty() {
        return Err(format!(
            "RISC-V PCREL HI20 relocation at {:#x} has an unnamed target",
            high.address
        )
        .into());
    }
    let kind = match (raw.r_type, high.r_type) {
        (object::elf::R_RISCV_PCREL_LO12_I, object::elf::R_RISCV_PCREL_HI20) => {
            RelocationKind::PcRelLo12I
        }
        (object::elf::R_RISCV_PCREL_LO12_S, object::elf::R_RISCV_PCREL_HI20) => {
            RelocationKind::PcRelLo12S
        }
        (object::elf::R_RISCV_PCREL_LO12_I, object::elf::R_RISCV_GOT_HI20) => {
            RelocationKind::GotPcRelLo12I
        }
        (object::elf::R_RISCV_PCREL_LO12_S, object::elf::R_RISCV_GOT_HI20) => {
            return Err(format!(
                "RISC-V GOT_HI20 at {:#x} is paired with a store relocation at {:#x}",
                high.address, raw.address
            )
            .into());
        }
        _ => unreachable!(),
    };
    Ok(Some(SectionRelocation {
        address: raw.address,
        kind,
        symbol: high.symbol.clone(),
        addend: high.addend,
    }))
}

pub(super) fn collect_section_relocations(
    file: &object::File<'_>,
    section_index: SectionIndex,
) -> Result<Vec<SectionRelocation>> {
    let section = file.section_by_index(section_index)?;
    let section_start = section.address();
    let section_end = section_start.wrapping_add(section.size());
    let mut raw = Vec::new();
    for (offset, relocation) in section.relocations() {
        let RelocationFlags::Elf { r_type } = relocation.flags() else {
            continue;
        };
        let RelocationTarget::Symbol(index) = relocation.target() else {
            continue;
        };
        let target = file.symbol_by_index(index)?;
        let address = if offset >= section_start && offset < section_end {
            offset
        } else {
            section_start.wrapping_add(offset)
        };
        raw.push(RawRelocation {
            address,
            r_type,
            symbol: target.name().unwrap_or_default().to_owned(),
            target_address: target.address(),
            target_section: target.section_index(),
            addend: relocation.addend(),
        });
    }

    let mut normalized = raw
        .iter()
        .map(|relocation| normalize(relocation, section_index, &raw))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    normalized.sort_by_key(|relocation| (relocation.address, relocation.kind as u8));
    Ok(normalized)
}
