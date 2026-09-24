//! Private, length-delimited typed result stream. Not a second archive parser.
use crate::*;
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum QuerySummary {
    Trace {
        summary: Box<TraceSummary>,
    },
    SemanticIr {
        id: ArtifactId,
        manifest: Box<SemanticIrManifest>,
    },
    Registers {
        summary: Box<RegisterSummary>,
    },
    EventRoute {
        summary: Box<EventRouteSummary>,
    },
    MemorySlice {
        summary: Box<MemorySliceSummary>,
    },
    Flow {
        summary: Box<FlowSummary>,
    },
    Navigation {
        summary: Box<NavigationSummary>,
    },
    Interfaces {
        summary: Box<InterfaceSummary>,
    },
    Coverage {
        id: PublicationId,
        selected_functions: InvestigationCoverage,
        extents: ExtentCoverageSummary,
    },
    StorageUsage {
        usage: StorageUsage,
    },
    Data {
        manifest: Box<DataManifest>,
    },
    TargetAudit {
        artifact: ArtifactId,
        decoder: String,
        semantics: String,
        ranges: Vec<ForbiddenTargetRange>,
        summary: TargetAuditSummary,
    },
    Execution {
        id: ArtifactId,
        manifest: Box<ExecutionManifest>,
    },
    KnowledgeValidation {
        expected_base: Option<KnowledgeRevisionId>,
    },
    RetainedPayload {
        id: ArtifactId,
        length: u64,
    },
    Legacy {
        manifest: Option<LegacyManifest>,
    },
    Preservation {
        restored: bool,
        summary: PreservationSummary,
    },
    Knowledge {
        status: KnowledgeStatus,
    },
    InvestigationPlan {
        plan: Box<InvestigationPlan>,
    },
    Publications {
        count: u64,
    },
    Publication {
        id: PublicationId,
        manifest: Box<InvestigationManifest>,
    },
    InvestigationStatus {
        status: InvestigationStatus,
    },
    Analyses {
        count: u64,
    },
    Analysis {
        id: FunctionAnalysisId,
        manifest: Box<FunctionManifest>,
    },
    LinkPlan {
        description: Box<LinkPlanDescription>,
    },
    Images {
        count: u64,
    },
    Image {
        id: PreparedImageId,
        manifest: Box<ImageManifest>,
    },
    Plan {
        description: Box<PlanDescription>,
    },
    Inspection {
        plan: PlanId,
        revision: RevisionId,
        scope: InspectionScope,
        revision_complete: bool,
    },
    Selection {
        revision: RevisionId,
        matches: u64,
        revision_complete: bool,
    },
    Inventory {
        revision_id: RevisionId,
        complete: bool,
    },
    Doctor {
        project: ProjectId,
        storage_schema: u32,
        checked_revisions: u64,
        checked_images: u64,
        checked_analyses: u64,
        checked_publications: u64,
        errors: u64,
        unfinished_runs: u64,
    },
}
impl QuerySummary {
    pub fn assessment(&self) -> ResultAssessment {
        match self {
            Self::Trace { summary } => ResultAssessment {
                comparison: summary.verdict,
                ..Default::default()
            },
            Self::Coverage {
                id,
                selected_functions,
                ..
            } => ResultAssessment::covered(
                CoverageSubject::Investigation(id.clone()),
                selected_functions.complete(),
            ),
            Self::Inventory {
                revision_id,
                complete,
            } => ResultAssessment::covered(
                CoverageSubject::Inventory(revision_id.clone()),
                *complete,
            ),
            Self::Analysis { id, manifest } => ResultAssessment::function(id.clone(), manifest),
            Self::Publication { id, manifest } => ResultAssessment::covered(
                CoverageSubject::Investigation(id.clone()),
                manifest.coverage.complete(),
            ),
            Self::Execution { id, manifest } => {
                ResultAssessment::execution(id.clone(), manifest.complete, manifest.verdict)
            }
            Self::TargetAudit {
                artifact, summary, ..
            } => {
                let mut a = ResultAssessment::covered(
                    CoverageSubject::StaticTargetAudit(artifact.clone()),
                    summary.coverage_gaps == 0,
                );
                a.check = Some(if summary.forbidden_targets != 0 {
                    CheckVerdict::Fail
                } else if summary.coverage_gaps != 0 {
                    CheckVerdict::Inconclusive
                } else {
                    CheckVerdict::Pass
                });
                a
            }
            Self::Doctor {
                errors,
                unfinished_runs,
                ..
            } => ResultAssessment::checked(*errors == 0 && *unfinished_runs == 0),
            Self::KnowledgeValidation { .. } => ResultAssessment::checked(true),
            Self::LinkPlan { description } => ResultAssessment::checked(description.ready()),
            _ => ResultAssessment::default(),
        }
    }
}
/// Borrowed callbacks. Retaining records requires the consumer's own capacity.
pub trait QuerySink: InventorySink + DoctorSink {
    fn trace(&mut self, _: &TraceRecord, _: &mut dyn RunControl) -> Result<()> {
        Err(Error::new(
            ErrorCode::InvalidRequest,
            "consumer does not support static traces",
        ))
    }
    fn semantic_ir(&mut self, _: &SemanticIrRecord, _: &mut dyn RunControl) -> Result<()> {
        Err(Error::new(
            ErrorCode::InvalidRequest,
            "consumer does not support semantic IR",
        ))
    }
    fn register(&mut self, _: &RegisterRecord, _: &mut dyn RunControl) -> Result<()> {
        Err(Error::new(
            ErrorCode::InvalidRequest,
            "consumer does not support registers",
        ))
    }
    fn event_route(&mut self, _: &EventRouteRecord, _: &mut dyn RunControl) -> Result<()> {
        Err(Error::new(
            ErrorCode::InvalidRequest,
            "consumer does not support event routes",
        ))
    }
    fn coverage(&mut self, _: &ExtentCoverageRecord, _: &mut dyn RunControl) -> Result<()> {
        Err(Error::new(
            ErrorCode::InvalidRequest,
            "consumer does not support extent coverage",
        ))
    }
    fn data(&mut self, _: &DataRecord, _: &mut dyn RunControl) -> Result<()> {
        Err(Error::new(
            ErrorCode::InvalidRequest,
            "consumer does not support data research",
        ))
    }
    fn target_audit(&mut self, _: &TargetAuditRecord, _: &mut dyn RunControl) -> Result<()> {
        Err(Error::new(
            ErrorCode::InvalidRequest,
            "consumer does not support audit findings",
        ))
    }
    fn legacy(&mut self, _: &LegacyRecord, _: &mut dyn RunControl) -> Result<()> {
        Err(Error::new(
            ErrorCode::InvalidRequest,
            "consumer does not support legacy records",
        ))
    }
    fn knowledge_entry(&mut self, _: &KnowledgeEntry, _: &mut dyn RunControl) -> Result<()> {
        Err(Error::new(
            ErrorCode::InvalidRequest,
            "consumer does not support knowledge entries",
        ))
    }
    fn knowledge_event(&mut self, _: &KnowledgeEvent, _: &mut dyn RunControl) -> Result<()> {
        Err(Error::new(
            ErrorCode::InvalidRequest,
            "consumer does not support knowledge events",
        ))
    }
    fn investigation_entry(&mut self, _: &PlanEntry, _: &mut dyn RunControl) -> Result<()> {
        Err(Error::new(
            ErrorCode::InvalidRequest,
            "consumer does not support investigation entries",
        ))
    }
    fn investigation_member(
        &mut self,
        _: &InvestigationMember,
        _: &mut dyn RunControl,
    ) -> Result<()> {
        Err(Error::new(
            ErrorCode::InvalidRequest,
            "consumer does not support investigation members",
        ))
    }
    fn publication(&mut self, _: &PublicationId, _: &mut dyn RunControl) -> Result<()> {
        Err(Error::new(
            ErrorCode::InvalidRequest,
            "consumer does not support publications",
        ))
    }
    fn finding(&mut self, _: &InvestigationFinding, _: &mut dyn RunControl) -> Result<()> {
        Err(Error::new(
            ErrorCode::InvalidRequest,
            "consumer does not support findings",
        ))
    }
    fn analysis(&mut self, _: &FunctionAnalysisId, _: &mut dyn RunControl) -> Result<()> {
        Err(Error::new(
            ErrorCode::InvalidRequest,
            "consumer does not support analyses",
        ))
    }
    fn execution_evidence(&mut self, _: &ExecutionEvidence, _: &mut dyn RunControl) -> Result<()> {
        Err(Error::new(
            ErrorCode::InvalidRequest,
            "consumer does not support execution evidence",
        ))
    }
    fn function_record(&mut self, _: &FunctionRecord, _: &mut dyn RunControl) -> Result<()> {
        Err(Error::new(
            ErrorCode::InvalidRequest,
            "consumer does not support function records",
        ))
    }
    fn image(&mut self, _id: &PreparedImageId, _control: &mut dyn RunControl) -> Result<()> {
        Err(Error::new(
            ErrorCode::InvalidRequest,
            "consumer does not support images",
        ))
    }
    fn link_observation(
        &mut self,
        _: &LinkObservationRecord,
        _: &mut dyn RunControl,
    ) -> Result<()> {
        Ok(())
    }
    fn image_mapping(&mut self, _: &ImageMapping, _: &mut dyn RunControl) -> Result<()> {
        Err(Error::new(
            ErrorCode::InvalidRequest,
            "consumer does not support image mappings",
        ))
    }

