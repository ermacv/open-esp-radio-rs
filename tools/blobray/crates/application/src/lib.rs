//! Shared application operations for humans and automation.
//!
//! This layer owns import ordering, external-member resolution and publication.
//! Inventory receives captured bytes only; storage never interprets ELF or AR.

mod data;
mod occurrence;
use blobray_artifacts::{INVENTORY_PRODUCER, MemberCursor, inspect_source};
use blobray_domain::*;
use blobray_store::{ObjectHeader, Project, Staging};
mod scenarios;
pub use scenarios::{ScenarioWork, prepare_scenario_worker};
mod temporary;
pub use temporary::{TemporaryStoragePolicy, TemporaryStorageStatus};
mod investigations;
pub use investigations::{InvestigationWork, prepare_investigation_worker};
mod legacy;
pub use legacy::LegacyRequest;
mod event_routes;
mod interfaces;
mod knowledge;
mod navigation;
mod registers;
mod semantic_ir;
mod trace;
pub use knowledge::{KnowledgeWork, prepare_knowledge_worker};
pub use semantic_ir::{IrWork, prepare_ir_worker};
mod companions;
mod execution;
mod execution_goals;
mod execution_memory;
pub use execution::{EXECUTION_ENVIRONMENT, ExecutionWork, prepare_execution_worker};
mod functions;
mod link_selection;
mod research;
pub use functions::{FunctionWork, prepare_function_worker};
mod linking;
pub use linking::{
    ImageWork, LinkInput, LinkInvocation, LinkOutput, LinkOutputSink, LinkPlan, LinkerHost,
    prepare_image_worker, read_link_plan, validate_link_plan,
};
mod planning;
pub use planning::*;
mod protocol;
mod selection;
pub use blobray_store::{
    OwnerIdentity, PreparedExecutionReceipt, PreparedFunctionReceipt, PreparedImageReceipt,
    PreparedImport, PreparedInvestigationReceipt, PreparedIrReceipt, PreparedKnowledgeReceipt,
    RunOperation, RunRecord,
};
pub use protocol::*;
mod jobs;
mod query;
mod resources;
pub use blobray_store::{
    DoctorReport, DoctorSink, DoctorSummary, PreservationSummary, read_progress,
};
pub use jobs::{Application, ApplicationLimits, RunHandle};
pub use query::QueryOutput;
mod read;
pub use read::{InventoryView, ReadView};
mod query_stream;
pub use query_stream::{QuerySink, QuerySummary, prepare_query, prepare_query_with_tools};
pub use resources::RunContext;
use std::path::{Path, PathBuf};

/// One ordered input occurrence. Roles and paths are not identity keys.
pub struct ImportInput {
    pub role: String,
    pub path: PathBuf,
    pub expected: Option<ArtifactId>,
}

pub fn create_project(path: &Path) -> Result<ProjectId> {
    Ok(Project::create(path)?.id().clone())
}

pub fn inventory(path: &Path, revision: Option<&RevisionId>) -> Result<Snapshot> {
    Project::open(path)?.snapshot(revision)
}

pub fn revisions(path: &Path) -> Result<Vec<RevisionId>> {
    Project::open(path)?.revisions()
}

