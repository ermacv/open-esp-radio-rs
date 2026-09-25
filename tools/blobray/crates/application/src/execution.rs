//! Concrete execution orchestration through the ordinary durable operation lifecycle.
use crate::execution_memory::Session;
use crate::*;
use std::{collections::BTreeMap, io::Write};
pub const EXECUTION_ENVIRONMENT: &str = "static-elf/byte-addressed-memory-1/phased-regions-1/physical-goals-1/stack-words-1/single-hart-atomics-1/devices-3/external-calls-1/runtime-interfaces-1/fifo-services-1/final-memory-1/physical-calls-1/reviewed-call-pairs-1/internal-timeline-1/reviewed-projections-1/reviewed-effects-1";
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionWork {
    pub schema: u32,
    pub run: RunId,
    pub project: OriginPath,
    /// Identity of the canonical request staged at admission (or, for a replay,
    /// retained by the project). Control messages never carry the request itself.
    pub request: ArtifactId,
    pub producer: ExecutionProducer,
    pub budget: ResourceBudget,
    pub started_ms: u64,
    pub deadline_ms: u64,
}
/// One mapped executable source of an execution target.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum SourceKey {
    Image(PreparedImageId),
    Input(RevisionId, u64),
}
fn source_keys(target: &ExecutionTarget) -> Vec<SourceKey> {
    let source = match &target.source {
        FunctionSource::Image { image } => SourceKey::Image(image.clone()),
        FunctionSource::Input { input } => SourceKey::Input(target.revision.clone(), *input),
    };
    std::iter::once(source)
        .chain(
            target
                .companions
                .iter()
                .map(|index| SourceKey::Input(target.revision.clone(), *index)),
        )
        .collect()
}
struct LoadedSegment<'a> {
    address: u32,
    memory_size: usize,
    flags: u32,
    bytes: ScratchBytes<'a>,
}
/// Executable segments of every source of a request, validated and loaded once.
/// Each fresh session copies them, so cold resets never reread retained sources.
struct Sources<'a> {
    loaded: BTreeMap<SourceKey, Vec<LoadedSegment<'a>>>,
}
impl<'a> Sources<'a> {
    fn load(
        project: &Project,
        request: &ExecutionRequest,
        memory: &'a WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        let targets: Vec<&ExecutionTarget> = std::iter::once(&request.vendor)
            .chain(&request.replacement)
            .collect();
        let mut loaded = BTreeMap::new();
        for target in &targets {
            if let FunctionSource::Image { image } = &target.source
                && project.image_manifest(image, c)?.0.plan.recipe.revision != target.revision
            {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "execution image revision differs",
                ));
            }
            for key in source_keys(target) {
                if loaded.contains_key(&key) {
                    continue;
                }
                let segments = match &key {
                    SourceKey::Image(image) => {
                        segments(&project.image_executable(image, c)?.1, memory, c)?
                    }
                    SourceKey::Input(revision, index) => {
                        let payload =
                            project.revision_input(revision, *index)?.ok_or_else(|| {
                                Error::new(ErrorCode::NotFound, "execution input is not captured")
                            })?;
                        segments(&project.open_payload(&payload, c)?, memory, c)?
                    }
                };
                loaded.insert(key, segments);
            }
        }
        Ok(Self { loaded })
    }
}
fn segments<'a>(
    source: &dyn ByteSource,
    memory: &'a WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<Vec<LoadedSegment<'a>>> {
    let mut result = Vec::new();
    blobray_artifacts::execution_segments(source, memory, c, &mut |segment, bytes, c| {
        let mut copy = memory.bytes(bytes.len(), c.position())?;
        for (chunk, source) in copy.chunks_mut(WORK_BLOCK).zip(bytes.chunks(WORK_BLOCK)) {
            c.checkpoint(1)?;
            chunk.copy_from_slice(source);
        }
        result
            .try_reserve(1)
            .map_err(|_| Error::new(ErrorCode::ResourceLimited, "segment allocation refused"))?;
        result.push(LoadedSegment {
            address: segment.address as u32,
            memory_size: segment.memory_size as usize,
            flags: segment.flags,
            bytes: copy,
        });
        Ok(())
    })?;
    Ok(result)
}
fn session<'a>(
    target: &ExecutionTarget,
    sources: &Sources<'_>,
    max_events: u32,
    memory: &'a WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<Session<'a>> {
    let mut session = Session::new(memory, max_events, c)?;
    for key in source_keys(target) {
        for segment in sources
            .loaded
            .get(&key)
            .ok_or_else(|| Error::new(ErrorCode::Integrity, "execution source was not prepared"))?
        {
            session.segment(
                segment.address,
                segment.memory_size,
                segment.flags,
                &segment.bytes,
                c,
            )?;
        }
    }
    Ok(session)
}
fn evidence(
    file: &mut impl Write,
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
    for chunk in &observation.final_memory {
        c.checkpoint(1)?;
        write_control_message(
            &mut *file,
            &ExecutionEvidence::FinalMemory {
                case,
                replacement,
                chunk: *chunk,
            },
        )?;
        file.write_all(b"\n").map_err(storage_io)?;
    }
    for model in &observation.models {
        c.checkpoint(1)?;
        write_control_message(
            &mut *file,
            &ExecutionEvidence::Model {
                case,
                replacement,
                observation: model.clone(),
            },
        )?;
        file.write_all(b"\n").map_err(storage_io)?;
    }
    for call in &observation.calls {
        c.checkpoint(1)?;
        write_control_message(
            &mut *file,
            &ExecutionEvidence::CallModel {
                case,
                replacement,
                observation: call.clone(),
            },
        )?;
        file.write_all(b"\n").map_err(storage_io)?;
    }
    for table in &observation.tables {
        c.checkpoint(1)?;
        write_control_message(
            &mut *file,
            &ExecutionEvidence::RuntimeTable {
                case,
                replacement,
                observation: table.clone(),
            },
        )?;
        file.write_all(b"\n").map_err(storage_io)?;
    }
    for service in &observation.services {
        c.checkpoint(1)?;
        write_control_message(
            &mut *file,
            &ExecutionEvidence::FifoService {
                case,
                replacement,
                observation: service.clone(),
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
    let mut control = blobray_store::TemporaryControl::new(c, disk);
    let result = (|| {
        let _control = memory.reserve(2 * 1024 * 1024, control.position())?;
        let project = Project::open(&work.project.to_path()?)?;
        let request = Staging::with_temporary_budget(stage, disk.clone())?.execution_request(
            &project,
            &work.request,
            memory,
            &mut control,
        )?;
        let request = &request;
        let goals = crate::execution_goals::prepare(&project, request, memory, &mut control)?;
        let tables = crate::execution_interfaces::prepare(&project, request, memory, &mut control)?;
        let pairs = project.execution_call_pairs(request, memory, &mut control)?;
        let projections = project.execution_projections(request, memory, &mut control)?;
        let effects = project.execution_effects(request, memory, &mut control)?;
        let sources = Sources::load(&project, request, memory, &mut control)?;
        let mut vendor = None;
        let mut replacement = None;
        let mut blocked = false;
        let mut complete = true;
        let mut verdict = request
            .replacement
            .as_ref()
            .map(|_| ComparisonVerdict::Match);
        // Records are small; buffer them so each is not its own metered write.
        let mut file = std::io::BufWriter::with_capacity(
            STREAM_BLOCK,
            disk.temporary(&stage.join("staging"))?,
        );
        for (index, case) in request.cases.iter().enumerate() {
            if case.reset == SessionReset::Cold {
                blocked = false;
                // Release both previous address spaces before acquiring either new one.
                vendor = None;
                replacement = None;
            }
            let mut position = control.position();
            position.table = Some(index as u64);
            control.set_position(position);
            control.checkpoint(1)?;
            let engine = Engine {
                sources: &sources,
                request,
                memory,
                executor,
                reset: case.reset,
                close_chain: request
                    .cases
                    .get(index + 1)
                    .is_none_or(|next| next.reset == SessionReset::Cold),
            };
            let left = engine.invoke(
                &request.vendor,
                &case.vendor,
                invocation_preparation(&tables, index, 0, goals[index][0], &mut control)?,
                &mut vendor,
                blocked,
                &mut control,
            )?;
            let right = match (&request.replacement, &case.replacement) {
                (Some(t), Some(i)) => Some(engine.invoke(
                    t,
                    i,
                    invocation_preparation(&tables, index, 1, goals[index][1], &mut control)?,
                    &mut replacement,
                    blocked,
                    &mut control,
                )?),
                _ => None,
            };
            evidence(&mut file, index as u32, false, &left, &mut control)?;
            if let Some(right) = &right {
                evidence(&mut file, index as u32, true, right, &mut control)?;
                control.phase(RunPhase::Compare)?;
                let comparison = blobray_verification::compare(
                    &left,
                    right,
                    case.relation.as_ref().ok_or_else(|| {
                        Error::new(ErrorCode::Integrity, "comparison relation missing")
                    })?,
                    &pairs.pairs,
                    selected_projection(case.relation.as_ref(), &projections.projections)?.map(
                        |resolved| blobray_verification::ProjectionComparison {
                            resolved,
                            vendor: &case.vendor,
                            replacement: case.replacement.as_ref().unwrap(),
                        },
                    ),
                    selected_effect_contract(case.relation.as_ref(), &effects.contracts)?,
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
            let phase_complete =
                left.completed() && right.as_ref().is_none_or(ExecutionObservation::completed);
            complete &= phase_complete;
            if !phase_complete {
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
        let file = file
            .into_inner()
            .map_err(|error| storage_io(error.into_error()))?;
        let records = staging.retain_temporary(file, &mut control)?;
        staging.execution_receipt(
            &ExecutionManifest {
                effect_contracts: effects.contracts,
                projections: projections.projections,
                call_pairs: pairs.pairs,
                schema: EXECUTION_SCHEMA,
                project: project.id().clone(),
                request: work.request.clone(),
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
    sources: &'a Sources<'a>,
    request: &'a ExecutionRequest,
    memory: &'a WorkingMemory,
    executor: &'a dyn Executor,
    reset: SessionReset,
    close_chain: bool,
}
impl<'a> Engine<'a> {
    fn invoke(
        &self,
        target: &ExecutionTarget,
        invocation: &Invocation,
        ready: InvocationPreparation<'_, '_>,
        slot: &mut Option<Session<'a>>,
        blocked: bool,
        c: &mut dyn RunControl,
    ) -> Result<ExecutionObservation> {
        let case = c.position().table;
        if blocked {
            let machine = slot.as_mut().ok_or_else(|| {
                Error::new(ErrorCode::Integrity, "blocked phase has no prior session")
            })?;
            return machine.observation(ExecutionStop::BlockedByPriorPhase, 0, self.close_chain, c);
        }
        if slot.is_none() {
            if self.reset == SessionReset::Warm {
                return Err(Error::new(
                    ErrorCode::Integrity,
                    "warm phase has no prior session",
                ));
            }
            *slot = Some(session(
                target,
                self.sources,
                self.request.max_events,
                self.memory,
                c,
            )?);
        }
        let machine = slot.as_mut().unwrap();
        let (stack, issue) = machine.phase(target, invocation, ready.tables, c)?;
        if let Some((instance, issue)) = issue {
            machine.capture_final_memory(invocation, c)?;
            return machine.observation(
                ExecutionStop::Incomplete {
                    pc: invocation.entry,
                    reason: ExecutionGap::RuntimeInterface {
                        instance: Some(instance),
                        issue,
                    },
                },
                0,
                self.close_chain,
                c,
            );
        }
        let goal = ready.goal;
        c.set_position(RunPosition {
            phase: RunPhase::Execute,
            table: case,
            ..Default::default()
        });
        c.checkpoint(0)?;
        let (stop, steps) = self.executor.execute(
            &ExecutionStart {
                entry: invocation.entry,
                stack,
                arguments: invocation.register_arguments(),
                goal,
            },
            machine,
            c,
        )?;
        machine.capture_final_memory(invocation, c)?;
        machine.observation(stop, steps, self.close_chain, c)
    }
}

struct InvocationPreparation<'a, 'm> {
    goal: ResolvedExecutionGoal,
    tables: &'a [crate::execution_interfaces::PreparedTable<'m>],
}
fn invocation_preparation<'a, 'm>(
    tables: &'a [crate::execution_interfaces::PreparedTable<'m>],
    phase: usize,
    side: usize,
    goal: Option<ResolvedExecutionGoal>,
    c: &mut dyn RunControl,
) -> Result<InvocationPreparation<'a, 'm>> {
    c.checkpoint(2 * (tables.len().max(1).ilog2() as u64 + 1))?;
    let start = tables.partition_point(|t| (t.phase, t.side) < (phase, side));
    let end = tables.partition_point(|t| (t.phase, t.side) <= (phase, side));
    Ok(InvocationPreparation {
        goal: goal.ok_or_else(|| Error::new(ErrorCode::Integrity, "execution goal unresolved"))?,
        tables: &tables[start..end],
    })
}
