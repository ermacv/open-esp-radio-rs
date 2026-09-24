//! Resolve all physical goal boundaries before allocating mutable sessions.
use crate::*;
struct Pending<'a> {
    phase: usize,
    side: usize,
    target: &'a ExecutionTarget,
    point: &'a ExecutionSymbol,
    goal: &'a ExecutionGoal,
}
impl Pending<'_> {
    fn same_object(&self, other: &Self) -> bool {
        self.target.revision == other.target.revision
            && self.point.source == other.point.source
            && self.point.symbol.object == other.point.symbol.object
    }
}
pub(crate) fn prepare<'m>(
    project: &Project,
    request: &ExecutionRequest,
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<AdmittedVec<'m, [Option<ResolvedExecutionGoal>; 2]>> {
    let mut result = AdmittedVec::new(memory);
    let mut pending = AdmittedVec::new(memory);
    for (phase, case) in request.cases.iter().enumerate() {
        result.push([None; 2], c.position())?;
        for (side, (input, target)) in std::iter::once((&case.vendor, &request.vendor))
            .chain(case.replacement.as_ref().zip(request.replacement.as_ref()))
            .enumerate()
        {
            match &input.goal {
                ExecutionGoal::Return => result[phase][side] = Some(ResolvedExecutionGoal::Return),
                ExecutionGoal::ObserveDequeue { .. } => {
                    result[phase][side] = Some(ResolvedExecutionGoal::ObserveDequeue)
                }
                ExecutionGoal::ReachSymbol { target: point }
                | ExecutionGoal::ObserveCall { target: point, .. } => pending.push(
                    Pending {
                        phase,
                        side,
                        target,
                        point,
                        goal: &input.goal,
                    },
                    c.position(),
                )?,
            }
        }
    }
    for (index, selected) in pending.iter().enumerate() {
        c.checkpoint(index as u64 + 1)?;
        if pending[..index].iter().any(|p| p.same_object(selected)) {
            continue;
        }
        let occurrence = KnowledgeOccurrence {
            revision: selected.target.revision.clone(),
            source: selected.point.source.clone(),
            object: selected.point.symbol.object.clone(),
            symbol: None,
        };
        crate::occurrence::with_source(project, &occurrence, memory, c, |capture, c| {
            capture.with_prepared(memory, c, |object, c| {
                for p in &*pending {
                    c.checkpoint(1)?;
                    if !p.same_object(selected) {
                        continue;
                    }
                    let address =
                        object.code_symbol_address(&p.point.symbol.object, &p.point.symbol, c)?;
                    result[p.phase][p.side] = Some(match p.goal {
                        ExecutionGoal::ReachSymbol { .. } => {
                            ResolvedExecutionGoal::ReachSymbol { address }
                        }
                        ExecutionGoal::ObserveCall { include_tail, .. } => {
                            ResolvedExecutionGoal::ObserveCall {
                                address,
                                include_tail: *include_tail,
                            }
                        }
                        ExecutionGoal::Return | ExecutionGoal::ObserveDequeue { .. } => {
                            return Err(Error::new(
                                ErrorCode::Integrity,
                                "unexpected pending return goal",
                            ));
                        }
                    });
                }
                Ok(())
            })
        })?;
    }
    Ok(result)
}
