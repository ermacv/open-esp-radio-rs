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
    let memory = WorkingMemory::new(
        work.budget
            .working_memory_bytes
            .ok_or_else(|| Error::new(ErrorCode::InvalidRequest, "working capacity missing"))?,
    )?;
    let disk = blobray_store::TemporaryBudget::open(stage)?;
    prepare_function_worker_in(stage, work, decoder, &memory, &disk, control)
}
pub(crate) fn prepare_function_worker_in(
    stage: &Path,
    work: &FunctionWork,
    decoder: &dyn FunctionSemantics,
    memory: &WorkingMemory,
    disk: &blobray_store::TemporaryBudget,
    control: &mut dyn RunControl,
) -> Result<blobray_store::PreparedFunctionReceipt> {
    if work.schema != 1 {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "unsupported function worker request",
        ));
    }
    let mut control = blobray_store::TemporaryControl {
        control,
        budget: disk,
    };
    let result = (|| {
        let _fixed = memory.reserve(1024 * 1024, control.position())?;
        let project = Project::open(&work.project.to_path()?)?;
        let revision =
            work.request.revision.as_ref().ok_or_else(|| {
                Error::new(ErrorCode::InvalidRequest, "function revision not frozen")
            })?;
        let occurrence = KnowledgeOccurrence {
            revision: revision.clone(),
            source: work.request.source.clone(),
            object: work.request.selector.object().clone(),
            symbol: work.request.selector.symbol().cloned(),
        };
        let engine = FunctionEngine {
            project: &project,
            stage,
            disk,
            memory,
            decoder,
        };
        crate::occurrence::with_source(&project, &occurrence, memory, &mut control, |capture, c| {
            engine.analyze(capture.bytes, &work.request, capture.payload, c)
        })
    })();
    control.memory_phases(&memory.phase_observations());
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
impl<'m> FunctionEngine<'m> {
    pub fn analyze(
        &self,
        source: &dyn ByteSource,
        request: &FunctionRequest,
        payload: &ArtifactId,
        control: &mut dyn RunControl,
    ) -> Result<PreparedFunctionReceipt> {
        let mut manifest = {
            let mut references = AdmittedVec::new(self.memory);
            blobray_artifacts::with_prepared_object(
                source,
                payload,
                self.memory,
                control,
                |object, c| self.analyze_local(object, &mut references, request, payload, c),
            )?
        }; // Both the prepared ELF and its reference indexes end before enrichment.
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
    pub fn analyze_prepared(
        &self,
        object: &mut blobray_artifacts::PreparedObject<'_, '_>,
        references: &mut AdmittedVec<'m, (u32, blobray_analysis::PreparedReferences<'m>)>,
        request: &FunctionRequest,
        payload: &ArtifactId,
        control: &mut dyn RunControl,
    ) -> Result<PreparedFunctionReceipt> {
        if request.research.is_some() {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "enrichment requires ending the prepared-object scope",
            ));
        }
        let manifest = self.analyze_local(object, references, request, payload, control)?;
        Staging::with_temporary_budget(self.stage, self.disk.clone())?
            .function_receipt(&manifest, control)
    }
    fn analyze_local(
        &self,
        object: &mut blobray_artifacts::PreparedObject<'_, '_>,
        references: &mut AdmittedVec<'m, (u32, blobray_analysis::PreparedReferences<'m>)>,
        request: &FunctionRequest,
        payload: &ArtifactId,
        control: &mut dyn RunControl,
    ) -> Result<FunctionManifest> {
        let mut position = RunPosition {
            phase: RunPhase::AnalyzeFunction,
            input: request.source.input(),
            member: match request.selector.object().location {
                ObjectLocation::Standalone => None,
                ObjectLocation::ArchiveMember { ordinal } => Some(ordinal),
            },
            ..Default::default()
        };
        position.artifact(payload);
        control.set_position(position);
        control.checkpoint(0)?;
        object.with_function(request, control, |view, control| {
            let recipe = FunctionRecipe {
                research: request.research.clone(),
                abi: view.abi,
                address_space: view.address_space,
                schema: 5,
                policy: 6,
                decoder: self.decoder.identity().into(),
                semantics: Some(self.decoder.semantic_identity().into()),
                project: self.project.id().clone(),
                revision: request.revision.clone().ok_or_else(|| {
                    Error::new(ErrorCode::InvalidRequest, "function revision not frozen")
                })?,
                source: request.source.clone(),
                selector: request.selector.clone(),
                payload: payload.clone(),
                section: view.section,
                extent: view.extent,
                user_extent: view.user_extent,
            };
            let mut records = Records {
                file: self.disk.temporary(&self.stage.join("staging"))?,
            };
            let index = if let Some(index) = references
                .iter()
                .position(|(section, _)| *section == view.section)
            {
                index
            } else {
                let prepared = blobray_analysis::PreparedReferences::new(
                    view.relocations,
                    view.section,
                    self.decoder,
                    self.memory,
                    control,
                )?;
                references.push((view.section, prepared), control.position())?;
                references.len() - 1
            };
            let summary = blobray_analysis::research(
                blobray_analysis::FunctionInput {
                    image: view.image,
                    section: view.section,
                    extent: view.extent,
                    bytes: view.code,
                    relocations: &references[index].1,
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
                schema: 5,
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
        })
    }
}
