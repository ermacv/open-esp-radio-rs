//! Captured PHY data/research, review and current-format preservation.
//!
//! Private inputs and generated evidence stay outside tracked source. The
//! scenario requires the identified PHY archive and ROM; assertions concern
//! captured facts, never qualification.
use crate::harness::{
    Budget, Input, Result, Runner, args, invalid, named_object, named_section, sha256,
};
use crate::i2c::{OBJECT_SHA, TABLE_SHA};
use crate::layout::{COMMAND_RAM, PHY_PARAM_BYTES, ROM_INPUT, ROM_INTERFACE_POINTER};
use crate::session::{path_arg, start_run};
use crate::{I2C_LIBRARY_SHA, ROM_SHA};
use blobray_application::QuerySummary;
use blobray_domain::{
    AbstractValue, AccessRoot, AccessStep, ArtifactId, AssertionState, CallAbi, CallDirection,
    ComparisonVerdict, ConstantOperand, ConstantProposalRequest, CoverageScope, DataByteOrder,
    DataLayout, DataManifest, DataProposalRequest, DataRecord, DataRequest, DataSelector,
    DefinitionClass, EvidenceRef, FlowEffectProfile, FlowGoal, FlowHop, FlowQuery, FlowRecord,
    FunctionAnalysisId, FunctionRecord, FunctionSource, ImageRegion, IntegerEncoding, IntegerTable,
    InterfaceAccessPath, InterfaceContract, InterfaceGuard, InterfaceInput, InterfaceObservation,
    InterfaceQuery, InterfaceSlot, InvestigationMember, InvestigationOutcome, IrBuildRequest,
    IrProfile, IrRoots, KnowledgeAction, KnowledgeChange, KnowledgeClaim, KnowledgeEntry,
    KnowledgeOccurrence, KnowledgeProposal, KnowledgeRevisionId, MemoryKind, MemorySliceQuery,
    MemorySliceRecord, MemorySliceSelection, NavigationFilter, NavigationQuery, NavigationRecord,
    NavigationScope, ObjectInventory, PlanEntry, PointerTable, PointerValue, PublicationId,
    RegisterQuery, RegisterRecord, Revision, RevisionId, SliceIssue, SubjectId, SymbolId,
    SymbolTableKind, TraceEvent, TraceObservation, TraceOutcome, TraceRecord, TraceRegister,
    TraceRequest, TraceSide, TraceTarget, TraceValue, ValueAlternative,
};
use blobray_next_host::wire::{ExportDocument, RecordDocument};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
};

/// SHA-256 of the eleven captured `phy_i2c.o` `.rodata` code pointers.
const POINTER_TABLE_SHA: &str = "85759b3811ff7dc47b03792ac85317be51431a3f9e01dcafce317ed736a391b0";
/// Absolute `.rela.rodata` targets of those pointers: static symbol indexes in
/// symbol table section 17 of the authenticated `phy_i2c.o`.
const POINTER_TARGETS: [u64; 11] = [36, 36, 36, 34, 34, 34, 36, 34, 34, 36, 36];
const POINTER_SYMBOL_TABLE: u32 = 17;
/// Linked-image placement of the command-memory research.
const LINK_CODE_START: &str = "0x10000000";
const LINK_DATA_START: &str = "0x20000000";
/// Independent first command: encode(0x67, 2, 7).
const FIRST_COMMAND: u32 = 0x0007_0267;

pub struct Options {
    pub binary: PathBuf,
    pub library: PathBuf,
    pub rom: PathBuf,
    pub linker: PathBuf,
    pub output: PathBuf,
    pub budget: Budget,
}

fn os(value: impl AsRef<OsStr>) -> OsString {
    value.as_ref().to_owned()
}

struct Research {
    runner: Runner,
    run: PathBuf,
    revision: RevisionId,
    inventory: Revision,
}

impl Research {
    fn call(&self, name: &str, args: Vec<OsString>) -> Result<()> {
        self.runner.call(name, &args, 0).map(|_| ())
    }
    fn json<T: DeserializeOwned>(&self, name: &str, args: Vec<OsString>) -> Result<T> {
        self.runner.json(name, &args)
    }
    fn run(&self, name: &str, args: Vec<OsString>) -> Result<blobray_application::RunRecord> {
        self.runner.run_record(name, &args, 0)
    }
    /// Run a request-file command and decode its document.
    fn request<T: DeserializeOwned>(
        &self,
        name: &str,
        command: &[&str],
        request: &impl Serialize,
        output: Option<&Path>,
    ) -> Result<T> {
        let mut argv: Vec<OsString> = command.iter().map(os).collect();
        argv.push("--request".into());
        argv.push(path_arg(&self.runner.doc(name, request)?));
        if let Some(output) = output {
            argv.push("--output".into());
            argv.push(path_arg(output));
        }
        self.json(name, argv)
    }
    /// Run a request-file command whose only result is the file at `output`.
    fn save(
        &self,
        name: &str,
        command: &[&str],
        request: &impl Serialize,
        output: &Path,
    ) -> Result<()> {
        let mut argv: Vec<OsString> = command.iter().map(os).collect();
        argv.push("--request".into());
        argv.push(path_arg(&self.runner.doc(name, request)?));
        argv.push("--output".into());
        argv.push(path_arg(output));
        // The runner retains stdout as `<label>.json`; keep it apart from `output`.
        self.call(&format!("save-{name}"), argv)
    }
    fn knowledge(&self, name: &str, command: Vec<OsString>) -> Result<KnowledgeRevisionId> {
        self.run(name, command)?
            .knowledge
            .ok_or_else(|| invalid(format!("{name}: no knowledge revision")))
    }
    /// The single proposed assertion of the current knowledge snapshot.
    fn proposed(&self, name: &str) -> Result<KnowledgeEntry> {
        let entries: RecordDocument<serde_json::Value> =
            self.json(name, args(["knowledge", "show"]))?;
        let mut proposed = vec![];
        for record in entries
            .records
            .into_iter()
            .filter(|r| r.kind == "assertion")
        {
            let entry: KnowledgeEntry = serde_json::from_value(record.value)?;
            if entry.state == AssertionState::Proposed {
                proposed.push(entry);
            }
        }
        match <[KnowledgeEntry; 1]>::try_from(proposed) {
            Ok([entry]) => Ok(entry),
            Err(_) => Err(invalid(format!("{name}: expected one proposed assertion"))),
        }
    }
    fn accept(
        &self,
        name: &str,
        base: &KnowledgeRevisionId,
        entry: &KnowledgeEntry,
        actor: &str,
        reason: &str,
    ) -> Result<KnowledgeRevisionId> {
        let mut command = args(["knowledge", "accept", "--base"]);
        command.extend([
            os(base.as_str()),
            "--assertion".into(),
            os(entry.id.as_str()),
            "--actor".into(),
            actor.into(),
            "--reason".into(),
            reason.into(),
        ]);
        self.knowledge(name, command)
    }
    fn export_data(
        &self,
        name: &str,
        revision: &KnowledgeRevisionId,
        entry: &KnowledgeEntry,
        directory: &str,
        expected: i32,
    ) -> Result<()> {
        let mut command = args(["export-data", "--revision"]);
        command.extend([
            os(revision.as_str()),
            "--assertion".into(),
            os(entry.id.as_str()),
            "--output".into(),
            path_arg(&self.run.join(directory)),
        ]);
        self.runner.call(name, &command, expected).map(|_| ())
    }
    fn occurrence(&self, object: &ObjectInventory) -> KnowledgeOccurrence {
        KnowledgeOccurrence {
            revision: self.revision.clone(),
            source: FunctionSource::Input { input: 0 },
            object: object.id.clone(),
            symbol: None,
        }
    }
    /// Members selected by exact name in one publication.
    fn members(&self, publication: &PublicationId, name: &str) -> Result<Vec<InvestigationMember>> {
        let mut command = args(["functions", "--id", publication.as_str(), "--name"]);
        command.push(name.into());
        let document: RecordDocument<InvestigationMember> = self.json(name, command)?;
        Ok(document.records.into_iter().map(|r| r.value).collect())
    }
    fn facts(&self, name: &str, analysis: &FunctionAnalysisId) -> Result<Vec<FunctionRecord>> {
        let document: RecordDocument<FunctionRecord> =
            self.json(name, args(["analysis", "--id", analysis.as_str()]))?;
        Ok(document.records.into_iter().map(|r| r.value).collect())
    }
    fn same_bytes(&self, left: &str, right: &str) -> Result<bool> {
        Ok(fs::read(self.run.join(left))? == fs::read(self.run.join(right))?)
    }
}

