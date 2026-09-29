//! Automatic investigation: plan an investigation of a whole library, then run
//! the plan, under one supervisor, worker, budget and publication.
use crate::*;
use serde::{Deserialize, Serialize};
use std::io::Write;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutomaticInvestigationWork {
    pub schema: u32,
    pub run: RunId,
    pub project: OriginPath,
    /// The request with its revision frozen at admission.
    pub request: InvestigationRequest,
    pub producer: FunctionProducer,
    pub budget: ResourceBudget,
    pub started_ms: u64,
    pub deadline_ms: u64,
}
fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, message)
}
/// The operation that runs `plan` of `request`.
pub(crate) fn operation(
    request: &InvestigationRequest,
    plan: &InvestigationPlan,
) -> Result<RunOperation> {
    Ok(RunOperation::Investigate {
        revision: request
            .revision
            .clone()
            .ok_or_else(|| invalid("unfrozen investigation"))?,
        plan: plan.id.clone(),
    })
}
/// Validate the plan a worker made against the request admitted for it.
pub(crate) fn validate_plan(
    project: &Project,
    request: &InvestigationRequest,
    producer: &FunctionProducer,
    plan: &InvestigationPlan,
) -> Result<()> {
    blobray_store::validate_investigation_plan(plan)?;
    if plan.recipe.project != *project.id()
        || &plan.recipe.request != request
        || &plan.recipe.producer != producer
    {
        return Err(Error::new(
            ErrorCode::Integrity,
            "automatic plan differs from the admitted request",
        ));
    }
    Ok(())
}
pub fn prepare_automatic_investigation_worker(
    stage: &Path,
    work: &AutomaticInvestigationWork,
    decoder: &dyn FunctionSemantics,
    c: &mut dyn RunControl,
) -> Result<PreparedReceipt> {
    if work.schema != 1 {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "unsupported automatic investigation request",
        ));
    }
    let memory = WorkingMemory::new(
        work.budget
            .working_memory_bytes
            .ok_or_else(|| invalid("working capacity missing"))?,
    )?;
    let disk = blobray_store::TemporaryBudget::open(stage)?;
    let mut control = blobray_store::TemporaryControl::new(c, &disk);
    let c: &mut dyn RunControl = &mut control;
    let result = (|| {
        let _capacity = memory.reserve(1024 * 1024, c.position())?;
        let project = Project::open(&work.project.to_path()?)?;
        let mut entries = disk.temporary(&stage.join("staging"))?;
        c.phase(RunPhase::PlanInvestigation)?;
        let plan = crate::investigations::enumerate(
            &project,
            &work.request,
            &work.producer,
            &memory,
            c,
            &mut |entry, c| {
                c.checkpoint(1)?;
                write_control_message(&mut entries, entry)?;
                entries.write_all(b"\n").map_err(storage_io)
            },
        )?;
        c.checkpoint(0)?;
        let investigation = InvestigationWork {
            schema: 1,
            run: work.run.clone(),
            project: work.project.clone(),
            plan: plan.clone(),
            budget: work.budget.clone(),
            started_ms: work.started_ms,
            deadline_ms: work.deadline_ms,
        };
        let receipt = crate::investigations::prepare_investigation_worker_in(
            stage,
            &investigation,
            decoder,
            &memory,
            &disk,
            Some(entries),
            c,
        )?;
        write_control_message(
            std::fs::File::create(stage.join("plan.json")).map_err(storage_io)?,
            &plan,
        )?;
        Ok(PreparedReceipt::Investigation(receipt))
    })();
    c.memory_phases(&memory.phase_observations());
    c.working_memory(memory.observation());
    result
}
