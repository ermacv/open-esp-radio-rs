//! Function selection and orchestration over the existing captured inventory.
use crate::*;
use std::io::Write;
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionWork {
    pub schema: u32,
    pub run: RunId,
    pub project: OriginPath,
    pub request: FunctionRequest,
    pub budget: ResourceBudget,
    pub started_ms: u64,
    pub deadline_ms: u64,
}
struct Records {
    file: blobray_store::TemporaryFile,
}
impl FunctionSink for Records {
    fn record(&mut self, record: &FunctionRecord, control: &mut dyn RunControl) -> Result<()> {
        control.checkpoint(1)?;
        write_control_message(&mut self.file, record)?;
        self.file.write_all(b"\n").map_err(storage_io)
    }
}
pub fn prepare_function_worker(
    stage: &Path,
    work: &FunctionWork,
    decoder: &dyn FunctionSemantics,
    control: &mut dyn RunControl,
) -> Result<blobray_store::PreparedFunctionReceipt> {
    if work.schema != 1 {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "unsupported function worker request",
        ));
    }
    let memory = WorkingMemory::new(
        work.budget
            .working_memory_bytes
            .ok_or_else(|| Error::new(ErrorCode::InvalidRequest, "working capacity missing"))?,
    )?;
    let disk = blobray_store::TemporaryBudget::open(stage)?;
    let mut control = blobray_store::TemporaryControl {
        control,
        budget: &disk,
    };
    let result = (|| {
        let _fixed = memory.reserve(1024 * 1024, control.position())?;
        let project = Project::open(&work.project.to_path()?)?;
        let revision =
            work.request.revision.as_ref().ok_or_else(|| {
                Error::new(ErrorCode::InvalidRequest, "function revision not frozen")
            })?;
        if let FunctionSource::Image { image } = &work.request.source {
            let image = project.image(image, &mut control)?;
            if &image.manifest.plan.recipe.revision != revision
                || work.request.symbol.object
                    != (ObjectId {
                        artifact: image.manifest.elf.clone(),
                        location: ObjectLocation::Standalone,
                    })
            {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "image function does not belong to the selected image/revision",
                ));
            }
            return FunctionEngine {
                project: &project,
                stage,
                disk: &disk,
                memory: &memory,
                decoder,
            }
            .analyze(&image.elf, &work.request, &image.manifest.elf, &mut control);
        }
        let scope =
            InspectionScope::Symbol {
                input: work.request.source.input().ok_or_else(|| {
                    Error::new(ErrorCode::InvalidRequest, "expected captured input")
                })?,
                symbol: work.request.symbol.clone(),
            };
        let mut probe = crate::selection::Probe::new(&scope);
        project.read_inventory(Some(revision), &memory, &mut control, &mut probe)?;
        if !probe.found {
            return Err(Error::new(
                ErrorCode::NotFound,
                "function occurrence absent from revision",
            ));
        }
        let payload = probe
            .binding
            .and_then(|b| b.payload)
            .ok_or_else(|| Error::new(ErrorCode::Unavailable, "function payload not captured"))?;
        let source = project.open_payload(&work.request.symbol.object.artifact, &mut control)?;
        let engine = FunctionEngine {
            project: &project,
            stage,
            disk: &disk,
            memory: &memory,
            decoder,
        };
        let consume = |source: &dyn ByteSource, control: &mut dyn RunControl| {
            engine.analyze(source, &work.request, &payload, control)
        };
        match work.request.symbol.object.location {
            ObjectLocation::Standalone => consume(&source, &mut control),
            ObjectLocation::ArchiveMember { ordinal } => {
                let mut cursor = MemberCursor::new(&source, &mut control)?;
                while let Some(member) = cursor.next(&memory, &mut control)? {
                    if member.ordinal != ordinal {
                        continue;
                    }
                    if let Some((offset, length)) = member.payload {
                        return consume(&SourceRange::new(&source, offset, length)?, &mut control);
                    }
                    return consume(&project.open_payload(&payload, &mut control)?, &mut control);
                }
                Err(Error::new(
                    ErrorCode::Integrity,
                    "captured archive occurrence missing",
                ))
            }
        }
    })();
    control.working_memory(memory.observation());
    result
}
/// Shared per-function execution. All reservations die before returning; results
/// are staged only. The caller owns selection and the whole-run resource scope.
pub(crate) struct FunctionEngine<'a> {
    pub project: &'a Project,
    pub stage: &'a Path,
    pub disk: &'a blobray_store::TemporaryBudget,
    pub memory: &'a WorkingMemory,
    pub decoder: &'a dyn FunctionSemantics,
}
impl FunctionEngine<'_> {
    pub fn analyze(
        &self,
        source: &dyn ByteSource,
        request: &FunctionRequest,
        payload: &ArtifactId,
        control: &mut dyn RunControl,
    ) -> Result<PreparedFunctionReceipt> {
        let mut position = RunPosition {
            phase: RunPhase::AnalyzeFunction,
            input: request.source.input(),
            member: match request.symbol.object.location {
                ObjectLocation::Standalone => None,
                ObjectLocation::ArchiveMember { ordinal } => Some(ordinal),
            },
            ..Default::default()
        };
        position.artifact(payload);
        control.set_position(position);
        control.checkpoint(0)?;
        let mut manifest = blobray_artifacts::with_function(
            source,
            payload,
            request,
            self.memory,
            control,
            |view, control| {
                let recipe = FunctionRecipe {
                    research: request.research.clone(),
                    abi: view.abi,
                    address_space: view.address_space,
                    schema: 4,
                    policy: 4,
                    decoder: self.decoder.identity().into(),
                    semantics: Some(self.decoder.semantic_identity().into()),
                    project: self.project.id().clone(),
                    revision: request.revision.clone().ok_or_else(|| {
                        Error::new(ErrorCode::InvalidRequest, "function revision not frozen")
                    })?,
                    source: request.source.clone(),
                    symbol: request.symbol.clone(),
                    payload: payload.clone(),
                    section: view.section,
                    extent: view.extent,
                    user_extent: view.user_extent,
                };
                let mut records = Records {
                    file: self.disk.temporary(&self.stage.join("staging"))?,
                };
                let summary = blobray_analysis::research(
                    blobray_analysis::FunctionInput {
                        image: view.image,
                        section: view.section,
                        extent: view.extent,
                        bytes: view.code,
                        relocations: view.relocations,
                        data_ranges: view.data_ranges,
                    },
                    self.decoder,
                    self.memory,
                    control,
                    &mut records,
                    request.research.as_ref().and_then(|r| r.abi),
                )?;
                let staging = Staging::with_temporary_budget(self.stage, self.disk.clone())?;
                let records = staging.retain_temporary(records.file, control)?;
                let manifest = FunctionManifest {
                    schema: 4,
                    recipe,
                    records,
                    coverage: summary.coverage,
                    instructions: summary.instructions,
                    blocks: summary.blocks,
                    edges: summary.edges,
                    references: summary.references,
                    gaps: summary.gaps,
                    semantics: Some(summary.semantics),
                };
                Ok(manifest)
            },
        )?;
        // End the ELF/code borrow and local graph phase before retaining summaries.
        let staging = Staging::with_temporary_budget(self.stage, self.disk.clone())?;
        crate::research::enrich(
            self.project,
            &staging,
            &mut manifest,
            self.memory,
            self.disk,
            self.stage,
            control,
        )?;
        staging.function_receipt(&manifest, control)
    }
}
