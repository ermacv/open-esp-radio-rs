mod support;

use blobray_application::{self as app, ImportInput};
use blobray_domain::*;
use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn cli(project: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_blobray"));
    if args
        .first()
        .is_some_and(|arg| matches!(*arg, "import" | "inventory" | "doctor"))
    {
        command
            .args(["--format", "json", args[0], "--limit-mode", "watchdog"])
            .args(&args[1..]);
    } else {
        command.args(args);
    }
    command
        .arg("--project")
        .arg(project)
        .args(["--format", "json"])
        .output()
        .unwrap()
}

fn json(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn input(path: &Path) -> ImportInput {
    ImportInput {
        role: "vendor".into(),
        path: path.into(),
        expected: None,
    }
}

#[test]
fn processes_reopen_identical_inventory_without_sources_and_after_project_move() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("lib.a");
    let project = temp.path().join("project");
    let elf = support::elf();
    fs::write(
        &source,
        support::archive(&[(b"dup.o", &elf), (b"dup.o", &elf)], false),
    )
    .unwrap();
    json(cli(&project, &["init"]));
    let first = json(cli(
        &project,
        &[
            "import",
            "--input",
            &format!("vendor={}", source.display()),
            "--input",
            &format!("vendor={}", source.display()),
        ],
    ));
    assert_eq!(first["run"]["assessment"]["coverage"]["status"], "complete");
    let first = json(cli(&project, &["inventory"]));
    let api = app::inventory(&project, None).unwrap();
    assert_eq!(first["snapshot"], serde_json::to_value(&api).unwrap());
    assert_eq!(api.revision.inputs.len(), 2);
    let objects = &api.revision.inputs[0].inventory.as_ref().unwrap().objects;
    assert_eq!(objects.len(), 2);
    assert_eq!(objects[0].name, objects[1].name);
    assert_eq!(objects[0].content, objects[1].content);
    assert_ne!(objects[0].id, objects[1].id);
    let mut wrong_owner = api.revision.clone();
    wrong_owner.inputs[0].inventory.as_mut().unwrap().objects[0]
        .id
        .artifact = ArtifactId::of_bytes(b"wrong owner");
    assert_eq!(
        wrong_owner.validate().unwrap_err().code,
        ErrorCode::Integrity
    );
    let a = objects[0].elf.as_ref().unwrap();
    let b = objects[1].elf.as_ref().unwrap();
    assert_ne!(a.symbols[0].id, b.symbols[0].id);
    assert_eq!(a.symbols[0].id.index, 0); // The null table entry is retained.
    assert!(
        a.symbols
            .iter()
            .any(|s| s.name.as_deref() == Some(b"local\xff") && s.binding == 0)
    );
    assert!(
        a.symbols
            .iter()
            .any(|s| s.name.as_deref() == Some(b"weak") && s.binding == 2)
    );
    assert!(
        a.symbols
            .iter()
            .any(|s| s.name.as_deref() == Some(b"external") && s.raw_section == 0)
    );
    assert!(
        a.symbols
            .iter()
            .any(|s| s.name.as_deref() == Some(b"common")
                && s.raw_section == object::elf::SHN_COMMON)
    );
    assert_eq!(a.relocations.len(), 1);
    let relocation = &a.relocations[0];
    assert_eq!(relocation.addend, Some(7));
    assert!(
        a.symbols
            .iter()
            .any(|s| s.id.index == u64::from(relocation.symbol_index)
                && s.id.table_section == relocation.symbol_table_section
                && s.name.as_deref() == Some(b"external"))
    );
    fs::remove_file(&source).unwrap();
    assert_eq!(first, json(cli(&project, &["inventory"])));
    let moved = temp.path().join("moved");
    fs::rename(project, &moved).unwrap();
    assert_eq!(first, json(cli(&moved, &["inventory"])));
}

