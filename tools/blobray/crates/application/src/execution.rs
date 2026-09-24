//! Concrete execution orchestration through the ordinary durable operation lifecycle.
use crate::execution_memory::Session;
use crate::*;
use std::io::Write;
pub const EXECUTION_ENVIRONMENT: &str = "static-elf/explicit-ram/stack-words-1/register-bank-1";
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionWork {
    pub schema: u32,
    pub run: RunId,
    pub project: OriginPath,
    pub request: ExecutionRequest,
    pub producer: ExecutionProducer,
    pub budget: ResourceBudget,
    pub started_ms: u64,
    pub deadline_ms: u64,
}
struct Input {
    selected: u64,
    payload: Option<ArtifactId>,
}
impl InventorySink for Input {
    fn input(&mut self, index: u64, input: &InputRecord, _: &mut dyn RunControl) -> Result<()> {
        if index == self.selected {
            self.payload = input.capture.artifact().cloned();
        }
        Ok(())
    }
}
impl ElfSink for Input {
    fn section(&mut self, _: &SectionRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn symbol(&mut self, _: &SymbolRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn relocation(&mut self, _: &RelocationRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn diagnostic(&mut self, _: &Diagnostic, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
}
fn input(
    project: &Project,
    revision: &RevisionId,
    index: u64,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<blobray_store::FileLease> {
    let mut sink = Input {
        selected: index,
        payload: None,
    };
    project.read_inventory(Some(revision), memory, c, &mut sink)?;
    project.open_payload(
        &sink
            .payload
            .ok_or_else(|| Error::new(ErrorCode::NotFound, "execution input is not captured"))?,
        c,
    )
}
fn session<'a>(
    project: &Project,
    target: &ExecutionTarget,
    max_events: u32,
    memory: &'a WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<Session<'a>> {
    let mut session = Session::new(memory, max_events, c)?;
    match &target.source {
        FunctionSource::Image { image } => {
            let image = project.image(image, c)?;
            if image.manifest.plan.recipe.revision != target.revision {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "execution image revision differs",
                ));
            }
            session.image(&image.elf, c)?;
        }
        FunctionSource::Input { input: index } => {
            session.image(&input(project, &target.revision, *index, memory, c)?, c)?
        }
    }
    for index in &target.companions {
        session.image(&input(project, &target.revision, *index, memory, c)?, c)?;
    }
    Ok(session)
}
fn evidence(
    file: &mut blobray_store::TemporaryFile,
    case: u32,
    replacement: bool,
    observation: &ExecutionObservation,
    c: &mut dyn RunControl,
) -> Result<()> {
    for event in &observation.events {
        c.checkpoint(1)?;
        write_control_message(
            &mut *file,
            &ExecutionEvidence::Event {
                case,
                replacement,
                event: event.clone(),
            },
        )?;
        file.write_all(b"\n").map_err(storage_io)?;
    }
    write_control_message(
        &mut *file,
        &ExecutionEvidence::Outcome {
            case,
            replacement,
            stop: observation.stop.clone(),
            steps: observation.steps,
        },
    )?;
    file.write_all(b"\n").map_err(storage_io)
}
pub fn prepare_execution_worker(
    stage: &Path,
    work: &ExecutionWork,
    executor: &dyn Executor,
    c: &mut dyn RunControl,
) -> Result<blobray_store::PreparedExecutionReceipt> {
    let memory = WorkingMemory::new(
        work.budget
            .working_memory_bytes
            .ok_or_else(|| Error::new(ErrorCode::InvalidRequest, "working memory missing"))?,
    )?;
    let disk = blobray_store::TemporaryBudget::open(stage)?;
    prepare_execution_worker_in(stage, work, executor, &memory, &disk, c)
}
pub(crate) fn prepare_execution_worker_in(
    stage: &Path,
    work: &ExecutionWork,
    executor: &dyn Executor,
    memory: &WorkingMemory,
    disk: &blobray_store::TemporaryBudget,
    c: &mut dyn RunControl,
) -> Result<blobray_store::PreparedExecutionReceipt> {
    if work.schema != 1
        || (work.producer.executor != executor.identity()
            || work.producer.environment != EXECUTION_ENVIRONMENT
            || work.producer.verifier != blobray_verification::VERIFIER)
    {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "execution implementation differs from recipe",
        ));
    }
    work.request.validate()?;
    let mut control = blobray_store::TemporaryControl {
        control: c,
        budget: disk,
    };
    let result = (|| {
        let _control = memory.reserve(2 * 1024 * 1024, control.position())?;
        let project = Project::open(&work.project.to_path()?)?;
        let request = &work.request;
        let mut vendor = None;
        let mut replacement = None;
        let mut blocked = false;
        let mut complete = true;
        let mut verdict = request
            .replacement
            .as_ref()
            .map(|_| ComparisonVerdict::Match);
        let mut file = disk.temporary(&stage.join("staging"))?;
        for (index, case) in request.cases.iter().enumerate() {
            let mut position = control.position();
            position.table = Some(index as u64);
            control.set_position(position);
            control.checkpoint(1)?;
            let engine = Engine {
                project: &project,
                request,
                memory,
                executor,
            };
            let left = engine.invoke(
                &request.vendor,
                &case.vendor,
                &mut vendor,
                blocked,
                &mut control,
            )?;
            let right = match (&request.replacement, &case.replacement) {
                (Some(t), Some(i)) => {
                    Some(engine.invoke(t, i, &mut replacement, blocked, &mut control)?)
                }
                _ => None,
            };
            evidence(&mut file, index as u32, false, &left, &mut control)?;
            if let Some(right) = &right {
                evidence(&mut file, index as u32, true, right, &mut control)?;
                control.phase(RunPhase::Compare)?;
                let comparison = blobray_verification::compare(
                    &left,
                    right,
                    request.compare_return,
                    &mut control,
                )?;
                verdict = Some(match (verdict.unwrap(), comparison.verdict) {
                    (ComparisonVerdict::Diff, _) | (_, ComparisonVerdict::Diff) => {
                        ComparisonVerdict::Diff
                    }
                    (ComparisonVerdict::Incomplete, _) | (_, ComparisonVerdict::Incomplete) => {
                        ComparisonVerdict::Incomplete
                    }
                    _ => ComparisonVerdict::Match,
                });
                write_control_message(
                    &mut file,
                    &ExecutionEvidence::Comparison {
                        case: index as u32,
                        result: comparison,
                    },
                )?;
                file.write_all(b"\n").map_err(storage_io)?;
            }
            let phase_complete = matches!(left.stop, ExecutionStop::Returned { .. })
                && right
                    .as_ref()
                    .is_none_or(|r| matches!(r.stop, ExecutionStop::Returned { .. }));
            complete &= phase_complete;
            if request.case_execution == CaseExecution::Stateful && !phase_complete {
                blocked = true;
            }
            if let Some(session) = &mut vendor {
                session.recycle(left);
            }
            if let (Some(session), Some(right)) = (&mut replacement, right) {
                session.recycle(right);
            }
        }
        drop(vendor);
        drop(replacement);
        let staging = Staging::with_temporary_budget(stage, disk.clone())?;
        let records = staging.retain_temporary(file, &mut control)?;
        staging.execution_receipt(
            &ExecutionManifest {
                schema: EXECUTION_SCHEMA,
                project: project.id().clone(),
                request: request.clone(),
                producer: work.producer.clone(),
                records,
                verdict,
                complete,
            },
            &mut control,
        )
    })();
    control.memory_phases(&memory.phase_observations());
    control.working_memory(memory.observation());
    result
}

struct Engine<'a> {
    project: &'a Project,
    request: &'a ExecutionRequest,
    memory: &'a WorkingMemory,
    executor: &'a dyn Executor,
}
impl<'a> Engine<'a> {
    fn invoke(
        &self,
        target: &ExecutionTarget,
        invocation: &Invocation,
        slot: &mut Option<Session<'a>>,
        blocked: bool,
        c: &mut dyn RunControl,
    ) -> Result<ExecutionObservation> {
        let case = c.position().table;
        if blocked {
            return Ok(ExecutionObservation {
                stop: ExecutionStop::BlockedByPriorPhase,
                steps: 0,
                events: Vec::new(),
            });
        }
        if self.request.case_execution == CaseExecution::Independent {
            *slot = None;
        }
        if slot.is_none() {
            *slot = Some(session(
                self.project,
                target,
                self.request.max_events,
                self.memory,
                c,
            )?);
        }
        let machine = slot.as_mut().unwrap();
        let stack = machine.phase(target, invocation, c)?;
        c.set_position(RunPosition {
            phase: RunPhase::Execute,
            table: case,
            ..Default::default()
        });
        c.checkpoint(0)?;
        let (stop, steps) = self.executor.execute(
            target.entry,
            stack,
            &invocation.register_arguments(),
            machine,
            c,
        )?;
        Ok(machine.observation(stop, steps))
    }
}