/// Worker-only preparation over a granted staging directory. It has no project
/// database or publication capability. Host composition must execute this entry
/// inside the requested resource containment. Errors leave no published revision.
pub fn prepare_import(
    stage: &Path,
    work: ImportWork,
    control: &mut dyn RunControl,
) -> Result<PreparedImport> {
    if work.schema != 2 {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "unsupported worker protocol",
        ));
    }
    let memory = WorkingMemory::new(work.budget.working_memory_bytes.ok_or_else(|| {
        Error::new(
            ErrorCode::InvalidRequest,
            "working memory budget required for new imports",
        )
    })?)?;
    let disk = blobray_store::TemporaryBudget::open(stage)?;
    let result = prepare_stream(
        stage,
        work,
        &memory,
        &mut blobray_store::TemporaryControl {
            control,
            budget: &disk,
        },
        disk.clone(),
    );
    control.memory_phases(&memory.phase_observations());
    control.working_memory(memory.observation());
    result
}
fn prepare_stream(
    stage: &Path,
    work: ImportWork,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    disk: blobray_store::TemporaryBudget,
) -> Result<PreparedImport> {
    if work.inputs.is_empty() || work.inputs.iter().any(|input| input.role.trim().is_empty()) {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "import requires inputs with nonempty roles",
        ));
    }
    // The host control protocol already bounds admission to 64 KiB. Account for
    // its decoded values and fixed stream I/O state independently of each object.
    let _control_memory = memory.reserve(256 * 1024, control.position())?;
    let mut stage = Staging::with_temporary_budget(stage, disk)?;
    control.phase(RunPhase::Serialize)?;
    let mut output = stage.stream_revision(
        work.project,
        work.parent,
        work.target,
        INVENTORY_PRODUCER,
        control,
    )?;
    for (ordinal, input) in work.inputs.into_iter().enumerate() {
        control.set_position(RunPosition {
            input: Some(ordinal as u64),
            phase: RunPhase::Capture,
            ..RunPosition::default()
        });
        control.checkpoint(1)?;
        let source = std::path::absolute(input.origin.to_path()?)
            .map_err(|e| Error::new(ErrorCode::Io, e.to_string()))?;
        let capture = stage.capture_controlled(&source, input.expected.as_ref(), control)?;
        let artifact = capture.artifact().cloned();
        let metadata = InputRecord {
            role: input.role,
            origin: OriginPath::from_path(&source),
            expected: input.expected,
            capture,
            external_members: Vec::new(),
            inventory: None,
        };
        let Some(artifact) = artifact else {
            let input = output.begin_input(metadata, ContainerKind::Unsupported, control)?;
            output.finish_input(input, false, None, control)?;
            continue;
        };
        let mut position = control.position();
        position.artifact(&artifact);
        control.set_position(position);
        let lease = stage.open_payload(&artifact, control)?;
        let mut cursor = match MemberCursor::new(&lease, control) {
            Ok(cursor) => cursor,
            Err(error) if error.code == ErrorCode::Integrity => {
                let mut magic = [0; 8];
                lease.read_at(0, &mut magic, control)?;
                let kind = if &magic == b"!<thin>\n" {
                    ContainerKind::ThinArchive
                } else {
                    ContainerKind::Archive
                };
                let input = output.begin_input(metadata, kind, control)?;
                output.finish_input(
                    input,
                    false,
                    Some(&Diagnostic {
                        code: DiagnosticCode::MalformedContainer,
                        context: "archive header; membership unknown".into(),
                        message: error.message,
                    }),
                    control,
                )?;
                continue;
            }
            Err(error) => return Err(error),
        };
        let kind = cursor.kind;
        let mut input = output.begin_input(metadata, kind, control)?;
        let mut framing_error = None;
        loop {
            let member = match cursor.next(memory, control) {
                Ok(Some(member)) => member,
                Ok(None) => break,
                Err(error) if error.code == ErrorCode::Integrity => {
                    framing_error = Some(Diagnostic {
                        code: DiagnosticCode::MalformedContainer,
                        context: "archive member; remaining membership unknown".into(),
                        message: error.message,
                    });
                    break;
                }
                Err(error) => return Err(error),
            };
            let mut position = control.position();
            position.member = Some(member.ordinal);
            position.table = None;
            position.entry = None;
            control.set_position(position);
            let id = ObjectId {
                artifact: artifact.clone(),
                location: match kind {
                    ContainerKind::Archive | ContainerKind::ThinArchive => {
                        ObjectLocation::ArchiveMember {
                            ordinal: member.ordinal,
                        }
                    }
                    _ => ObjectLocation::Standalone,
                },
            };
            let mut sink = output.begin_object()?;
            let mut content = None;
            let mut elf = None;
            if let Some((offset, length)) = member.payload {
                match SourceRange::new(&lease, offset, length) {
                    Ok(range) => {
                        let (digest, header) =
                            inspect_source(&range, &id, memory, control, &mut sink)?;
                        content = Some(digest);
                        elf = header;
                    }
                    Err(error) => sink.diagnostic(
                        &Diagnostic {
                            code: DiagnosticCode::MalformedObject,
                            context: "member payload".into(),
                            message: error.message,
                        },
                        control,
                    )?,
                }
            } else {
                let name = member.name.as_deref().unwrap_or_default();
                let _binding_memory = memory.reserve(
                    (name.len() as u64)
                        .checked_mul(8)
                        .and_then(|n| n.checked_add(32768))
                        .ok_or_else(|| {
                            Error::new(ErrorCode::ResourceLimited, "member binding size overflow")
                        })?,
                    control.position(),
                )?;
                let (origin, capture) = match member_path(name) {
                    Ok(path) => {
                        let path = source
                            .parent()
                            .expect("absolute source has parent")
                            .join(path);
                        (
                            Some(OriginPath::from_path(&path)),
                            stage.capture_controlled(&path, None, control)?,
                        )
                    }
                    Err(diagnostic) => (None, Capture::Unavailable { diagnostic }),
                };
                content = capture.artifact().cloned();
                if let Some(artifact) = &content {
                    let external = stage.open_payload(artifact, control)?;
                    let (digest, header) =
                        inspect_source(&external, &id, memory, control, &mut sink)?;
                    if &digest != artifact {
                        return Err(Error::new(
                            ErrorCode::Integrity,
                            "thin content changed during inspection",
                        ));
                    }
                    elf = header;
                } else {
                    sink.diagnostic(&Diagnostic { code: DiagnosticCode::MissingMember, context: "thin member".into(), message: "external payload was not captured; see the revision's member binding".into() }, control)?;
                }
                output.bind_member(
                    &mut input,
                    &ExternalMember {
                        ordinal: member.ordinal,
                        name: name.to_vec(),
                        origin,
                        capture,
                    },
                    control,
                )?;
            }
            control.phase(RunPhase::Serialize)?;
            output.finish_object(
                &mut input,
                sink,
                ObjectHeader {
                    id: &id,
                    name: member.name.as_deref(),
                    content: content.as_ref(),
                    elf: elf.as_ref(),
                },
                control,
            )?;
        }
        output.finish_input(
            input,
            framing_error.is_none(),
            framing_error.as_ref(),
            control,
        )?;
    }
    output.finish(memory, control)
}

