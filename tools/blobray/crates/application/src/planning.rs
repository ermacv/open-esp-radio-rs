//! Immutable inspection recipes. Planning never publishes or accepts knowledge.
use crate::*;
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanRequest {
    pub revision: Option<RevisionId>,
    pub scope: InspectionScope,
    /// Execution budget; planning has its own separately supplied budget.
    pub budget: ResourceBudget,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedBinding {
    pub input: u64,
    pub capture: Capture,
    pub payload: Option<ArtifactId>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectionRecipe {
    pub schema: u32,
    pub operation_version: u32,
    pub result_schema: u32,
    pub project: ProjectId,
    pub revision: RevisionId,
    pub scope: InspectionScope,
    pub target: Target,
    pub inventory_producer: String,
    /// Whole-revision dependencies are represented by the manifest identity.
    pub binding: Option<SelectedBinding>,
    pub revision_complete: bool,
    pub budget: ResourceBudget,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanDescription {
    pub id: PlanId,
    pub recipe: InspectionRecipe,
}
impl PlanDescription {
    pub(crate) fn new(recipe: InspectionRecipe) -> Result<Self> {
        let mut bytes = Vec::new();
        crate::protocol::write_request(&mut bytes, &recipe)?;
        let description = Self {
            id: ArtifactId::of_bytes(&bytes).as_str().parse()?,
            recipe,
        };
        crate::protocol::write_request(std::io::sink(), &description)?;
        Ok(description)
    }
    pub fn validate(&self) -> Result<()> {
        if self.recipe.schema != 1
            || self.recipe.operation_version != 1
            || self.recipe.result_schema != 1
        {
            return Err(Error::new(
                ErrorCode::Incompatible,
                "unsupported inspection plan version",
            ));
        }
        self.recipe.budget.validate()?;
        if self.recipe.budget.working_memory_bytes.is_none() {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "plan requires working capacity",
            ));
        }
        if Self::new(self.recipe.clone())?.id != self.id {
            return Err(Error::new(ErrorCode::Integrity, "plan identity mismatch"));
        }
        Ok(())
    }
    pub fn read(reader: impl Read) -> Result<Self> {
        let mut bytes = Vec::new();
        reader
            .take(CONTROL_MESSAGE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(io)?;
        if bytes.len() > CONTROL_MESSAGE_BYTES {
            return Err(Error::new(ErrorCode::InvalidRequest, "plan exceeds 64 KiB"));
        }
        let description: Self = serde_json::from_slice(&bytes)
            .map_err(|e| Error::new(ErrorCode::InvalidRequest, e.to_string()))?;
        description.validate()?;
        Ok(description)
    }
}
/// An owned immutable manifest and recipe. Clones share its admission slot.
/// ```compile_fail
/// fn denied(plan: blobray_application::Plan) { plan.writer(); }
/// ```
#[derive(Clone)]
pub struct Plan {
    inner: Arc<PlanInner>,
}
struct PlanInner {
    description: PlanDescription,
    output: Mutex<QueryOutput>,
    manifest: PathBuf,
}
impl Plan {
    pub(crate) fn from_output(output: QueryOutput) -> Result<Self> {
        let QuerySummary::Plan { description } = output.summary() else {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "operation did not produce a plan",
            ));
        };
        description.validate()?;
        Ok(Self {
            inner: Arc::new(PlanInner {
                description: (**description).clone(),
                manifest: output.manifest_path().to_owned(),
                output: Mutex::new(output),
            }),
        })
    }
    /// The recipe borrow cannot outlive the owning plan.
    /// ```compile_fail
    /// fn escape(plan: blobray_application::Plan) -> &'static blobray_application::PlanDescription {
    ///     plan.description()
    /// }
    /// ```
    pub fn description(&self) -> &PlanDescription {
        &self.inner.description
    }
    /// One budgeted delivery of the portable description. Does not write the project.
    pub fn write(&self, writer: &mut dyn Write, cancelled: &dyn Fn() -> bool) -> Result<()> {
        self.inner
            .output
            .lock()
            .unwrap()
            .deliver(cancelled, |_, _, control| {
                let mut bytes = Vec::new();
                crate::protocol::write_request(&mut bytes, self.description())?;
                for chunk in bytes.chunks(WORK_BLOCK) {
                    control.bytes(chunk.len())?;
                    writer.write_all(chunk).map_err(io)?;
                }
                writer.flush().map_err(io)
            })
    }
    pub(crate) fn manifest_path(&self) -> &Path {
        &self.inner.manifest
    }
}
impl Application {
    pub fn start_plan(
        &self,
        project: &Path,
        request: PlanRequest,
        planning_budget: ResourceBudget,
    ) -> Result<RunHandle> {
        request.budget.validate()?;
        if request.budget.working_memory_bytes.is_none() {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "execution working capacity required",
            ));
        }
        self.start_query(project, ReadQuery::Plan { request }, planning_budget)
    }
    pub fn plan(
        &self,
        project: &Path,
        request: PlanRequest,
        planning_budget: ResourceBudget,
    ) -> Result<Plan> {
        self.start_plan(project, request, planning_budget)?
            .take_plan()
    }
    pub fn start_reopen_plan(
        &self,
        project: &Path,
        description: PlanDescription,
        planning_budget: ResourceBudget,
    ) -> Result<RunHandle> {
        description.validate()?;
        self.start_query(
            project,
            ReadQuery::ReopenPlan {
                description: Box::new(description),
            },
            planning_budget,
        )
    }
    pub fn reopen_plan(
        &self,
        project: &Path,
        description: PlanDescription,
        planning_budget: ResourceBudget,
    ) -> Result<Plan> {
        self.start_reopen_plan(project, description, planning_budget)?
            .take_plan()
    }
    pub fn start_run(&self, plan: &Plan) -> Result<RunHandle> {
        let description = plan.description().clone();
        self.start_query_owned(
            Path::new("."),
            ReadQuery::ExecutePlan {
                description: Box::new(description.clone()),
                manifest: OriginPath::from_path(plan.manifest_path()),
            },
            description.recipe.budget,
            Some(plan.clone()),
        )
    }
}
fn io(e: std::io::Error) -> Error {
    storage_io(e)
}
