#![cfg(target_os = "linux")]
#[allow(dead_code)]
mod support;
use blobray_application::in_process::{Executable, Limits};
use blobray_application::library::register_accesses;
use blobray_domain::*;
use object::{
    Architecture, BinaryFormat, Endianness, SectionKind, SymbolFlags, SymbolKind, SymbolScope,
    write::{Object, Symbol, SymbolSection},
};
use std::process::Command;

/// A relocatable object with one function `entry` of `size` bytes over `code`.
fn object(code: &[u32], size: u64) -> Vec<u8> {
    let bytes: Vec<u8> = code.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = obj.add_section(Vec::new(), b".text.entry".to_vec(), SectionKind::Text);
    obj.append_section_data(text, &bytes, 2);
    obj.add_symbol(Symbol {
        name: b"entry".to_vec(),
        value: 0,
        size,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text),
        flags: SymbolFlags::None,
    });
    obj.write().unwrap()
}

// lui t0,0x20; lw t1,0(t0); andi t2,t1,0xf; andi t1,t1,-16;
// ori t1,t1,3; sw t1,0(t0); lb t2,1(t0); lw t2,0(a0); ret.
const REGISTERS: [u32; 9] = [
    0x000202b7, 0x0002a303, 0x00f37393, 0xff037313, 0x00336313, 0x0062a023, 0x00128383, 0x00052383,
    0x00008067,
];

fn accesses(
    inputs: &[Executable],
    ranges: &[ImageRegion],
) -> Result<(RegisterAccessSummary, Vec<RegisterAccess>)> {
    let memory = WorkingMemory::new(64 * 1024 * 1024).unwrap();
    let mut control = Limits::new(DEFAULT_WORK_UNITS, std::time::Duration::from_secs(60));
    let mut records = Vec::new();
    let summary = register_accesses(
        inputs,
        ranges,
        &blobray_backend_riscv::RiscvDecoder,
        &memory,
        &mut control,
        &mut |record, _| {
            records.push(record.clone());
            Ok(())
        },
    )?;
    Ok((summary, records))
}

fn library() -> Executable {
    let code = object(&REGISTERS, 36);
    Executable::new(support::archive(
        &[(b"first.o", &code), (b"second.o", &code)],
        false,
    ))
}

#[test]
fn every_library_function_reports_its_masked_and_unresolved_accesses() {
    let (summary, records) = accesses(&[library()], &[]).unwrap();
    assert_eq!(summary.functions, 2, "{records:?}");
    assert_eq!((summary.blocked_functions, summary.gaps), (0, 0));
    assert!(summary.unresolved_addresses > 0);
    let observed: Vec<_> = records
        .iter()
        .map(|r| match r {
            RegisterAccess::Observation {
                function,
                address,
                mask,
                ..
            } => (function, *address, *mask),
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert!(
        observed.iter().all(|(function, _, _)| function.input == 0
            && function.name.as_deref() == Some(&b"entry"[..]))
    );
    // Both archive members are analyzed, each under its own occurrence.
    let members: std::collections::BTreeSet<_> =
        observed.iter().map(|(f, _, _)| &f.symbol.object).collect();
    assert_eq!(members.len(), 2);
    for kind in [
        RegisterMaskKind::ReadSelection,
        RegisterMaskKind::WriteReplacement,
    ] {
        assert!(
            observed
                .iter()
                .any(|(_, address, mask)| *address == Some(0x20000)
                    && *mask == Some(RegisterMask { kind, bits: 15 }))
        );
    }
}

#[test]
fn ranges_select_resolved_addresses_and_bad_ranges_are_rejected() {
    let inside = [ImageRegion {
        start: 0x20000,
        length: 4,
    }];
    let (summary, records) = accesses(&[library()], &inside).unwrap();
    assert!(records.iter().all(|r| matches!(
        r,
        RegisterAccess::Observation {
            address: None | Some(0x20000 | 0x20001),
            ..
        }
    )));
    let outside = [ImageRegion {
        start: 0x40000,
        length: 4,
    }];
    let (without, _) = accesses(&[library()], &outside).unwrap();
    assert!(without.observations < summary.observations);
    assert_eq!(without.unresolved_addresses, summary.unresolved_addresses);
    for bad in [
        vec![ImageRegion {
            start: 0x20000,
            length: 0,
        }],
        vec![ImageRegion {
            start: u32::MAX,
            length: 2,
        }],
        vec![inside[0]; 257],
    ] {
        let error = accesses(&[library()], &bad).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidRequest, "{error:?}");
    }
}

#[test]
fn unanalyzable_functions_and_foreign_objects_are_reported_not_skipped() {
    let without_size = object(&REGISTERS, 0);
    let inputs = [
        Executable::new(support::archive(
            &[
                (b"unsized.o", &without_size),
                (b"notes.txt", b"not an object"),
            ],
            false,
        )),
        Executable::new(b"neither an archive nor an ELF".to_vec()),
    ];
    let (summary, records) = accesses(&inputs, &[]).unwrap();
    assert_eq!(summary.functions, 0);
    assert!(records.iter().any(|r| matches!(
        r,
        RegisterAccess::Blocked { function, error }
            if function.input == 0 && error.code == ErrorCode::NeedsExtent
    )));
    assert!(records.iter().any(|r| matches!(
        r,
        RegisterAccess::Gap { input: 0, object: Some(_), reason }
            if reason.contains("not a supported ELF32")
    )));
    assert!(records.iter().any(|r| matches!(
        r,
        RegisterAccess::Gap { input: 1, reason, .. } if reason == "unsupported input format"
    )));
    assert_eq!(summary.blocked_functions, 1);
}

#[test]
fn the_command_streams_one_document_of_every_access() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.a");
    let library = library();
    std::fs::write(&path, library.bytes()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["--format", "json", "register-accesses", "--input"])
        .arg(format!("code={}", path.display()))
        .args(["--range", "0x20000:4"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let document: blobray_next_host::wire::RegisterAccessDocument =
        serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        document.schema,
        blobray_next_host::wire::REGISTER_ACCESSES_SCHEMA
    );
    assert_eq!(document.inputs[0].role, "code");
    assert_eq!(&document.inputs[0].sha256, library.id());
    let (summary, records) = accesses(
        &[library],
        &[ImageRegion {
            start: 0x20000,
            length: 4,
        }],
    )
    .unwrap();
    assert_eq!(document.summary, summary);
    assert_eq!(document.records, records);
    let human = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["register-accesses", "--input"])
        .arg(format!("code={}", path.display()))
        .output()
        .unwrap();
    assert!(human.status.success(), "{human:?}");
    assert!(String::from_utf8_lossy(&human.stdout).starts_with("2 functions"));
}
