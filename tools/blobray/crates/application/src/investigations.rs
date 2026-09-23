//! Selection owns coverage; execution reuses the function engine and a single
//! run's budgets. Plans contain a digest, never a resident library-sized list.
use crate::*;
use blobray_store::{EntryDigest, investigation_plan, validate_investigation_plan};
use std::io::Write;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvestigationWork {
    pub schema: u32,
    pub run: RunId,
    pub project: OriginPath,
    pub plan: InvestigationPlan,
    pub budget: ResourceBudget,
    pub started_ms: u64,
    pub deadline_ms: u64,
}
type EntrySink<'a> = dyn FnMut(&PlanEntry, &mut dyn RunControl) -> Result<()> + 'a;
struct Enumeration<'a> {
    request: &'a InvestigationRequest,
    sink: &'a mut EntrySink<'a>,
    digest: EntryDigest,
    input: u64,
    selected: bool,
    seen_inputs: Vec<bool>,
    used_extents: Vec<bool>,
    object: Option<ObjectInventory>,
    executable: Vec<u32>,
    functions: u64,
}
impl Enumeration<'_> {
    fn emit(&mut self, entry: PlanEntry, c: &mut dyn RunControl) -> Result<()> {
        c.checkpoint(1)?;
        self.digest.include(&entry)?;
        (self.sink)(&entry, c)
    }
    fn gap(&mut self, reason: impl Into<String>, c: &mut dyn RunControl) -> Result<()> {
        if let Some(image) = &self.request.image {
            return self.emit(
                PlanEntry::ImageGap {
                    image: image.clone(),
                    reason: reason.into(),
                },
                c,
            );
        }
        self.emit(
            PlanEntry::Gap {
                input: self.input,
                object: self.object.as_ref().map(|o| o.id.clone()),
                reason: reason.into(),
            },
            c,
        )
    }
    fn finish_object(&mut self, c: &mut dyn RunControl) -> Result<()> {
        if self.selected
            && self.functions == 0
            && (!self.executable.is_empty()
                || self.request.image.is_some()
                || self
                    .object
                    .as_ref()
                    .and_then(|o| o.elf.as_ref())
                    .is_some_and(|elf| elf.object_type == 2))
        {
            self.gap(
                "executable sections have no defined static function symbols",
                c,
            )?;
        }
        self.object = None;
        self.executable.clear();
        self.functions = 0;
        Ok(())
    }
}
impl InventorySink for Enumeration<'_> {
    fn input(&mut self, n: u64, r: &InputRecord, c: &mut dyn RunControl) -> Result<()> {
        self.finish_object(c)?;
        self.input = n;
        self.selected = match &self.request.inputs {
            None => true,
            Some(inputs) => inputs.iter().position(|i| *i == n).is_some_and(|i| {
                self.seen_inputs[i] = true;
                true
            }),
        };
        if self.selected {
            self.emit(
                PlanEntry::Input {
                    input: n,
                    role: r.role.clone(),
                },
                c,
            )?;
            if let Capture::Unavailable { diagnostic } = &r.capture {
                self.gap(format!("input unavailable: {}", diagnostic.message), c)?;
            }
        }
        Ok(())
    }
    fn container(
        &mut self,
        kind: ContainerKind,
        complete: bool,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        if self.selected {
            if !complete {
                self.gap("archive member enumeration incomplete", c)?;
            }
            if kind == ContainerKind::Unsupported {
                self.gap("unsupported input format", c)?;
            }
        }
        Ok(())
    }
    fn object(&mut self, o: &ObjectInventory, c: &mut dyn RunControl) -> Result<()> {
        self.finish_object(c)?;
        if !self.selected {
            return Ok(());
        }
        self.object = Some(o.clone());
        let supported = o.elf.as_ref().is_some_and(|e| {
            e.bits == 32 && e.little_endian && e.machine == 243 && matches!(e.object_type, 1 | 2)
        });
        self.emit(
            PlanEntry::Object {
                input: self.input,
                object: o.id.clone(),
                payload: o.content.clone(),
                supported,
            },
            c,
        )?;
        if o.content.is_none() {
            self.gap("object payload unavailable", c)?;
        }
        if !supported {
            self.gap(
                "object is not a supported ELF32 little-endian RISC-V relocatable or executable",
                c,
            )?;
        }
        Ok(())
    }
}
impl ElfSink for Enumeration<'_> {
    fn section(&mut self, r: &SectionRecord, c: &mut dyn RunControl) -> Result<()> {
        if self.selected && r.flags & 4 != 0 && r.size != 0 {
            if self.executable.len() == self.executable.capacity() {
                return Err(Error::new(
                    ErrorCode::ResourceLimited,
                    "executable section capacity exhausted",
                ));
            }
            match self.executable.binary_search(&r.index) {
                Ok(_) => return Err(Error::new(ErrorCode::Integrity, "duplicate section index")),
                Err(i) => self.executable.insert(i, r.index),
            }
        }
        c.checkpoint(1)
    }
    fn symbol(&mut self, r: &SymbolRecord, c: &mut dyn RunControl) -> Result<()> {
        if !self.selected || r.symbol_type != 2 || r.id.table != SymbolTableKind::Static {
            return Ok(());
        }
        let section = if r.raw_section == 0xffff {
            r.extended_section
        } else if r.raw_section > 0 && r.raw_section < 0xff00 {
            Some(r.raw_section as u32)
        } else {
            None
        };
        if !section.is_some_and(|s| self.executable.binary_search(&s).is_ok()) {
            return Ok(());
        }
        self.functions += 1;
        let o = self
            .object
            .as_ref()
            .ok_or_else(|| Error::new(ErrorCode::Integrity, "function without object"))?;
        let Some(payload) = o.content.clone() else {
            return self.gap("function payload unavailable", c);
        };
        let source = self.request.image.as_ref().map_or(
            FunctionSource::Input { input: self.input },
            |image| FunctionSource::Image {
                image: image.clone(),
            },
        );
        let extent = self
            .request
            .extents
            .iter()
            .position(|e| e.source == source && e.symbol == r.id)
            .map(|i| {
                self.used_extents[i] = true;
                self.request.extents[i].extent
            });
        self.emit(
            PlanEntry::Function {
                address_space: if self.request.image.is_some()
                    || o.elf.as_ref().is_some_and(|e| e.object_type == 2)
                {
                    CodeAddressSpace::Image
                } else {
                    CodeAddressSpace::Section
                },
                declared_extent: extent.unwrap_or(CodeRange {
                    start: r.value,
                    length: r.size,
                }),
                request: FunctionRequest {
                    research: None,
                    revision: self.request.revision.clone(),
                    source,
                    symbol: r.id.clone(),
                    extent,
                },
                name: r.name.clone(),
                payload,
            },
            c,
        )
    }
    fn relocation(&mut self, _: &RelocationRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn diagnostic(&mut self, r: &Diagnostic, c: &mut dyn RunControl) -> Result<()> {
        if self.selected {
            self.gap(format!("{:?}: {}: {}", r.code, r.context, r.message), c)?;
        }
        Ok(())
    }
}
pub(crate) fn enumerate(
    project: &Project,
    request: &InvestigationRequest,
    producer: &FunctionProducer,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    sink: &mut EntrySink<'_>,
) -> Result<InvestigationPlan> {
    write_control_message(std::io::sink(), request)?;
    let original_request = request;
    if request.image.is_some() && request.inputs.is_some() {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "image selection cannot include object inputs or source-object reviewed extents",
        ));
    }
    let _review_memory = memory.reserve(2 * 1024 * 1024, control.position())?;
    let mut resolved = request.clone();
    for reference in &request.reviewed_extents {
        let entry = project.knowledge_entry(&reference.revision, &reference.assertion, control)?;
        if entry.state != AssertionState::Accepted
            || request.revision.as_ref() != Some(&entry.proposal.occurrence.revision)
        {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "reviewed extent must be accepted at the selected knowledge revision and match the source revision",
            ));
        }
        let KnowledgeClaim::FunctionExtent { extent } = entry.proposal.claim else {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "review reference is not a function extent",
            ));
        };
        let symbol = entry
            .proposal
            .occurrence
            .symbol
            .ok_or_else(|| Error::new(ErrorCode::Integrity, "reviewed extent has no symbol"))?;
        resolved.extents.push(FunctionExtent {
            source: entry.proposal.occurrence.source,
            symbol,
            extent,
        });
    }
    write_control_message(std::io::sink(), &resolved)?;
    let request = &resolved;
    let revision = request.revision.as_ref().ok_or_else(|| {
        Error::new(
            ErrorCode::InvalidRequest,
            "investigation revision not frozen",
        )
    })?;
    if let Some(inputs) = &request.inputs
        && (inputs.is_empty()
            || inputs
                .iter()
                .enumerate()
                .any(|(i, x)| inputs[..i].contains(x)))
    {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "input selection is empty or repeated",
        ));
    }
    for (i, extent) in request.extents.iter().enumerate() {
        control.checkpoint(1)?;
        if request.extents[..i]
            .iter()
            .any(|e| e.source == extent.source && e.symbol == extent.symbol)
        {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "repeated extent override",
            ));
        }
    }
    let _fixed = memory.reserve(1024 * 1024, control.position())?;
    let mut executable = Vec::new();
    executable.try_reserve_exact(16384).map_err(|_| {
        Error::new(
            ErrorCode::ResourceLimited,
            "section index allocation failed",
        )
    })?;
    let mut scan = Enumeration {
        request,
        sink,
        digest: EntryDigest::default(),
        input: 0,
        selected: false,
        seen_inputs: vec![false; request.inputs.as_ref().map_or(0, Vec::len)],
        used_extents: vec![false; request.extents.len()],
        object: None,
        executable,
        functions: 0,
    };
    if let Some(image) = &request.image {
        let image = project.image(image, control)?;
        if &image.manifest.plan.recipe.revision != revision {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "image belongs to another source revision",
            ));
        }
        scan.selected = true;
        let object = ObjectId {
            artifact: image.manifest.elf.clone(),
            location: ObjectLocation::Standalone,
        };
        scan.object = Some(ObjectInventory {
            id: object.clone(),
            name: None,
            content: Some(image.manifest.elf.clone()),
            elf: None,
            diagnostics: Vec::new(),
        });
        scan.emit(
            PlanEntry::Image {
                image: image.id,
                payload: image.manifest.elf,
                synthetic: image.manifest.synthetic,
            },
            control,
        )?;
        blobray_artifacts::inspect_source(&image.elf, &object, memory, control, &mut scan)?;
    } else {
        project.read_inventory(Some(revision), memory, control, &mut scan)?;
    }
    scan.finish_object(control)?;
    if scan.seen_inputs.contains(&false) || scan.used_extents.contains(&false) {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "selected input or extent override does not identify an enumerated function",
        ));
    }
    let count = scan.digest.count;
    let functions = scan.digest.functions;
    let entries = scan.digest.finish()?;
    investigation_plan(InvestigationRecipe {
        schema: 2,
        policy: 2,
        project: project.id().clone(),
        request: original_request.clone(),
        producer: producer.clone(),
        entries,
        entry_count: count,
        functions,
    })
}
/// Executes one frozen selection with a shared function engine and one resource
/// context. Staged results are invisible until the supervisor commits the receipt.
pub fn prepare_investigation_worker(
    stage: &Path,
    work: &InvestigationWork,
    decoder: &dyn FunctionSemantics,
    control: &mut dyn RunControl,
) -> Result<PreparedInvestigationReceipt> {
    let memory = WorkingMemory::new(
        work.budget
            .working_memory_bytes
            .ok_or_else(|| Error::new(ErrorCode::InvalidRequest, "working capacity missing"))?,
    )?;
    let disk = blobray_store::TemporaryBudget::open(stage)?;
    prepare_investigation_worker_in(stage, work, decoder, &memory, &disk, None, control)
}
pub(crate) fn prepare_investigation_worker_in(
    stage: &Path,
    work: &InvestigationWork,
    decoder: &dyn FunctionSemantics,
    memory: &WorkingMemory,
    disk: &blobray_store::TemporaryBudget,
    planned_entries: Option<blobray_store::TemporaryFile>,
    control: &mut dyn RunControl,
) -> Result<PreparedInvestigationReceipt> {
    validate_investigation_plan(&work.plan)?;
    if work.schema != 1
        || work.plan.recipe.producer.decoder != decoder.identity()
        || work.plan.recipe.producer.semantics != decoder.semantic_identity()
    {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "investigation producer differs from plan",
        ));
    }
    let mut metered = blobray_store::TemporaryControl {
        control,
        budget: disk,
    };
    let c: &mut dyn RunControl = &mut metered;
    let result = (|| {
        let _fixed = memory.reserve(2 * 1024 * 1024, c.position())?;
        let project = Project::open(&work.project.to_path()?)?;
        if project.id() != &work.plan.recipe.project {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "plan belongs to another project",
            ));
        }
        // Selection is verified before analyzing any function. The stream is a
        // quota-owned temporary artifact, not a second in-memory work queue.
        let staging = Staging::with_temporary_budget(stage, disk.clone())?;
        let entries = if let Some(entries) = planned_entries {
            entries
        } else {
            let mut entries = disk.temporary(&stage.join("staging"))?;
            let actual = enumerate(
                &project,
                &work.plan.recipe.request,
                &work.plan.recipe.producer,
                memory,
                c,
                &mut |e, _| {
                    write_control_message(&mut entries, e)?;
                    entries.write_all(b"\n").map_err(storage_io)
                },
            )?;
            if actual != work.plan {
                return Err(Error::new(
                    ErrorCode::Integrity,
                    "saved plan differs from captured inventory",
                ));
            }
            entries
        };
        let entries = staging.retain_temporary(entries, c)?;
        let entries = staging.open_payload(&entries, c)?;
        let engine = crate::functions::FunctionEngine {
            project: &project,
            stage,
            disk,
            memory,
            decoder,
        };
        let mut members = disk.temporary(&stage.join("staging"))?;
        let mut coverage = InvestigationCoverage::default();
        let mut container: Option<(ArtifactId, blobray_store::FileLease)> = None;
        let mut index = None;
        let mut cursor = blobray_store::JsonlCursor::new(&entries)?;
        let mut pending: Option<PlanEntry> = cursor.next(c)?;
        while let Some(entry) = pending.as_ref() {
            let PlanEntry::Function {
                request, payload, ..
            } = entry
            else {
                let member = InvestigationMember {
                    entry: pending.take().unwrap(),
                    outcome: InvestigationOutcome::Recorded,
                };
                coverage.include(&member);
                write_control_message(&mut members, &member)?;
                members.write_all(b"\n").map_err(storage_io)?;
                pending = cursor.next(c)?;
                continue;
            };
            let id = request.symbol.object.clone();
            let function_source = request.source.clone();
            let payload = payload.clone();
            if container.as_ref().is_none_or(|(a, _)| a != &id.artifact) {
                container = Some((id.artifact.clone(), project.open_payload(&id.artifact, c)?));
                index = None;
            }
            let source = &container.as_ref().unwrap().1;
            let range = match id.location {
                ObjectLocation::Standalone => Some((0, source.len())),
                ObjectLocation::ArchiveMember { ordinal } => {
                    if index.is_none() {
                        index = Some(blobray_artifacts::MemberIndex::new(source, memory, c)?);
                    }
                    index.as_ref().unwrap().get(ordinal)?
                }
            };
            let thin = if range.is_none() {
                Some(project.open_payload(&payload, c)?)
            } else {
                None
            };
            let range_source = range
                .map(|(offset, length)| SourceRange::new(source, offset, length))
                .transpose()?;
            let object_source: &dyn ByteSource = if let Some(source) = &range_source {
                source
            } else {
                thin.as_ref().unwrap()
            };
            let mut references = AdmittedVec::new(memory);
            let mut process = |mut prepared: Option<
                &mut blobray_artifacts::PreparedObject<'_, '_>,
            >,
                               blocked: Option<&Error>,
                               c: &mut dyn RunControl|
             -> Result<()> {
                loop {
                    let entry = pending.take().unwrap();
                    let PlanEntry::Function {
                        request,
                        payload: expected,
                        ..
                    } = &entry
                    else {
                        unreachable!()
                    };
                    if expected != &payload {
                        return Err(Error::new(
                            ErrorCode::Integrity,
                            "object group has conflicting payloads",
                        ));
                    }
                    let analyzed = if let Some(error) = blocked {
                        Err(error.clone())
                    } else {
                        engine.analyze_prepared(
                            prepared.as_deref_mut().unwrap(),
                            &mut references,
                            request,
                            &payload,
                            c,
                        )
                    };
                    let outcome = match analyzed {
                        Ok(receipt) => {
                            let m = staging.staged_function(&receipt.analysis, c)?;
                            InvestigationOutcome::Analyzed {
                                analysis: receipt.analysis,
                                complete: m.coverage.complete()
                                    && m.semantics.is_some_and(|s| s.complete),
                            }
                        }
                        Err(error) if blocked_function(&error) => {
                            InvestigationOutcome::Blocked { error }
                        }
                        Err(error) => return Err(error),
                    };
                    let member = InvestigationMember { entry, outcome };
                    coverage.include(&member);
                    write_control_message(&mut members, &member)?;
                    members.write_all(b"\n").map_err(storage_io)?;
                    pending = cursor.next(c)?;
                    if !matches!(&pending, Some(PlanEntry::Function { request, .. }) if request.symbol.object == id && request.source == function_source)
                    {
                        break;
                    }
                }
                Ok(())
            };
            let mut entered = false;
            let result = blobray_artifacts::with_prepared_object(
                object_source,
                &payload,
                memory,
                c,
                |object, c| {
                    entered = true;
                    process(Some(object), None, c)
                },
            );
            match result {
                Err(error) if !entered && blocked_function(&error) => {
                    process(None, Some(&error), c)?
                }
                other => other?,
            }
        }
        let members = staging.retain_temporary(members, c)?;
        staging.investigation_receipt(
            &InvestigationManifest {
                schema: 1,
                plan: work.plan.clone(),
                members,
                coverage,
            },
            c,
        )
    })();
    c.memory_phases(&memory.phase_observations());
    c.working_memory(memory.observation());
    result
}
pub(crate) fn matches_filter(filter: &InvestigationFilter, r: &FunctionRecord) -> bool {
    match (filter, r) {
        (InvestigationFilter::Calls { callee, unresolved_only, .. }, FunctionRecord::Transfer { target, .. }) => {
            (!*unresolved_only || matches!(target, AbstractValue::Unknown)) && callee.is_none_or(|a| matches!(target, AbstractValue::ImageAddress { address } if *address == a))
        }
        (
            InvestigationFilter::Accesses {
                address,
                symbol,
                unknown_only,
            },
            FunctionRecord::MemoryAccess {
                address: actual, ..
            },
        ) => {
            (!*unknown_only || matches!(actual, AbstractValue::Unknown | AbstractValue::Expression { .. }))
                && address
                    .is_none_or(|a| matches!(actual,AbstractValue::Constant{value} | AbstractValue::ImageAddress { address: value } if *value==a))
                && symbol
                    .as_ref()
                    .is_none_or(|s| matches!(actual,AbstractValue::Symbol{symbol,..} if symbol==s))
        }
        (
            InvestigationFilter::References { symbol, address },
            FunctionRecord::Reference { raw, target, addend, known, .. },
        ) => symbol
            .as_ref()
            .is_none_or(|s| &target.symbol == s || &raw.target.symbol == s)
            && address.is_none_or(|a| *known && matches!(target.definition, SymbolDefinition::Section | SymbolDefinition::Absolute) && addend.and_then(|v| target.offset.checked_add_signed(v)) == Some(u64::from(a))),
        (InvestigationFilter::References { symbol: None, address: Some(a) }, FunctionRecord::MemoryAccess { address, .. }) => matches!(address, AbstractValue::Constant { value } | AbstractValue::ImageAddress { address: value } if value == a),
        (InvestigationFilter::References { symbol: None, address: Some(a) }, FunctionRecord::Value { value: AbstractValue::ImageAddress { address }, .. }) => address == a,
        _ => false,
    }
}

fn blocked_function(error: &Error) -> bool {
    matches!(
        error.code,
        ErrorCode::NeedsExtent
            | ErrorCode::Incompatible
            | ErrorCode::InvalidRequest
            | ErrorCode::Unavailable
    )
}
