//! Lossless capture adapter for legacy workspaces. Unsupported semantics stay explicit.
use crate::*;
use std::{collections::BTreeSet, fs, io::Write};
use toml_edit::{DocumentMut, Item};
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyRequest {
    pub manifest: OriginPath,
    pub run_spec: Option<OriginPath>,
    /// Additional private cache/evidence roots not declared by the legacy manifest.
    #[serde(default)]
    pub roots: Vec<OriginPath>,
    /// Exact source-role bindings; additional to a schema-1 run spec, never guessed by filename.
    #[serde(default)]
    pub inputs: Vec<ImportBinding>,
    pub target: Target,
}
struct Queue {
    pending: Vec<PathBuf>,
    seen: BTreeSet<PathBuf>,
    bytes: usize,
}
impl Queue {
    fn push(&mut self, path: PathBuf) -> Result<()> {
        let absolute = std::path::absolute(path).map_err(storage_io)?;
        let normalized = normalize(&absolute);
        if !self.seen.contains(&normalized) {
            if self.seen.len() >= 16384
                || normalized.as_os_str().len().max(absolute.as_os_str().len()) > 4096
                || self.bytes + normalized.as_os_str().len().max(absolute.as_os_str().len())
                    > 4 * 1024 * 1024
            {
                return Err(Error::new(
                    ErrorCode::ResourceLimited,
                    "legacy discovery exceeds 16384 paths or 4096-byte path",
                ));
            }
            self.bytes += normalized.as_os_str().len().max(absolute.as_os_str().len());
            self.seen.insert(normalized.clone());
            self.pending.push(absolute);
        }
        Ok(())
    }
}
fn normalize(path: &Path) -> PathBuf {
    // Resolve parent components using filesystem semantics: lexical `..` would
    // change the target when an intermediate component is a symlink. Do not
    // resolve the final component; a symlink has its own preservation record.
    match (
        path.parent().and_then(|p| p.canonicalize().ok()),
        path.file_name(),
    ) {
        (Some(parent), Some(name)) => parent.join(name),
        _ => path.to_path_buf(), // A missing parent remains an explicit unavailable capture.
    }
}
fn unresolved(reason: impl Into<String>) -> LegacyOutcome {
    LegacyOutcome::PreservedUnresolved {
        reason: reason.into(),
    }
}
fn unsupported(reason: impl Into<String>) -> LegacyOutcome {
    LegacyOutcome::Unsupported {
        reason: reason.into(),
    }
}
fn record(
    file: &mut blobray_store::TemporaryFile,
    value: &LegacyRecord,
    manifest: &mut LegacyManifest,
    control: &mut dyn RunControl,
) -> Result<()> {
    control.checkpoint(1)?;
    write_control_message(&mut *file, value)?;
    file.write_all(b"\n").map_err(storage_io)?;
    manifest.record_count += 1;
    match value.outcome {
        LegacyOutcome::Converted { .. } => manifest.converted += 1,
        LegacyOutcome::MissingPayload { .. } => manifest.missing += 1,
        LegacyOutcome::Unsupported { .. } => manifest.unsupported += 1,
        _ => (),
    };
    Ok(())
}
fn document<'a>(
    stage: &Staging,
    capture: &Capture,
    memory: &'a WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<(DocumentMut, MemoryReservation<'a>)> {
    let Capture::Captured { artifact, length } = capture else {
        return Err(Error::new(
            ErrorCode::Unavailable,
            "legacy document unavailable",
        ));
    };
    if *length > 8 * 1024 * 1024 {
        return Err(Error::new(
            ErrorCode::ResourceLimited,
            "legacy TOML document exceeds 8 MiB",
        ));
    }
    let reservation = memory.reserve(
        length
            .checked_mul(32)
            .and_then(|n| n.checked_add(65536))
            .ok_or_else(|| {
                Error::new(ErrorCode::ResourceLimited, "legacy document size overflow")
            })?,
        control.position(),
    )?;
    let source = stage.open_payload(artifact, control)?;
    let bytes = read_scratch(&source, memory, control)?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| Error::new(ErrorCode::InvalidRequest, "legacy TOML is not UTF-8"))?;
    let doc = text
        .parse::<DocumentMut>()
        .map_err(|e| Error::new(ErrorCode::InvalidRequest, e.to_string()))?;
    Ok((doc, reservation))
}
fn path_key(key: &str) -> bool {
    matches!(
        key,
        "target-spec"
            | "ecosystem-packs"
            | "chip-pack"
            | "run-spec"
            | "verification-addon"
            | "pack"
            | "packs"
            | "default-pack"
            | "path"
            | "output"
            | "report"
            | "evidence-index"
            | "policy"
            | "model-inputs"
            | "profiles"
            | "dispositions"
            | "baselines"
            | "schema-path"
            | "svd"
            | "reference"
            | "cache"
            | "cache-root"
    )
}
fn discover(
    doc: &DocumentMut,
    path: &Path,
    queue: &mut Queue,
    control: &mut dyn RunControl,
) -> Result<()> {
    // Explicit worklist bounded by document size; no input-controlled call-stack recursion.
    let mut work: Vec<(&str, &Item, u32)> = doc.iter().map(|(k, v)| (k, v, 0)).collect();
    while let Some((key, item, depth)) = work.pop() {
        control.checkpoint(1)?;
        if depth > 64 {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "legacy TOML nesting exceeds 64",
            ));
        }
        if let Some(table) = item.as_table() {
            for (k, v) in table.iter() {
                work.push((k, v, depth + 1));
            }
        }
        if let Some(tables) = item.as_array_of_tables() {
            for table in tables {
                for (k, v) in table.iter() {
                    work.push((k, v, depth + 1));
                }
            }
        }
        if !path_key(key) {
            continue;
        }
        let base = path.parent().unwrap();
        if let Some(value) = item.as_str() {
            queue.push(base.join(value))?;
        }
        if let Some(values) = item.as_array() {
            for value in values {
                if let Some(value) = value.as_str() {
                    // Legacy profile names are IDs; only explicit file paths are dependencies.
                    if key != "profiles" || value.contains('/') || value.ends_with(".toml") {
                        queue.push(base.join(value))?;
                    }
                }
            }
        }
    }
    Ok(())
}
fn run_inputs(doc: &DocumentMut, path: &Path, inputs: &mut Vec<ImportBinding>) -> Result<()> {
    if doc.get("schema").and_then(Item::as_integer) != Some(1) {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "run spec requires schema 1",
        ));
    }
    let rows = doc
        .get("inputs")
        .and_then(Item::as_array_of_tables)
        .ok_or_else(|| Error::new(ErrorCode::InvalidRequest, "run spec inputs missing"))?;
    for row in rows {
        let role = row
            .get("role")
            .and_then(Item::as_str)
            .ok_or_else(|| Error::new(ErrorCode::InvalidRequest, "run spec role missing"))?;
        let source = row
            .get("path")
            .and_then(Item::as_str)
            .ok_or_else(|| Error::new(ErrorCode::InvalidRequest, "run spec path missing"))?;
        inputs.push(ImportBinding {
            role: role.into(),
            origin: OriginPath::from_path(&path.parent().unwrap().join(source)),
            expected: None,
        });
        write_control_message(std::io::sink(), inputs)?;
    }
    Ok(())
}
fn owner() -> OwnerIdentity {
    OwnerIdentity {
        pid: std::process::id(),
        start_ticks: 0,
        boot_id: "private-legacy-build".into(),
    }
}
/// Builds a complete new project under the query's owned workspace. Source files are read only.
pub(crate) fn prepare_legacy(
    stage: &Path,
    work: &QueryWork,
    request: &LegacyRequest,
    decoder: &dyn FunctionDecoder,
    memory: &WorkingMemory,
    disk: &blobray_store::TemporaryBudget,
    control: &mut dyn RunControl,
) -> Result<PreservationSummary> {
    // Bound path catalog and TOML control state separately from per-document reservations.
    let _catalog = memory.reserve(16 * 1024 * 1024, control.position())?;
    disk.reserve(16 * 1024 * 1024)?; // bounded private SQLite metadata and rollback journals
    let destination = stage.join("restored");
    let project = Project::create(&destination)?;
    let mut writer = project.writer()?;
    let state = destination.join(".blobray-next");
    let mut capture_stage = Staging::with_temporary_budget(&state, disk.clone())?;
    let source = std::path::absolute(request.manifest.to_path()?).map_err(storage_io)?;
    let mut private_work = work.clone();
    private_work.project = OriginPath::from_path(stage);
    let work = &private_work;
    let mut queue = Queue {
        pending: Vec::new(),
        seen: BTreeSet::new(),
        bytes: 0,
    };
    queue.push(source.parent().unwrap().to_path_buf())?;
    for root in &request.roots {
        queue.push(root.to_path()?)?;
    }
    let mut inputs = request.inputs.clone();
    // Capture/parse the entry point once; all interpretation uses the retained bytes.
    let source_capture = capture_stage.capture_controlled(&source, None, control)?;
    let (source_doc, _source_memory) = document(&capture_stage, &source_capture, memory, control)?;
    let run_spec = request
        .run_spec
        .as_ref()
        .map(OriginPath::to_path)
        .transpose()?
        .or_else(|| {
            source_doc
                .get("run-spec")
                .and_then(Item::as_str)
                .map(|s| source.parent().unwrap().join(s))
        });
    if let Some(run_spec) = run_spec {
        queue.push(run_spec.clone())?;
        let capture = capture_stage.capture_controlled(&run_spec, None, control)?;
        let (doc, _reservation) = document(&capture_stage, &capture, memory, control)?;
        run_inputs(&doc, &run_spec, &mut inputs)?;
    }
    for input in &inputs {
        queue.push(input.origin.to_path()?)?;
    }
    if !inputs.is_empty() {
        let (mut run, run_stage) = writer.register_operation(
            work.budget.clone(),
            owner(),
            RunOperation::Import,
            |_| {},
        )?;
        run.state = RunState::Running;
        writer.update_run(&run)?;
        let receipt = crate::prepare_stream(
            &run_stage,
            ImportWork {
                schema: 2,
                run: run.id.clone(),
                project: project.id().clone(),
                parent: None,
                target: request.target,
                inputs,
                budget: work.budget.clone(),
                started_ms: work.started_ms,
                deadline_ms: work.deadline_ms,
            },
            memory,
            control,
            disk.clone(),
        )?;
        run.state = RunState::Validating;
        writer.update_run(&run)?;
        let retained = writer.retain_candidate(&run, &receipt, control)?;
        writer.publish_run(&mut run, retained)?;
        writer.cleanup_stage(&run.id)?;
    }
    let mut catalog = disk.temporary(&state.join("staging"))?;
    let mut manifest = LegacyManifest {
        schema: 1,
        project: project.id().clone(),
        source: OriginPath::from_path(&source),
        records: ArtifactId::of_bytes(b"pending"),
        record_count: 0,
        converted: 0,
        missing: 0,
        unsupported: 0,
    };
    let mut files = 0u64;
    let mut bytes = 0u64;
    while let Some(path) = queue.pending.pop() {
        control.checkpoint(1)?;
        let metadata = fs::symlink_metadata(&path);
        if let Ok(metadata) = &metadata {
            if metadata.is_symlink() {
                let target = fs::read_link(&path).map_err(storage_io)?;
                queue.push(path.parent().unwrap().join(&target))?;
                let mut file = disk.temporary(&state.join("staging"))?;
                let encoded = serde_json::to_vec(&OriginPath::from_path(&target))
                    .map_err(|e| Error::new(ErrorCode::Integrity, e.to_string()))?;
                file.write_all(&encoded).map_err(storage_io)?;
                let id = capture_stage.retain_temporary(file, control)?;
                record(
                    &mut catalog,
                    &LegacyRecord {
                        origin: OriginPath::from_path(&path),
                        selector: "symlink-target".into(),
                        capture: Capture::Captured {
                            artifact: id,
                            length: encoded.len() as u64,
                        },
                        outcome: unresolved(
                            "link target retained separately; path alias is not semantic identity",
                        ),
                    },
                    &mut manifest,
                    control,
                )?;
                continue;
            }
            if metadata.is_dir() {
                for entry in fs::read_dir(&path).map_err(storage_io)? {
                    control.checkpoint(1)?;
                    queue.push(entry.map_err(storage_io)?.path())?;
                }
                continue;
            }
        }
        let capture = capture_stage.capture_controlled(&path, None, control)?;
        let base = LegacyRecord {
            origin: OriginPath::from_path(&path),
            selector: "file".into(),
            capture: capture.clone(),
            outcome: unresolved(
                "original bytes preserved; semantic conversion is recorded separately",
            ),
        };
        let Capture::Captured { artifact, length } = &capture else {
            let reason = if let Capture::Unavailable { diagnostic } = &capture {
                diagnostic.message.clone()
            } else {
                unreachable!()
            };
            record(
                &mut catalog,
                &LegacyRecord {
                    outcome: LegacyOutcome::MissingPayload { reason },
                    ..base
                },
                &mut manifest,
                control,
            )?;
            continue;
        };
        files += 1;
        bytes = bytes
            .checked_add(*length)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "legacy size overflow"))?;
        if path.extension().is_some_and(|e| e == "toml") {
            let parsed = document(&capture_stage, &capture, memory, control);
            match parsed {
                Ok((doc, _reservation)) => {
                    discover(&doc, &path, &mut queue, control)?;
                    record(&mut catalog, &base, &mut manifest, control)?;
                    // Every top-level table/value and every array-table item has an explicit outcome.
                    for (key, item) in doc.iter() {
                        if let Some(rows) = item.as_array_of_tables() {
                            for (n, row) in rows.iter().enumerate() {
                                let outcome = if key == "boundaries"
                                    && doc.get("schema").and_then(Item::as_integer) == Some(1)
                                {
                                    convert_boundary(
                                        &project,
                                        &mut writer,
                                        work,
                                        row,
                                        artifact,
                                        decoder,
                                        memory,
                                        disk,
                                        control,
                                    )?
                                } else {
                                    unsupported(
                                        "legacy table retained; no active validator for this representation",
                                    )
                                };
                                record(
                                    &mut catalog,
                                    &LegacyRecord {
                                        selector: format!("{key}[{n}]"),
                                        outcome,
                                        ..base.clone()
                                    },
                                    &mut manifest,
                                    control,
                                )?;
                            }
                        } else {
                            record(
                                &mut catalog,
                                &LegacyRecord {
                                    selector: key.into(),
                                    outcome: unsupported(
                                        "legacy field/table retained without semantic activation",
                                    ),
                                    ..base.clone()
                                },
                                &mut manifest,
                                control,
                            )?;
                        }
                    }
                }
                Err(error) if error.code == ErrorCode::InvalidRequest => record(
                    &mut catalog,
                    &LegacyRecord {
                        outcome: unsupported(format!("unparsed TOML preserved: {}", error.message)),
                        ..base
                    },
                    &mut manifest,
                    control,
                )?,
                Err(error) => return Err(error),
            }
        } else {
            record(
                &mut catalog,
                &LegacyRecord {
                    outcome: unsupported(
                        "opaque file retained verbatim, including compressed history and cached evidence; embedded references are not interpreted",
                    ),
                    ..base
                },
                &mut manifest,
                control,
            )?;
        }
    }
    manifest.records = capture_stage.retain_temporary(catalog, control)?;
    writer.retain_legacy_catalog(&manifest, &capture_stage, control)?;
    drop(writer);
    project.visit_legacy(control, &mut |_, _| Ok(()))?;
    Ok(PreservationSummary {
        project: project.id().clone(),
        objects: files,
        bytes,
    })
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct Boundary {
    source: String,
    artifact_sha256: ArtifactId,
    member: Option<String>,
    section: String,
    entry_offset: u64,
    end_exclusive_offset: u64,
    status: String,
    name: Option<String>,
    reason: Option<String>,
}
struct MatchBoundary<'a> {
    boundary: &'a Boundary,
    input: u64,
    selected: bool,
    object: bool,
    section: Option<(u32, u64)>,
    payload: Option<ArtifactId>,
    found: Option<(u64, SymbolId, ArtifactId, u64)>,
    count: u64,
}
impl InventorySink for MatchBoundary<'_> {
    fn input(&mut self, n: u64, input: &InputRecord, _: &mut dyn RunControl) -> Result<()> {
        self.input = n;
        self.selected = (input.role == self.boundary.source
            || input.role == format!("source-artifact:{}", self.boundary.source)
            || input.role == format!("source-inventory:{}", self.boundary.source))
            && input.capture.artifact() == Some(&self.boundary.artifact_sha256);
        Ok(())
    }
    fn object(&mut self, object: &ObjectInventory, _: &mut dyn RunControl) -> Result<()> {
        self.object = self.selected
            && object.name.as_deref() == self.boundary.member.as_ref().map(|s| s.as_bytes());
        self.payload = object.content.clone();
        self.section = None;
        Ok(())
    }
}
impl ElfSink for MatchBoundary<'_> {
    fn section(&mut self, s: &SectionRecord, _: &mut dyn RunControl) -> Result<()> {
        if self.object && s.name.as_deref() == Some(self.boundary.section.as_bytes()) {
            if self.section.is_some() {
                self.object = false;
            } else {
                self.section = Some((s.index, s.file_offset));
            }
        }
        Ok(())
    }
    fn symbol(&mut self, s: &SymbolRecord, _: &mut dyn RunControl) -> Result<()> {
        let section = if s.raw_section == 0xffff {
            s.extended_section
        } else {
            Some(u32::from(s.raw_section))
        };
        if self.object
            && s.symbol_type == 2
            && self
                .section
                .is_some_and(|(index, _)| Some(index) == section)
            && s.value == self.boundary.entry_offset
        {
            self.count += 1;
            if let Some(payload) = &self.payload {
                self.found = Some((
                    self.input,
                    s.id.clone(),
                    payload.clone(),
                    self.section.unwrap().1,
                ));
            }
        }
        Ok(())
    }
    fn relocation(&mut self, _: &RelocationRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn diagnostic(&mut self, _: &Diagnostic, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
}
#[allow(clippy::too_many_arguments)]
fn private_change(
    writer: &mut blobray_store::Writer,
    project: &Project,
    work: &QueryWork,
    change: KnowledgeChange,
    decoder: &dyn FunctionDecoder,
    memory: &WorkingMemory,
    disk: &blobray_store::TemporaryBudget,
    control: &mut dyn RunControl,
) -> Result<(KnowledgeRevisionId, AssertionId)> {
    writer.check_metadata_capacity(4 * 1024 * 1024)?;
    let (mut run, stage) = writer.register_operation(
        work.budget.clone(),
        owner(),
        RunOperation::Knowledge {
            change: change.clone(),
        },
        |_| {},
    )?;
    let result: Result<(KnowledgeRevisionId, AssertionId)> = (|| {
        run.state = RunState::Running;
        writer.update_run(&run)?;
        let receipt = crate::knowledge::prepare_with(
            &stage,
            &KnowledgeWork {
                schema: 1,
                run: run.id.clone(),
                project: OriginPath::from_path(&work.project.to_path()?),
                change,
                budget: work.budget.clone(),
                started_ms: work.started_ms,
                deadline_ms: work.deadline_ms,
            },
            decoder,
            memory,
            disk,
            control,
        )?;
        run.state = RunState::Validating;
        writer.update_run(&run)?;
        let retained = writer.retain_knowledge(&run, &receipt, control)?;
        writer.publish_knowledge(&mut run, retained, control)?;
        let event = project.knowledge_manifest(&receipt.revision, control)?;
        Ok((receipt.revision, event.assertion))
    })();
    if let Err(error) = &result {
        run.state = RunState::Failed;
        run.error = Some(error.clone());
        writer.update_run(&run)?;
    }
    writer.cleanup_stage(&run.id)?;
    result
}
#[allow(clippy::too_many_arguments)]
fn convert_boundary(
    project: &Project,
    writer: &mut blobray_store::Writer,
    work: &QueryWork,
    row: &toml_edit::Table,
    document: &ArtifactId,
    decoder: &dyn FunctionDecoder,
    memory: &WorkingMemory,
    disk: &blobray_store::TemporaryBudget,
    control: &mut dyn RunControl,
) -> Result<LegacyOutcome> {
    let boundary: Boundary = match toml_edit::de::from_str(&row.to_string()) {
        Ok(value) => value,
        Err(error) => {
            return Ok(unsupported(format!(
                "boundary representation is unsupported: {error}"
            )));
        }
    };
    if !matches!(
        boundary.status.as_str(),
        "accepted" | "rejected" | "unreviewed"
    ) {
        return Ok(unsupported("unknown boundary review status"));
    }
    let Some(revision) = project.current()? else {
        return Ok(unresolved("no captured source revision for exact binding"));
    };
    let mut scan = MatchBoundary {
        boundary: &boundary,
        input: 0,
        selected: false,
        object: false,
        section: None,
        payload: None,
        found: None,
        count: 0,
    };
    project.read_inventory(Some(&revision), memory, control, &mut scan)?;
    if scan.count != 1 {
        return Ok(unresolved(format!(
            "exact source digest/member/section/entry has {} function occurrences",
            scan.count
        )));
    }
    let Some((input, symbol, payload, file_offset)) = scan.found else {
        return Ok(unresolved("matched occurrence has no retained bytes"));
    };
    let Some(length) = boundary
        .end_exclusive_offset
        .checked_sub(boundary.entry_offset)
        .filter(|n| *n > 0)
    else {
        return Ok(unresolved("invalid legacy extent"));
    };
    let Some(start) = file_offset.checked_add(boundary.entry_offset) else {
        return Ok(unresolved("legacy byte range overflows"));
    };
    let subject: SubjectId = format!(
        "legacy.{}.{}.{}",
        boundary.artifact_sha256, input, symbol.index
    )
    .try_into()?;
    let proposal = KnowledgeProposal {
        subject,
        occurrence: KnowledgeOccurrence {
            revision,
            source: FunctionSource::Input { input },
            object: symbol.object.clone(),
            symbol: Some(symbol),
        },
        claim: KnowledgeClaim::FunctionExtent {
            extent: CodeRange {
                start: boundary.entry_offset,
                length,
            },
        },
        evidence: vec![
            EvidenceRef::Source {
                payload,
                range: CodeRange { start, length },
            },
            EvidenceRef::Document {
                payload: document.clone(),
            },
        ],
        note: boundary.name.clone(),
    };
    let reason = boundary
        .reason
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            "preserved legacy boundary decision; no reviewer authentication asserted".into()
        });
    let mut private_work = work.clone();
    private_work.project = OriginPath::from_path(&work.project.to_path()?.join("restored"));
    let attempt: Result<LegacyOutcome> = (|| {
        let (mut head, assertion) = private_change(
            writer,
            project,
            &private_work,
            KnowledgeChange {
                expected_base: project.current_knowledge()?,
                actor: "legacy-import".into(),
                reason: reason.clone(),
                action: KnowledgeAction::Propose { proposal },
            },
            decoder,
            memory,
            disk,
            control,
        )?;
        if boundary.status != "unreviewed" {
            head = private_change(
                writer,
                project,
                &private_work,
                KnowledgeChange {
                    expected_base: Some(head),
                    actor: "legacy-import".into(),
                    reason,
                    action: KnowledgeAction::Review {
                        assertion,
                        decision: if boundary.status == "accepted" {
                            ReviewDecision::Accept
                        } else {
                            ReviewDecision::Reject
                        },
                        supersedes: None,
                    },
                },
                decoder,
                memory,
                disk,
                control,
            )?
            .0;
        }
        Ok(LegacyOutcome::Converted { revision: head })
    })();
    match attempt {
        Err(error)
            if matches!(
                error.code,
                ErrorCode::InvalidRequest
                    | ErrorCode::Conflict
                    | ErrorCode::NotFound
                    | ErrorCode::Incompatible
                    | ErrorCode::NeedsExtent
            ) =>
        {
            Ok(unresolved(error.message))
        }
        other => other,
    }
}
