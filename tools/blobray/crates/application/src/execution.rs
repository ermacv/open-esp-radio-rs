//! Concrete execution orchestration through the ordinary durable operation lifecycle.
use crate::execution_memory::Session;
use crate::*;
use std::{collections::BTreeMap, io::Write};
pub const EXECUTION_ENVIRONMENT: &str = "static-elf/boot-data-1/entry-registers-1/byte-addressed-memory-1/phased-regions-1/physical-goals-1/stack-words-1/single-hart-atomics-1/devices-4/external-calls-2/runtime-interfaces-1/fifo-services-1/final-memory-1/physical-calls-1/reviewed-call-pairs-1/internal-timeline-1/reviewed-projections-1/reviewed-effects-1";
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
pub(crate) struct LoadedSegment<'a> {
    pub address: u32,
    pub memory_size: usize,
    pub flags: u32,
    pub bytes: ScratchBytes<'a>,
}
/// Executable segments of every source of a request, validated and loaded once.
/// Each fresh session copies them, so cold resets never reread retained sources.
pub(crate) struct Sources<'a> {
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
        Self::load_targets(project, &targets, memory, c)
    }
    pub(crate) fn load_targets(
        project: &Project,
        targets: &[&ExecutionTarget],
        memory: &'a WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        let mut loaded = BTreeMap::new();
        for target in targets {
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
    /// Segments of explicit executables: `executables[i]` holds the ELF bytes
    /// of each target's sources in `source_keys` order (its source, then its
    /// companions). No project participates.
    pub(crate) fn from_executables(
        targets: &[(&ExecutionTarget, &[&[u8]])],
        memory: &'a WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        let mut loaded = BTreeMap::new();
        for (target, executables) in targets {
            let keys = source_keys(target);
            if keys.len() != executables.len() {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "one executable is required per target source",
                ));
            }
            for (key, bytes) in keys.into_iter().zip(executables.iter()) {
                if loaded.contains_key(&key) {
                    continue;
                }
                loaded.insert(key, segments(bytes, memory, c)?);
            }
        }
        Ok(Self { loaded })
    }
    /// Every loaded segment of `target`'s sources.
    pub(crate) fn target_segments(
        &self,
        target: &ExecutionTarget,
    ) -> impl Iterator<Item = &LoadedSegment<'a>> {
        source_keys(target)
            .into_iter()
            .filter_map(|key| self.loaded.get(&key))
            .flatten()
    }
}
/// The retained executable of one source of `target`, in `source_keys` order.
pub(crate) fn target_executables(
    project: &Project,
    target: &ExecutionTarget,
    c: &mut dyn RunControl,
    visit: &mut dyn FnMut(&dyn ByteSource, &mut dyn RunControl) -> Result<()>,
) -> Result<()> {
    for key in source_keys(target) {
        match &key {
            SourceKey::Image(image) => visit(&project.image_executable(image, c)?.1, c)?,
            SourceKey::Input(revision, index) => {
                let payload = project.revision_input(revision, *index)?.ok_or_else(|| {
                    Error::new(ErrorCode::NotFound, "execution input is not captured")
                })?;
                visit(&project.open_payload(&payload, c)?, c)?;
            }
        }
    }
    Ok(())
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
    coverage: crate::execution_coverage::CodeCoverage<'a>,
    c: &mut dyn RunControl,
) -> Result<Session<'a>> {
    let mut session = Session::new(memory, max_events, coverage, c)?;
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
/// Receives each evidence record in stream order.
pub(crate) type Emit<'e> = dyn FnMut(ExecutionEvidence, &mut dyn RunControl) -> Result<()> + 'e;

