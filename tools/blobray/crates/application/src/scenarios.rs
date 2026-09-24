//! Concrete user actions under one supervisor, worker, budget and publication.
use crate::*;
use serde::{Deserialize, Serialize};
use std::io::Write;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioWork {
    pub schema: u32,
    pub run: RunId,
    pub project: OriginPath,
    pub request: ScenarioRequest,
    pub budget: ResourceBudget,
    pub started_ms: u64,
    pub deadline_ms: u64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Resolution {
    pub operation: RunOperation,
    pub plan: Option<InvestigationPlan>,
}
fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, message)
}
fn resolve(
    project: &Project,
    action: &ScenarioRequest,
    memory: &WorkingMemory,
    mut entries: Option<&mut blobray_store::TemporaryFile>,
    c: &mut dyn RunControl,
) -> Result<Resolution> {
    let operation = match action {
        ScenarioRequest::ProposeEffectContract { request } => RunOperation::Knowledge {
            change: crate::effect_contracts::propose(project, request, memory, c)?,
        },
        ScenarioRequest::ProposeProjection { request } => RunOperation::Knowledge {
            change: crate::layout_projections::propose(project, request, memory, c)?,
        },
        ScenarioRequest::ProposeCallPair { request } => RunOperation::Knowledge {
            change: crate::call_pairs::propose(project, request, memory, c)?,
        },
        ScenarioRequest::ProposeData { request } => RunOperation::Knowledge {
            change: crate::data::propose_data(project, request, memory, c)?,
        },
        ScenarioRequest::ProposeConstant { request } => RunOperation::Knowledge {
            change: crate::data::propose_constant(project, request, c)?,
        },
        ScenarioRequest::Investigate { request, producer } => {
            c.phase(RunPhase::PlanInvestigation)?;
            let plan = crate::investigations::enumerate(
                project,
                request,
                producer,
                memory,
                c,
                &mut |entry, c| {
                    c.checkpoint(1)?;
                    if let Some(output) = &mut entries {
                        write_control_message(&mut **output, entry)?;
                        output.write_all(b"\n").map_err(storage_io)?;
                    }
                    Ok(())
                },
            )?;
            return Ok(Resolution {
                operation: RunOperation::Investigate {
                    revision: request
                        .revision
                        .clone()
                        .ok_or_else(|| invalid("unfrozen investigation"))?,
                    plan: plan.id.clone(),
                },
                plan: Some(plan),
            });
        }
        ScenarioRequest::Research { request } => {
            if request.options.publication != request.publication {
                return Err(invalid("research selection and options differ"));
            }
            let publication = project.publication(&request.publication, c)?;
            let mut selected = None;
            let mut count = 0;
            c.measure(WorkMetric::PublicationPasses, 1);
            blobray_store::visit_jsonl(
                &publication.members,
                c,
                |member: InvestigationMember, c| {
                    c.checkpoint(1)?;
                    if let PlanEntry::Function {
                        request: function,
                        name,
                        address_space,
                        declared_extent,
                        ..
                    } = member.entry
                        && request
                            .name
                            .as_ref()
                            .is_none_or(|n| name.as_ref().is_some_and(|a| a == n.as_bytes()))
                        && request.address.is_none_or(|a| {
                            address_space == CodeAddressSpace::Image
                                && declared_extent.start == u64::from(a)
                        })
                    {
                        count += 1;
                        if count == 1 {
                            selected = Some(function);
                        } else {
                            selected = None;
                        }
                    }
                    Ok(())
                },
            )?;
            let mut function =
                selected.ok_or_else(|| invalid("research requires one exact function"))?;
            function.research = Some(request.options.clone());
            RunOperation::AnalyzeFunction { request: function }
        }
        ScenarioRequest::ProposeRegister { request } => {
            let analysis = project.analysis(&request.analysis, c)?;
            let recipe = analysis.manifest.recipe;
            let change = KnowledgeChange {
                expected_base: request.expected_base.clone(),
                actor: request.actor.clone(),
                reason: request.reason.clone(),
                action: KnowledgeAction::Propose {
                    proposal: KnowledgeProposal {
                        subject: request.subject.clone(),
                        occurrence: KnowledgeOccurrence {
                            revision: recipe.revision,
                            source: recipe.source,
                            object: recipe.selector.object().clone(),
                            symbol: recipe.selector.symbol().cloned(),
                        },
                        claim: KnowledgeClaim::MmioRegister {
                            register: request.register.clone(),
                        },
                        evidence: vec![EvidenceRef::Analysis {
                            analysis: request.analysis.clone(),
                            record: None,
                        }],
                        note: None,
                    },
                },
            };
            blobray_knowledge::validate_change(&change)?;
            RunOperation::Knowledge { change }
        }
        ScenarioRequest::Replay {
            execution,
            producer,
        } => {
            let previous = project.execution(execution, memory, c)?;
            if previous.manifest.producer != *producer {
                return Err(Error::new(
                    ErrorCode::Incompatible,
                    "replay implementation unavailable",
                ));
            }
            RunOperation::Execute {
                request: previous.manifest.request.clone(),
                producer: producer.clone(),
            }
        }
    };
    Ok(Resolution {
        operation,
        plan: None,
    })
}
/// Validate resolved parameters against the immutable inputs selected at admission.
pub(crate) fn validate_resolution(
    project: &Project,
    action: &ScenarioRequest,
    resolution: &Resolution,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<()> {
    if let ScenarioRequest::Investigate { request, producer } = action {
        let plan = resolution
            .plan
            .as_ref()
            .ok_or_else(|| invalid("automatic planning omitted plan"))?;
        blobray_store::validate_investigation_plan(plan)?;
        if plan.recipe.project != *project.id()
            || &plan.recipe.request != request
            || &plan.recipe.producer != producer
            || resolution.operation
                != (RunOperation::Investigate {
                    revision: request
                        .revision
                        .clone()
                        .ok_or_else(|| invalid("unfrozen investigation"))?,
                    plan: plan.id.clone(),
                })
        {
            return Err(Error::new(
                ErrorCode::Integrity,
                "resolved plan differs from admitted request",
            ));
        }
    } else if resolve(project, action, memory, None, c)? != *resolution {
        return Err(Error::new(
            ErrorCode::Integrity,
            "resolved scenario differs from admitted inputs",
        ));
    }
    Ok(())
}
pub fn prepare_scenario_worker(
    stage: &Path,
    work: &ScenarioWork,
    decoder: &dyn FunctionSemantics,
    executor: &dyn Executor,
    c: &mut dyn RunControl,
) -> Result<PreparedReceipt> {
    if work.schema != 1 {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "unsupported scenario request",
        ));
    }
    let memory = WorkingMemory::new(
        work.budget
            .working_memory_bytes
            .ok_or_else(|| invalid("working capacity missing"))?,
    )?;
    let disk = blobray_store::TemporaryBudget::open(stage)?;
    let mut control = blobray_store::TemporaryControl {
        control: c,
        budget: &disk,
    };
    let c: &mut dyn RunControl = &mut control;
    let result = (|| {
        let _capacity = memory.reserve(1024 * 1024, c.position())?;
        let project = Project::open(&work.project.to_path()?)?;
        let mut entries = if matches!(work.request, ScenarioRequest::Investigate { .. }) {
            Some(disk.temporary(&stage.join("staging"))?)
        } else {
            None
        };
        let resolution = resolve(&project, &work.request, &memory, entries.as_mut(), c)?;
        c.checkpoint(0)?;
        let result = match &resolution.operation {
            RunOperation::Investigate { .. } => {
                let request = InvestigationWork {
                    schema: 1,
                    run: work.run.clone(),
                    project: work.project.clone(),
                    plan: resolution.plan.clone().unwrap(),
                    budget: work.budget.clone(),
                    started_ms: work.started_ms,
                    deadline_ms: work.deadline_ms,
                };
                PreparedReceipt::Investigation(
                    crate::investigations::prepare_investigation_worker_in(
                        stage,
                        &request,
                        decoder,
                        &memory,
                        &disk,
                        entries.take(),
                        c,
                    )?,
                )
            }
            RunOperation::AnalyzeFunction { request } => {
                let request = FunctionWork {
                    schema: 1,
                    run: work.run.clone(),
                    project: work.project.clone(),
                    request: request.clone(),
                    budget: work.budget.clone(),
                    started_ms: work.started_ms,
                    deadline_ms: work.deadline_ms,
                };
                PreparedReceipt::Function(crate::functions::prepare_function_worker_in(
                    stage, &request, decoder, &memory, &disk, c,
                )?)
            }
            RunOperation::Knowledge { change } => {
                let request = KnowledgeWork {
                    schema: 1,
                    run: work.run.clone(),
                    project: work.project.clone(),
                    change: change.clone(),
                    budget: work.budget.clone(),
                    started_ms: work.started_ms,
                    deadline_ms: work.deadline_ms,
                };
                PreparedReceipt::Knowledge(crate::knowledge::prepare_with(
                    stage, &request, decoder, &memory, &disk, c,
                )?)
            }
            RunOperation::Execute { request, producer } => {
                let request = ExecutionWork {
                    schema: 1,
                    run: work.run.clone(),
                    project: work.project.clone(),
                    request: request.clone(),
                    producer: producer.clone(),
                    budget: work.budget.clone(),
                    started_ms: work.started_ms,
                    deadline_ms: work.deadline_ms,
                };
                PreparedReceipt::Execution(crate::execution::prepare_execution_worker_in(
                    stage, &request, executor, &memory, &disk, c,
                )?)
            }
            _ => return Err(invalid("invalid scenario resolution")),
        };
        write_control_message(
            std::fs::File::create(stage.join("resolved.json")).map_err(storage_io)?,
            &resolution,
        )?;
        Ok(result)
    })();
    c.memory_phases(&memory.phase_observations());
    c.working_memory(memory.observation());
    result
}
