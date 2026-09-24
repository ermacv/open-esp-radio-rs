//! Exact static observable paths through supplied saved IR; no machine executor or storage.
use blobray_domain::*;
mod expressions;
mod index;
pub use expressions::Canonical;
use expressions::{Eval, constant, number};
use index::Index;
#[cfg(test)]
mod tests;
pub type Emitter<'a> = dyn FnMut(&TraceRecord, &mut dyn RunControl) -> Result<()> + 'a;
pub struct TraceFunction<'a> {
    pub id: &'a FunctionAnalysisId,
    pub manifest: &'a FunctionManifest,
    pub records: &'a [FunctionRecord],
}
#[derive(Clone, Copy)]
pub struct TraceLink {
    pub caller: usize,
    pub offset: u64,
    pub callee: Option<usize>,
    pub issue: Option<TraceBlocker>,
}
pub struct TraceInput<'a> {
    pub functions: &'a [TraceFunction<'a>],
    pub links: &'a [TraceLink],
    pub entry: usize,
    pub target: &'a TraceTarget,
    pub observation: &'a TraceObservation,
    pub side: TraceSide,
}
pub struct Collected<'m> {
    pub outcome: TraceOutcome,
    pub events: AdmittedVec<'m, TraceEvent>,
}
fn integrity(message: &str) -> Error {
    Error::new(ErrorCode::Integrity, message)
}
struct Frame<'m> {
    function: usize,
    invocation: u64,
    cursor: usize,
    arguments: [Option<TraceValue>; 32],
    visited: AdmittedVec<'m, bool>,
    eval: Eval<'m>,
    /// Parent call instruction and whether this invocation replaces the parent (tail transfer).
    continuation: Option<(usize, bool)>,
}
impl<'m> Frame<'m> {
    fn new(
        function: usize,
        invocation: u64,
        arguments: [Option<TraceValue>; 32],
        index: &Index<'_, '_>,
        continuation: Option<(usize, bool)>,
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Option<Self>> {
        let Some(cursor) = index.lookup(index.function.manifest.recipe.extent.start, c)? else {
            return Ok(None);
        };
        let mut visited = AdmittedVec::new(memory);
        for _ in &*index.instructions {
            c.checkpoint(1)?;
            visited.push(false, c.position())?;
        }
        Ok(Some(Self {
            function,
            invocation,
            cursor,
            arguments,
            visited,
            eval: Eval::new(index, memory, c)?,
            continuation,
        }))
    }
}
fn observed(
    observation: &TraceObservation,
    address: u32,
    width: u8,
    c: &mut dyn RunControl,
) -> Result<std::result::Result<bool, TraceBlocker>> {
    if !matches!(width, 1 | 2 | 4) {
        return Ok(Err(TraceBlocker::UnsupportedEffect));
    }
    let end = u64::from(address) + u64::from(width);
    if end > 1u64 << 32 {
        return Ok(Err(TraceBlocker::CrossingRange));
    }
    c.checkpoint(observation.ranges.len().max(1).ilog2() as u64 + 1)?;
    let first = observation
        .ranges
        .partition_point(|r| u64::from(r.start) + r.length <= u64::from(address));
    let Some(range) = observation.ranges.get(first) else {
        return Ok(Ok(false));
    };
    if u64::from(range.start) >= end {
        return Ok(Ok(false));
    }
    if address < range.start || end > u64::from(range.start) + range.length {
        return Ok(Err(TraceBlocker::CrossingRange));
    }
    Ok(Ok(true))
}
fn branch(test: BranchTest, left: TraceValue, right: TraceValue) -> Option<bool> {
    if left == right {
        return Some(matches!(
            test,
            BranchTest::Eq | BranchTest::Ge | BranchTest::Geu
        ));
    }
    let (a, b) = (number(Some(left))?, number(Some(right))?);
    Some(match test {
        BranchTest::Eq => a == b,
        BranchTest::Ne => a != b,
        BranchTest::Lt => (a as i32) < (b as i32),
        BranchTest::Ge => (a as i32) >= (b as i32),
        BranchTest::Ltu => a < b,
        BranchTest::Geu => a >= b,
    })
}
/// Retained events contain only selected ordered observables. Unknown values stop
/// the path and preserve its prefix; they never become default values.
pub fn extract<'m>(
    input: &TraceInput<'_>,
    memory: &'m WorkingMemory,
    canonical: &mut Canonical<'m>,
    c: &mut dyn RunControl,
    emit: &mut Emitter<'_>,
) -> Result<Collected<'m>> {
    input.target.validate()?;
    input.observation.validate()?;
    let mut result = Collected {
        outcome: TraceOutcome {
            exact: false,
            events: 0,
            invocations: 0,
        },
        events: AdmittedVec::new(memory),
    };
    if input.entry >= input.functions.len() {
        return Err(integrity("trace entry is absent"));
    }
    let mut indexes = AdmittedVec::new(memory);
    for function in input.functions {
        indexes.push(Index::new(function, memory, c)?, c.position())?;
    }
    for (i, link) in input.links.iter().enumerate() {
        c.checkpoint(1)?;
        if link.caller >= indexes.len()
            || link.callee.is_some_and(|n| n >= indexes.len())
            || i > 0
                && (input.links[i - 1].caller, input.links[i - 1].offset)
                    >= (link.caller, link.offset)
        {
            return Err(integrity("invalid static trace call index"));
        }
    }
    let mut complete = AdmittedVec::new(memory);
    for (caller, index) in indexes.iter().enumerate() {
        complete.push(
            index.complete_for_trace(caller, input.links, c)?,
            c.position(),
        )?;
    }
    let mut arguments = [None; 32];
    arguments[0] = Some(constant(0));
    for (r, arg) in arguments.iter_mut().enumerate().skip(1) {
        *arg = Some(
            if let Some(value) = input
                .target
                .registers
                .iter()
                .find(|v| usize::from(v.register) == r)
            {
                constant(value.value)
            } else {
                canonical.intern(
                    TraceExpression::EntryRegister { register: r as u8 },
                    c,
                    emit,
                )?
            },
        );
    }
    let mut active = AdmittedVec::new(memory);
    for _ in input.functions {
        active.push(false, c.position())?;
    }
    let mut frames = AdmittedVec::new(memory);
    let Some(root) = Frame::new(
        input.entry,
        0,
        arguments,
        &indexes[input.entry],
        None,
        memory,
        c,
    )?
    else {
        emit(
            &TraceRecord::Blocked {
                side: input.side,
                site: None,
                reason: TraceBlocker::MissingInstruction,
            },
            c,
        )?;
        return Ok(result);
    };
    frames.push(root, c.position())?;
    active[input.entry] = true;
    result.outcome.invocations = 1;
    emit(
        &TraceRecord::Invocation {
            side: input.side,
            invocation: 0,
            parent: None,
            analysis: input.functions[input.entry].id.clone(),
        },
        c,
    )?;
    let mut returning: Option<(Option<TraceValue>, Option<TraceValue>)> = None;
    loop {
        c.checkpoint(1)?;
        if let Some((low, high)) = returning.take() {
            let ended = frames
                .pop()
                .ok_or_else(|| integrity("trace frame missing on return"))?;
            active[ended.function] = false;
            match ended.continuation {
                None => {
                    result.outcome.exact = true;
                    break;
                }
                Some((_, true)) => {
                    returning = Some((low, high));
                    continue;
                }
                Some((call, false)) => {
                    let parent = frames
                        .last_mut()
                        .ok_or_else(|| integrity("trace parent missing"))?;
                    parent.eval.returns[call] = Some((low, high));
                }
            }
        }
        let frame = frames
            .last_mut()
            .ok_or_else(|| integrity("trace frame missing"))?;
        let index = &indexes[frame.function];
        let instruction = &index.instructions[frame.cursor];
        let recipe = &index.function.manifest.recipe;
        let mut position = RunPosition {
            phase: RunPhase::AnalyzeValues,
            input: match recipe.source {
                FunctionSource::Input { input } => Some(input),
                FunctionSource::Image { .. } => None,
            },
            member: match recipe.selector.object().location {
                ObjectLocation::Standalone => None,
                ObjectLocation::ArchiveMember { ordinal } => Some(ordinal),
            },
            table: Some(u64::from(recipe.section)),
            entry: Some(instruction.offset),
            ..Default::default()
        };
        position.artifact(&recipe.payload);
        c.set_position(position);
        c.checkpoint(1)?;
        let site = TraceSite {
            invocation: frame.invocation,
            analysis: index.function.id.clone(),
            record: instruction.record,
            offset: instruction.offset,
        };
        macro_rules! blocked {
            ($reason:expr) => {{
                emit(
                    &TraceRecord::Blocked {
                        side: input.side,
                        site: Some(site.clone()),
                        reason: $reason,
                    },
                    c,
                )?;
                break;
            }};
        }
        if index.function.manifest.recipe.research.is_some() {
            blocked!(TraceBlocker::ComposedInterpretation);
        }
        if !complete[frame.function] || index.function.manifest.semantics.is_none() {
            blocked!(TraceBlocker::PartialFunction);
        }
        if frame.visited[frame.cursor] {
            blocked!(TraceBlocker::Loop);
        }
        frame.visited[frame.cursor] = true;
        if instruction
            .gap
            .is_some_and(|g| g != SemanticGapReason::OpaqueCall)
        {
            blocked!(TraceBlocker::UnsupportedEffect);
        }
        if let Some((record, access, width, address, value)) = instruction.memory {
            if !matches!(access, MemoryKind::Load | MemoryKind::Store) {
                blocked!(TraceBlocker::UnsupportedEffect);
            }
            let address = frame
                .eval
                .value(address, index, &frame.arguments, canonical, c, emit)?;
            let Some(address) = number(address) else {
                blocked!(TraceBlocker::UnknownAddress);
            };
            let selected = match observed(input.observation, address, width, c)? {
                Ok(v) => v,
                Err(reason) => {
                    blocked!(reason);
                }
            };
            if selected {
                let value = if access == MemoryKind::Store {
                    let Some(value) = value else {
                        blocked!(TraceBlocker::UnknownValue);
                    };
                    let Some(value) =
                        frame
                            .eval
                            .value(value, index, &frame.arguments, canonical, c, emit)?
                    else {
                        blocked!(TraceBlocker::UnknownValue);
                    };
                    Some(value)
                } else {
                    None
                };
                let event = TraceEvent::Memory {
                    access,
                    address,
                    width,
                    value,
                };
                let ordinal = result.events.len() as u64;
                result.events.push(event, c.position())?;
                if access == MemoryKind::Load {
                    frame.eval.reads[frame.cursor] = Some((ordinal, width, address));
                }
                emit(
                    &TraceRecord::Event {
                        side: input.side,
                        index: ordinal,
                        site: TraceSite {
                            record,
                            ..site.clone()
                        },
                        event,
                    },
                    c,
                )?;
            }
        }
        if let Some((fm, predecessor, successor)) = instruction.fence {
            if fm != 0 || predecessor > 15 || successor > 15 {
                blocked!(TraceBlocker::UnsupportedFence);
            }
            if input.observation.fences {
                let event = TraceEvent::Fence {
                    predecessor,
                    successor,
                };
                let ordinal = result.events.len() as u64;
                result.events.push(event, c.position())?;
                emit(
                    &TraceRecord::Event {
                        side: input.side,
                        index: ordinal,
                        site: site.clone(),
                        event,
                    },
                    c,
                )?;
            }
        }
        c.checkpoint(input.links.len().max(1).ilog2() as u64 + 1)?;
        let link = input
            .links
            .binary_search_by_key(&(frame.function, instruction.offset), |l| {
                (l.caller, l.offset)
            })
            .ok()
            .map(|i| input.links[i]);
        let next = instruction
            .offset
            .checked_add(u64::from(instruction.decoded.length))
            .ok_or_else(|| integrity("trace offset overflow"))?;
        let direct_call = matches!(
            instruction.decoded.flow,
            InstructionFlow::Jump { link: true, .. } | InstructionFlow::Indirect { link: true, .. }
        );
        if direct_call || link.is_some() {
            let Some(link) = link else {
                blocked!(TraceBlocker::UnresolvedCall);
            };
            if let Some(reason) = link.issue {
                blocked!(reason);
            }
            let Some(callee) = link.callee else {
                blocked!(TraceBlocker::OutsideProfile);
            };
            if active[callee] {
                blocked!(TraceBlocker::RecursiveCall);
            }
            let Some(inputs) = instruction.inputs else {
                blocked!(TraceBlocker::MissingCallInputs);
            };
            if inputs.len() != 32 {
                return Err(integrity("trace call input register count differs"));
            }
            let Some(effect) = &instruction.link_effect else {
                blocked!(TraceBlocker::UnsupportedEffect);
            };
            if (direct_call && !matches!(effect.register, 1 | 5))
                || (!direct_call && effect.register != 0)
            {
                blocked!(TraceBlocker::UnsupportedEffect);
            }
            // Validate the saved producer contract without interpreting instruction bytes.
            let expected = if effect.register == 0 {
                AbstractValue::Constant { value: 0 }
            } else {
                match recipe.address_space {
                    CodeAddressSpace::Image => AbstractValue::ImageAddress {
                        address: next as u32,
                    },
                    CodeAddressSpace::Section => AbstractValue::Section {
                        section: recipe.section,
                        offset: i64::try_from(next)
                            .map_err(|_| integrity("transfer section offset overflow"))?,
                    },
                }
            };
            if *effect.value != expected {
                return Err(integrity(&format!(
                    "inconsistent transfer effect at record {}",
                    effect.record
                )));
            }
            let mut arguments = [None; 32];
            for (dest, value) in arguments.iter_mut().zip(inputs) {
                *dest = frame
                    .eval
                    .value(value, index, &frame.arguments, canonical, c, emit)?;
            }
            if effect.register != 0 {
                // A section-relative return address stays unknown to this physical
                // trace profile; never retain the caller's previous link value.
                arguments[usize::from(effect.register)] =
                    frame
                        .eval
                        .value(effect.value, index, &frame.arguments, canonical, c, emit)?;
            }
            arguments[0] = Some(constant(0));
            let call = frame.cursor;
            if direct_call {
                let Some(cursor) = index.lookup(next, c)? else {
                    blocked!(TraceBlocker::MissingInstruction);
                };
                frame.cursor = cursor;
            }
            let invocation = result.outcome.invocations;
            let Some(child) = Frame::new(
                callee,
                invocation,
                arguments,
                &indexes[callee],
                Some((call, !direct_call)),
                memory,
                c,
            )?
            else {
                blocked!(TraceBlocker::MissingInstruction);
            };
            frames.push(child, c.position())?;
            active[callee] = true;
            result.outcome.invocations += 1;
            emit(
                &TraceRecord::Invocation {
                    side: input.side,
                    invocation,
                    parent: Some(site),
                    analysis: indexes[callee].function.id.clone(),
                },
                c,
            )?;
            continue;
        }
        if instruction.gap == Some(SemanticGapReason::OpaqueCall) {
            blocked!(TraceBlocker::UnresolvedCall);
        }
        let destination = match instruction.decoded.flow {
            InstructionFlow::Next => next,
            InstructionFlow::Branch { displacement } => {
                let Some((test, left, right)) = instruction.condition else {
                    blocked!(TraceBlocker::UnknownCondition);
                };
                let left = frame
                    .eval
                    .value(left, index, &frame.arguments, canonical, c, emit)?;
                let right = frame
                    .eval
                    .value(right, index, &frame.arguments, canonical, c, emit)?;
                let Some(taken) = left.zip(right).and_then(|(a, b)| branch(test, a, b)) else {
                    blocked!(TraceBlocker::UnknownCondition);
                };
                emit(
                    &TraceRecord::Branch {
                        side: input.side,
                        site: site.clone(),
                        taken,
                    },
                    c,
                )?;
                if taken {
                    match crate::target(instruction.offset, i64::from(displacement)) {
                        Some(n) => n,
                        None => {
                            blocked!(TraceBlocker::MissingInstruction);
                        }
                    }
                } else {
                    next
                }
            }
            InstructionFlow::Jump {
                displacement,
                link: false,
            } => match crate::target(instruction.offset, i64::from(displacement)) {
                Some(n) => n,
                None => {
                    blocked!(TraceBlocker::MissingInstruction);
                }
            },
            InstructionFlow::Indirect {
                base: 1 | 5,
                offset: 0,
                link: false,
            } => {
                let Some((low, high)) = instruction.returns else {
                    blocked!(TraceBlocker::NoReturn);
                };
                let low = frame
                    .eval
                    .value(low, index, &frame.arguments, canonical, c, emit)?;
                let high = frame
                    .eval
                    .value(high, index, &frame.arguments, canonical, c, emit)?;
                emit(
                    &TraceRecord::Return {
                        side: input.side,
                        site,
                        low,
                        high,
                    },
                    c,
                )?;
                returning = Some((low, high));
                continue;
            }
            InstructionFlow::Indirect { .. } => {
                blocked!(TraceBlocker::UnexpandedTransfer);
            }
            InstructionFlow::Stop => {
                blocked!(TraceBlocker::NoReturn);
            }
            InstructionFlow::Jump { link: true, .. } => unreachable!(),
        };
        let Some(cursor) = index.lookup(destination, c)? else {
            blocked!(TraceBlocker::MissingInstruction);
        };
        frame.cursor = cursor;
    }
    result.outcome.events = result.events.len() as u64;
    Ok(result)
}
/// Exact equal canonical expressions prove the selected relation. A difference in
/// opaque symbolic expressions is undecided, not evidence of physical inequality.
pub fn compare(
    left: &Collected<'_>,
    right: &Collected<'_>,
    c: &mut dyn RunControl,
    emit: &mut Emitter<'_>,
) -> Result<ComparisonVerdict> {
    let mut undecided = false;
    for i in 0..left.events.len().max(right.events.len()) {
        c.checkpoint(1)?;
        let (a, b) = (left.events.get(i).copied(), right.events.get(i).copied());
        if a == b {
            continue;
        }
        // A missing event proves a difference only after that side has ended.
        // Before then the observed events are just a prefix of its trace.
        if a.is_none() && !left.outcome.exact || b.is_none() && !right.outcome.exact {
            break;
        }
        if let (
            Some(TraceEvent::Memory {
                access: a_access,
                address: a_address,
                width: a_width,
                value: Some(a_value),
            }),
            Some(TraceEvent::Memory {
                access: b_access,
                address: b_address,
                width: b_width,
                value: Some(b_value),
            }),
        ) = (a, b)
            && (a_access, a_address, a_width) == (b_access, b_address, b_width)
            && (number(Some(a_value)).is_none() || number(Some(b_value)).is_none())
        {
            undecided = true;
            continue;
        }
        emit(
            &TraceRecord::Difference {
                index: i as u64,
                left: a,
                right: b,
            },
            c,
        )?;
        return Ok(ComparisonVerdict::Diff);
    }
    if undecided {
        emit(
            &TraceRecord::Blocked {
                side: TraceSide::Right,
                site: None,
                reason: TraceBlocker::UnresolvedValueEquality,
            },
            c,
        )?;
        Ok(ComparisonVerdict::Incomplete)
    } else if left.outcome.exact && right.outcome.exact {
        Ok(ComparisonVerdict::Match)
    } else {
        Ok(ComparisonVerdict::Incomplete)
    }
}