fn member_path(name: &[u8]) -> std::result::Result<PathBuf, Diagnostic> {
    if name.is_empty() || name.contains(&0) {
        return Err(Diagnostic {
            code: DiagnosticCode::MalformedName,
            context: "thin member path".into(),
            message: "empty or NUL-containing member path".into(),
        });
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(PathBuf::from(std::ffi::OsString::from_vec(name.to_vec())))
    }
    #[cfg(windows)]
    {
        std::str::from_utf8(name)
            .map(PathBuf::from)
            .map_err(|_| Diagnostic {
                code: DiagnosticCode::MalformedName,
                context: "thin member path".into(),
                message: "member bytes cannot be represented as a Windows path".into(),
            })
    }
}

pub fn doctor(path: &Path) -> Result<DoctorReport> {
    Project::doctor(path)
}
pub fn runs(path: &Path) -> Result<Vec<RunRecord>> {
    Project::open(path)?.runs()
}

/// Stream persisted inventory with explicit resources; callbacks do not authorize writes.
pub fn inventory_stream(
    path: &Path,
    revision: Option<&RevisionId>,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    sink: &mut dyn InventorySink,
) -> Result<InventoryView> {
    let view = ReadView::open(path)?;
    let selected = match revision {
        Some(id) => id.clone(),
        None => view
            .current()?
            .ok_or_else(|| Error::new(ErrorCode::NotFound, "no imported revision"))?,
    };
    view.inventory(&selected, memory, control, sink)
}
pub fn doctor_stream(
    path: &Path,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    sink: &mut dyn DoctorSink,
) -> Result<DoctorSummary> {
    ReadView::open(path)?.doctor(memory, control, sink)
}

pub use blobray_verification::VERIFIER as EXECUTION_VERIFIER;

mod audit;

mod coverage;

mod flow;

mod devices;

mod external_calls;