fn evidence(
    emit: &mut Emit<'_>,
    case: u32,
    replacement: bool,
    observation: &ExecutionObservation,
    c: &mut dyn RunControl,
) -> Result<()> {
    for event in &observation.events {
        c.checkpoint(1)?;
        emit(
            ExecutionEvidence::Event {
                case,
                replacement,
                event: event.clone(),
            },
            c,
        )?;
    }
    for chunk in &observation.final_memory {
        c.checkpoint(1)?;
        emit(
            ExecutionEvidence::FinalMemory {
                case,
                replacement,
                chunk: *chunk,
            },
            c,
        )?;
    }
    for model in &observation.models {
        c.checkpoint(1)?;
        emit(
            ExecutionEvidence::Model {
                case,
                replacement,
                observation: model.clone(),
            },
            c,
        )?;
    }
    for call in &observation.calls {
        c.checkpoint(1)?;
        emit(
            ExecutionEvidence::CallModel {
                case,
                replacement,
                observation: call.clone(),
            },
            c,
        )?;
    }
    for table in &observation.tables {
        c.checkpoint(1)?;
        emit(
            ExecutionEvidence::RuntimeTable {
                case,
                replacement,
                observation: table.clone(),
            },
            c,
        )?;
    }
    for service in &observation.services {
        c.checkpoint(1)?;
        emit(
            ExecutionEvidence::FifoService {
                case,
                replacement,
                observation: service.clone(),
            },
            c,
        )?;
    }
    emit(
        ExecutionEvidence::Outcome {
            case,
            replacement,
            stop: observation.stop.clone(),
            steps: observation.steps,
        },
        c,
    )
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
        // Records are small; buffer them so each is not its own metered write.
        let mut file = std::io::BufWriter::with_capacity(
            STREAM_BLOCK,
            blobray_store::ExecutionRecordWriter::new(disk.temporary(&stage.join("staging"))?),
        );
        let RunOutcome {
            verdict, complete, ..
        } = run_resolved(
            &Resolved {
                request,
                goals: &goals,
                tables: &tables,
                pairs: &pairs.pairs,
                projections: &projections.projections,
                effects: &effects.contracts,
                sources: &sources,
                patches: &[],
            },
            &mut VendorSide::Execute(None),
            None,
            executor,
            memory,
            &mut |record, _| {
                write_control_message(&mut file, &record)?;
                file.write_all(b"\n").map_err(storage_io)
            },
            &mut control,
        )?;
        let staging = Staging::with_temporary_budget(stage, disk.clone())?;
        let file = file
            .into_inner()
            .map_err(|error| storage_io(error.into_error()))?
            .finish()
            .map_err(storage_io)?;
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

/// Resolved inputs of one execution request.
pub(crate) struct Resolved<'r, 'm> {
    pub request: &'r ExecutionRequest,
    pub goals: &'r [[Option<ResolvedExecutionGoal>; 2]],
    pub tables: &'r [crate::execution_interfaces::PreparedTable<'m>],
    pub pairs: &'r [ResolvedCallPair],
    pub projections: &'r [ResolvedProjection],
    pub effects: &'r [ResolvedEffectContract],
    pub sources: &'r Sources<'m>,
    /// Byte patches applied to every replacement session's loaded image.
    pub patches: &'r [crate::in_process::ImagePatch],
}

