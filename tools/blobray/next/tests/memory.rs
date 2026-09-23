#![cfg(target_os = "linux")]
mod support;
use blobray_application::{ReadQuery, RunRecord};

use blobray_application as app;
use blobray_domain::*;
use blobray_next_host::linux::LinuxHost;
use std::{fs, path::Path, process::Command, sync::Arc};

fn application() -> app::Application {
    app::Application::new(Arc::new(LinuxHost::new(
        env!("CARGO_BIN_EXE_blobray").into(),
        None,
    )))
}
fn budget() -> ResourceBudget {
    ResourceBudget {
        mode: LimitMode::Watchdog,
        working_memory_bytes: Some(1024 * 1024),
        poll_ms: 5,
        grace_ms: 100,
        ..Default::default()
    }
}
fn import(project: &Path, source: &Path) -> RunRecord {
    application()
        .start_import(
            project,
            vec![app::ImportInput {
                role: "vendor".into(),
                path: source.into(),
                expected: None,
            }],
            Target::Riscv32Ilp32,
            budget(),
        )
        .unwrap()
        .wait()
}
fn query(project: &Path, operation: &str, extra: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args([
            operation,
            "--limit-mode",
            "watchdog",
            "--working-memory-mib",
            "1",
            "--format",
            "json",
            "--project",
        ])
        .arg(project)
        .args(extra)
        .output()
        .unwrap()
}
fn project_bytes(root: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    fn visit(root: &Path, path: &Path, result: &mut Vec<(std::path::PathBuf, Vec<u8>)>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, result);
            } else {
                result.push((
                    path.strip_prefix(root).unwrap().into(),
                    fs::read(path).unwrap(),
                ));
            }
        }
    }
    let mut result = Vec::new();
    visit(root, root, &mut result);
    result.sort_by(|a, b| a.0.cmp(&b.0));
    result
}
#[test]
fn archive_and_saved_inventory_can_exceed_working_capacity() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    app::create_project(&project).unwrap();
    let source = temp.path().join("many.a");
    let elf = support::elf();
    let members = vec![(b"same.o".as_slice(), elf.as_slice()); 3000];
    let archive = support::archive(&members, false);
    assert!(archive.len() > 1024 * 1024);
    fs::write(&source, archive).unwrap();
    let record = import(&project, &source);
    assert_eq!(record.state, RunState::Completed, "{:?}", record.error);
    let memory = record
        .diagnostics
        .unwrap()
        .progress
        .unwrap()
        .working_memory
        .unwrap();
    assert!(memory.peak_reserved_bytes <= 1024 * 1024);
    assert_eq!(memory.reserved_bytes, 0);
    fs::remove_file(source).unwrap();
    let before = project_bytes(&project);
    let result = query(&project, "inventory", &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stdout.len() > 1024 * 1024);
    let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(
        value["snapshot"]["revision"]["inputs"][0]["inventory"]["objects"]
            .as_array()
            .unwrap()
            .len(),
        3000
    );
    let doctor = query(&project, "doctor", &[]);
    assert!(
        doctor.status.success(),
        "{}",
        String::from_utf8_lossy(&doctor.stderr)
    );
    let doctor: serde_json::Value = serde_json::from_slice(&doctor.stdout).unwrap();
    assert_eq!(doctor["checked_revisions"], 1);
    assert_eq!(before, project_bytes(&project));
}
#[test]
fn thin_payloads_are_released_between_occurrences() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    app::create_project(&project).unwrap();
    let member = temp.path().join("large.o");
    let mut elf = support::elf();
    elf.resize(128 * 1024, 0);
    fs::write(&member, &elf).unwrap();
    let source = temp.path().join("thin.a");
    fs::write(
        &source,
        support::archive(&vec![(b"large.o".as_slice(), elf.as_slice()); 32], true),
    )
    .unwrap();
    let result = import(&project, &source);
    assert_eq!(result.state, RunState::Completed, "{:?}", result.error);
    fs::remove_file(member).unwrap();
    fs::remove_file(source).unwrap();
    let snapshot = app::inventory(&project, None).unwrap();
    assert_eq!(snapshot.revision.inputs[0].external_members.len(), 32);
    assert!(query(&project, "inventory", &[]).status.success());
}
#[test]
fn exhausted_object_buffer_is_typed_and_never_replaces_current() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    app::create_project(&project).unwrap();
    let source = temp.path().join("object.o");
    fs::write(&source, support::elf()).unwrap();
    let first = import(&project, &source);
    assert_eq!(first.state, RunState::Completed);
    let before = app::inventory(&project, None).unwrap();
    let mut bytes = support::elf();
    bytes.resize(2 * 1024 * 1024, 0);
    fs::write(&source, bytes).unwrap();
    let result = import(&project, &source);
    assert_eq!(result.state, RunState::ResourceLimited);
    let error = result.error.unwrap();
    assert_eq!(error.code, ErrorCode::ResourceLimited);
    let memory = error.memory.unwrap();
    assert_eq!(memory.requested_bytes, 2 * 1024 * 1024);
    assert!(memory.available_bytes < memory.requested_bytes);
    assert_eq!(app::inventory(&project, None).unwrap(), before);
    assert!(
        fs::read_dir(project.join(".blobray-next/staging"))
            .unwrap()
            .next()
            .is_none()
    );
}
#[test]
fn read_queries_enforce_budgets_without_project_writes_or_partial_stdout() {
    let temp = tempfile::tempdir().unwrap();
    app::create_project(temp.path()).unwrap();
    let source = temp.path().join("fixture.o");
    fs::write(&source, support::elf()).unwrap();
    assert_eq!(import(temp.path(), &source).state, RunState::Completed);
    let before = project_bytes(temp.path());
    for operation in ["inventory", "doctor"] {
        let result = query(temp.path(), operation, &["--max-work-units", "1"]);
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        let value: serde_json::Value = serde_json::from_slice(&result.stderr).unwrap();
        assert_eq!(value["query"]["state"], "resource-limited");
        assert!(value["query"]["diagnostics"]["progress"].is_object());
    }
    assert_eq!(before, project_bytes(temp.path()));
}

