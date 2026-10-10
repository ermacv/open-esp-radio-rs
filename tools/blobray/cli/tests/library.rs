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
use oer_riscv_model::*;
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
        None,
        ranges,
        &oer_riscv_lift::RiscvDecoder,
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
    let document: blobray_cli::wire::RegisterAccessDocument =
        serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document.schema, blobray_cli::wire::REGISTER_ACCESSES_SCHEMA);
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

#[test]
fn the_command_reports_every_record_of_the_named_functions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.a");
    let library = library();
    std::fs::write(&path, library.bytes()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["--format", "json", "function-records", "--input"])
        .arg(format!("code={}", path.display()))
        .args(["--function", "entry"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let document: blobray_cli::wire::FunctionRecordsDocument =
        serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document.schema, blobray_cli::wire::FUNCTION_RECORDS_SCHEMA);
    assert_eq!(&document.inputs[0].sha256, library.id());
    assert!(document.missing.is_empty());
    // One `entry` in each of the two members, both analyzed to the end of
    // their nine instructions, with the register load among their records.
    assert_eq!(document.functions.len(), 2);
    for function in &document.functions {
        let blobray_cli::wire::NamedFunction::Analyzed { records, .. } = function else {
            panic!("`entry` is analyzable: {function:?}");
        };
        assert!(
            records
                .iter()
                .any(|record| matches!(record, FunctionRecord::MemoryAccess { .. }))
        );
    }

    let absent = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["function-records", "--input"])
        .arg(format!("code={}", path.display()))
        .args(["--function", "entry", "--function", "absent"])
        .output()
        .unwrap();
    assert!(!absent.status.success(), "a name no input defines fails");
    let human = String::from_utf8_lossy(&absent.stdout);
    assert!(human.contains("absent: no input defines it"), "{human}");
    assert!(human.contains("entry (input 0)"), "{human}");
}

#[test]
fn the_command_reports_the_accesses_of_one_field() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.a");
    let library = library();
    std::fs::write(&path, library.bytes()).unwrap();
    // `lw t2, 0(a0)` reads field 0 of the entry `a0`; the accesses through
    // `lui t0, 0x20` are absolute and no field.
    let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["--format", "json", "field-accesses", "--input"])
        .arg(format!("code={}", path.display()))
        .args(["--offset", "0", "--width", "4"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let document: blobray_cli::wire::FieldAccessesDocument =
        serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document.schema, blobray_cli::wire::FIELD_ACCESSES_SCHEMA);
    assert_eq!(&document.inputs[0].sha256, library.id());
    assert!(document.blocked.is_empty());
    // One `entry` in each of the two members.
    assert_eq!(document.functions.len(), 2);
    for function in &document.functions {
        assert_eq!(function.accesses.len(), 1, "{function:?}");
        let access = &function.accesses[0];
        assert_eq!(access.offset, 28);
        assert_eq!(access.width, 4);
        assert_eq!(access.path, [0]);
        assert_eq!(
            access.root,
            blobray_cli::field::FieldRoot::EntryRegister { register: 10 }
        );
    }

    let human = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["field-accesses", "--input"])
        .arg(format!("code={}", path.display()))
        .args(["--offset", "4"])
        .output()
        .unwrap();
    assert!(human.status.success(), "{human:?}");
    let human = String::from_utf8_lossy(&human.stdout);
    assert!(human.contains("0 functions access the field"), "{human}");
}

#[test]
fn filters_select_observations_while_the_summary_counts_the_whole_analysis() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.a");
    std::fs::write(&path, library().bytes()).unwrap();
    let run = |extra: &[&str], json: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_blobray"));
        if json {
            command.args(["--format", "json"]);
        }
        let output = command
            .arg("register-accesses")
            .arg("--input")
            .arg(format!("code={}", path.display()))
            .args(extra)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        output.stdout
    };
    let parse = |bytes: Vec<u8>| -> blobray_cli::wire::RegisterAccessDocument {
        serde_json::from_slice(&bytes).unwrap()
    };
    let all = parse(run(&[], true));
    assert!(all.groups.is_none(), "groups only with --group-by");
    let word = parse(run(
        &["--address", "0x20002", "--group-by", "address"],
        true,
    ));
    assert_eq!(
        word.summary, all.summary,
        "a filter never changes the summary"
    );
    let observations = |document: &blobray_cli::wire::RegisterAccessDocument| {
        document
            .records
            .iter()
            .filter(|record| matches!(record, blobray_domain::RegisterAccess::Observation { .. }))
            .count()
    };
    assert!(observations(&word) > 0 && observations(&word) <= observations(&all));
    assert!(word.records.iter().all(|record| match record {
        blobray_domain::RegisterAccess::Observation { address, .. } =>
            address.is_some_and(|address| address & !3 == 0x20000),
        _ => true,
    }));
    let groups = word.groups.expect("address groups");
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].word, Some(0x20000));
    let absent = parse(run(
        &["--function", "absent", "--group-by", "function"],
        true,
    ));
    assert_eq!(observations(&absent), 0);
    assert_eq!(absent.groups, Some(Vec::new()));
    let human = String::from_utf8(run(&["--address", "0x20000"], false)).unwrap();
    let mut lines = human.lines();
    assert_eq!(lines.next(), Some("0x00020000"), "{human}");
    assert!(
        human.lines().last().unwrap().ends_with("unresolved)"),
        "{human}"
    );
}
