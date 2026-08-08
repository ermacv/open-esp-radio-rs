//! Executable-section loading for final-image coverage audits.

use std::{fs, path::Path};

use object::{FileKind, Object, ObjectKind, ObjectSection, SectionFlags, SectionKind};

use crate::Result;

use super::model::ExecutableSection;

/// Load every executable section from a fully linked RV32 ELF image.
///
/// This deliberately does not use the symbol table: LTO may make functions
/// local or omit their names, while a final-image policy must cover the bytes
/// that can actually execute.
#[tracing::instrument(
    name = "load_riscv_executable_sections",
    skip_all,
    fields(path = %path.display())
)]
pub fn load_executable_sections(path: &Path) -> Result<Vec<ExecutableSection>> {
    let data = fs::read(path)?;
    if FileKind::parse(data.as_slice())? != FileKind::Elf32 {
        return Err("executable-section audit requires an ELF32 artifact".into());
    }
    let file = object::File::parse(data.as_slice())?;
    if file.architecture() != object::Architecture::Riscv32 {
        return Err("executable-section audit requires a RISC-V 32-bit artifact".into());
    }
    if !file.is_little_endian() {
        return Err("executable-section audit requires a little-endian artifact".into());
    }
    if file.kind() == ObjectKind::Relocatable {
        return Err("executable-section audit requires a fully linked ELF image".into());
    }

    let mut sections = Vec::new();
    for section in file.sections() {
        let executable = section.kind() == SectionKind::Text
            || matches!(
                section.flags(),
                SectionFlags::Elf { sh_flags }
                    if sh_flags & u64::from(object::elf::SHF_EXECINSTR) != 0
            );
        if !executable || section.size() == 0 {
            continue;
        }
        sections.push(ExecutableSection {
            name: section.name().unwrap_or("<unnamed>").to_owned(),
            address: section.address(),
            bytes: section.data()?.to_vec(),
        });
    }
    if sections.is_empty() {
        return Err("ELF image has no executable sections".into());
    }
    sections.sort_by_key(|section| section.address);
    Ok(sections)
}