#[test]
fn one_large_symbol_table_streams_records_under_the_same_capacity() {
    use object::{
        Architecture, BinaryFormat, Endianness, SymbolFlags, SymbolKind, SymbolScope,
        write::{Object, Symbol, SymbolSection},
    };
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    app::create_project(&project).unwrap();
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    for index in 0..5000 {
        object.add_symbol(Symbol {
            name: format!("symbol_{index}").into_bytes(),
            value: 0,
            size: 0,
            kind: SymbolKind::Unknown,
            scope: SymbolScope::Linkage,
            weak: false,
            section: SymbolSection::Undefined,
            flags: SymbolFlags::None,
        });
    }
    let source = temp.path().join("symbols.o");
    fs::write(&source, object.write().unwrap()).unwrap();
    let result = import(&project, &source);
    assert_eq!(result.state, RunState::Completed, "{:?}", result.error);
    let output = query(&project, "inventory", &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.len() > 1024 * 1024);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["snapshot"]["revision"]["inputs"][0]["inventory"]["objects"][0]["elf"]["symbols"]
            .as_array()
            .unwrap()
            .len(),
        5001
    );
}

#[test]
fn read_query_cancellation_deadline_and_delivery_keep_project_read_only() {
    let temp = tempfile::tempdir().unwrap();
    app::create_project(temp.path()).unwrap();
    let source = temp.path().join("fixture.o");
    fs::write(&source, support::elf()).unwrap();
    assert_eq!(import(temp.path(), &source).state, RunState::Completed);
    let before = project_bytes(temp.path());
    let host = || Arc::new(LinuxHost::new(env!("CARGO_BIN_EXE_blobray").into(), None));
    for (cancelled, timeout, expected) in [
        (true, 900_000, RunState::Cancelled),
        (false, 1, RunState::TimedOut),
    ] {
        let application = app::Application::new(host());
        let handle = application
            .start_query(
                temp.path(),
                ReadQuery::Inventory { revision: None },
                ResourceBudget {
                    timeout_ms: timeout,
                    ..budget()
                },
            )
            .unwrap();
        if cancelled {
            assert!(handle.cancel());
        }
        let report = handle.wait();
        assert_eq!(report.state, expected, "{report:?}");
    }
    let application = app::Application::new(host());
    let mut result = application
        .query(
            temp.path(),
            ReadQuery::Inventory { revision: None },
            budget(),
        )
        .unwrap();
    assert_eq!(result.report.state, RunState::Completed);
    let mut called = false;
    let error = result
        .manifest(&|| true, |_, _, _| {
            called = true;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Cancelled);
    assert!(!called);
    assert_eq!(
        result
            .manifest(&|| false, |_, _, _| Ok(()))
            .unwrap_err()
            .code,
        ErrorCode::InvalidRequest
    );
    assert_eq!(before, project_bytes(temp.path()));
}

#[test]
fn partial_inventory_has_the_same_assessment_in_handle_and_output() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    app::create_project(&project).unwrap();
    let source = temp.path().join("mixed.a");
    fs::write(
        &source,
        support::archive(
            &[(b"valid.o", &support::elf()), (b"unknown", b"unsupported")],
            false,
        ),
    )
    .unwrap();
    let imported = import(&project, &source);
    assert_eq!(imported.state, RunState::Completed);
    assert_eq!(
        imported
            .assessment
            .as_ref()
            .unwrap()
            .coverage
            .as_ref()
            .unwrap()
            .status,
        CoverageStatus::Partial
    );
    let app = application();
    let handle = app
        .start_query(&project, ReadQuery::Inventory { revision: None }, budget())
        .unwrap();
    let run = handle.wait();
    assert_eq!(run.state, RunState::Completed, "{:?}", run.error);
    let output = handle.take_output().unwrap();
    assert_eq!(run.assessment.as_ref(), Some(output.assessment()));
    assert_eq!(run.assessment, imported.assessment);
    assert!(matches!(
        output.summary(),
        app::QuerySummary::Inventory {
            complete: false,
            ..
        }
    ));
    let value: serde_json::Value =
        serde_json::from_slice(&query(&project, "inventory", &[]).stdout).unwrap();
    assert_eq!(value["assessment"]["coverage"]["status"], "partial");
    assert_eq!(
        value["assessment"]["coverage"]["subject"]["id"],
        imported.revision.unwrap().as_str()
    );
}
