#![cfg(target_os = "linux")]
mod linked;
mod support;
use blobray_application as app;
use blobray_domain::*;
use blobray_next_host::linux::LinuxHost;
use object::{
    Architecture, BinaryFormat, Endianness, RelocationFlags, SectionKind, SymbolFlags, SymbolKind,
    SymbolScope,
    write::{Object, Relocation, Symbol, SymbolSection},
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};
fn linker() -> PathBuf {
    std::env::var_os("BLOBRAY_TEST_LLD")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/usr/bin/ld.lld".into())
}
fn budget() -> ResourceBudget {
    ResourceBudget {
        mode: LimitMode::Watchdog,
        working_memory_bytes: Some(32 * 1024 * 1024),
        timeout_ms: 30000,
        poll_ms: 5,
        grace_ms: 100,
        ..Default::default()
    }
}
fn application(root: &Path) -> app::Application {
    app::Application::with_temporary_storage(
        Arc::new(LinuxHost::new(
            env!("CARGO_BIN_EXE_blobray-next").into(),
            None,
        )),
        app::ApplicationLimits::default(),
        app::TemporaryStoragePolicy {
            root: Some(root.into()),
            ..Default::default()
        },
    )
    .unwrap()
}
fn symbol(name: &[u8], section: SymbolSection, size: u64, kind: SymbolKind) -> Symbol {
    Symbol {
        name: name.into(),
        value: 0,
        size,
        kind,
        scope: SymbolScope::Linkage,
        weak: false,
        section,
        flags: SymbolFlags::None,
    }
}
fn object(entry: bool) -> Vec<u8> {
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = obj.add_section(
        Vec::new(),
        if entry {
            b".text.entry".to_vec()
        } else {
            b".text.helper".to_vec()
        },
        SectionKind::Text,
    );
    if entry {
        obj.append_section_data(
            text,
            &[
                0x97, 0, 0, 0, 0xe7, 0x80, 0, 0, 0x67, 0x80, 0, 0, 0, 0, 0, 0,
            ],
            4,
        );
        obj.add_symbol(symbol(
            b"entry",
            SymbolSection::Section(text),
            16,
            SymbolKind::Text,
        ));
        let helper = obj.add_symbol(symbol(
            b"helper",
            SymbolSection::Undefined,
            0,
            SymbolKind::Unknown,
        ));
        let data = obj.add_symbol(symbol(
            b"value",
            SymbolSection::Undefined,
            0,
            SymbolKind::Unknown,
        ));
        obj.add_relocation(
            text,
            Relocation {
                offset: 0,
                symbol: helper,
                addend: 0,
                flags: RelocationFlags::Elf {
                    r_type: object::elf::R_RISCV_CALL,
                },
            },
        )
        .unwrap();
        obj.add_relocation(
            text,
            Relocation {
                offset: 12,
                symbol: data,
                addend: 0,
                flags: RelocationFlags::Elf {
                    r_type: object::elf::R_RISCV_32,
                },
            },
        )
        .unwrap();
    } else {
        obj.append_section_data(text, &[0x67, 0x80, 0, 0], 4);
        obj.add_symbol(symbol(
            b"helper",
            SymbolSection::Section(text),
            4,
            SymbolKind::Text,
        ));
        let data = obj.add_section(Vec::new(), b".data".to_vec(), SectionKind::Data);
        obj.append_section_data(data, &[0x78, 0x56, 0x34, 0x12], 4);
        obj.add_symbol(symbol(
            b"value",
            SymbolSection::Section(data),
            4,
            SymbolKind::Data,
        ));
    }
    obj.write().unwrap()
}
struct Fixture {
    dir: tempfile::TempDir,
    project: PathBuf,
    app: app::Application,
    request: LinkRequest,
    revision: RevisionId,
}
fn fixture(thin: bool, dependency: bool) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    app::create_project(&project).unwrap();
    let app = application(&dir.path().join("runtime"));
    let root = dir.path().join("entry.a");
    let entry = object(true);
    let helper = object(false);
    if thin {
        fs::write(dir.path().join("entry.o"), &entry).unwrap();
    }
    fs::write(&root, support::archive(&[(b"entry.o", &entry)], thin)).unwrap();
    let mut inputs = vec![app::ImportInput {
        role: "root".into(),
        path: root,
        expected: None,
    }];
    if dependency {
        let path = dir.path().join("helper.a");
        fs::write(&path, support::archive(&[(b"helper.o", &helper)], false)).unwrap();
        inputs.push(app::ImportInput {
            role: "companion".into(),
            path,
            expected: None,
        });
    }
    app.import(&project, inputs, Target::Riscv32Ilp32, budget())
        .unwrap();
    let snapshot = app::inventory(&project, None).unwrap();
    let symbol = snapshot.revision.inputs[0]
        .inventory
        .as_ref()
        .unwrap()
        .objects[0]
        .elf
        .as_ref()
        .unwrap()
        .symbols
        .iter()
        .find(|s| s.name.as_deref() == Some(b"entry"))
        .unwrap()
        .id
        .clone();
    let revision = snapshot.revision_id;
    let request = LinkRequest {
        companions: Vec::new(),
        revision: Some(revision.clone()),
        inputs: if dependency { vec![0, 1] } else { vec![0] },
        entry: EntrySelection { input: 0, symbol },
        roots: Vec::new(),
        layout: ImageLayout {
            code: ImageRegion {
                start: 0x10000000,
                length: 65536,
            },
            data: ImageRegion {
                start: 0x20000000,
                length: 65536,
            },
        },
    };
    Fixture {
        dir,
        project,
        app,
        request,
        revision,
    }
}
#[test]
fn real_lld_publishes_closed_image_and_reopens_after_source_deletion() {
    let f = fixture(true, true);
    let _unused = support::elf();
    let plan = f
        .app
        .link_plan(&f.project, f.request.clone(), &linker(), budget())
        .unwrap();
    assert!(
        plan.description().ready(),
        "{:?}",
        plan.description().blockers
    );
    let mut saved = Vec::new();
    plan.write(&mut saved, &|| false).unwrap();
    let description = app::read_link_plan(saved.as_slice()).unwrap();
    for name in ["entry.a", "entry.o", "helper.a"] {
        fs::remove_file(f.dir.path().join(name)).unwrap();
    }
    let run = f
        .app
        .start_prepare_image(&f.project, &description, &linker(), budget())
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::Completed, "{:?}", run.error);
    let id = run.image.unwrap();
    assert_eq!(
        app::inventory(&f.project, None).unwrap().revision_id,
        f.revision
    );
    let mut output = f
        .app
        .query(
            &f.project,
            app::ReadQuery::Image {
                id: id.clone(),
                export: true,
            },
            budget(),
        )
        .unwrap();
    let export = f.dir.path().join("export");
    output.export_image(&export, &|| false).unwrap();
    let bytes = fs::read(export.join("image.elf")).unwrap();
    assert_eq!(&bytes[..4], b"\x7fELF");
    assert!(bytes.windows(4).any(|b| b == [0x78, 0x56, 0x34, 0x12]));
    let manifest: ImageManifest =
        serde_json::from_slice(&fs::read(export.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest.entry, 0x10000000);
    assert_eq!(manifest.plan, description);
    assert_eq!(manifest.roots[0].selection, f.request.entry);
    assert!(
        String::from_utf8(fs::read(export.join("extraction.tsv")).unwrap())
            .unwrap()
            .contains("i1-m0.o")
    );
    assert!(matches!(
        f.app
            .query(&f.project, app::ReadQuery::Doctor, budget())
            .unwrap()
            .summary(),
        app::QuerySummary::Doctor {
            checked_images: 1,
            errors: 0,
            ..
        }
    ));
    let second = f
        .app
        .start_prepare_image(&f.project, &description, &linker(), budget())
        .unwrap()
        .wait();
    assert_eq!(second.image, Some(id.clone()), "{:?}", second.error);
    drop(output);
    drop(plan);
    f.app.shutdown();
    let moved = f.dir.path().join("moved");
    fs::rename(&f.project, &moved).unwrap();
    let opened = application(&f.dir.path().join("runtime2"))
        .query(
            &moved,
            app::ReadQuery::Image { id, export: false },
            budget(),
        )
        .unwrap();
    assert!(
        matches!(opened.summary(),app::QuerySummary::Image{manifest,..} if manifest.elf.as_str()==ArtifactId::of_bytes(&bytes).as_str())
    );
}
#[test]
fn unresolved_link_preserves_revision_and_does_not_publish() {
    let f = fixture(false, false);
    let plan = f
        .app
        .link_plan(&f.project, f.request, &linker(), budget())
        .unwrap();
    assert!(
        plan.description().ready(),
        "{:?}",
        plan.description().blockers
    );
    let record = f
        .app
        .start_prepare_image(&f.project, plan.description(), &linker(), budget())
        .unwrap()
        .wait();
    assert_eq!(record.state, RunState::Failed);
    assert_eq!(record.error.unwrap().code, ErrorCode::LinkFailed);
    assert!(record.image.is_none());
    assert_eq!(
        app::inventory(&f.project, None).unwrap().revision_id,
        f.revision
    );
    let list = f
        .app
        .query(&f.project, app::ReadQuery::Images, budget())
        .unwrap();
    assert!(matches!(
        list.summary(),
        app::QuerySummary::Images { count: 0 }
    ));
}
#[test]
fn cli_plan_prepare_inspect_and_export_use_the_same_application_contract() {
    let f = fixture(false, true);
    let request = f.dir.path().join("request.json");
    let plan = f.dir.path().join("plan.json");
    fs::write(&request, serde_json::to_vec(&f.request).unwrap()).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_blobray-next"));
    command
        .args(["link-plan", "--project"])
        .arg(&f.project)
        .arg("--request")
        .arg(&request)
        .arg("--output")
        .arg(&plan)
        .arg("--linker")
        .arg(linker())
        .args(["--limit-mode", "watchdog"]);
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = Command::new(env!("CARGO_BIN_EXE_blobray-next"))
        .args(["prepare-image", "--project"])
        .arg(&f.project)
        .arg("--plan")
        .arg(&plan)
        .arg("--linker")
        .arg(linker())
        .args(["--limit-mode", "watchdog", "--format", "json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let record: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let id = record["run"]["image"].as_str().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_blobray-next"))
        .args(["image", "--project"])
        .arg(&f.project)
        .args(["--id", id, "--limit-mode", "watchdog", "--format", "json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let export = f.dir.path().join("cli-export");
    let output = Command::new(env!("CARGO_BIN_EXE_blobray-next"))
        .args(["export-image", "--project"])
        .arg(&f.project)
        .args(["--id", id, "--limit-mode", "watchdog"])
        .arg("--output")
        .arg(&export)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(export.join("manifest.json").exists());
}

fn custom_fixture(inputs: Vec<(&str, Vec<u8>)>, entry_input: usize, member: usize) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    app::create_project(&project).unwrap();
    let app = application(&dir.path().join("runtime"));
    let count = inputs.len();
    let inputs = inputs
        .into_iter()
        .map(|(name, bytes)| {
            let path = dir.path().join(name);
            fs::write(&path, bytes).unwrap();
            app::ImportInput {
                role: "code".into(),
                path,
                expected: None,
            }
        })
        .collect();
    app.import(&project, inputs, Target::Riscv32Ilp32, budget())
        .unwrap();
    let snapshot = app::inventory(&project, None).unwrap();
    let symbol = snapshot.revision.inputs[entry_input]
        .inventory
        .as_ref()
        .unwrap()
        .objects[member]
        .elf
        .as_ref()
        .unwrap()
        .symbols
        .iter()
        .find(|s| s.name.as_deref() == Some(b"entry"))
        .unwrap()
        .id
        .clone();
    let revision = snapshot.revision_id;
    let request = LinkRequest {
        companions: Vec::new(),
        revision: Some(revision.clone()),
        inputs: (0..count as u64).collect(),
        entry: EntrySelection {
            input: entry_input as u64,
            symbol,
        },
        roots: vec![],
        layout: ImageLayout {
            code: ImageRegion {
                start: 0x10000000,
                length: 65536,
            },
            data: ImageRegion {
                start: 0x20000000,
                length: 65536,
            },
        },
    };
    Fixture {
        dir,
        project,
        app,
        request,
        revision,
    }
}
fn prepare(f: &Fixture) -> app::RunRecord {
    let plan = f
        .app
        .link_plan(&f.project, f.request.clone(), &linker(), budget())
        .unwrap();
    assert!(
        plan.description().ready(),
        "{:?}",
        plan.description().blockers
    );
    f.app
        .start_prepare_image(&f.project, plan.description(), &linker(), budget())
        .unwrap()
        .wait()
}
fn assert_unpublished(f: &Fixture) {
    assert_eq!(
        app::inventory(&f.project, None).unwrap().revision_id,
        f.revision
    );
    assert!(matches!(
        f.app
            .query(&f.project, app::ReadQuery::Images, budget())
            .unwrap()
            .summary(),
        app::QuerySummary::Images { count: 0 }
    ));
}
#[test]
fn duplicate_names_and_repeated_bindings_preserve_exact_occurrences() {
    let entry = object(true);
    let archive = support::archive(&[(b"same.o", &entry), (b"same.o", &entry)], false);
    let f = custom_fixture(
        vec![
            ("first.a", archive.clone()),
            ("second.a", archive),
            ("data.o", object(false)),
        ],
        1,
        1,
    );
    let run = prepare(&f);
    assert_eq!(run.state, RunState::Completed, "{:?}", run.error);
    let output = f
        .app
        .query(
            &f.project,
            app::ReadQuery::Image {
                id: run.image.unwrap(),
                export: false,
            },
            budget(),
        )
        .unwrap();
    assert!(
        matches!(output.summary(), app::QuerySummary::Image { manifest, .. } if manifest.roots[0].selection == f.request.entry)
    );
}
#[test]
fn standalone_objects_and_explicit_additional_root_are_retained() {
    let mut f = custom_fixture(
        vec![("entry.o", object(true)), ("data.o", object(false))],
        0,
        0,
    );
    let inventory = app::inventory(&f.project, None).unwrap();
    let helper = inventory.revision.inputs[1]
        .inventory
        .as_ref()
        .unwrap()
        .objects[0]
        .elf
        .as_ref()
        .unwrap()
        .symbols
        .iter()
        .find(|s| s.name.as_deref() == Some(b"helper"))
        .unwrap()
        .id
        .clone();
    f.request.roots.push(EntrySelection {
        input: 1,
        symbol: helper,
    });
    let run = prepare(&f);
    assert_eq!(run.state, RunState::Completed, "{:?}", run.error);
    let output = f
        .app
        .query(
            &f.project,
            app::ReadQuery::Image {
                id: run.image.unwrap(),
                export: false,
            },
            budget(),
        )
        .unwrap();
    assert!(
        matches!(output.summary(), app::QuerySummary::Image { manifest, .. } if manifest.roots.len() == 2)
    );
}
#[test]
fn conflicting_strong_objects_fail_without_publication() {
    let f = custom_fixture(
        vec![
            ("entry.o", object(true)),
            ("one.o", object(false)),
            ("two.o", object(false)),
        ],
        0,
        0,
    );
    let run = prepare(&f);
    assert_eq!(run.error.unwrap().code, ErrorCode::LinkFailed);
    assert_unpublished(&f);
}
fn test_linker(directory: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = directory.join("test-linker");
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'LLD 22.1.8'; exit 0; fi\n{body}\n"
        ),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}