#[test]
fn thin_members_are_captured_by_occurrence_and_missing_members_stay_visible() {
    let temp = tempfile::tempdir().unwrap();
    let sources = temp.path().join("sources");
    fs::create_dir(&sources).unwrap();
    let elf = support::elf();
    fs::write(sources.join("present.o"), &elf).unwrap();
    let source = sources.join("thin.a");
    fs::write(
        &source,
        support::archive(
            &[
                (b"present.o", &elf),
                (b"missing.o", &elf),
                (b"present.o", &elf),
            ],
            true,
        ),
    )
    .unwrap();
    let project = temp.path().join("project");
    app::create_project(&project).unwrap();
    let first = app_import(&project, vec![input(&source)], Target::Riscv32Ilp32).unwrap();
    assert!(!first.revision.complete());
    let record = &first.revision.inputs[0];
    assert_eq!(record.external_members.len(), 3);
    assert!(matches!(
        record.external_members[1].capture,
        Capture::Unavailable { .. }
    ));
    let objects = &record.inventory.as_ref().unwrap().objects;
    assert_eq!(objects.len(), 3);
    assert!(objects[0].elf.is_some() && objects[2].elf.is_some());
    assert_eq!(
        objects[1].diagnostics[0].code,
        DiagnosticCode::MissingMember
    );
    assert_ne!(objects[0].id, objects[2].id);
    // Same thin container, changed external bytes: a distinct captured binding.
    fs::write(sources.join("present.o"), b"replacement").unwrap();
    let second = app_import(&project, vec![input(&source)], Target::Riscv32Ilp32).unwrap();
    assert_eq!(
        first.revision.inputs[0].capture,
        second.revision.inputs[0].capture
    );
    assert_ne!(
        first.revision.inputs[0].external_members[0].capture,
        second.revision.inputs[0].external_members[0].capture
    );
    fs::remove_dir_all(sources).unwrap();
    assert_eq!(
        first,
        app::inventory(&project, Some(&first.revision_id)).unwrap()
    );
    assert_eq!(second, app::inventory(&project, None).unwrap());
}

#[test]
fn mixed_payloads_and_broken_membership_report_partial_coverage() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    app::create_project(&project).unwrap();
    let source = temp.path().join("mixed.a");
    let elf = support::elf();
    let mut bytes = support::archive(
        &[
            (b"good.o", &elf),
            (b"bad\xff.o", b"\x7fELFbroken"),
            (b"text", b"unsupported"),
        ],
        false,
    );
    bytes.extend_from_slice(b"broken next member header");
    fs::write(&source, bytes).unwrap();
    let snapshot = app_import(
        &project,
        vec![input(&source), input(&temp.path().join("missing.a"))],
        Target::Riscv32Ilp32,
    )
    .unwrap();
    assert!(!snapshot.revision.complete());
    let inventory = snapshot.revision.inputs[0].inventory.as_ref().unwrap();
    assert_eq!(inventory.objects.len(), 3);
    assert!(!inventory.members_complete);
    assert!(inventory.objects[0].elf.is_some());
    assert_eq!(
        inventory.objects[1].name.as_deref(),
        Some(b"bad\xff.o".as_slice())
    );
    assert_eq!(
        inventory.objects[1].diagnostics[0].code,
        DiagnosticCode::MalformedObject
    );
    assert_eq!(
        inventory.objects[2].diagnostics[0].code,
        DiagnosticCode::UnsupportedFormat
    );
    assert_eq!(
        inventory.diagnostics[0].code,
        DiagnosticCode::MalformedContainer
    );
    assert!(matches!(
        snapshot.revision.inputs[1].capture,
        Capture::Unavailable { .. }
    ));
    assert_eq!(snapshot, app::inventory(&project, None).unwrap());
}

