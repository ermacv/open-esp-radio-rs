#![cfg(target_os = "linux")]
mod support;
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
        poll_ms: 2,
        grace_ms: 100,
        ..Default::default()
    }
}
fn import(app: &app::Application, project: &Path, paths: &[&Path]) -> RevisionId {
    app.import(
        project,
        paths
            .iter()
            .map(|path| app::ImportInput {
                role: "duplicate".into(),
                path: (*path).into(),
                expected: None,
            })
            .collect(),
        Target::Riscv32Ilp32,
        budget(),
    )
    .unwrap()
    .revision
    .unwrap()
}
#[derive(Default)]
struct Records {
    fail: bool,
    candidates: Vec<SelectionCandidate>,
    inputs: Vec<u64>,
    objects: Vec<ObjectInventory>,
    symbols: Vec<SymbolRecord>,
    sections: Vec<SectionRecord>,
    relocations: Vec<RelocationRecord>,
    diagnostics: Vec<Diagnostic>,
}
impl ElfSink for Records {
    fn section(&mut self, r: &SectionRecord, _: &mut dyn RunControl) -> Result<()> {
        self.sections.push(r.clone());
        Ok(())
    }
    fn symbol(&mut self, r: &SymbolRecord, _: &mut dyn RunControl) -> Result<()> {
        if self.fail {
            return Err(Error::new(ErrorCode::Io, "consumer closed"));
        }
        self.symbols.push(r.clone());
        Ok(())
    }
    fn relocation(&mut self, r: &RelocationRecord, _: &mut dyn RunControl) -> Result<()> {
        self.relocations.push(r.clone());
        Ok(())
    }
    fn diagnostic(&mut self, r: &Diagnostic, _: &mut dyn RunControl) -> Result<()> {
        self.diagnostics.push(r.clone());
        Ok(())
    }
}
impl InventorySink for Records {
    fn input(&mut self, n: u64, _: &InputRecord, _: &mut dyn RunControl) -> Result<()> {
        self.inputs.push(n);
        Ok(())
    }
    fn object(&mut self, r: &ObjectInventory, _: &mut dyn RunControl) -> Result<()> {
        self.objects.push(r.clone());
        Ok(())
    }
}
impl app::DoctorSink for Records {
    fn error(&mut self, _: &Error, _: &mut dyn RunControl) -> Result<()> {
        panic!("unexpected doctor error")
    }
    fn unfinished(&mut self, _: &RunId, _: &mut dyn RunControl) -> Result<()> {
        panic!("unexpected doctor record")
    }
}
impl app::QuerySink for Records {
    fn candidate(&mut self, r: &SelectionCandidate, _: &mut dyn RunControl) -> Result<()> {
        self.candidates.push(r.clone());
        Ok(())
    }
    fn summary(&mut self, _: &app::QuerySummary, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
}
fn search(
    app: &app::Application,
    project: &Path,
    kind: SelectionKind,
    name: &[u8],
    input: Option<u64>,
) -> Records {
    let mut output = app
        .query(
            project,
            app::ReadQuery::Select {
                revision: None,
                request: SelectionRequest {
                    kind,
                    name: name.into(),
                    input,
                },
            },
            budget(),
        )
        .unwrap();
    let mut records = Records::default();
    output.records(&|| false, &mut records).unwrap();
    assert!(
        matches!(output.summary(), app::QuerySummary::Selection { matches,.. } if *matches==records.candidates.len() as u64)
    );
    records
}
fn run(app: &app::Application, plan: &app::Plan) -> Records {
    let handle = app.start_run(plan).unwrap();
    let mut output = handle.take_output().unwrap();
    let mut records = Records::default();
    output.records(&|| false, &mut records).unwrap();
    records
}
fn plan(app: &app::Application, project: &Path, candidate: &SelectionCandidate) -> app::Plan {
    app.plan(
        project,
        app::PlanRequest {
            revision: Some(candidate.revision.clone()),
            scope: candidate.scope.clone(),
            budget: budget(),
        },
        budget(),
    )
    .unwrap()
}
fn project_bytes(project: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    fn visit(root: &Path, path: &Path, records: &mut Vec<(std::path::PathBuf, Vec<u8>)>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, records);
            } else {
                records.push((
                    path.strip_prefix(root).unwrap().into(),
                    fs::read(path).unwrap(),
                ));
            }
        }
    }
    let mut result = Vec::new();
    visit(project, project, &mut result);
    result.sort();
    result
}
#[test]
fn repeated_inputs_members_and_symbols_remain_independently_selectable() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let source = temp.path().join("lib.a");
    let elf = support::elf();
    fs::write(
        &source,
        support::archive(&[(b"dup.o", &elf), (b"dup.o", &elf)], false),
    )
    .unwrap();
    app::create_project(&project).unwrap();
    let application = application();
    import(&application, &project, &[&source, &source]);
    let before = project_bytes(&project);
    let objects = search(
        &application,
        &project,
        SelectionKind::Object,
        b"dup.o",
        None,
    );
    assert_eq!(objects.candidates.len(), 4);
    assert_eq!(
        search(
            &application,
            &project,
            SelectionKind::Object,
            b"dup.o",
            Some(1)
        )
        .candidates
        .len(),
        2
    );
    let symbols = search(&application, &project, SelectionKind::Symbol, b"same", None);
    assert_eq!(symbols.candidates.len(), 4);
    for candidate in &symbols.candidates {
        let plan = plan(&application, &project, candidate);
        let records = run(&application, &plan);
        assert_eq!(records.inputs, vec![candidate.scope.input().unwrap()]);
        assert_eq!(records.symbols.len(), 1);
        assert_eq!(records.sections.len(), 1);
        assert_eq!(records.objects.len(), 1);
        let InspectionScope::Symbol { symbol, .. } = &candidate.scope else {
            panic!()
        };
        assert_eq!(&records.symbols[0].id, symbol);
    }
    assert_eq!(
        search(
            &application,
            &project,
            SelectionKind::Symbol,
            b"local\xff",
            Some(0)
        )
        .candidates
        .len(),
        2
    );
    let external = search(
        &application,
        &project,
        SelectionKind::Symbol,
        b"external",
        Some(0),
    );
    let records = run(
        &application,
        &plan(&application, &project, &external.candidates[0]),
    );
    assert!(records.sections.is_empty());
    assert_eq!(records.relocations.len(), 1);
    assert_eq!(project_bytes(&project), before);
}
#[test]
fn thin_bindings_plan_roundtrip_and_live_manifest_survive_changes_and_move() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let archive = temp.path().join("lib.a");
    let payload = temp.path().join("entry.o");
    let elf = support::elf();
    fs::write(&payload, &elf).unwrap();
    fs::write(&archive, support::archive(&[(b"entry.o", &elf)], true)).unwrap();
    app::create_project(&project).unwrap();
    let application = application();
    let first = import(&application, &project, &[&archive]);
    let candidate = search(&application, &project, SelectionKind::Symbol, b"same", None)
        .candidates
        .remove(0);
    let old = plan(&application, &project, &candidate);
    let old_records = run(&application, &old);
    let mut saved = Vec::new();
    old.write(&mut saved, &|| false).unwrap();
    assert!(old.write(&mut Vec::new(), &|| false).is_err());
    let parsed = app::PlanDescription::read(saved.as_slice()).unwrap();
    assert_eq!(&parsed, old.description());
    // Same thin header/selector, different captured payload at the same origin.
    let mut changed = elf.clone();
    let at = changed
        .windows(4)
        .position(|w| w == [0x13, 0, 0, 0])
        .unwrap();
    changed[at] = 0x33;
    fs::write(&payload, &changed).unwrap();
    let second = import(&application, &project, &[&archive]);
    assert_ne!(first, second);
    let latest = search(&application, &project, SelectionKind::Symbol, b"same", None)
        .candidates
        .remove(0);
    assert_eq!(candidate.scope, latest.scope);
    assert_ne!(candidate.payload, latest.payload);
    let new = plan(&application, &project, &latest);
    assert_ne!(old.description().id, new.description().id);
    fs::remove_file(&archive).unwrap();
    fs::remove_file(&payload).unwrap();
    let moved = temp.path().join("moved");
    fs::rename(&project, &moved).unwrap();
    let before = project_bytes(&moved);
    let reopened = application.reopen_plan(&moved, parsed, budget()).unwrap();
    assert_eq!(old.description(), reopened.description());
    assert_eq!(run(&application, &reopened).symbols, old_records.symbols);
    assert_eq!(run(&application, &old).objects, old_records.objects);
    assert_eq!(project_bytes(&moved), before);
    // A live plan owns metadata independently of project paths; it is not a GC pin.
    fs::remove_dir_all(&moved).unwrap();
    assert_eq!(run(&application, &old).symbols, old_records.symbols);
}
#[test]
fn plans_reject_tampering_missing_targets_wrong_projects_and_unknown_versions() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let source = temp.path().join("input.o");
    fs::write(&source, support::elf()).unwrap();
    app::create_project(&project).unwrap();
    let application = application();
    import(&application, &project, &[&source]);
    let candidate = search(&application, &project, SelectionKind::Symbol, b"same", None)
        .candidates
        .remove(0);
    let plan = plan(&application, &project, &candidate);
    let mut changed = plan.description().clone();
    changed.recipe.budget.timeout_ms += 1;
    assert_eq!(changed.validate().unwrap_err().code, ErrorCode::Integrity);
    let mut unknown = plan.description().clone();
    unknown.recipe.operation_version = 99;
    assert_eq!(
        unknown.validate().unwrap_err().code,
        ErrorCode::Incompatible
    );
    let wrong = temp.path().join("wrong");
    app::create_project(&wrong).unwrap();
    assert!(
        application
            .reopen_plan(&wrong, plan.description().clone(), budget())
            .is_err()
    );
    let error = application
        .plan(
            &project,
            app::PlanRequest {
                revision: None,
                scope: InspectionScope::Input { input: 999 },
                budget: budget(),
            },
            budget(),
        )
        .err()
        .unwrap();
    assert_eq!(error.code, ErrorCode::NotFound);
    let serialized = serde_json::to_vec(plan.description()).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&serialized).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("unexpected".into(), serde_json::json!(true));
    assert!(app::PlanDescription::read(serde_json::to_vec(&value).unwrap().as_slice()).is_err());
    assert!(app::PlanDescription::read(vec![b' '; 65537].as_slice()).is_err());
}
#[test]
fn cli_plan_and_run_share_api_results_and_saved_plan_is_not_overwritten() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let source = temp.path().join("input.o");
    fs::write(&source, support::elf()).unwrap();
    app::create_project(&project).unwrap();
    let application = application();
    import(&application, &project, &[&source]);
    let candidate = search(&application, &project, SelectionKind::Symbol, b"same", None)
        .candidates
        .remove(0);
    let expected = plan(&application, &project, &candidate);
    let request = temp.path().join("request.json");
    let saved = temp.path().join("plan.json");
    fs::write(
        &request,
        serde_json::to_vec(&app::PlanRequest {
            revision: Some(candidate.revision),
            scope: candidate.scope,
            budget: budget(),
        })
        .unwrap(),
    )
    .unwrap();
    let invoke = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_blobray"))
            .args(args)
            .arg("--project")
            .arg(&project)
            .args(["--limit-mode", "watchdog", "--format", "json"])
            .output()
            .unwrap()
    };
    let first = invoke(&[
        "plan",
        "--request",
        request.to_str().unwrap(),
        "--output",
        saved.to_str().unwrap(),
    ]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let mut tiny_request: serde_json::Value =
        serde_json::from_slice(&fs::read(&request).unwrap()).unwrap();
    tiny_request["budget"]["max_work_units"] = serde_json::json!(1);
    let tiny_path = temp.path().join("tiny-request.json");
    fs::write(&tiny_path, serde_json::to_vec(&tiny_request).unwrap()).unwrap();
    let tiny_plan = invoke(&["plan", "--request", tiny_path.to_str().unwrap()]);
    assert!(tiny_plan.status.success());
    let tiny_saved = temp.path().join("tiny-plan.json");
    fs::write(&tiny_saved, &tiny_plan.stdout).unwrap();
    let failed = invoke(&["run", "--plan", tiny_saved.to_str().unwrap()]);
    assert!(!failed.status.success());
    assert!(failed.stdout.is_empty());
    let diagnostic: serde_json::Value = serde_json::from_slice(&failed.stderr).unwrap();
    assert_eq!(diagnostic["query"]["state"], "resource-limited");
    assert!(diagnostic["query"]["diagnostics"]["progress"].is_object());
    let bytes = fs::read(&saved).unwrap();
    assert_eq!(
        &app::PlanDescription::read(bytes.as_slice()).unwrap(),
        expected.description()
    );
    assert!(
        !invoke(&[
            "plan",
            "--request",
            request.to_str().unwrap(),
            "--output",
            saved.to_str().unwrap()
        ])
        .status
        .success()
    );
    assert_eq!(fs::read(&saved).unwrap(), bytes);
    let executed = invoke(&["run", "--plan", saved.to_str().unwrap()]);
    assert!(
        executed.status.success(),
        "{}",
        String::from_utf8_lossy(&executed.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&executed.stdout).unwrap();
    assert_eq!(json["summary"]["plan"], expected.description().id.as_str());
    let symbol = json["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["kind"] == "symbol")
        .unwrap();
    assert_eq!(
        symbol["value"],
        serde_json::to_value(&run(&application, &expected).symbols[0]).unwrap()
    );
    let selected = invoke(&["select", "--kind", "symbol", "--name-hex", "6c6f63616cff"]);
    assert!(selected.status.success());
    let json: serde_json::Value = serde_json::from_slice(&selected.stdout).unwrap();
    assert_eq!(json["summary"]["matches"], 1);
}

#[test]
fn static_and_dynamic_symbol_tables_keep_distinct_selectors() {
    // Add a dynamic symbol table referencing the same strings/entries. This is
    // valid ELF input, not a fabricated inventory association.
    let mut elf = support::elf();
    let shoff = u32::from_le_bytes(elf[32..36].try_into().unwrap()) as usize;
    let shsize = u16::from_le_bytes(elf[46..48].try_into().unwrap()) as usize;
    let count = u16::from_le_bytes(elf[48..50].try_into().unwrap()) as usize;
    let mut headers = elf[shoff..shoff + shsize * count].to_vec();
    let symtab = headers
        .chunks_exact(shsize)
        .find(|h| u32::from_le_bytes(h[4..8].try_into().unwrap()) == 2)
        .unwrap();
    let mut dynamic = symtab.to_vec();
    dynamic[4..8].copy_from_slice(&11u32.to_le_bytes());
    headers.extend_from_slice(&dynamic);
    let new_offset = elf.len() as u32;
    elf.extend_from_slice(&headers);
    elf[32..36].copy_from_slice(&new_offset.to_le_bytes());
    elf[48..50].copy_from_slice(&((count + 1) as u16).to_le_bytes());
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let source = temp.path().join("input.o");
    fs::write(&source, elf).unwrap();
    app::create_project(&project).unwrap();
    let application = application();
    import(&application, &project, &[&source]);
    let records = search(&application, &project, SelectionKind::Symbol, b"same", None);
    assert_eq!(records.candidates.len(), 2);
    let kinds: Vec<_> = records
        .candidates
        .iter()
        .map(|c| {
            let InspectionScope::Symbol { symbol, .. } = &c.scope else {
                panic!()
            };
            symbol.table
        })
        .collect();
    assert!(kinds.contains(&SymbolTableKind::Static));
    assert!(kinds.contains(&SymbolTableKind::Dynamic));
    for candidate in records.candidates {
        let result = run(&application, &plan(&application, &project, &candidate));
        assert_eq!(result.symbols.len(), 1);
        let InspectionScope::Symbol { symbol, .. } = candidate.scope else {
            panic!()
        };
        assert_eq!(result.symbols[0].id, symbol);
    }
}

#[test]
fn equal_thin_containers_in_one_revision_bind_each_input_separately() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let elf = support::elf();
    let mut changed = elf.clone();
    let at = changed
        .windows(4)
        .position(|w| w == [0x13, 0, 0, 0])
        .unwrap();
    changed[at] = 0x33;
    let mut archives = Vec::new();
    for (name, bytes) in [("a", elf.as_slice()), ("b", changed.as_slice())] {
        let dir = temp.path().join(name);
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("entry.o"), bytes).unwrap();
        let archive = dir.join("lib.a");
        fs::write(&archive, support::archive(&[(b"entry.o", &elf)], true)).unwrap();
        archives.push(archive);
    }
    app::create_project(&project).unwrap();
    let application = application();
    import(&application, &project, &[&archives[0], &archives[1]]);
    let candidates = search(
        &application,
        &project,
        SelectionKind::Object,
        b"entry.o",
        None,
    )
    .candidates;
    assert_eq!(candidates.len(), 2);
    assert_ne!(candidates[0].payload, candidates[1].payload);
    for candidate in candidates {
        let plan = plan(&application, &project, &candidate);
        assert_eq!(
            plan.description().recipe.binding.as_ref().unwrap().payload,
            candidate.payload
        );
        assert_eq!(
            run(&application, &plan).objects[0].content,
            candidate.payload
        );
    }
}