fn analyzed(member: &InvestigationMember) -> FunctionAnalysisId {
    match &member.outcome {
        InvestigationOutcome::Analyzed { analysis, .. } => analysis.clone(),
        other => panic!("function was not analyzed: {other:?}"),
    }
}

fn from_input(member: &InvestigationMember, input: u64) -> bool {
    matches!(&member.entry, PlanEntry::Function { request, .. }
        if request.source == FunctionSource::Input { input })
}

fn constant(value: u32) -> AbstractValue {
    AbstractValue::Constant { value }
}

/// Offset of a saved fact that has one.
fn fact_offset(fact: &FunctionRecord) -> Option<u64> {
    Some(match fact {
        FunctionRecord::Transfer { offset, .. }
        | FunctionRecord::CallResolution { offset, .. }
        | FunctionRecord::MemoryAccess { offset, .. }
        | FunctionRecord::Value { offset, .. }
        | FunctionRecord::CalleeEffect { offset, .. } => *offset,
        _ => return None,
    })
}

fn subject(value: &str) -> Result<SubjectId> {
    SubjectId::try_from(value.to_owned()).map_err(|e| invalid(format!("{e:?}")))
}

fn summary<T>(document: &RecordDocument<T>) -> &QuerySummary {
    &document.summary
}

pub fn exercise(options: &Options) -> Result<PathBuf> {
    let run = start_run(&options.output)?;
    let runner = Runner::new(&options.binary, &run, run.join("project"), options.budget)?;
    let library = std::path::absolute(&options.library)?;
    let rom = std::path::absolute(&options.rom)?;
    let (revision, _) = runner.capture(&[
        Input {
            role: "phy",
            path: &library,
            sha256: Some(I2C_LIBRARY_SHA),
        },
        Input {
            role: "rom",
            path: &rom,
            sha256: Some(ROM_SHA),
        },
    ])?;
    let inventory = runner.inventory()?;
    crate::session::verify_rom_symbols(&inventory, ROM_INPUT as usize)?;
    let mut r = Research {
        runner,
        run: run.clone(),
        revision,
        inventory,
    };
    let publication = r
        .run("whole", args(["analyze-project"]))?
        .publication
        .ok_or_else(|| invalid("analysis published nothing"))?;

    // Independent ROM instruction reading: four branches form base 0x2010e000
    // plus -0x7a8/-0x7a0/-0x798/-0x790, then one shared load/store pair.
    let rows = r.members(&publication, "tsf_hal_set_tbtt_rf_ctrl_disable")?;
    assert_eq!(rows.len(), 1);
    let finite_analysis = analyzed(&rows[0]);
    let finite_records = r.facts("finite-address-facts", &finite_analysis)?;
    let finite_addresses: Vec<ValueAlternative> =
        [0x2010_d858, 0x2010_d860, 0x2010_d868, 0x2010_d870]
            .map(|value| ValueAlternative::Constant { value })
            .to_vec();
    for (expected_offset, expected_access) in [
        (0x2f82_bcb6u64, MemoryKind::Load),
        (0x2f82_bcc0, MemoryKind::Store),
    ] {
        assert!(finite_records.iter().any(|fact| matches!(fact,
            FunctionRecord::MemoryAccess { offset, access, width: 4, address: AbstractValue::Alternatives { values }, .. }
                if *offset == expected_offset && *access == expected_access && values.values() == finite_addresses)));
    }
    let mut command = args([
        "export-analysis",
        "--id",
        finite_analysis.as_str(),
        "--output",
    ]);
    command.push(path_arg(&run.join("finite-export")));
    r.call("finite-export", command)?;

    // This real callback loads the table from mutable memory at 0x2f07fc3c,
    // then calls slot +8. Without a reviewed binding the destination stays unknown.
    let callback_analysis = r
        .run(
            "real-callback",
            args([
                "research",
                "--id",
                publication.as_str(),
                "--address",
                "0x2f829fa8",
                "--abi-contract",
                "riscv-integer",
            ]),
        )?
        .analysis
        .ok_or_else(|| invalid("callback research published no analysis"))?;
    let callback_records = r.facts("real-callback-facts", &callback_analysis)?;
    const CALLBACK_SITE: u64 = 0x2f82_9fb6;
    assert!(callback_records.iter().any(|fact| matches!(
        fact,
        FunctionRecord::Transfer {
            offset: CALLBACK_SITE,
            call: true,
            target: AbstractValue::Unknown
        }
    )));
    assert!(callback_records.iter().any(|fact| matches!(
        fact,
        FunctionRecord::CallResolution {
            offset: CALLBACK_SITE,
            analysis: None,
            ..
        }
    )));
    assert!(
        !callback_records
            .iter()
            .any(|fact| matches!(fact, FunctionRecord::CalleeEffect { .. }))
    );

    let mut callback_query = InterfaceQuery {
        input: InterfaceInput::Analysis {
            analysis: callback_analysis.clone(),
            abi: Some(CallAbi::RiscvInteger),
        },
        knowledge: None,
    };
    let discovery: RecordDocument<InterfaceObservation> =
        r.request("callback-discovery", &["interfaces"], &callback_query, None)?;
    assert_eq!(discovery.records.len(), 1);
    let callback_slot = discovery.records[0].value.clone();
    let callback_path = InterfaceAccessPath {
        root: AccessRoot::Address {
            address: ROM_INTERFACE_POINTER,
        },
        path: vec![AccessStep::LoadPointer { offset: 0 }],
        slot: 8,
    };
    assert_eq!(callback_slot.offset, CALLBACK_SITE);
    assert_eq!(callback_slot.paths, std::slice::from_ref(&callback_path));
    assert_eq!(callback_slot.target, Some(AbstractValue::Unknown));
    assert!(callback_slot.issue.is_none() && callback_slot.bindings.is_empty());
    let QuerySummary::Interfaces {
        summary: callback_summary,
    } = summary(&discovery).clone()
    else {
        return Err(invalid("interfaces returned another summary"));
    };

    let coverage: RecordDocument<serde_json::Value> =
        r.json("coverage", args(["coverage", "--id", publication.as_str()]))?;
    assert_eq!(
        coverage.assessment.coverage.as_ref().map(|c| c.scope),
        Some(CoverageScope::SelectedFunctionExtents)
    );
    let QuerySummary::Coverage { extents, .. } = &coverage.summary else {
        return Err(invalid("coverage returned another summary"));
    };
    assert!(extents.objects > 0);

    // Unlike archive-local research, this image resolves ordinary calls to exact
    // captured ROM definitions. Its final tail transfer retains the profile's limits.
    let plan = run.join("linked-plan.json");
    let linker = path_arg(&std::path::absolute(&options.linker)?);
    let mut command = args([
        "link-plan",
        "--entry",
        "phy_i2c_master_cmd_mem_init",
        "--entry-input",
        "0",
        "--inputs",
        "0",
        "--code-start",
        LINK_CODE_START,
        "--data-start",
        LINK_DATA_START,
        "--linker",
    ]);
    command.push(linker.clone());
    for companion in [
        "phy_encode_i2c_master",
        "phy_i2c_master_fill",
        "phy_get_data_sat",
    ] {
        command.extend(["--companion".into(), format!("1:{companion}").into()]);
    }
    command.extend(["--output".into(), path_arg(&plan)]);
    r.call("link-plan", command)?;
    let mut command = args(["prepare-image", "--plan"]);
    command.extend([path_arg(&plan), "--linker".into(), linker]);
    let image = r
        .run("prepare-image", command)?
        .image
        .ok_or_else(|| invalid("no image"))?;
    let linked_publication = r
        .run(
            "linked-analysis",
            args(["analyze-project", "--image", image.as_str()]),
        )?
        .publication
        .ok_or_else(|| invalid("linked analysis published nothing"))?;
    let mut callees = BTreeMap::new();
    for name in [
        "phy_encode_i2c_master",
        "phy_i2c_master_fill",
        "phy_get_data_sat",
    ] {
        let matches: Vec<_> = r
            .members(&publication, name)?
            .into_iter()
            .filter(|m| from_input(m, 1))
            .collect();
        assert_eq!(matches.len(), 1, "{name}");
        callees.insert(name, analyzed(&matches[0]));
    }
    let fill = callees["phy_i2c_master_fill"].clone();
    let encode = callees["phy_encode_i2c_master"].clone();

    // Exact static trace on authenticated ROM bytes: fill(index, value) writes one
    // u32 to 0x2010fc00 + index*4. This expectation is independent of the exporter.
    let ir_request = IrBuildRequest {
        scope: NavigationScope {
            revision: r.revision.clone(),
            publications: vec![],
            analyses: vec![fill.clone()],
            knowledge: None,
        },
        profiles: vec![IrProfile {
            name: "fill".into(),
            roots: IrRoots::All,
            include_reachable: true,
        }],
    };
    let mut command = args(["ir", "build", "--request"]);
    command.push(path_arg(&r.runner.doc("ir-build", &ir_request)?));
    let saved_ir = r
        .run("build-semantic-ir", command)?
        .semantic_ir
        .ok_or_else(|| invalid("no semantic IR"))?;
    let mut command = args(["ir", "show", saved_ir.as_str(), "--output"]);
    command.push(path_arg(&run.join("semantic-ir-export.json")));
    r.call("export-semantic-ir", command)?;
    let trace_target = TraceTarget {
        ir: saved_ir.clone(),
        profile: "fill".into(),
        entry: fill.clone(),
        abi: CallAbi::RiscvInteger,
        registers: vec![
            TraceRegister {
                register: 10,
                value: 0,
            },
            TraceRegister {
                register: 11,
                value: FIRST_COMMAND,
            },
        ],
    };
    let trace_request = TraceRequest {
        left: trace_target.clone(),
        right: Some(trace_target),
        observation: TraceObservation {
            ranges: vec![ImageRegion {
                start: COMMAND_RAM,
                length: 256,
            }],
            fences: true,
        },
    };
    let static_trace: RecordDocument<TraceRecord> =
        r.request("static-fill-trace", &["trace"], &trace_request, None)?;
    let trace_verdict = |document: &RecordDocument<TraceRecord>| match &document.summary {
        QuerySummary::Trace { summary } => summary.verdict,
        _ => None,
    };
    assert_eq!(trace_verdict(&static_trace), Some(ComparisonVerdict::Match));
    let QuerySummary::Trace {
        summary: trace_summary,
    } = &static_trace.summary
    else {
        unreachable!()
    };
    assert_eq!(
        trace_summary.left,
        TraceOutcome {
            exact: true,
            events: 1,
            invocations: 1
        }
    );
    let left_events: Vec<TraceEvent> = static_trace
        .records
        .iter()
        .filter_map(|record| match &record.value {
            TraceRecord::Event {
                side: TraceSide::Left,
                event,
                ..
            } => Some(*event),
            _ => None,
        })
        .collect();
    assert_eq!(
        left_events,
        [TraceEvent::Memory {
            access: MemoryKind::Store,
            address: COMMAND_RAM,
            width: 4,
            value: Some(TraceValue::Constant {
                value: FIRST_COMMAND
            })
        }]
    );
    let mut changed = trace_request.clone();
    changed.right.as_mut().unwrap().registers[1].value = FIRST_COMMAND + 1;
    let different: RecordDocument<TraceRecord> =
        r.request("static-fill-diff", &["trace"], &changed, None)?;
    assert_eq!(trace_verdict(&different), Some(ComparisonVerdict::Diff));
    let mut unknown = trace_request.clone();
    unknown.right.as_mut().unwrap().registers = vec![TraceRegister {
        register: 11,
        value: FIRST_COMMAND,
    }];
    let incomplete: RecordDocument<TraceRecord> =
        r.request("static-fill-incomplete", &["trace"], &unknown, None)?;
    assert_eq!(
        trace_verdict(&incomplete),
        Some(ComparisonVerdict::Incomplete)
    );
    r.save(
        "static-trace-export",
        &["trace"],
        &trace_request,
        &run.join("static-trace-export.json"),
    )?;

    let mut linked_facts: Option<Vec<FunctionRecord>> = None;
    let mut linked_analysis = None;
    let mut phase_samples = vec![];
    let working_memory = options.budget.working_memory_mib * 1024 * 1024;
    for sample in 0..3 {
        let mut command = args([
            "research",
            "--id",
            linked_publication.as_str(),
            "--name",
            "phy_i2c_master_cmd_mem_init",
            "--abi-contract",
            "riscv-integer",
            "--companion-publication",
        ]);
        command.push(os(publication.as_str()));
        let result = r.run(&format!("research-{sample}"), command)?;
        let analysis = result
            .analysis
            .clone()
            .ok_or_else(|| invalid("no linked analysis"))?;
        let records = r.facts(&format!("research-facts-{sample}"), &analysis)?;
        let resolved: Vec<&FunctionAnalysisId> = records
            .iter()
            .filter_map(|fact| match fact {
                FunctionRecord::CallResolution {
                    analysis: Some(a), ..
                } => Some(a),
                _ => None,
            })
            .collect();
        assert_eq!(
            resolved.iter().copied().cloned().collect::<BTreeSet<_>>(),
            callees.values().cloned().collect()
        );
        // The authenticated root has 45 encode calls, 44 ordinary fill calls,
        // three saturation calls and one final fill tail transfer (not composed).
        for (name, count) in [
            ("phy_encode_i2c_master", 45),
            ("phy_i2c_master_fill", 44),
            ("phy_get_data_sat", 3),
        ] {
            assert_eq!(
                resolved.iter().filter(|a| ***a == callees[name]).count(),
                count,
                "{name}"
            );
        }
        let effects: Vec<&FunctionRecord> = records
            .iter()
            .filter(|fact| matches!(fact, FunctionRecord::CalleeEffect { analysis, .. } if *analysis == fill))
            .collect();
        assert_eq!(effects.len(), 44);
        // Independent instruction interpretation: encode(0x67, 2, 7), then fill(0, ...).
        assert!(effects.iter().any(|fact| matches!(fact,
            FunctionRecord::CalleeEffect { address, value: Some(value), width: 4, .. }
                if *address == constant(COMMAND_RAM) && *value == constant(FIRST_COMMAND))));
        if let Some(previous) = &linked_facts {
            assert_eq!(&records, previous);
        }
        linked_facts = Some(records);
        linked_analysis = Some(analysis);
        let phases = result
            .diagnostics
            .as_ref()
            .and_then(|d| d.progress.as_ref())
            .map(|p| p.phases)
            .ok_or_else(|| invalid("research reported no progress"))?;
        for phase in [phases.load_research, phases.compose_research] {
            assert!(phase.work_units > 0);
            assert!(0 < phase.peak_reserved_bytes && phase.peak_reserved_bytes <= working_memory);
        }
        phase_samples.push(serde_json::json!({"load_research": phases.load_research, "compose_research": phases.compose_research}));
    }
    fs::write(
        run.join("linked-phase-measurements.json"),
        serde_json::to_vec_pretty(&phase_samples)?,
    )?;
    let linked_facts = linked_facts.unwrap();
    let linked_analysis = linked_analysis.unwrap();

    // Navigate exactly the saved linked root and its selected physical callees.
    // No whole-project implicit scope, linking or research is allowed in this read.
    let scope = NavigationScope {
        revision: r.revision.clone(),
        publications: vec![],
        analyses: std::iter::once(linked_analysis.clone())
            .chain(callees.values().cloned())
            .collect(),
        knowledge: None,
    };
    let mut nav_request = NavigationQuery {
        scope: scope.clone(),
        filter: NavigationFilter::Functions { function: None },
    };
    let functions: RecordDocument<NavigationRecord> =
        r.request("navigation-functions", &["navigate"], &nav_request, None)?;
    let root = functions
        .records
        .iter()
        .find_map(|record| match &record.value {
            NavigationRecord::Function { function, .. } if function.analysis == linked_analysis => {
                Some(function.location.clone())
            }
            _ => None,
        })
        .ok_or_else(|| invalid("linked root not navigated"))?;
    let QuerySummary::Navigation {
        summary: nav_summary,
    } = &functions.summary
    else {
        unreachable!()
    };
    assert_eq!(
        (nav_summary.selected_analyses, nav_summary.analyses_read),
        (4, 0)
    );
    nav_request.filter = NavigationFilter::Calls {
        function: Some(root),
        direction: CallDirection::Callees,
    };
    let nav_calls: RecordDocument<NavigationRecord> =
        r.request("navigation-calls", &["navigate"], &nav_request, None)?;
    let QuerySummary::Navigation {
        summary: calls_summary,
    } = &nav_calls.summary
    else {
        unreachable!()
    };
    assert_eq!(calls_summary.analyses_read, 4);
    for (name, count) in [
        ("phy_encode_i2c_master", 45),
        ("phy_i2c_master_fill", 44),
        ("phy_get_data_sat", 3),
    ] {
        let callee = &callees[name];
        let matched: Vec<_> = nav_calls
            .records
            .iter()
            .filter_map(|record| match &record.value {
                NavigationRecord::Call {
                    saved_resolution: Some(saved),
                    record,
                    offset,
                    candidates,
                    focus_match,
                    ..
                } if saved == callee => Some((*record, *offset, candidates, *focus_match)),
                _ => None,
            })
            .collect();
        assert_eq!(matched.len(), count, "{name}");
        for (record, offset, candidates, focus_match) in matched {
            assert_eq!(focus_match, Some(true));
            assert!(candidates.iter().any(|c| c.analysis == *callee));
            assert_eq!(fact_offset(&linked_facts[record as usize]), Some(offset));
        }
    }
    r.save(
        "navigation-export",
        &["navigate"],
        &nav_request,
        &run.join("navigation-export.json"),
    )?;
    let mut flow_request = FlowQuery {
        scope: scope.clone(),
        root: linked_analysis.clone(),
        goal: FlowGoal::Function {
            analysis: encode.clone(),
        },
        max_depth: 8,
    };
    let flow_path: RecordDocument<FlowRecord> =
        r.request("flow-target", &["flow"], &flow_request, None)?;
    let QuerySummary::Flow {
        summary: flow_summary,
    } = &flow_path.summary
    else {
        unreachable!()
    };
    assert_eq!(flow_summary.target_reached, Some(true));
    assert!(flow_path.records.iter().any(|record| matches!(&record.value,
        FlowRecord::Function { parent: Some(FlowHop { caller, callee, .. }), .. } if *caller == linked_analysis && *callee == encode)));
    flow_request.goal = FlowGoal::Effects {
        profile: FlowEffectProfile::Memory,
        address: None,
    };
    let flow_effects: RecordDocument<FlowRecord> =
        r.request("flow-effects", &["flow"], &flow_request, None)?;
    let QuerySummary::Flow {
        summary: effects_summary,
    } = &flow_effects.summary
    else {
        unreachable!()
    };
    assert_eq!(effects_summary.facts_passes, 8);
    let flow_writes: Vec<(u64, &FunctionRecord)> = flow_effects
        .records
        .iter()
        .filter_map(|record| match &record.value {
            FlowRecord::Effect { analysis, record, fact, .. }
                if *analysis == linked_analysis && matches!(fact.as_ref(), FunctionRecord::CalleeEffect { analysis, .. } if *analysis == fill) =>
            {
                Some((*record, fact.as_ref()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(flow_writes.len(), 44);
    assert!(
        flow_writes
            .iter()
            .all(|(record, fact)| linked_facts[*record as usize] == **fact)
    );
    assert!(flow_writes.iter().any(|(_, fact)| matches!(fact, FunctionRecord::CalleeEffect { value: Some(v), .. } if *v == constant(FIRST_COMMAND))));

    let register_request = RegisterQuery {
        scope: NavigationScope {
            revision: r.revision.clone(),
            publications: vec![],
            analyses: vec![finite_analysis.clone(), linked_analysis.clone()],
            knowledge: None,
        },
        ranges: vec![
            ImageRegion {
                start: 0x2010_d858,
                length: 28,
            },
            ImageRegion {
                start: COMMAND_RAM,
                length: 176,
            },
        ],
    };
    let register_catalog: RecordDocument<RegisterRecord> =
        r.request("register-catalog", &["registers"], &register_request, None)?;
    let observations: Vec<(
        &FunctionAnalysisId,
        &FunctionRecord,
        Option<u32>,
        Option<u8>,
    )> = register_catalog
        .records
        .iter()
        .filter_map(|record| match &record.value {
            RegisterRecord::Observation {
                function,
                fact,
                address,
                alternative,
                ..
            } => Some((&function.analysis, fact.as_ref(), *address, *alternative)),
            _ => None,
        })
        .collect();
    let finite_accesses: Vec<_> = observations
        .iter()
        .filter(|(analysis, fact, ..)| {
            **analysis == finite_analysis && matches!(fact, FunctionRecord::MemoryAccess { .. })
        })
        .collect();
    assert_eq!(finite_accesses.len(), 8);
    assert_eq!(
        finite_accesses
            .iter()
            .filter_map(|o| o.2)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([0x2010_d858, 0x2010_d860, 0x2010_d868, 0x2010_d870])
    );
    assert!(finite_accesses.iter().all(|o| o.3.is_some()));
    let composed: Vec<_> = observations
        .iter()
        .filter(|(_, fact, ..)| matches!(fact, FunctionRecord::CalleeEffect { analysis, .. } if *analysis == fill))
        .collect();
    assert_eq!(composed.len(), 44);
    assert_eq!(
        composed.iter().filter_map(|o| o.2).collect::<BTreeSet<_>>(),
        (0..44)
            .map(|i| COMMAND_RAM + 4 * i)
            .collect::<BTreeSet<_>>()
    );
    let QuerySummary::Registers {
        summary: register_summary,
    } = &register_catalog.summary
    else {
        unreachable!()
    };
    assert_eq!(register_summary.declarations, 0);
    r.save(
        "register-catalog-export",
        &["registers"],
        &register_request,
        &run.join("register-catalog-export.json"),
    )?;
    let address_request = FlowQuery {
        goal: FlowGoal::Effects {
            profile: FlowEffectProfile::Memory,
            address: Some(COMMAND_RAM),
        },
        ..flow_request.clone()
    };
    let address_effects: RecordDocument<FlowRecord> =
        r.request("flow-address", &["flow"], &address_request, None)?;
    let matched: Vec<&FunctionRecord> = address_effects
        .records
        .iter()
        .filter_map(|record| match &record.value {
            FlowRecord::Effect {
                fact,
                address_match: Some(true),
                ..
            } => Some(fact.as_ref()),
            _ => None,
        })
        .collect();
    assert_eq!(matched.len(), 1);
    assert!(
        matches!(matched[0], FunctionRecord::CalleeEffect { value: Some(v), .. } | FunctionRecord::MemoryAccess { value: Some(v), .. } if *v == constant(FIRST_COMMAND))
    );
    // Other unknown-address facts remain visible; a focused query cannot prove absence.
    assert!(address_effects.records.iter().any(|record| matches!(
        &record.value,
        FlowRecord::Effect {
            address_match: None,
            ..
        }
    )));
    r.save(
        "flow-export",
        &["flow"],
        &flow_request,
        &run.join("flow-export.json"),
    )?;

    // Independent prologue decoding: sw ra,28(sp) after a 32-byte frame allocation,
    // at linked root + 10. The first ordinary call is root + 24, the second + 36.
    // The saved root has an unexpanded tail, so this local witness remains a candidate.
    let transfer_at = |address: u64| {
        linked_facts
            .iter()
            .position(|fact| matches!(fact, FunctionRecord::Transfer { offset, .. } if *offset == address))
            .map(|i| i as u64)
            .ok_or_else(|| invalid(format!("no transfer at {address:#x}")))
    };
    let slice_request = MemorySliceQuery {
        analysis: linked_analysis.clone(),
        anchor: transfer_at(0x1000_0018)?,
        abi: Some(CallAbi::RiscvInteger),
        locations: vec![MemorySliceSelection::Stack {
            offset: -4,
            width: 4,
        }],
    };
    let memory_slice: RecordDocument<MemorySliceRecord> =
        r.request("memory-slice", &["memory-slice"], &slice_request, None)?;
    let definitions: Vec<_> = memory_slice
        .records
        .iter()
        .filter_map(|record| match &record.value {
            MemorySliceRecord::Definition {
                record,
                offset,
                class,
                fact,
                witness,
                ..
            } => Some((*record, *offset, *class, fact.as_ref(), witness)),
            _ => None,
        })
        .collect();
    assert_eq!(definitions.len(), 1);
    let (record, offset, class, fact, witness) = &definitions[0];
    assert_eq!(*offset, 0x1000_000a);
    assert_eq!(**fact, linked_facts[*record as usize]);
    assert!(matches!(
        fact,
        FunctionRecord::MemoryAccess {
            address: AbstractValue::EntryStack { offset: -4 },
            ..
        }
    ));
    assert_eq!(
        **witness,
        [
            0x1000_000a,
            0x1000_000c,
            0x1000_000e,
            0x1000_0010,
            0x1000_0012,
            0x1000_0014,
            0x1000_0018
        ]
    );
    assert_eq!(*class, DefinitionClass::Candidate);
    assert!(
        memory_slice
            .records
            .iter()
            .any(|record| matches!(&record.value,
        MemorySliceRecord::Location { issues, .. } if *issues == [SliceIssue::PartialControlFlow]))
    );
    let after_request = MemorySliceQuery {
        anchor: transfer_at(0x1000_0024)?,
        ..slice_request.clone()
    };
    let after: RecordDocument<MemorySliceRecord> = r.request(
        "memory-slice-after-call",
        &["memory-slice"],
        &after_request,
        None,
    )?;
    assert!(after.records.iter().any(|record| matches!(
        &record.value,
        MemorySliceRecord::Barrier {
            issue: SliceIssue::CallClobber,
            offset: 0x1000_0018,
            ..
        }
    )));
    r.save(
        "memory-slice-export",
        &["memory-slice"],
        &slice_request,
        &run.join("memory-slice-export.json"),
    )?;
    let usage: RecordDocument<serde_json::Value> =
        r.json("storage-usage", args(["storage-usage"]))?;
    let QuerySummary::StorageUsage { usage } = &usage.summary else {
        return Err(invalid("storage usage summary"));
    };
    assert!(usage.cas.logical_bytes > 0 && !usage.reachability_assessed);

    // Captured data, table/constant/pointer review and current-format preservation.
    let analysis_of = |name: &str| -> Result<FunctionAnalysisId> {
        Ok(analyzed(&r.members(&publication, name)?[0]))
    };
    let force_gain = analysis_of("phy_force_dig_gain")?;
    let gain_compensation = analysis_of("phy_txgain_comp_pacfg_new")?;
    let command_memory = analysis_of("phy_i2c_master_cmd_mem_init")?;
    let mut queries = BTreeMap::new();
    let phy_reg = named_object(&r.inventory, 0, "phy_reg.o")?.clone();
    let register_data = DataRequest {
        occurrence: r.occurrence(&phy_reg),
        ranges: vec![],
        analyses: vec![force_gain, gain_compensation.clone()],
        pointer_table: None,
    };
    queries.insert("registers", serde_json::to_value(&register_data)?);
    let facts: RecordDocument<DataRecord> =
        r.request("registers", &["data"], &register_data, None)?;
    let constants: Vec<(u64, u32)> = facts
        .records
        .iter()
        .filter_map(|record| match &record.value {
            DataRecord::Analysis {
                analysis,
                ordinal,
                record,
                ..
            } if *analysis == gain_compensation => match record.as_ref() {
                FunctionRecord::Value {
                    value: AbstractValue::Constant { value },
                    ..
                } => Some((*ordinal, *value)),
                _ => None,
            },
            _ => None,
        })
        .collect();
    println!("constant candidates {constants:?}");
    assert!(facts.records.iter().any(|record| matches!(&record.value,
        DataRecord::Analysis { record, .. } if matches!(record.as_ref(), FunctionRecord::MemoryAccess { .. }))));
    let phy_i2c = named_object(&r.inventory, 0, "phy_i2c.o")?.clone();
    let table_section = named_section(&phy_i2c, ".rodata.CSWTCH.51")?.clone();
    assert_eq!(table_section.size, 200);
    let table_selector = DataSelector::Section {
        section: table_section.index,
        offset: 0,
        length: table_section.size,
    };
    let table_query = DataRequest {
        occurrence: r.occurrence(&phy_i2c),
        ranges: vec![table_selector.clone()],
        analyses: vec![command_memory],
        pointer_table: None,
    };
    queries.insert("i2c-table", serde_json::to_value(&table_query)?);
    r.request::<ExportDocument>(
        "i2c-table",
        &["data"],
        &table_query,
        Some(&run.join("i2c-observations")),
    )?;
    let table = fs::read(run.join("i2c-observations/data.bin"))?;
    assert_eq!(table.len(), 200);
    // Established independently from the authenticated archive, not from Blobray's manifest.
    assert_eq!(
        sha256(&fs::read(run.join("i2c-observations/object.elf"))?),
        OBJECT_SHA
    );
    assert_eq!(sha256(&table), TABLE_SHA);
    let proposal = DataProposalRequest {
        analyses: table_query.analyses.clone(),
        occurrence: table_query.occurrence.clone(),
        subject: subject("phy.captured-i2c-table")?,
        selector: table_selector,
        layout: DataLayout::Integer(IntegerTable {
            encoding: IntegerEncoding { width: 1, signed: false, byte_order: DataByteOrder::Little },
            count: 200,
            stride: 1,
        }),
        purpose: "Exact byte representation of the captured read-only table; no inferred hardware interpretation".into(),
        applicability: "Only the selected source occurrence and section range".into(),
        expected_base: None,
        actor: "source-byte-review".into(),
        reason: "Verified captured range and digest, not a qualification claim".into(),
    };
    let proposed = propose(
        &r,
        "propose-table",
        &["knowledge", "propose-data"],
        "table",
        &proposal,
    )?;
    let table_entry = r.proposed("table-claims")?;
    r.export_data("pending-refused", &proposed, &table_entry, "pending", 1)?;
    let base = r.accept(
        "accept-table",
        &proposed,
        &table_entry,
        "source-byte-review",
        "Confirmed physical byte representation only",
    )?;
    r.export_data("export-table", &base, &table_entry, "accepted-table", 0)?;
    assert!(r.same_bytes("accepted-table/data.bin", "i2c-observations/data.bin")?);

    let phy_init = named_object(&r.inventory, 0, "phy_init.o")?.clone();
    let parameter = phy_init
        .elf
        .as_ref()
        .and_then(|elf| {
            elf.symbols
                .iter()
                .find(|s| s.name.as_deref() == Some(b"phy_param".as_slice()))
        })
        .ok_or_else(|| invalid("phy_param missing"))?;
    assert_eq!(parameter.size, u64::from(PHY_PARAM_BYTES));
    let initial_query = DataRequest {
        occurrence: r.occurrence(&phy_init),
        ranges: vec![DataSelector::Symbol {
            symbol: parameter.id.clone(),
            length: None,
        }],
        analyses: vec![],
        pointer_table: None,
    };
    queries.insert("initial", serde_json::to_value(&initial_query)?);
    let initial: ExportDocument = r.request(
        "initial",
        &["data"],
        &initial_query,
        Some(&run.join("initial")),
    )?;
    let QuerySummary::Data { manifest } = &initial.summary else {
        return Err(invalid("data summary"));
    };
    assert!(manifest.spans[0].writable);
    assert_eq!(
        fs::read(run.join("initial/data.bin"))?.len(),
        PHY_PARAM_BYTES as usize
    );

    // Review only a known raw bit pattern backed by an exact saved record.
    assert!(constants.iter().any(|(_, v)| *v == 0x30000));
    let (ordinal, value) = *constants
        .iter()
        .find(|(_, v)| *v == 0xfb00)
        .ok_or_else(|| invalid("0xfb00 constant missing"))?;
    let constant_request = ConstantProposalRequest {
        analysis: gain_compensation.clone(),
        record: ordinal,
        operand: ConstantOperand::Value,
        value,
        subject: subject("phy.gain-compensation-instruction-constant")?,
        purpose: "Raw 0xfb00 insertion mask for the gain compensation byte at bits 8..15; other byte updates remain in the retained analysis".into(),
        applicability: "Only this function in the selected current PHY artifact".into(),
        expected_base: Some(base.clone()),
        actor: "source-byte-review".into(),
        reason: "Known operand checked against retained instruction/value record".into(),
    };
    let proposed = propose(
        &r,
        "propose-constant",
        &["knowledge", "propose-constant"],
        "constant",
        &constant_request,
    )?;
    let constant_entry = r.proposed("constant-claims")?;
    let base = r.accept(
        "accept-constant",
        &proposed,
        &constant_entry,
        "source-byte-review",
        "Confirmed exact known operand, not a hardware qualification claim",
    )?;
    r.export_data(
        "export-constant",
        &base,
        &constant_entry,
        "accepted-constant",
        0,
    )?;
    assert!(fs::read(run.join("accepted-constant/data.bin"))?.is_empty());
    let constant_revision = base.clone();

    // Authenticated ELF32 .rela.rodata records independently establish eleven
    // absolute references to local code labels. These are pointer observations,
    // not discovered function boundaries or reviewed callback ABI contracts.
    let pointer_section = named_section(&phy_i2c, ".rodata")?.clone();
    let pointer_layout = PointerTable {
        count: 11,
        stride: 4,
    };
    let pointer_selector = DataSelector::Section {
        section: pointer_section.index,
        offset: 0,
        length: 44,
    };
    let pointer_request = DataRequest {
        occurrence: r.occurrence(&phy_i2c),
        ranges: vec![pointer_selector.clone()],
        analyses: vec![],
        pointer_table: Some(pointer_layout.clone()),
    };
    let pointers: ExportDocument = r.request(
        "pointers",
        &["data"],
        &pointer_request,
        Some(&run.join("pointer-observations")),
    )?;
    let QuerySummary::Data { manifest } = &pointers.summary else {
        return Err(invalid("data summary"));
    };
    assert_eq!(
        manifest.pointers.as_ref().map(|p| p.defined_symbols),
        Some(11)
    );
    let pointer_values: Vec<DataRecord> =
        fs::read_to_string(run.join("pointer-observations/records.jsonl"))?
            .lines()
            .map(serde_json::from_str)
            .collect::<std::result::Result<Vec<DataRecord>, _>>()?
            .into_iter()
            .filter(|record| matches!(record, DataRecord::Pointer { .. }))
            .collect();
    assert_eq!(pointer_values.len(), 11);
    for (i, record) in pointer_values.iter().enumerate() {
        let expected = PointerValue::DefinedSymbol {
            symbol: SymbolId {
                object: phy_i2c.id.clone(),
                table: SymbolTableKind::Static,
                table_section: POINTER_SYMBOL_TABLE,
                index: POINTER_TARGETS[i],
            },
            addend: 0,
        };
        assert!(
            matches!(record, DataRecord::Pointer { index, offset, value, .. }
            if *index == i as u64 && *offset == 4 * i as u64 && *value == expected)
        );
    }
    assert_eq!(
        sha256(&fs::read(run.join("pointer-observations/data.bin"))?),
        POINTER_TABLE_SHA
    );
    let pointer_proposal = DataProposalRequest {
        analyses: vec![],
        occurrence: pointer_request.occurrence.clone(),
        subject: subject("phy.captured-code-pointer-table")?,
        selector: pointer_selector,
        layout: DataLayout::Pointers(pointer_layout),
        purpose: "Captured absolute relocation targets; no inferred function boundaries".into(),
        applicability: "Authenticated phy_i2c.o table only".into(),
        expected_base: Some(base.clone()),
        actor: "source-byte-review".into(),
        reason: "Independent ELF relocation identities and table digest".into(),
    };
    let proposed = propose(
        &r,
        "propose-pointers",
        &["knowledge", "propose-data"],
        "pointer-proposal",
        &pointer_proposal,
    )?;
    let pointer_entry = r.proposed("pointer-claims")?;
    let base = r.accept(
        "accept-pointers",
        &proposed,
        &pointer_entry,
        "source-byte-review",
        "Exact captured representation only",
    )?;
    r.export_data(
        "export-pointers",
        &base,
        &pointer_entry,
        "accepted-pointers",
        0,
    )?;
    let pointer_revision = base.clone();

    // Review only the structural path independently established by the ROM
    // instructions: lui/lw capture global 0x2f07fc3c, lw +8 loads the callback,
    // jalr invokes it. Neither signature nor semantic purpose follows from the bytes.
    let contract = InterfaceContract {
        root: callback_path.root.clone(),
        path: callback_path.path.clone(),
        layout_version: "captured-rom-callback/1".into(),
        layout_bytes: 12,
        pointer_bytes: 4,
        abi: CallAbi::RiscvInteger,
        index_domains: vec![],
        guards: vec![InterfaceGuard::CapturedPayload {
            payload: callback_summary.payload.clone(),
        }],
        slots: vec![InterfaceSlot {
            offset: callback_path.slot,
            name: "captured-slot-8".into(),
            semantic: None,
            signature: None,
        }],
        purpose: "Exact captured callback load path; signature and runtime target unknown".into(),
        applicability: "Authenticated ROM occurrence only; no hardware behavior assertion".into(),
    };
    let change = KnowledgeChange {
        expected_base: Some(base.clone()),
        actor: "source-instruction-review".into(),
        reason: "Independent ROM instruction offsets and operands".into(),
        action: KnowledgeAction::Propose {
            proposal: KnowledgeProposal {
                subject: subject("phy.captured-rom-callback")?,
                occurrence: callback_summary.occurrence.clone(),
                claim: KnowledgeClaim::Interface { contract: Box::new(contract) },
                evidence: vec![EvidenceRef::Analysis { analysis: callback_analysis.clone(), record: Some(callback_slot.record) }],
                note: Some("Structural identity only; no inferred signature, semantic binding or runtime guard satisfaction".into()),
            },
        },
    };
    let mut command = args(["knowledge", "apply", "--change"]);
    command.push(path_arg(&r.runner.doc("callback-proposal", &change)?));
    let proposed = r.knowledge("propose-callback", command)?;
    let callback_entry = r.proposed("callback-claims")?;
    let base = r.accept(
        "accept-callback",
        &proposed,
        &callback_entry,
        "source-instruction-review",
        "Confirmed physical load path only",
    )?;
    callback_query.knowledge = Some(base.clone());
    let matched_raw: serde_json::Value =
        r.request("callback-matched", &["interfaces"], &callback_query, None)?;
    let matched: RecordDocument<InterfaceObservation> =
        serde_json::from_value(matched_raw.clone())?;
    let QuerySummary::Interfaces {
        summary: matched_summary,
    } = &matched.summary
    else {
        unreachable!()
    };
    assert_eq!(
        (
            matched_summary.matched_accepted,
            matched_summary.unresolved_paths
        ),
        (1, 0)
    );
    let binding = &matched.records[0].value.bindings[0];
    assert_eq!(
        (&binding.assertion, binding.state),
        (&callback_entry.id, AssertionState::Accepted)
    );
    assert!(binding.signature.is_none() && binding.semantic.is_none());
    assert_eq!(
        matched.records[0].value.target,
        Some(AbstractValue::Unknown)
    );
    r.save(
        "callback-export",
        &["interfaces"],
        &callback_query,
        &run.join("callback-export.json"),
    )?;

    for directory in [
        "i2c-observations",
        "accepted-table",
        "initial",
        "accepted-constant",
        "accepted-pointers",
    ] {
        let d = run.join(directory);
        let manifest: DataManifest = serde_json::from_slice(&fs::read(d.join("manifest.json"))?)?;
        let object = fs::read(d.join("object.elf"))?;
        let data = fs::read(d.join("data.bin"))?;
        assert_eq!(
            ArtifactId::of_bytes(&object),
            manifest.payload,
            "{directory}"
        );
        for span in &manifest.spans {
            let range =
                &object[span.file_range.start as usize..][..span.file_range.length as usize];
            assert_eq!(ArtifactId::of_bytes(range), span.digest, "{directory}");
            assert_eq!(
                range,
                &data[span.export_offset as usize..][..span.file_range.length as usize]
            );
        }
    }
    r.call("doctor", args(["doctor"]))?;
    let moved = run.join("moved");
    fs::rename(&r.runner.project, &moved)?;
    r.runner.project = moved;
    r.export_data("moved", &base, &table_entry, "moved-table", 0)?;
    let backup = run.join("project.blobray");
    let mut command = args(["backup", "--output"]);
    command.push(path_arg(&backup));
    r.call("backup", command)?;
    r.runner.project = run.join("restored");
    let mut command = args(["restore", "--backup"]);
    command.push(path_arg(&backup));
    r.call("restore", command)?;
    r.export_data("restored", &base, &table_entry, "restored-table", 0)?;
    r.export_data(
        "restored-constant",
        &constant_revision,
        &constant_entry,
        "restored-constant",
        0,
    )?;
    r.export_data(
        "restored-pointers",
        &pointer_revision,
        &pointer_entry,
        "restored-pointers",
        0,
    )?;
    for name in ["object.elf", "data.bin", "records.jsonl", "manifest.json"] {
        assert!(r.same_bytes(
            &format!("restored-pointers/{name}"),
            &format!("accepted-pointers/{name}")
        )?);
        assert!(r.same_bytes(
            &format!("restored-constant/{name}"),
            &format!("accepted-constant/{name}")
        )?);
    }
    r.call("restored-doctor", args(["doctor"]))?;
    for name in ["object.elf", "data.bin", "records.jsonl"] {
        assert!(r.same_bytes(
            &format!("accepted-table/{name}"),
            &format!("restored-table/{name}")
        )?);
    }
    // The table export differs only in the knowledge revision it was read from.
    let mut accepted: DataManifest =
        serde_json::from_slice(&fs::read(run.join("accepted-table/manifest.json"))?)?;
    let restored: DataManifest =
        serde_json::from_slice(&fs::read(run.join("restored-table/manifest.json"))?)?;
    accepted.knowledge = restored.knowledge.clone();
    assert_eq!(accepted, restored);
    fs::write(
        run.join("acceptance.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "revision": r.revision, "publication": publication, "knowledge": base,
            "table": table_entry.id, "constant": constant_entry.id, "constant_value": value,
            "sources_removed": true, "moved_restored": true, "requests": queries,
        }))?,
    )?;

    // Reading restored composed evidence needs no linking, original files or replay.
    assert_eq!(
        r.facts("restored-research", &linked_analysis)?,
        linked_facts
    );
    assert_eq!(
        r.facts("restored-finite-facts", &finite_analysis)?,
        finite_records
    );
    assert_eq!(
        r.facts("restored-callback-facts", &callback_analysis)?,
        callback_records
    );
    let mut command = args([
        "export-analysis",
        "--id",
        finite_analysis.as_str(),
        "--output",
    ]);
    command.push(path_arg(&run.join("restored-finite-export")));
    r.call("restored-finite-export", command)?;
    for file in ["manifest.json", "records.jsonl"] {
        assert!(r.same_bytes(
            &format!("finite-export/{file}"),
            &format!("restored-finite-export/{file}")
        )?);
    }
    let restored_raw: serde_json::Value = r.request(
        "restored-interfaces",
        &["interfaces"],
        &callback_query,
        None,
    )?;
    assert_eq!(restored_raw, matched_raw);
    for (name, command, request, export) in [
        (
            "interface",
            "interfaces",
            serde_json::to_value(&callback_query)?,
            "callback-export.json",
        ),
        (
            "navigation",
            "navigate",
            serde_json::to_value(&nav_request)?,
            "navigation-export.json",
        ),
        (
            "register-catalog",
            "registers",
            serde_json::to_value(&register_request)?,
            "register-catalog-export.json",
        ),
        (
            "memory-slice",
            "memory-slice",
            serde_json::to_value(&slice_request)?,
            "memory-slice-export.json",
        ),
        (
            "flow",
            "flow",
            serde_json::to_value(&flow_request)?,
            "flow-export.json",
        ),
        (
            "static-trace",
            "trace",
            serde_json::to_value(&trace_request)?,
            "static-trace-export.json",
        ),
    ] {
        let restored_export = run.join(format!("restored-{export}"));
        r.save(
            &format!("restored-{name}-export"),
            &[command],
            &request,
            &restored_export,
        )?;
        assert!(
            r.same_bytes(export, &format!("restored-{export}"))?,
            "{name}"
        );
    }
    let restored_navigation: RecordDocument<NavigationRecord> =
        r.request("restored-navigation", &["navigate"], &nav_request, None)?;
    assert_eq!(
        serde_json::to_value(&restored_navigation)?,
        serde_json::to_value(&nav_calls)?
    );
    let restored_flow: RecordDocument<FlowRecord> =
        r.request("restored-flow", &["flow"], &flow_request, None)?;
    assert_eq!(
        serde_json::to_value(&restored_flow)?,
        serde_json::to_value(&flow_effects)?
    );
    let restored_registers: RecordDocument<RegisterRecord> = r.request(
        "restored-register-catalog",
        &["registers"],
        &register_request,
        None,
    )?;
    assert_eq!(
        serde_json::to_value(&restored_registers)?,
        serde_json::to_value(&register_catalog)?
    );
    let restored_slice: RecordDocument<MemorySliceRecord> = r.request(
        "restored-memory-slice",
        &["memory-slice"],
        &slice_request,
        None,
    )?;
    assert_eq!(
        serde_json::to_value(&restored_slice)?,
        serde_json::to_value(&memory_slice)?
    );
    let restored_trace: RecordDocument<TraceRecord> =
        r.request("restored-static-trace", &["trace"], &trace_request, None)?;
    assert_eq!(
        serde_json::to_value(&restored_trace)?,
        serde_json::to_value(&static_trace)?
    );
    let mut command = args(["ir", "show", saved_ir.as_str(), "--output"]);
    command.push(path_arg(&run.join("restored-semantic-ir-export.json")));
    r.call("save-restored-semantic-ir", command)?;
    assert!(r.same_bytes(
        "semantic-ir-export.json",
        "restored-semantic-ir-export.json"
    )?);
    Ok(run)
}

/// Submit a knowledge proposal request and return the proposed revision.
fn propose(
    r: &Research,
    name: &str,
    command: &[&str],
    document: &str,
    request: &impl Serialize,
) -> Result<KnowledgeRevisionId> {
    let mut argv: Vec<OsString> = command.iter().map(os).collect();
    argv.push("--request".into());
    argv.push(path_arg(&r.runner.doc(document, request)?));
    r.knowledge(name, argv)
}
