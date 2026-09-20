//! CLI coverage for companion-provided data relocations, not just calls.

use object::{
    Architecture, BinaryFormat, Endianness, SectionKind, SymbolFlags, SymbolKind, SymbolScope,
    write::{Object, Relocation, Symbol, SymbolSection},
};
use std::{io::Write, path::Path, process::Command};

fn symbol(name: &str, value: u64, size: u64, kind: SymbolKind, section: SymbolSection) -> Symbol {
    Symbol {
        name: name.as_bytes().to_vec(),
        value,
        size,
        kind,
        scope: SymbolScope::Dynamic,
        weak: false,
        section,
        flags: SymbolFlags::None,
    }
}

#[test]
fn execute_run_links_companion_data_before_relocation_validation() {
    let directory = tempfile::tempdir().unwrap();
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = object.add_section(Vec::new(), b".text.entry".to_vec(), SectionKind::Text);
    // lui a0,hi(data+0x32); addi a0,a0,lo(data+0x32); ret
    object.append_section_data(
        text,
        &[0x37, 0x05, 0, 0, 0x13, 0x05, 0x05, 0, 0x67, 0x80, 0, 0],
        4,
    );
    object.add_symbol(symbol(
        "entry",
        0,
        12,
        SymbolKind::Text,
        SymbolSection::Section(text),
    ));
    let data = object.add_symbol(symbol(
        "external_data",
        0,
        0,
        SymbolKind::Data,
        SymbolSection::Undefined,
    ));
    for (offset, r_type) in [
        (0, object::elf::R_RISCV_HI20),
        (4, object::elf::R_RISCV_LO12_I),
    ] {
        object
            .add_relocation(
                text,
                Relocation {
                    offset,
                    symbol: data,
                    addend: 0x32,
                    flags: object::RelocationFlags::Elf { r_type },
                },
            )
            .unwrap();
    }
    let member = object.write().unwrap();
    let mut archive = b"!<arch>\n".to_vec();
    writeln!(
        archive,
        "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`",
        "entry.o/",
        0,
        0,
        0,
        "100644",
        member.len()
    )
    .unwrap();
    archive.extend_from_slice(&member);
    if !member.len().is_multiple_of(2) {
        archive.push(b'\n');
    }
    let archive_path = directory.path().join("input.a");
    std::fs::write(&archive_path, archive).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for provided in [false, true] {
        let mut companion =
            Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
        companion.add_symbol(symbol(
            if provided {
                "external_data"
            } else {
                "unrelated"
            },
            0x2f07_fc40,
            0,
            SymbolKind::Data,
            SymbolSection::Absolute,
        ));
        // The real archive entry must retain precedence over companion names.
        companion.add_symbol(symbol(
            "entry",
            0x1234_0000,
            0,
            SymbolKind::Text,
            SymbolSection::Absolute,
        ));
        let companion_path = directory.path().join("companion.elf");
        std::fs::write(&companion_path, companion.write().unwrap()).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_blobray-generic"))
            .current_dir(&root)
            .args([
                "advanced",
                "execute",
                "run",
                "--project",
                "tools/blobray/tests/fixtures/generic-project/vendor-project.toml",
                "--artifact",
            ])
            .arg(&archive_path)
            .arg("--companion")
            .arg(&companion_path)
            .args(["--symbol", "entry", "--format", "json", "--color", "never"])
            .output()
            .unwrap();
        if provided {
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(report["summary"]["return_value"], 0x2f07_fc72_u32);
            assert_eq!(report["summary"]["complete"], true);
        } else {
            assert!(!output.status.success());
            assert!(String::from_utf8_lossy(&output.stderr).contains("unresolved ELF relocation"));
        }
    }
}