#[test]
fn plan_and_run_limits_fail_without_writes_and_plans_hold_admission() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let source = temp.path().join("input.o");
    fs::write(&source, support::elf()).unwrap();
    app::create_project(&project).unwrap();
    let seed = application();
    import(&seed, &project, &[&source]);
    let before = project_bytes(&project);
    let application = app::Application::with_limits(
        Arc::new(LinuxHost::new(env!("CARGO_BIN_EXE_blobray").into(), None)),
        app::ApplicationLimits {
            max_operations: 1,
            event_capacity: 4,
        },
    )
    .unwrap();
    let request = app::PlanRequest {
        revision: None,
        scope: InspectionScope::Revision,
        budget: budget(),
    };
    let handle = application
        .start_plan(&project, request.clone(), budget())
        .unwrap();
    let plan = handle.take_plan().unwrap();
    assert!(handle.take_plan().is_err());
    assert_eq!(handle.wait().state, RunState::Completed);
    drop(handle);
    let clone = plan.clone();
    drop(plan);
    assert!(matches!(
        application.start_run(&clone),
        Err(Error {
            code: ErrorCode::Busy,
            ..
        })
    ));
    drop(clone);
    let mut tiny = budget();
    tiny.max_work_units = Some(1);
    let failed = application
        .start_plan(&project, request.clone(), tiny.clone())
        .unwrap();
    assert_eq!(failed.wait().state, RunState::ResourceLimited);
    drop(failed);
    let mut no_memory = budget();
    no_memory.working_memory_bytes = Some(1024);
    let failed = application
        .start_plan(&project, request.clone(), no_memory)
        .unwrap();
    assert_eq!(failed.wait().state, RunState::ResourceLimited);
    drop(failed);
    let plan = seed
        .plan(
            &project,
            app::PlanRequest {
                budget: tiny,
                ..request.clone()
            },
            budget(),
        )
        .unwrap();
    let run = seed.start_run(&plan).unwrap();
    assert_eq!(run.wait().state, RunState::ResourceLimited);
    let mut expired = budget();
    expired.timeout_ms = 1;
    let plan = seed
        .plan(
            &project,
            app::PlanRequest {
                budget: expired,
                ..request
            },
            budget(),
        )
        .unwrap();
    let run = seed.start_run(&plan).unwrap();
    assert_eq!(run.wait().state, RunState::TimedOut);
    assert_eq!(project_bytes(&project), before);
}

