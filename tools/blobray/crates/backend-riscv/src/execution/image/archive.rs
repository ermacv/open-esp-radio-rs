//! Entry-rooted archive execution through a deterministic analysis link.

use std::{path::Path, process::Command};

use super::ExecutableImage;
use crate::Result;

impl ExecutableImage {
    /// Load an ELF or prepare an archive's selected entry for execution.
    ///
    /// Archive addresses describe an analysis image, not firmware placement.
    /// Retained relocations let execution reject reachable missing definitions
    /// instead of treating the linker's placeholder bytes as executable truth.
    pub fn load_entry(path: &Path, symbol: &str) -> Result<Self> {
        let is_archive = crate::read_artifact(path)?.starts_with(b"!<arch>\n");
        if !is_archive {
            return Self::load(path);
        }
        let temporary = tempfile::tempdir()?;
        let output = temporary.path().join("execution.elf");
        let mut linker = analysis_linker()?;
        let result = linker
            .args([
                "-m",
                "elf32lriscv",
                "--gc-sections",
                "--no-relax",
                "--emit-relocs",
                "--unresolved-symbols=ignore-all",
                "-Ttext=0x40000000",
                "--entry",
            ])
            .arg(symbol)
            .arg("--undefined")
            .arg(symbol)
            .arg("-o")
            .arg(&output)
            .arg(std::fs::canonicalize(path)?)
            .output()
            .map_err(|error| format!("cannot run archive execution linker (set BLOBRAY_RISCV_LINKER to a GNU-compatible RV32 linker): {error}"))?;
        if !result.status.success() {
            return Err(format!(
                "archive execution link failed for {} entry {symbol}: {}",
                path.display(),
                String::from_utf8_lossy(&result.stderr).trim()
            )
            .into());
        }
        let image = Self::load(&output)?;
        if !image.symbols_by_name.contains_key(symbol) {
            return Err(format!(
                "archive {} has no executable entry {symbol}",
                path.display()
            )
            .into());
        }
        Ok(image)
    }
}