    fn memory_slice(&mut self, _: &MemorySliceRecord, _: &mut dyn RunControl) -> Result<()> {
        Err(Error::new(
            ErrorCode::InvalidRequest,
            "consumer does not support memory slices",
        ))
    }
    fn flow(&mut self, _: &FlowRecord, _: &mut dyn RunControl) -> Result<()> {
        Err(Error::new(
            ErrorCode::InvalidRequest,
            "consumer does not support flow records",
        ))
    }
    fn navigation(&mut self, _: &NavigationRecord, _: &mut dyn RunControl) -> Result<()> {
        Err(Error::new(
            ErrorCode::InvalidRequest,
            "consumer does not support navigation records",
        ))
    }
    fn interface(&mut self, _: &InterfaceObservation, _: &mut dyn RunControl) -> Result<()> {
        Err(Error::new(
            ErrorCode::InvalidRequest,
            "consumer does not support interfaces",
        ))
    }
    fn candidate(
        &mut self,
        _candidate: &SelectionCandidate,
        _control: &mut dyn RunControl,
    ) -> Result<()> {
        Err(Error::new(
            ErrorCode::InvalidRequest,
            "consumer does not support selection candidates",
        ))
    }
    fn summary(&mut self, summary: &QuerySummary, control: &mut dyn RunControl) -> Result<()>;
}
#[derive(Serialize)]
enum RecordRef<'a> {
    Trace(&'a TraceRecord),
    SemanticIr(&'a SemanticIrRecord),
    Register(&'a RegisterRecord),
    EventRoute(&'a EventRouteRecord),
    MemorySlice(&'a MemorySliceRecord),
    Flow(&'a FlowRecord),
    Navigation(&'a NavigationRecord),
    Interface(&'a InterfaceObservation),
    Coverage(&'a ExtentCoverageRecord),
    Data(&'a DataRecord),
    TargetAudit(&'a TargetAuditRecord),
    Execution(&'a ExecutionEvidence),
    Legacy(&'a LegacyRecord),
    KnowledgeEntry(&'a KnowledgeEntry),
    KnowledgeEvent(&'a KnowledgeEvent),
    InvestigationEntry(&'a PlanEntry),
    InvestigationMember(&'a InvestigationMember),
    Publication(&'a PublicationId),
    Finding(&'a InvestigationFinding),
    Image(&'a PreparedImageId),
    ImageMapping(&'a ImageMapping),
    LinkObservation(&'a LinkObservationRecord),
    Analysis(&'a FunctionAnalysisId),
    Function(&'a FunctionRecord),
    Revision(&'a RevisionHeader),
    Candidate(&'a SelectionCandidate),
    Input(u64, &'a InputRecord),
    Object(&'a ObjectInventory),
    External(&'a ExternalMember),
    Section(&'a SectionRecord),
    Symbol(&'a SymbolRecord),
    Relocation(&'a RelocationRecord),
    Diagnostic(&'a Diagnostic),
    Error(&'a Error),
    Unfinished(&'a RunId),
}
#[derive(Deserialize)]
enum Record {
    Trace(TraceRecord),
    SemanticIr(SemanticIrRecord),
    Register(RegisterRecord),
    EventRoute(EventRouteRecord),
    MemorySlice(MemorySliceRecord),
    Flow(FlowRecord),
    Navigation(NavigationRecord),
    Interface(InterfaceObservation),
    Coverage(ExtentCoverageRecord),
    Data(DataRecord),
    TargetAudit(TargetAuditRecord),
    Execution(ExecutionEvidence),
    Legacy(LegacyRecord),
    KnowledgeEntry(KnowledgeEntry),
    KnowledgeEvent(KnowledgeEvent),
    InvestigationEntry(PlanEntry),
    InvestigationMember(InvestigationMember),
    Publication(PublicationId),
    Finding(InvestigationFinding),
    Image(PreparedImageId),
    ImageMapping(ImageMapping),
    LinkObservation(LinkObservationRecord),
    Analysis(FunctionAnalysisId),
    Function(FunctionRecord),
    Revision(RevisionHeader),
    Candidate(SelectionCandidate),
    Input(u64, InputRecord),
    Object(ObjectInventory),
    External(ExternalMember),
    Section(SectionRecord),
    Symbol(SymbolRecord),
    Relocation(RelocationRecord),
    Diagnostic(Diagnostic),
    Error(Error),
    Unfinished(RunId),
}
fn io(e: std::io::Error) -> Error {
    storage_io(e)
}
struct Metered<'a> {
    file: &'a mut blobray_store::TemporaryFile,
    control: &'a mut dyn RunControl,
    failure: Option<Error>,
}
impl Write for Metered<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let size = bytes.len().min(WORK_BLOCK);
        if let Err(e) = self.control.bytes(size) {
            self.failure = Some(e.clone());
            return Err(std::io::Error::other(e));
        }
        self.file.write(&bytes[..size]).inspect_err(|e| {
            if let Some(typed) = e.get_ref().and_then(|e| e.downcast_ref::<Error>()) {
                self.failure = Some(typed.clone());
            } else if matches!(
                e.kind(),
                std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded
            ) {
                self.failure = Some(Error::new(ErrorCode::DiskFull, e.to_string()));
            }
        })
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}
struct Spool {
    file: blobray_store::TemporaryFile,
}
impl Spool {
    fn push(&mut self, record: RecordRef<'_>, control: &mut dyn RunControl) -> Result<()> {
        control.checkpoint(1)?;
        let start = self.file.stream_position().map_err(io)?;
        self.file.write_all(&[0; 8]).map_err(io)?;
        let mut writer = Metered {
            file: &mut self.file,
            control,
            failure: None,
        };
        let result = serde_json::to_writer(&mut writer, &record);
        if let Some(e) = writer.failure {
            return Err(e);
        }
        result.map_err(|e| Error::new(ErrorCode::Io, e.to_string()))?;
        let end = self.file.stream_position().map_err(io)?;
        self.file.seek(SeekFrom::Start(start)).map_err(io)?;
        self.file
            .write_all(&(end - start - 8).to_le_bytes())
            .map_err(io)?;
        self.file.seek(SeekFrom::Start(end)).map_err(io)?;
        Ok(())
    }
}
impl ElfSink for Spool {
    fn section(&mut self, r: &SectionRecord, c: &mut dyn RunControl) -> Result<()> {
        self.push(RecordRef::Section(r), c)
    }
    fn symbol(&mut self, r: &SymbolRecord, c: &mut dyn RunControl) -> Result<()> {
        self.push(RecordRef::Symbol(r), c)
    }
    fn relocation(&mut self, r: &RelocationRecord, c: &mut dyn RunControl) -> Result<()> {
        self.push(RecordRef::Relocation(r), c)
    }
    fn diagnostic(&mut self, r: &Diagnostic, c: &mut dyn RunControl) -> Result<()> {
        self.push(RecordRef::Diagnostic(r), c)
    }
}
impl InventorySink for Spool {
    fn revision(&mut self, r: &RevisionHeader, c: &mut dyn RunControl) -> Result<()> {
        self.push(RecordRef::Revision(r), c)
    }
    fn input(&mut self, n: u64, r: &InputRecord, c: &mut dyn RunControl) -> Result<()> {
        self.push(RecordRef::Input(n, r), c)
    }
    fn object(&mut self, r: &ObjectInventory, c: &mut dyn RunControl) -> Result<()> {
        self.push(RecordRef::Object(r), c)
    }
    fn external(&mut self, r: &ExternalMember, c: &mut dyn RunControl) -> Result<()> {
        self.push(RecordRef::External(r), c)
    }
}
impl DoctorSink for Spool {
    fn error(&mut self, r: &Error, c: &mut dyn RunControl) -> Result<()> {
        self.push(RecordRef::Error(r), c)
    }
    fn unfinished(&mut self, r: &RunId, c: &mut dyn RunControl) -> Result<()> {
        self.push(RecordRef::Unfinished(r), c)
    }
}
impl QuerySink for Spool {
    fn candidate(&mut self, r: &SelectionCandidate, c: &mut dyn RunControl) -> Result<()> {
        self.push(RecordRef::Candidate(r), c)
    }
    fn summary(&mut self, _: &QuerySummary, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
}
/// Worker entry: immutable semantic request, read capability and private output.
pub fn prepare_query(stage: &Path, work: &QueryWork, control: &mut dyn RunControl) -> Result<()> {
    prepare_query_with_tools(stage, work, control, &crate::linking::NoLinker, None)
}
pub fn prepare_query_with_tools(
    stage: &Path,
    work: &QueryWork,
    control: &mut dyn RunControl,
    linker: &dyn LinkerHost,
    decoder: Option<&dyn FunctionSemantics>,
) -> Result<()> {
    if work.schema != 2 {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "unsupported query protocol",
        ));
    }
    let memory = WorkingMemory::new(
        work.budget
            .working_memory_bytes
            .ok_or_else(|| Error::new(ErrorCode::InvalidRequest, "working capacity missing"))?,
    )?;
    let disk = blobray_store::TemporaryBudget::open(stage)?;
    let mut metered = blobray_store::TemporaryControl {
        control,
        budget: &disk,
    };
    let control: &mut dyn RunControl = &mut metered;
    let result = (|| {
        let _fixed = memory.reserve(65536, control.position())?;

        let mut spool = Spool {
            file: disk.create(&stage.join("query-records"))?,
        };
        let summary = match &work.query {
            ReadQuery::Trace { request } => {
                let project = Project::open(&work.project.to_path()?)?;
                QuerySummary::Trace {
                    summary: Box::new(crate::trace::query(
                        &project,
                        request,
                        &memory,
                        control,
                        &mut |r, c| spool.push(RecordRef::Trace(r), c),
                    )?),
                }
            }
            ReadQuery::SemanticIr { id } => {
                let project = Project::open(&work.project.to_path()?)?;
                QuerySummary::SemanticIr {
                    id: id.clone(),
                    manifest: Box::new(crate::semantic_ir::read(
                        &project,
                        id,
                        &memory,
                        control,
                        &mut |r, c| spool.push(RecordRef::SemanticIr(r), c),
                    )?),
                }
            }
            ReadQuery::Registers { request } => {
                let project = Project::open(&work.project.to_path()?)?;
                QuerySummary::Registers {
                    summary: Box::new(crate::registers::query(
                        &project,
                        request,
                        &memory,
                        control,
                        &mut |r, c| spool.push(RecordRef::Register(r), c),
                    )?),
                }
            }
            ReadQuery::EventRoute { request } => {
                let project = Project::open(&work.project.to_path()?)?;
                QuerySummary::EventRoute {
                    summary: Box::new(crate::event_routes::query(
                        &project,
                        request,
                        &memory,
                        control,
                        &mut |r, c| spool.push(RecordRef::EventRoute(r), c),
                    )?),
                }
            }
            ReadQuery::MemorySlice { request } => {
                let project = Project::open(&work.project.to_path()?)?;
                let lease = project.analysis(&request.analysis, control)?;
                let records = crate::research::load_records(&lease.records, &memory, control)?;
                let summary = blobray_analysis::memory_slice::inspect(
                    &lease.manifest.recipe,
                    lease.manifest.coverage,
                    &records,
                    request,
                    &memory,
                    control,
                    &mut |r, c| spool.push(RecordRef::MemorySlice(r), c),
                )?;
                QuerySummary::MemorySlice {
                    summary: Box::new(summary),
                }
            }
            ReadQuery::Flow { request } => {
                let project = Project::open(&work.project.to_path()?)?;
                let summary =
                    crate::flow::query(&project, request, &memory, control, &mut |r, c| {
                        spool.push(RecordRef::Flow(r), c)
                    })?;
                QuerySummary::Flow {
                    summary: Box::new(summary),
                }
            }
            ReadQuery::Navigate { request } => {
                let project = Project::open(&work.project.to_path()?)?;
                let summary =
                    crate::navigation::query(&project, request, &memory, control, &mut |r, c| {
                        spool.push(RecordRef::Navigation(r), c)
                    })?;
                QuerySummary::Navigation {
                    summary: Box::new(summary),
                }
            }
            ReadQuery::Interfaces { request } => {
                let project = Project::open(&work.project.to_path()?)?;
                let summary = crate::interfaces::discover(
                    &project,
                    request,
                    decoder,
                    &memory,
                    control,
                    &mut |r, c| spool.push(RecordRef::Interface(r), c),
                )?;
                QuerySummary::Interfaces {
                    summary: Box::new(summary),
                }
            }
            ReadQuery::Coverage { id } => {
                let project = Project::open(&work.project.to_path()?)?;
                let (selected_functions, extents) =
                    crate::coverage::report(&project, id, &memory, control, &mut |r, c| {
                        spool.push(RecordRef::Coverage(r), c)
                    })?;
                QuerySummary::Coverage {
                    id: id.clone(),
                    selected_functions,
                    extents,
                }
            }
            ReadQuery::StorageUsage => QuerySummary::StorageUsage {
                usage: ReadView::open(&work.project.to_path()?)?.storage_usage(&memory, control)?,
            },
            ReadQuery::Data { request } => {
                let project = Project::open(&work.project.to_path()?)?;
                let manifest = crate::data::prepare(
                    stage,
                    &project,
                    request,
                    None,
                    decoder,
                    &memory,
                    &disk,
                    control,
                    &mut |r, c| spool.push(RecordRef::Data(r), c),
                )?;
                QuerySummary::Data {
                    manifest: Box::new(manifest),
                }
            }
            ReadQuery::ReviewedData {
                revision,
                assertion,
            } => {
                let project = Project::open(&work.project.to_path()?)?;
                let _envelope = memory.reserve(2 * 1024 * 1024, control.position())?;
                let (request, entry) =
                    crate::data::accepted_request(&project, revision, assertion, &memory, control)?;
                let manifest = crate::data::prepare(
                    stage,
                    &project,
                    &request,
                    Some((revision.clone(), entry)),
                    decoder,
                    &memory,
                    &disk,
                    control,
                    &mut |r, c| spool.push(RecordRef::Data(r), c),
                )?;
                QuerySummary::Data {
                    manifest: Box::new(manifest),
                }
            }
            ReadQuery::AuditTargets { artifact, ranges } => {
                let decoder = decoder.ok_or_else(|| {
                    Error::new(ErrorCode::Incompatible, "target audit decoder unavailable")
                })?;
                let (artifact, summary) = crate::audit::audit_targets(
                    artifact,
                    ranges,
                    decoder,
                    &memory,
                    control,
                    &mut |r, c| spool.push(RecordRef::TargetAudit(r), c),
                )?;
                QuerySummary::TargetAudit {
                    artifact,
                    decoder: decoder.identity().into(),
                    semantics: decoder.semantic_identity().into(),
                    ranges: ranges.clone(),
                    summary,
                }
            }
            ReadQuery::ValidateKnowledge { change } => {
                std::fs::create_dir(stage.join("objects")).map_err(storage_io)?;
                std::fs::create_dir(stage.join("staging")).map_err(storage_io)?;
                crate::knowledge::prepare_with(
                    stage,
                    &KnowledgeWork {
                        schema: 1,
                        run: work.run.clone(),
                        project: work.project.clone(),
                        change: change.clone(),
                        budget: work.budget.clone(),
                        started_ms: work.started_ms,
                        deadline_ms: work.deadline_ms,
                    },
                    decoder.ok_or_else(|| {
                        Error::new(ErrorCode::Incompatible, "knowledge decoder unavailable")
                    })?,
                    &memory,
                    &disk,
                    control,
                )?;
                QuerySummary::KnowledgeValidation {
                    expected_base: change.expected_base.clone(),
                }
            }
            ReadQuery::RetainedPayload { id } => {
                let source = Project::open(&work.project.to_path()?)?.open_payload(id, control)?;
                let mut output = disk.create(&stage.join("payload.bin"))?;
                let mut offset = 0;
                let mut buffer = [0; WORK_BLOCK];
                while offset < source.len() {
                    control.checkpoint(1)?;
                    let n = (source.len() - offset).min(buffer.len() as u64) as usize;
                    source.read_at(offset, &mut buffer[..n], control)?;
                    output.write_all(&buffer[..n]).map_err(storage_io)?;
                    offset += n as u64;
                }
                output.sync_all().map_err(storage_io)?;
                QuerySummary::RetainedPayload {
                    id: id.clone(),
                    length: source.len(),
                }
            }
            ReadQuery::ImportLegacy { request } => QuerySummary::Preservation {
                restored: true,
                summary: crate::legacy::prepare_legacy(
                    stage,
                    work,
                    request,
                    decoder.ok_or_else(|| {
                        Error::new(ErrorCode::Incompatible, "knowledge decoder unavailable")
                    })?,
                    &memory,
                    &disk,
                    control,
                )?,
            },
            ReadQuery::Legacy => {
                let _fixed = memory.reserve(2 * 1024 * 1024, control.position())?;
                QuerySummary::Legacy {
                    manifest: Project::open(&work.project.to_path()?)?
                        .visit_legacy(control, &mut |r, c| spool.push(RecordRef::Legacy(r), c))?,
                }
            }
            ReadQuery::Backup => {
                let _fixed = memory.reserve(2 * 1024 * 1024, control.position())?;
                QuerySummary::Preservation {
                    restored: false,
                    summary: Project::open(&work.project.to_path()?)?
                        .backup(stage, &disk, control)?,
                }
            }
            ReadQuery::Restore { bundle } => {
                let _fixed = memory.reserve(2 * 1024 * 1024, control.position())?;
                QuerySummary::Preservation {
                    restored: true,
                    summary: blobray_store::restore_backup(
                        &bundle.to_path()?,
                        &stage.join("restored"),
                        &disk,
                        &memory,
                        control,
                    )?,
                }
            }
            ReadQuery::Knowledge { revision, history } => {
                let _envelope = memory.reserve(2 * 1024 * 1024, control.position())?;
                let project = Project::open(&work.project.to_path()?)?;
                let status =
                    project.knowledge_entries(revision.as_ref(), control, &mut |entry, c| {
                        if !history {
                            spool.push(RecordRef::KnowledgeEntry(entry), c)?;
                        }
                        Ok(())
                    })?;
                if *history {
                    project.knowledge_history(
                        revision.as_ref(),
                        control,
                        &mut |id, manifest, c| {
                            spool.push(
                                RecordRef::KnowledgeEvent(&KnowledgeEvent {
                                    revision: id.clone(),
                                    manifest: manifest.clone(),
                                }),
                                c,
                            )
                        },
                    )?;
                }
                QuerySummary::Knowledge { status }
            }
            ReadQuery::PlanInvestigation { request, producer } => {
                let project = Project::open(&work.project.to_path()?)?;
                let plan = crate::investigations::enumerate(
                    &project,
                    request,
                    producer,
                    &memory,
                    control,
                    &mut |e, c| spool.push(RecordRef::InvestigationEntry(e), c),
                )?;
                QuerySummary::InvestigationPlan {
                    plan: Box::new(plan),
                }
            }
            ReadQuery::Publications => {
                let mut count = 0;
                Project::open(&work.project.to_path()?)?.publications(control, &mut |id, c| {
                    spool.push(RecordRef::Publication(id), c)?;
                    count += 1;
                    Ok(())
                })?;
                QuerySummary::Publications { count }
            }
            ReadQuery::InvestigationStatus => {
                let _fixed = memory.reserve(1024 * 1024, control.position())?;
                QuerySummary::InvestigationStatus {
                    status: Project::open(&work.project.to_path()?)?
                        .investigation_status(control)?,
                }
            }
            ReadQuery::Publication { id, filter } => {
                let _fixed = memory.reserve(1024 * 1024, control.position())?;
                let project = Project::open(&work.project.to_path()?)?;
                let publication = project.publication(id, control)?;
                blobray_store::visit_jsonl(
                    &publication.members,
                    control,
                    |member: InvestigationMember, c| {
                        if matches!(filter, InvestigationFilter::Members) {
                            return spool.push(RecordRef::InvestigationMember(&member), c);
                        }
                        if let InvestigationFilter::Functions { name, address } = filter {
                            if let PlanEntry::Function {
                                name: actual,
                                address_space,
                                declared_extent,
                                ..
                            } = &member.entry
                            {
                                let named = name.as_ref().is_none_or(|n| {
                                    actual.as_ref().is_some_and(|a| a == n.as_bytes())
                                });
                                let addressed = address.is_none_or(|a| {
                                    *address_space == CodeAddressSpace::Image
                                        && declared_extent.start == u64::from(a)
                                });
                                if named && addressed {
                                    spool.push(RecordRef::InvestigationMember(&member), c)?;
                                }
                            }
                            return Ok(());
                        }
                        if let (
                            PlanEntry::Function { request, .. },
                            InvestigationOutcome::Analyzed { analysis, .. },
                        ) = (&member.entry, &member.outcome)
                        {
                            let function = project.analysis(analysis, c)?;
                            if matches!(
                                filter,
                                InvestigationFilter::References {
                                    address: Some(_),
                                    ..
                                }
                            ) && function.manifest.recipe.address_space
                                != CodeAddressSpace::Image
                            {
                                return Ok(());
                            }
                            if let InvestigationFilter::Calls { caller, .. } = filter
                                && (function.manifest.recipe.address_space
                                    != CodeAddressSpace::Image
                                    || caller.is_some_and(|a| {
                                        function.manifest.recipe.extent.start != u64::from(a)
                                    }))
                            {
                                return Ok(());
                            }
                            blobray_store::visit_jsonl(
                                &function.records,
                                c,
                                |record: FunctionRecord, c| {
                                    if crate::investigations::matches_filter(filter, &record) {
                                        spool.push(
                                            RecordRef::Finding(&InvestigationFinding {
                                                publication: id.clone(),
                                                request: request.clone(),
                                                analysis: analysis.clone(),
                                                record,
                                            }),
                                            c,
                                        )?;
                                    }
                                    Ok(())
                                },
                            )?;
                        }
                        Ok(())
                    },
                )?;
                QuerySummary::Publication {
                    id: id.clone(),
                    manifest: Box::new(publication.manifest),
                }
            }
            ReadQuery::Analyses => {
                let project = Project::open(&work.project.to_path()?)?;
                let mut count = 0;
                project.analyses(control, &mut |id, control| {
                    spool.push(RecordRef::Analysis(id), control)?;
                    count += 1;
                    Ok(())
                })?;
                QuerySummary::Analyses { count }
            }
            ReadQuery::Execution { id } => {
                let _fixed = memory.reserve(2 * 1024 * 1024, control.position())?;
                let result =
                    Project::open(&work.project.to_path()?)?.execution(id, &memory, control)?;
                blobray_store::validate_execution_records(
                    &result.manifest,
                    &result.records,
                    &memory,
                    control,
                )?;
                blobray_store::visit_jsonl(&result.records, control, |r: ExecutionEvidence, c| {
                    spool.push(RecordRef::Execution(&r), c)
                })?;
                QuerySummary::Execution {
                    id: id.clone(),
                    manifest: Box::new(result.manifest),
                }
            }
            ReadQuery::Analysis { id, export } => {
                let _fixed = memory.reserve(1024 * 1024, control.position())?;
                let result = Project::open(&work.project.to_path()?)?.analysis(id, control)?;
                blobray_store::visit_jsonl(
                    &result.records,
                    control,
                    |record: FunctionRecord, control| {
                        spool.push(RecordRef::Function(&record), control)
                    },
                )?;
                if *export {
                    copy_source(
                        &result.records,
                        &mut disk.create(&stage.join("records.jsonl"))?,
                        control,
                    )?;
                    copy_source(
                        &result.manifest_bytes,
                        &mut disk.create(&stage.join("manifest.json"))?,
                        control,
                    )?;
                }
                QuerySummary::Analysis {
                    id: id.clone(),
                    manifest: Box::new(result.manifest),
                }
            }

            ReadQuery::NamedLinkPlan {
                request,
                linker: executable,
            } => {
                let path = work.project.to_path()?;
                let (selected, matches, revision_complete) =
                    crate::link_selection::resolve(&path, request, &memory, control, &mut spool)?;
                if let Some(request) = selected {
                    let description = crate::linking::make_link_plan(
                        &path,
                        &request,
                        &executable.to_path()?,
                        linker,
                        &crate::linking::LinkWorkspace::for_query(stage, &disk, &memory),
                        &memory,
                        control,
                    )?;
                    let source = Project::open(&path)?
                        .open_payload(&description.recipe.revision.as_str().parse()?, control)?;
                    let mut output = disk.create(&stage.join("query-manifest"))?;
                    copy_source(&source, &mut output, control)?;
                    output.sync_all().map_err(io)?;
                    QuerySummary::LinkPlan {
                        description: Box::new(description),
                    }
                } else {
                    QuerySummary::Selection {
                        revision: request.revision.clone().unwrap(),
                        matches,
                        revision_complete,
                    }
                }
            }
            ReadQuery::LinkPlan {
                request,
                linker: executable,
            } => {
                let description = crate::linking::make_link_plan(
                    &work.project.to_path()?,
                    request,
                    &executable.to_path()?,
                    linker,
                    &crate::linking::LinkWorkspace::for_query(stage, &disk, &memory),
                    &memory,
                    control,
                )?;
                let source = Project::open(&work.project.to_path()?)?
                    .open_payload(&description.recipe.revision.as_str().parse()?, control)?;
                let mut output = disk.create(&stage.join("query-manifest"))?;
                copy_source(&source, &mut output, control)?;
                output.sync_all().map_err(io)?;
                QuerySummary::LinkPlan {
                    description: Box::new(description),
                }
            }
            ReadQuery::Images => {
                let project = Project::open(&work.project.to_path()?)?;
                let mut count = 0;
                project.images(control, &mut |id, control| {
                    spool.push(RecordRef::Image(id), control)?;
                    count += 1;
                    Ok(())
                })?;
                QuerySummary::Images { count }
            }
            ReadQuery::Image { id, export } => {
                let _metadata =
                    memory.reserve(crate::linking::LINK_METADATA_BYTES, control.position())?;
                let image = Project::open(&work.project.to_path()?)?.image(id, control)?;
                blobray_store::visit_jsonl(
                    &image.provenance,
                    control,
                    |mapping: ImageMapping, c| spool.push(RecordRef::ImageMapping(&mapping), c),
                )?;
                blobray_store::visit_jsonl(
                    &image.observations,
                    control,
                    |r: LinkObservationRecord, c| spool.push(RecordRef::LinkObservation(&r), c),
                )?;
                if *export {
                    for (name, source) in [
                        ("image.elf", &image.elf),
                        ("manifest.json", &image.manifest_bytes),
                        ("link.map", &image.map),
                        ("extraction.raw", &image.extraction),
                        ("provenance.jsonl", &image.provenance),
                        ("observations.jsonl", &image.observations),
                    ] {
                        copy_source(source, &mut disk.create(&stage.join(name))?, control)?;
                    }
                }
                QuerySummary::Image {
                    id: id.clone(),
                    manifest: Box::new(image.manifest),
                }
            }
            ReadQuery::Inventory {
                revision: Some(revision),
            } => {
                let view = ReadView::open(&work.project.to_path()?)?;
                let inventory = view.inventory(revision, &memory, control, &mut spool)?;
                let mut output = disk.create(&stage.join("query-manifest"))?;
                copy_source(inventory.manifest(), &mut output, control)?;
                output.sync_all().map_err(io)?;
                QuerySummary::Inventory {
                    revision_id: inventory.revision_id().clone(),
                    complete: inventory.complete(),
                }
            }
            ReadQuery::Inventory { revision: None } => {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "query admission must select a revision",
                ));
            }
            ReadQuery::Select { revision, request } => {
                let revision = revision.as_ref().ok_or_else(|| {
                    Error::new(ErrorCode::InvalidRequest, "selection needs fixed revision")
                })?;
                let view = ReadView::open(&work.project.to_path()?)?;
                let mut search = crate::selection::Search::new(request, revision, &mut spool);
                let inventory = view.inventory(revision, &memory, control, &mut search)?;
                QuerySummary::Selection {
                    revision: revision.clone(),
                    matches: search.count,
                    revision_complete: inventory.complete(),
                }
            }
            ReadQuery::Plan { request } => {
                request.budget.validate()?;
                let revision = request.revision.as_ref().ok_or_else(|| {
                    Error::new(ErrorCode::InvalidRequest, "plan needs fixed revision")
                })?;
                let description = make_plan(
                    disk.create(&stage.join("query-manifest"))?,
                    &work.project.to_path()?,
                    revision,
                    &request.scope,
                    &request.budget,
                    &memory,
                    control,
                )?;
                description.validate()?;
                QuerySummary::Plan {
                    description: Box::new(description),
                }
            }
            ReadQuery::ReopenPlan { description } => {
                description.validate()?;
                let recipe = &description.recipe;
                let actual = make_plan(
                    disk.create(&stage.join("query-manifest"))?,
                    &work.project.to_path()?,
                    &recipe.revision,
                    &recipe.scope,
                    &recipe.budget,
                    &memory,
                    control,
                )?;
                if actual != **description {
                    return Err(Error::new(
                        ErrorCode::Integrity,
                        "plan dependencies differ from retained revision/project",
                    ));
                }
                QuerySummary::Plan {
                    description: description.clone(),
                }
            }
            ReadQuery::ExecutePlan {
                description,
                manifest,
            } => {
                description.validate()?;
                let recipe = &description.recipe;
                if recipe.budget != work.budget {
                    return Err(Error::new(
                        ErrorCode::Integrity,
                        "execution budget differs from plan",
                    ));
                }
                let _metadata = memory.reserve(256 * 1024, control.position())?;
                let lease = blobray_store::ManifestLease::open(
                    &manifest.to_path()?,
                    recipe.project.clone(),
                    &recipe.revision,
                    control,
                )?;
                let mut probe = crate::selection::Probe::new(&recipe.scope);
                let complete = lease.visit(&memory, control, &mut probe)?;
                let actual = PlanDescription::new(probe.recipe(
                    recipe.revision.clone(),
                    complete,
                    recipe.budget.clone(),
                )?)?;
                if actual != **description {
                    return Err(Error::new(
                        ErrorCode::Integrity,
                        "plan differs from captured manifest",
                    ));
                }
                let mut filter =
                    crate::selection::Filter::new(&recipe.scope, probe.section, &mut spool);
                lease.visit(&memory, control, &mut filter)?;
                QuerySummary::Inspection {
                    plan: description.id.clone(),
                    revision: recipe.revision.clone(),
                    scope: recipe.scope.clone(),
                    revision_complete: complete,
                }
            }
            ReadQuery::Doctor => {
                let view = ReadView::open(&work.project.to_path()?)?;
                let s = view.doctor(&memory, control, &mut spool)?;
                QuerySummary::Doctor {
                    project: s.project,
                    storage_schema: s.storage_schema,
                    checked_revisions: s.checked_revisions,
                    checked_images: s.checked_images,
                    checked_analyses: s.checked_analyses,
                    checked_publications: s.checked_publications,
                    errors: s.errors,
                    unfinished_runs: s.unfinished_runs,
                }
            }
        };
        spool.file.sync_all().map_err(io)?;
        crate::protocol::write_request(disk.create(&stage.join("query-summary.json"))?, &summary)
    })();
    control.memory_phases(&memory.phase_observations());
    control.working_memory(memory.observation());
    result
}
pub(crate) fn copy_source(
    source: &dyn ByteSource,
    output: &mut dyn Write,
    control: &mut dyn RunControl,
) -> Result<()> {
    let mut offset = 0;
    let mut bytes = [0; WORK_BLOCK];
    while offset < source.len() {
        let count = (source.len() - offset).min(WORK_BLOCK as u64) as usize;
        source.read_at(offset, &mut bytes[..count], control)?;
        control.bytes(count)?;
        output.write_all(&bytes[..count]).map_err(io)?;
        offset += count as u64;
    }
    Ok(())
}
pub(crate) fn visit(
    file: &mut File,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    sink: &mut dyn QuerySink,
) -> Result<()> {
    let end = file.metadata().map_err(io)?.len();
    while file.stream_position().map_err(io)? < end {
        control.checkpoint(1)?;
        let mut header = [0; 8];
        file.read_exact(&mut header).map_err(io)?;
        let size = u64::from_le_bytes(header);
        if size > end - file.stream_position().map_err(io)? {
            return Err(Error::new(
                ErrorCode::WorkerProtocol,
                "truncated query record",
            ));
        }
        let charge = size
            .checked_mul(64)
            .and_then(|n| n.checked_add(4096))
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "record capacity overflow"))?;
        let _decoded = memory.reserve(charge, control.position())?;
        let mut bytes = memory.bytes(
            usize::try_from(size)
                .map_err(|_| Error::new(ErrorCode::ResourceLimited, "record too large"))?,
            control.position(),
        )?;
        for chunk in bytes.chunks_mut(WORK_BLOCK) {
            control.bytes(chunk.len())?;
            file.read_exact(chunk).map_err(io)?;
        }
        let record: Record = serde_json::from_slice(&bytes)
            .map_err(|e| Error::new(ErrorCode::WorkerProtocol, e.to_string()))?;
        match record {
            Record::Trace(r) => sink.trace(&r, control)?,
            Record::SemanticIr(r) => sink.semantic_ir(&r, control)?,
            Record::Register(r) => sink.register(&r, control)?,
            Record::EventRoute(r) => sink.event_route(&r, control)?,
            Record::MemorySlice(r) => sink.memory_slice(&r, control)?,
            Record::Flow(r) => sink.flow(&r, control)?,
            Record::Navigation(r) => sink.navigation(&r, control)?,
            Record::Interface(r) => sink.interface(&r, control)?,
            Record::TargetAudit(r) => sink.target_audit(&r, control)?,
            Record::Legacy(r) => sink.legacy(&r, control)?,
            Record::KnowledgeEntry(r) => sink.knowledge_entry(&r, control)?,
            Record::KnowledgeEvent(r) => sink.knowledge_event(&r, control)?,
            Record::InvestigationEntry(r) => sink.investigation_entry(&r, control)?,
            Record::InvestigationMember(r) => sink.investigation_member(&r, control)?,
            Record::Publication(r) => sink.publication(&r, control)?,
            Record::Finding(r) => sink.finding(&r, control)?,
            Record::Image(id) => sink.image(&id, control)?,
            Record::LinkObservation(r) => sink.link_observation(&r, control)?,
            Record::ImageMapping(mapping) => sink.image_mapping(&mapping, control)?,
            Record::Analysis(id) => sink.analysis(&id, control)?,
            Record::Execution(record) => sink.execution_evidence(&record, control)?,
            Record::Data(record) => sink.data(&record, control)?,
            Record::Coverage(record) => sink.coverage(&record, control)?,
            Record::Function(record) => sink.function_record(&record, control)?,
            Record::Revision(r) => sink.revision(&r, control)?,
            Record::Candidate(r) => sink.candidate(&r, control)?,
            Record::Input(n, r) => sink.input(n, &r, control)?,
            Record::Object(r) => sink.object(&r, control)?,
            Record::External(r) => sink.external(&r, control)?,
            Record::Section(r) => sink.section(&r, control)?,
            Record::Symbol(r) => sink.symbol(&r, control)?,
            Record::Relocation(r) => sink.relocation(&r, control)?,
            Record::Diagnostic(r) => sink.diagnostic(&r, control)?,
            Record::Error(r) => sink.error(&r, control)?,
            Record::Unfinished(r) => sink.unfinished(&r, control)?,
        }
    }
    Ok(())
}

fn make_plan(
    mut output: blobray_store::TemporaryFile,
    project: &Path,
    revision: &RevisionId,
    scope: &InspectionScope,
    budget: &ResourceBudget,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<PlanDescription> {
    let _metadata = memory.reserve(256 * 1024, control.position())?;
    let view = ReadView::open(project)?;
    let mut probe = crate::selection::Probe::new(scope);
    let inventory = view.inventory(revision, memory, control, &mut probe)?;
    let description = PlanDescription::new(probe.recipe(
        revision.clone(),
        inventory.complete(),
        budget.clone(),
    )?)?;
    copy_source(inventory.manifest(), &mut output, control)?;
    output.sync_all().map_err(io)?;
    Ok(description)
}

#[cfg(test)]
mod assessment_tests {
    use super::*;
    #[test]
    fn checks_are_not_coverage_and_comparison_is_not_a_check() {
        let summary = QuerySummary::Doctor {
            project: ArtifactId::of_bytes(b"p").as_str().parse().unwrap(),
            storage_schema: 8,
            checked_revisions: 1,
            checked_images: 0,
            checked_analyses: 0,
            checked_publications: 0,
            errors: 1,
            unfinished_runs: 0,
        };
        let assessment = summary.assessment();
        assert_eq!(assessment.check, Some(CheckVerdict::Fail));
        assert!(assessment.coverage.is_none());
        assert!(!assessment.check_passed());
        for verdict in [
            ComparisonVerdict::Match,
            ComparisonVerdict::Diff,
            ComparisonVerdict::Incomplete,
        ] {
            let assessment = ResultAssessment::execution(
                ArtifactId::of_bytes(b"evidence"),
                false,
                Some(verdict),
            );
            assert!(!assessment.is_complete());
            assert!(assessment.check_passed());
            assert_eq!(assessment.comparison, Some(verdict));
        }
        assert_eq!(
            QuerySummary::Analyses { count: 1 }.assessment(),
            ResultAssessment::default()
        );
    }
}