#[test]
fn incomplete_inventory_is_inspectable_without_claiming_complete_coverage() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let source = temp.path().join("mixed.a");
    let elf = support::elf();
    fs::write(
        &source,
        support::archive(&[(b"good.o", &elf), (b"unknown.o", b"unsupported")], false),
    )
    .unwrap();
    app::create_project(&project).unwrap();
    let application = application();
    import(&application, &project, &[&source]);
    let candidates = search(
        &application,
        &project,
        SelectionKind::Object,
        b"unknown.o",
        None,
    )
    .candidates;
    let plan = plan(&application, &project, &candidates[0]);
    assert!(!plan.description().recipe.revision_complete);
    let mut output = application.start_run(&plan).unwrap().take_output().unwrap();
    assert!(matches!(
        output.summary(),
        app::QuerySummary::Inspection {
            revision_complete: false,
            ..
        }
    ));
    let mut records = Records::default();
    output.records(&|| false, &mut records).unwrap();
    assert_eq!(records.objects.len(), 1);
    assert!(records.objects[0].elf.is_none());
    assert!(!records.diagnostics.is_empty());
    let good = search(
        &application,
        &project,
        SelectionKind::Object,
        b"good.o",
        None,
    )
    .candidates;
    let selected = application
        .plan(
            &project,
            app::PlanRequest {
                revision: Some(good[0].revision.clone()),
                scope: good[0].scope.clone(),
                budget: budget(),
            },
            budget(),
        )
        .unwrap();
    assert!(!selected.description().recipe.revision_complete);
    let mut output = application
        .start_run(&selected)
        .unwrap()
        .take_output()
        .unwrap();
    assert_eq!(
        output
            .records(&|| true, &mut Records::default())
            .unwrap_err()
            .code,
        ErrorCode::Cancelled
    );
    assert_eq!(
        output
            .records(&|| false, &mut Records::default())
            .unwrap_err()
            .code,
        ErrorCode::InvalidRequest
    );
}