/// Receives the step log of each finished replacement session.
pub(crate) type StepSink<'s, 'm> =
    dyn FnMut(crate::execution_steps::StepLog<'m>, &mut dyn RunControl) -> Result<()> + 's;

/// How the vendor side of a resolved request is obtained.
pub(crate) enum VendorSide<'v> {
    /// Execute it; with a sink, keep each case's observation.
    Execute(Option<&'v mut Vec<ExecutionObservation>>),
    /// Reuse the observations and coverage of an earlier execution of the
    /// same vendor side.
    Reuse {
        cases: &'v [ExecutionObservation],
        coverage: &'v [ExecutionCoverage],
    },
}

/// Aggregate outcome of a resolved request.
pub(crate) struct RunOutcome {
    pub verdict: Option<ComparisonVerdict>,
    pub complete: bool,
    /// False when an incomplete replacement case blocked the vendor side of
    /// the next warm case, so the vendor observations depend on the
    /// replacement. Reuse then stops at that case, before emitting it.
    pub vendor_independent: bool,
}

/// Execute every case of a resolved request, handing each evidence record to
/// `emit` in stream order.
pub(crate) fn run_resolved<'m>(
    resolved: &Resolved<'_, 'm>,
    vendor: &mut VendorSide<'_>,
    mut steps: Option<&mut StepSink<'_, 'm>>,
    executor: &dyn Executor,
    memory: &'m WorkingMemory,
    emit: &mut Emit<'_>,
    control: &mut dyn RunControl,
) -> Result<RunOutcome> {
    let request = resolved.request;
    let mut sides: [Side<'m>; 2] = Default::default();
    sides[1].record = steps.is_some();
    sides[1].patched = true;
    let mut blocked = false;
    // Whether `blocked` holds only because the replacement did not complete.
    let mut blocked_by_replacement = false;
    let mut vendor_independent = true;
    let mut complete = true;
    let mut verdict = request
        .replacement
        .as_ref()
        .map(|_| ComparisonVerdict::Match);
    for (index, case) in request.cases.iter().enumerate() {
        if case.reset == SessionReset::Cold {
            blocked = false;
            blocked_by_replacement = false;
            // Release both previous address spaces before acquiring either new one.
            for side in &mut sides {
                side.release();
            }
            if let Some(sink) = steps.as_mut() {
                for log in sides[1].steps.drain(..) {
                    sink(log, control)?;
                }
            }
        }
        let mut position = control.position();
        position.table = Some(index as u64);
        control.set_position(position);
        control.checkpoint(1)?;
        let engine = Engine {
            sources: resolved.sources,
            request,
            memory,
            executor,
            reset: case.reset,
            stack_fill: case.stack_fill,
            patches: resolved.patches,
            close_chain: request
                .cases
                .get(index + 1)
                .is_none_or(|next| next.reset == SessionReset::Cold),
        };
        if blocked_by_replacement {
            vendor_independent = false;
            if matches!(vendor, VendorSide::Reuse { .. }) {
                break;
            }
        }
        let left = match vendor {
            VendorSide::Reuse { cases, .. } => {
                std::borrow::Cow::Borrowed(cases.get(index).ok_or_else(|| {
                    Error::new(ErrorCode::Integrity, "reused vendor case is missing")
                })?)
            }
            VendorSide::Execute(_) => std::borrow::Cow::Owned(engine.invoke(
                &request.vendor,
                &case.vendor,
                invocation_preparation(
                    resolved.tables,
                    index,
                    0,
                    resolved.goals[index][0],
                    control,
                )?,
                &mut sides[0],
                blocked,
                control,
            )?),
        };
        let right = match (&request.replacement, &case.replacement) {
            (Some(t), Some(i)) => Some(engine.invoke(
                t,
                i,
                invocation_preparation(
                    resolved.tables,
                    index,
                    1,
                    resolved.goals[index][1],
                    control,
                )?,
                &mut sides[1],
                blocked,
                control,
            )?),
            _ => None,
        };
        evidence(emit, index as u32, false, &left, control)?;
        if let Some(right) = &right {
            evidence(emit, index as u32, true, right, control)?;
            control.phase(RunPhase::Compare)?;
            let comparison = blobray_verification::compare(
                &left,
                right,
                case.relation.as_ref().ok_or_else(|| {
                    Error::new(ErrorCode::Integrity, "comparison relation missing")
                })?,
                resolved.pairs,
                selected_projection(case.relation.as_ref(), resolved.projections)?.map(
                    |resolved| blobray_verification::ProjectionComparison {
                        resolved,
                        vendor: &case.vendor,
                        replacement: case.replacement.as_ref().unwrap(),
                    },
                ),
                selected_effect_contract(case.relation.as_ref(), resolved.effects)?,
                control,
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
            if let Some(steps) = sides[1].session.as_mut().and_then(|s| s.steps()) {
                let relation = case.relation.as_ref().unwrap();
                let compared = blobray_verification::compared_observations(
                    right,
                    relation,
                    resolved.pairs,
                    selected_projection(Some(relation), resolved.projections)?
                        .map(|p| &p.projection),
                    selected_effect_contract(Some(relation), resolved.effects)?,
                    control,
                )?;
                steps.end_case(
                    crate::execution_steps::CaseSinks::of(
                        &compared,
                        right,
                        case.replacement.as_ref().unwrap(),
                    ),
                    control,
                )?;
            }
            emit(
                ExecutionEvidence::Comparison {
                    case: index as u32,
                    result: comparison,
                },
                control,
            )?;
        }
        let phase_complete =
            left.completed() && right.as_ref().is_none_or(ExecutionObservation::completed);
        complete &= phase_complete;
        if !phase_complete && !blocked {
            blocked = true;
            blocked_by_replacement = left.completed();
        }
        if let VendorSide::Execute(Some(sink)) = vendor {
            sink.push(left.as_ref().clone());
        }
        if let (Some(session), std::borrow::Cow::Owned(left)) = (&mut sides[0].session, left) {
            session.recycle(left);
        }
        if let (Some(session), Some(right)) = (&mut sides[1].session, right) {
            session.recycle(right);
        }
    }
    if !vendor_independent && matches!(vendor, VendorSide::Reuse { .. }) {
        return Ok(RunOutcome {
            verdict,
            complete,
            vendor_independent,
        });
    }
    for (index, side) in sides.iter_mut().enumerate() {
        side.release();
        if index == 1 && request.replacement.is_none() {
            break;
        }
        let parts = match vendor {
            VendorSide::Reuse { coverage, .. } if index == 0 => coverage.to_vec(),
            _ => side.coverage.finish(memory, control)?.0.split(),
        };
        for part in parts {
            control.checkpoint(1)?;
            emit(
                ExecutionEvidence::Coverage {
                    replacement: index == 1,
                    coverage: part,
                },
                control,
            )?;
        }
    }
    if let Some(sink) = steps.as_mut() {
        for log in sides[1].steps.drain(..) {
            sink(log, control)?;
        }
    }
    Ok(RunOutcome {
        verdict,
        complete,
        vendor_independent,
    })
}

/// One side's live session and the coverage and step logs that outlive its
/// sessions.
#[derive(Default)]
struct Side<'a> {
    session: Option<Session<'a>>,
    coverage: crate::execution_coverage::CodeCoverage<'a>,
    steps: Vec<crate::execution_steps::StepLog<'a>>,
    /// Whether this side's sessions record step logs.
    record: bool,
    /// Whether this side's sessions apply the request's image patches.
    patched: bool,
}
impl Side<'_> {
    /// Drop the session, keeping what it reached and recorded.
    fn release(&mut self) {
        if let Some(mut session) = self.session.take() {
            self.steps.extend(session.take_steps());
            self.coverage = session.into_coverage();
        }
    }
}