#[test]
fn changed_tool_and_working_capacity_fail_as_typed_runs() {
    let f = fixture(false, true);
    let plan = f
        .app
        .link_plan(&f.project, f.request.clone(), &linker(), budget())
        .unwrap();
    let replacement = test_linker(f.dir.path(), "exit 0");
    let run = f
        .app
        .start_prepare_image(&f.project, plan.description(), &replacement, budget())
        .unwrap()
        .wait();
    assert_eq!(run.error.unwrap().code, ErrorCode::SourceChanged);
    let mut small = budget();
    small.working_memory_bytes = Some(1024 * 1024);
    let run = f
        .app
        .start_prepare_image(&f.project, plan.description(), &linker(), small)
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::ResourceLimited);
    assert!(run.error.unwrap().memory.is_some());
    assert_unpublished(&f);
}
#[test]
fn streaming_linker_output_obeys_disk_quota() {
    let f = fixture(false, true);
    let tool = test_linker(f.dir.path(), "exec /usr/bin/head -c 4194304 /dev/zero");
    let plan = f
        .app
        .link_plan(&f.project, f.request.clone(), &tool, budget())
        .unwrap();
    let limited = app::Application::with_temporary_storage(
        Arc::new(LinuxHost::new(
            env!("CARGO_BIN_EXE_blobray-next").into(),
            None,
        )),
        app::ApplicationLimits::default(),
        app::TemporaryStoragePolicy {
            root: Some(f.dir.path().join("limited")),
            operation_bytes: 2 * TEMPORARY_CONTROL_BYTES,
            total_bytes: 4 * TEMPORARY_CONTROL_BYTES,
        },
    )
    .unwrap();
    let run = limited
        .start_prepare_image(&f.project, plan.description(), &tool, budget())
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::ResourceLimited, "{:?}", run.error);
    assert!(run.error.unwrap().storage.is_some());
    assert_eq!(limited.temporary_storage_status().reserved_bytes, 0);
    assert_unpublished(&f);
}
#[test]
fn image_cancellation_and_deadline_reap_linker_and_preserve_revision() {
    use std::time::{Duration, Instant};
    let f = fixture(false, true);
    let tool = test_linker(f.dir.path(), "exec /bin/sleep 30");
    let plan = f
        .app
        .link_plan(&f.project, f.request.clone(), &tool, budget())
        .unwrap();
    let job = f
        .app
        .start_prepare_image(&f.project, plan.description(), &tool, budget())
        .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while job.status().state == RunState::Registered && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(job.cancel());
    assert_eq!(job.wait().state, RunState::Cancelled);
    let mut short = budget();
    short.timeout_ms = 250;
    let run = f
        .app
        .start_prepare_image(&f.project, plan.description(), &tool, short)
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::TimedOut, "{:?}", run.error);
    assert_unpublished(&f);
}
#[test]
fn export_refuses_existing_directory_without_touching_it() {
    let f = fixture(false, true);
    let run = prepare(&f);
    let mut output = f
        .app
        .query(
            &f.project,
            app::ReadQuery::Image {
                id: run.image.unwrap(),
                export: true,
            },
            budget(),
        )
        .unwrap();
    let destination = f.dir.path().join("existing");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("keep"), b"original").unwrap();
    assert!(output.export_image(&destination, &|| false).is_err());
    assert_eq!(fs::read(destination.join("keep")).unwrap(), b"original");
    assert_eq!(fs::read_dir(destination).unwrap().count(), 1);
}