#[test]
fn digest_failure_preserves_current_and_corruption_never_reimports_from_source() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let source = temp.path().join("input.o");
    fs::write(&source, support::elf()).unwrap();
    app::create_project(&project).unwrap();
    let first = app_import(&project, vec![input(&source)], Target::Riscv32Ilp32).unwrap();
    let mut wrong = input(&source);
    wrong.expected = Some(ArtifactId::of_bytes(b"wrong"));
    assert_eq!(
        app_import(&project, vec![wrong], Target::Riscv32Ilp32)
            .unwrap_err()
            .code,
        ErrorCode::DigestMismatch
    );
    assert_eq!(
        app::revisions(&project).unwrap(),
        vec![first.revision_id.clone()]
    );
    assert_eq!(app::inventory(&project, None).unwrap(), first);
    let artifact = first.revision.inputs[0].capture.artifact().unwrap();
    let retained = project
        .join(".blobray-next/objects")
        .join(artifact.as_str());
    fs::write(&retained, b"corrupt").unwrap();
    assert_eq!(
        app::inventory(&project, None).unwrap_err().code,
        ErrorCode::Integrity
    );
    assert_eq!(
        app_import(&project, vec![input(&source)], Target::Riscv32Ilp32)
            .unwrap_err()
            .code,
        ErrorCode::Integrity
    );
    assert_eq!(fs::read(retained).unwrap(), b"corrupt");
    assert_eq!(app::revisions(&project).unwrap(), vec![first.revision_id]);
}

#[test]
fn revision_selection_is_project_scoped_and_manifests_are_verified() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    let source = temp.path().join("input.o");
    fs::write(&source, support::elf()).unwrap();
    assert_eq!(
        app::inventory(&a, None).unwrap_err().code,
        ErrorCode::NotFound
    );
    assert!(!a.exists());
    app::create_project(&a).unwrap();
    app::create_project(&b).unwrap();
    let first = app_import(&a, vec![input(&source)], Target::Riscv32Ilp32).unwrap();
    assert_eq!(
        app::inventory(&b, Some(&first.revision_id))
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    fs::write(
        a.join(".blobray-next/objects")
            .join(first.revision_id.as_str()),
        b"{}",
    )
    .unwrap();
    assert_eq!(
        app::inventory(&a, None).unwrap_err().code,
        ErrorCode::Integrity
    );
}

#[test]
fn cli_error_is_machine_readable_and_does_not_publish() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    json(cli(&project, &["init"]));
    let output = cli(
        &project,
        &["import", "--input", "role=missing", "--expect", "2=bad"],
    );
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "invalid-request");
    assert!(app::revisions(&project).unwrap().is_empty());
    let output = cli(&project, &["inventory", "--revision", "invalid-id"]);
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "invalid-request");
}

#[cfg(unix)]
#[test]
fn non_utf8_source_paths_round_trip_without_loss() {
    use std::os::unix::ffi::OsStringExt;
    let temp = tempfile::tempdir().unwrap();
    let source = temp
        .path()
        .join(std::ffi::OsString::from_vec(b"input\xff.o".to_vec()));
    let project = temp.path().join("project");
    fs::write(&source, support::elf()).unwrap();
    app::create_project(&project).unwrap();
    let mut binding = std::ffi::OsString::from("vendor=");
    binding.push(&source);
    let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args([
            "import",
            "--limit-mode",
            "watchdog",
            "--format",
            "json",
            "--project",
        ])
        .arg(&project)
        .arg("--input")
        .arg(binding)
        .output()
        .unwrap();
    let result = json(output);
    let api = app::inventory(&project, None).unwrap();
    assert_eq!(
        result["run"]["revision"],
        serde_json::to_value(&api.revision_id).unwrap()
    );
    let OriginPath::UnixBytes { bytes } = &api.revision.inputs[0].origin else {
        panic!("Unix origin required")
    };
    assert!(bytes.ends_with(b"input\xff.o"));
}

fn app_import(path: &Path, inputs: Vec<ImportInput>, target: Target) -> Result<Snapshot> {
    let app = app::Application::new(std::sync::Arc::new(
        blobray_next_host::linux::LinuxHost::new(env!("CARGO_BIN_EXE_blobray").into(), None),
    ));
    let budget = ResourceBudget {
        mode: LimitMode::Watchdog,
        ..ResourceBudget::default()
    };
    let run = app.import(path, inputs, target, budget)?;
    app::inventory(path, run.revision.as_ref())
}