struct Engine<'a, 'm> {
    sources: &'a Sources<'m>,
    request: &'a ExecutionRequest,
    memory: &'m WorkingMemory,
    executor: &'a dyn Executor,
    reset: SessionReset,
    stack_fill: Option<u8>,
    patches: &'a [crate::in_process::ImagePatch],
    close_chain: bool,
}
impl<'m> Engine<'_, 'm> {
    fn invoke(
        &self,
        target: &ExecutionTarget,
        invocation: &Invocation,
        ready: InvocationPreparation<'_, '_>,
        side: &mut Side<'m>,
        blocked: bool,
        c: &mut dyn RunControl,
    ) -> Result<ExecutionObservation> {
        let case = c.position().table;
        if blocked {
            let machine = side.session.as_mut().ok_or_else(|| {
                Error::new(ErrorCode::Integrity, "blocked phase has no prior session")
            })?;
            return machine.observation(ExecutionStop::BlockedByPriorPhase, 0, self.close_chain, c);
        }
        if side.session.is_none() {
            if self.reset == SessionReset::Warm {
                return Err(Error::new(
                    ErrorCode::Integrity,
                    "warm phase has no prior session",
                ));
            }
            side.session = Some(session(
                target,
                self.sources,
                self.request.max_events,
                self.memory,
                std::mem::take(&mut side.coverage),
                c,
            )?);
            let created = side.session.as_mut().unwrap();
            if side.patched {
                for patch in self.patches {
                    created.patch(patch, c)?;
                }
            }
            if side.record {
                created.record_steps();
            }
        }
        let machine = side.session.as_mut().unwrap();
        let (stack, issue) = machine.phase(target, self.stack_fill, invocation, ready.tables, c)?;
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