#[test]
fn killed_image_coordinator_is_abandoned_without_publication() {
    use std::{
        process::Stdio,
        time::{Duration, Instant},
    };
    let f = fixture(false, true);
    let marker = f.dir.path().join("linking");
    let tool = test_linker(
        f.dir.path(),
        &format!("echo ready > '{}'; exec /bin/sleep 30", marker.display()),
    );
    let plan = f
        .app
        .link_plan(&f.project, f.request.clone(), &tool, budget())
        .unwrap();
    let path = f.dir.path().join("plan.json");
    fs::write(&path, serde_json::to_vec(plan.description()).unwrap()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_blobray-next"))
        .args(["prepare-image", "--project"])
        .arg(&f.project)
        .arg("--plan")
        .arg(&path)
        .arg("--linker")
        .arg(&tool)
        .args(["--limit-mode", "watchdog"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while !marker.exists() {
        assert!(Instant::now() < until, "linker never started");
        std::thread::sleep(Duration::from_millis(10));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    let recovered = loop {
        match f.app.recover(&f.project) {
            Ok(runs) => break runs,
            Err(e) if e.code == ErrorCode::Busy && Instant::now() < until => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(e) => panic!("{e}"),
        }
    };
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].state, RunState::Abandoned);
    assert!(recovered[0].image.is_none());
    assert!(f.app.recover(&f.project).unwrap().is_empty());
    assert_unpublished(&f);
}
#[test]
fn bad_link_output_and_corrupt_retained_payload_fail_closed() {
    let f = fixture(false, true);
    let tool = test_linker(f.dir.path(), "printf 'not an ELF'");
    let plan = f
        .app
        .link_plan(&f.project, f.request.clone(), &tool, budget())
        .unwrap();
    let run = f
        .app
        .start_prepare_image(&f.project, plan.description(), &tool, budget())
        .unwrap()
        .wait();
    assert_eq!(run.error.unwrap().code, ErrorCode::LinkBlocked);
    assert_unpublished(&f);
    let run = prepare(&f);
    let id = run.image.unwrap();
    let output = f
        .app
        .query(
            &f.project,
            app::ReadQuery::Image {
                id: id.clone(),
                export: false,
            },
            budget(),
        )
        .unwrap();
    let app::QuerySummary::Image { manifest, .. } = output.summary() else {
        panic!()
    };
    fs::write(
        f.project
            .join(".blobray-next/objects")
            .join(manifest.elf.as_str()),
        b"corrupt",
    )
    .unwrap();
    assert_eq!(
        f.app
            .query(
                &f.project,
                app::ReadQuery::Image { id, export: false },
                budget()
            )
            .err()
            .unwrap()
            .code,
        ErrorCode::Integrity
    );
    assert!(
        matches!(f.app.query(&f.project,app::ReadQuery::Doctor,budget()).unwrap().summary(),app::QuerySummary::Doctor{errors,..} if *errors>0)
    );
}
#[test]
fn unsupported_abi_and_map_line_injection_are_plan_blockers() {
    for mutation in [0, 1] {
        let mut helper = object(false);
        if mutation == 0 {
            helper[36..40].copy_from_slice(&object::elf::EF_RISCV_RVE.to_le_bytes());
        } else {
            let offset = helper.windows(6).position(|s| s == b"helper").unwrap();
            helper[offset] = b'\n';
        }
        let f = custom_fixture(vec![("entry.o", object(true)), ("bad.o", helper)], 0, 0);
        let plan = f
            .app
            .link_plan(&f.project, f.request.clone(), &linker(), budget())
            .unwrap();
        assert!(!plan.description().ready());
        assert_eq!(
            f.app
                .start_prepare_image(&f.project, plan.description(), &linker(), budget())
                .err()
                .unwrap()
                .code,
            ErrorCode::LinkBlocked
        );
        assert_unpublished(&f);
    }
}

fn helper_with_common(weak: bool) -> Vec<u8> {
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = obj.add_section(Vec::new(), b".text.helper".to_vec(), SectionKind::Text);
    obj.append_section_data(text, &[0x67, 0x80, 0, 0], 4);
    let mut helper = symbol(b"helper", SymbolSection::Section(text), 4, SymbolKind::Text);
    helper.weak = weak;
    obj.add_symbol(helper);
    let mut data = symbol(b"value", SymbolSection::Common, 4, SymbolKind::Data);
    data.value = 4;
    obj.add_symbol(data);
    obj.write().unwrap()
}
#[test]
fn weak_definitions_and_common_data_link_under_declared_policy() {
    let f = custom_fixture(
        vec![
            ("entry.o", object(true)),
            (
                "weak.a",
                support::archive(&[(b"helper.o", &helper_with_common(true))], false),
            ),
        ],
        0,
        0,
    );
    let run = prepare(&f);
    assert_eq!(run.state, RunState::Completed, "{:?}", run.error);
    let result = f
        .app
        .query(
            &f.project,
            app::ReadQuery::Image {
                id: run.image.unwrap(),
                export: false,
            },
            budget(),
        )
        .unwrap();
    assert!(
        matches!(result.summary(),app::QuerySummary::Image{manifest,..} if manifest.segments.iter().any(|s|s.memory_size>s.file_size))
    );
}
#[test]
fn missing_thin_member_blocks_even_when_entry_is_available() {
    let entry = object(true);
    let helper = object(false);
    let f = custom_fixture(
        vec![
            ("entry.o", entry),
            (
                "missing.a",
                support::archive(&[(b"absent.o", &helper)], true),
            ),
        ],
        0,
        0,
    );
    let plan = f
        .app
        .link_plan(&f.project, f.request.clone(), &linker(), budget())
        .unwrap();
    assert!(!plan.description().ready());
    assert_unpublished(&f);
}

#[test]
fn linker_stderr_flood_is_bounded_and_its_exit_is_distinct_from_worker_exit() {
    let f = fixture(false, true);
    let tool = test_linker(
        f.dir.path(),
        "/usr/bin/head -c 131072 /dev/zero >&2; exit 42",
    );
    let plan = f
        .app
        .link_plan(&f.project, f.request.clone(), &tool, budget())
        .unwrap();
    let run = f
        .app
        .start_prepare_image(&f.project, plan.description(), &tool, budget())
        .unwrap()
        .wait();
    assert_eq!(run.state, RunState::Failed);
    let diagnostics = run.diagnostics.unwrap();
    let tool = diagnostics.linker.unwrap();
    assert_eq!(tool.exit_code, Some(42));
    assert_eq!(tool.stderr_tail.len(), 8192);
    assert!(tool.stderr_truncated);
    assert_eq!(diagnostics.exit.unwrap().code, Some(0));
    assert_unpublished(&f);
}