#[test]
fn large_selection_streams_under_small_capacity_and_consumer_failure_is_final() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let source = temp.path().join("many.a");
    let elf = support::elf();
    let members: Vec<_> = (0..256)
        .map(|_| (b"same.o".as_slice(), elf.as_slice()))
        .collect();
    fs::write(&source, support::archive(&members, false)).unwrap();
    app::create_project(&project).unwrap();
    let application = application();
    import(&application, &project, &[&source]);
    let mut output = application
        .query(
            &project,
            app::ReadQuery::Select {
                revision: None,
                request: SelectionRequest {
                    kind: SelectionKind::Symbol,
                    name: b"same".to_vec(),
                    input: None,
                },
            },
            budget(),
        )
        .unwrap();
    let mut found = Records::default();
    output.records(&|| false, &mut found).unwrap();
    assert_eq!(found.candidates.len(), 256);
    assert!(
        output
            .report
            .diagnostics
            .progress
            .unwrap()
            .working_memory
            .unwrap()
            .peak_reserved_bytes
            <= budget().working_memory_bytes.unwrap()
    );
    let plan = plan(&application, &project, &found.candidates[255]);
    let mut output = application.start_run(&plan).unwrap().take_output().unwrap();
    let error = output
        .records(
            &|| false,
            &mut Records {
                fail: true,
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Io);
    assert_eq!(error.message, "consumer closed");
    assert_eq!(
        output
            .records(&|| false, &mut Records::default())
            .unwrap_err()
            .code,
        ErrorCode::InvalidRequest
    );
}