fn analysis_linker() -> Result<Command> {
    if let Some(path) = std::env::var_os("BLOBRAY_RISCV_LINKER") {
        return Ok(Command::new(path));
    }
    let sysroot = Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()
        .map_err(|error| {
            format!("cannot locate rust-lld via rustc; set BLOBRAY_RISCV_LINKER: {error}")
        })?;
    let version = Command::new("rustc").arg("-vV").output()?;
    if !sysroot.status.success() || !version.status.success() {
        return Err("archive execution requires rust-lld or BLOBRAY_RISCV_LINKER (GNU-compatible RV32 linker)".into());
    }
    let version = String::from_utf8_lossy(&version.stdout);
    let host = version
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .ok_or("rustc did not report its host for archive execution")?;
    let path = Path::new(String::from_utf8_lossy(&sysroot.stdout).trim())
        .join("lib/rustlib")
        .join(host)
        .join("bin")
        .join(format!("rust-lld{}", std::env::consts::EXE_SUFFIX));
    let mut command = Command::new(path);
    command.args(["-flavor", "gnu"]);
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        MmioMap,
        execution::{Scenario, execute},
    };
    use object::write::{Object, Relocation, Symbol, SymbolSection};
    use object::{
        Architecture, BinaryFormat, Endianness, SectionKind, SymbolFlags, SymbolKind, SymbolScope,
    };

    fn archive(missing_call: bool) -> (tempfile::TempDir, std::path::PathBuf) {
        archive_objects(missing_call, false)
    }

    fn archive_objects(
        missing_call: bool,
        supply_callee: bool,
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
        let text = object.add_section(Vec::new(), b".text.entry".to_vec(), SectionKind::Text);
        let code: &[u8] = if missing_call {
            &[0x17, 0x03, 0, 0, 0x67, 0, 0x03, 0] // tail missing
        } else {
            &[0x13, 0x05, 0xa0, 0x02, 0x67, 0x80, 0, 0] // return 42
        };
        object.append_section_data(text, code, 4);
        object.add_symbol(Symbol {
            name: b"entry".to_vec(),
            value: 0,
            size: code.len() as u64,
            kind: SymbolKind::Text,
            scope: SymbolScope::Linkage,
            weak: false,
            section: SymbolSection::Section(text),
            flags: SymbolFlags::None,
        });
        if missing_call {
            let target = object.add_symbol(Symbol {
                name: b"missing".to_vec(),
                value: 0,
                size: 0,
                kind: SymbolKind::Text,
                scope: SymbolScope::Dynamic,
                weak: false,
                section: SymbolSection::Undefined,
                flags: SymbolFlags::None,
            });
            object
                .add_relocation(
                    text,
                    Relocation {
                        offset: 0,
                        symbol: target,
                        addend: 0,
                        flags: object::RelocationFlags::Elf {
                            r_type: object::elf::R_RISCV_CALL_PLT,
                        },
                    },
                )
                .unwrap();
        }
        let mut archive = b"!<arch>\n".to_vec();
        append_member(&mut archive, "entry.o/", &object.write().unwrap());
        if supply_callee {
            let mut callee =
                Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
            let text = callee.add_section(Vec::new(), b".text.missing".to_vec(), SectionKind::Text);
            callee.append_section_data(text, &[0x13, 0x05, 0xa0, 0x02, 0x67, 0x80, 0, 0], 4);
            callee.add_symbol(Symbol {
                name: b"missing".to_vec(),
                value: 0,
                size: 8,
                kind: SymbolKind::Text,
                scope: SymbolScope::Dynamic,
                weak: false,
                section: SymbolSection::Section(text),
                flags: SymbolFlags::None,
            });
            append_member(&mut archive, "callee.o/", &callee.write().unwrap());
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input.a");
        std::fs::write(&path, archive).unwrap();
        (directory, path)
    }

    fn append_member(archive: &mut Vec<u8>, name: &str, member: &[u8]) {
        archive.extend_from_slice(
            format!(
                "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
                name,
                0,
                0,
                0,
                "100644",
                member.len()
            )
            .as_bytes(),
        );
        archive.extend_from_slice(member);
        if !member.len().is_multiple_of(2) {
            archive.push(b'\n');
        }
    }

    #[test]
    fn archive_resolves_a_callee_from_another_member() {
        let (_directory, path) = archive_objects(true, true);
        let image = ExecutableImage::load_entry(&path, "entry").unwrap();
        let map = MmioMap {
            registers: Vec::new(),
            regions: Vec::new(),
        };
        let result = execute(&image, &map, "entry", Scenario::default()).unwrap();
        assert_eq!(result.return_value, 42);
        assert!(
            image
                .coverage_inventory("entry")
                .unwrap()
                .unresolved_edges
                .is_empty()
        );
    }

    #[test]
    fn archive_entry_executes_without_a_user_linked_elf() {
        let (_directory, path) = archive(false);
        let image = ExecutableImage::load_entry(&path, "entry").unwrap();
        let map = MmioMap {
            registers: Vec::new(),
            regions: Vec::new(),
        };
        let result = execute(&image, &map, "entry", Scenario::default()).unwrap();
        assert_eq!(result.return_value, 42);
    }

    #[test]
    fn archive_missing_callee_remains_unresolved() {
        let (_directory, path) = archive(true);
        let image = ExecutableImage::load_entry(&path, "entry").unwrap();
        let inventory = image.coverage_inventory("entry").unwrap();
        assert!(
            inventory
                .unresolved_edges
                .values()
                .any(|value| value.contains("missing"))
        );
        let map = MmioMap {
            registers: Vec::new(),
            regions: Vec::new(),
        };
        assert!(execute(&image, &map, "entry", Scenario::default()).is_err());
    }

    #[test]
    fn archive_missing_data_is_not_silently_materialized_as_zero() {
        let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
        let text = object.add_section(Vec::new(), b".text.entry".to_vec(), SectionKind::Text);
        object.append_section_data(
            text,
            &[0x37, 0x05, 0, 0, 0x13, 0x05, 0x05, 0, 0x67, 0x80, 0, 0],
            4,
        );
        object.add_symbol(Symbol {
            name: b"entry".to_vec(),
            value: 0,
            size: 12,
            kind: SymbolKind::Text,
            scope: SymbolScope::Dynamic,
            weak: false,
            section: SymbolSection::Section(text),
            flags: SymbolFlags::None,
        });
        let target = object.add_symbol(Symbol {
            name: b"missing_data".to_vec(),
            value: 0,
            size: 0,
            kind: SymbolKind::Data,
            scope: SymbolScope::Dynamic,
            weak: false,
            section: SymbolSection::Undefined,
            flags: SymbolFlags::None,
        });
        for (offset, r_type) in [
            (0, object::elf::R_RISCV_HI20),
            (4, object::elf::R_RISCV_LO12_I),
        ] {
            object
                .add_relocation(
                    text,
                    Relocation {
                        offset,
                        symbol: target,
                        addend: 0,
                        flags: object::RelocationFlags::Elf { r_type },
                    },
                )
                .unwrap();
        }
        let mut archive = b"!<arch>\n".to_vec();
        append_member(&mut archive, "data.o/", &object.write().unwrap());
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input.a");
        std::fs::write(&path, archive).unwrap();
        let image = ExecutableImage::load_entry(&path, "entry").unwrap();
        let map = MmioMap {
            registers: Vec::new(),
            regions: Vec::new(),
        };
        let error = execute(&image, &map, "entry", Scenario::default())
            .unwrap_err()
            .to_string();
        assert!(error.contains("unresolved ELF relocation"));
        assert!(error.contains("missing_data"));
    }

    #[test]
    fn archive_missing_entry_is_rejected() {
        let (_directory, path) = archive(false);
        let error = ExecutableImage::load_entry(&path, "absent").unwrap_err();
        assert!(error.to_string().contains("no executable entry absent"));
    }
}
