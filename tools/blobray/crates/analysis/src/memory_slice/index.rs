//! One admitted instruction/access index over the borrowed saved stream.
use super::*;
pub(super) struct Instruction {
    pub offset: u64,
    pub clobber: Option<(usize, SliceIssue)>,
}
pub(super) struct Access<'m> {
    pub node: usize,
    pub record: usize,
    pub kind: MemoryKind,
    pub locations: Vec<SliceLocation>,
    pub issue: Option<AccessIssue>,
    _capacity: MemoryReservation<'m>,
}
pub(super) struct Index<'m> {
    pub instructions: AdmittedVec<'m, Instruction>,
    pub edges: AdmittedVec<'m, reaching::Edge>,
    pub accesses: AdmittedVec<'m, Access<'m>>,
    pub by_record: AdmittedVec<'m, usize>,
    pub reachable: AdmittedVec<'m, bool>,
    pub before: AdmittedVec<'m, bool>,
    pub entry: usize,
    pub anchor: usize,
    pub closed: bool,
}
pub(super) fn node(
    instructions: &[Instruction],
    offset: u64,
    c: &mut dyn RunControl,
) -> Result<usize> {
    c.checkpoint(instructions.len().max(1).ilog2() as u64 + 1)?;
    instructions
        .binary_search_by_key(&offset, |n| n.offset)
        .map_err(|_| integrity("saved fact does not name an instruction"))
}
pub(super) fn writes(kind: MemoryKind) -> bool {
    matches!(
        kind,
        MemoryKind::Store | MemoryKind::StoreConditional | MemoryKind::Atomic
    )
}
impl<'m> Index<'m> {
    pub fn new(
        records: &[FunctionRecord],
        recipe: &FunctionRecipe,
        coverage: FunctionCoverage,
        request: &MemorySliceQuery,
        facts: &Facts<'_, '_>,
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        let anchor_record = records
            .get(
                usize::try_from(request.anchor)
                    .map_err(|_| invalid("anchor record exceeds host index"))?,
            )
            .ok_or_else(|| invalid("anchor record is absent"))?;
        let anchor_offset = match anchor_record {
            FunctionRecord::Transfer { offset, .. } => *offset,
            FunctionRecord::Instruction {
                offset, decoded, ..
            } if matches!(
                decoded.flow,
                InstructionFlow::Jump { link: true, .. }
                    | InstructionFlow::Indirect { link: true, .. }
            ) =>
            {
                *offset
            }
            FunctionRecord::MemoryAccess { offset, access, .. } if writes(*access) => *offset,
            _ => {
                return Err(invalid(
                    "publication anchor must be an exact local call, transfer or write record",
                ));
            }
        };
        let mut instructions = AdmittedVec::new(memory);
        for (record, fact) in records.iter().enumerate() {
            c.checkpoint(1)?;
            if let FunctionRecord::Instruction {
                offset, decoded, ..
            } = fact
            {
                let call = matches!(
                    decoded.flow,
                    InstructionFlow::Jump { link: true, .. }
                        | InstructionFlow::Indirect { link: true, .. }
                );
                instructions.push(
                    Instruction {
                        offset: *offset,
                        clobber: call.then_some((record, SliceIssue::CallClobber)),
                    },
                    c.position(),
                )?;
            }
        }
        charge_sort(instructions.len(), c)?;
        instructions.sort_unstable_by_key(|n| n.offset);
        let entry = node(&instructions, recipe.extent.start, c)?;
        let anchor = node(&instructions, anchor_offset, c)?;
        let mut edges = AdmittedVec::new(memory);
        let mut closed = coverage.complete();
        for (record, fact) in records.iter().enumerate() {
            c.checkpoint(1)?;
            match fact {
                FunctionRecord::Edge {
                    from,
                    target: Some(to),
                    relation,
                    external: false,
                } if *relation != EdgeKind::Call => {
                    let from = node(&instructions, *from, c)?;
                    match node(&instructions, *to, c) {
                        Ok(to) => edges.push(
                            crate::flow::Arc {
                                from,
                                to,
                                record: record as u64,
                            },
                            c.position(),
                        )?,
                        Err(error)
                            if error.code == ErrorCode::Integrity && !coverage.complete() =>
                        {
                            closed = false
                        }
                        Err(error) => return Err(error),
                    }
                }
                FunctionRecord::SemanticGap { offset, reason } => {
                    let problem = match reason {
                        SemanticGapReason::UnsupportedInstruction
                        | SemanticGapReason::ConflictingBoundary
                        | SemanticGapReason::UnresolvedRelocation => {
                            Some(SliceIssue::UnsupportedSemantics)
                        }
                        SemanticGapReason::OpaqueCall => Some(SliceIssue::CallClobber),
                        SemanticGapReason::UnexpandedControlFlow
                        | SemanticGapReason::AlternativeLimit => None,
                    };
                    if let Some(problem) = problem {
                        let n = node(&instructions, *offset, c)?;
                        instructions[n].clobber = Some((record, problem));
                    }
                }
                _ => (),
            }
        }
        charge_sort(edges.len(), c)?;
        edges.sort_unstable_by_key(|e| (e.from, e.to, e.record));
        let reached =
            crate::flow::reachable(instructions.len(), &edges, entry, u32::MAX, memory, c)?;
        let mut reachable = AdmittedVec::new(memory);
        for r in reached.iter() {
            c.checkpoint(1)?;
            reachable.push(r.is_some(), c.position())?;
        }
        drop(reached);
        let mut reverse = AdmittedVec::new(memory);
        for e in edges.iter() {
            reverse.push(
                reaching::Edge {
                    from: e.from,
                    to: e.to,
                },
                c.position(),
            )?;
        }
        charge_sort(reverse.len(), c)?;
        reverse.sort_unstable_by_key(|e| (e.to, e.from));
        let cyclic = cycles::identify(instructions.len(), &edges, &reverse, memory, c)?;
        drop(edges);
        let mut stable = AdmittedVec::new(memory);
        let stable_value = |value: &AbstractValue, stable: &[bool]| match value {
            AbstractValue::Expression { id } => stable.get(*id as usize).copied().unwrap_or(false),
            AbstractValue::Unknown | AbstractValue::Alternatives { .. } => false,
            _ => true,
        };
        for fact in records {
            c.checkpoint(1)?;
            if let FunctionRecord::Expression {
                id,
                offset,
                origin,
                expression,
            } = fact
            {
                if *id as usize != stable.len() {
                    return Err(integrity("noncanonical expression identity"));
                }
                c.checkpoint(instructions.len().max(1).ilog2() as u64 + 1)?;
                let once = origin.is_none()
                    && closed
                    && instructions
                        .binary_search_by_key(offset, |n| n.offset)
                        .ok()
                        .is_some_and(|n| reachable[n] && !cyclic[n]);
                let immutable = match expression {
                    Expression::EntryRegister { .. } => origin.is_none(),
                    Expression::Integer { left, right, .. } => {
                        once || stable_value(left, &stable) && stable_value(right, &stable)
                    }
                    Expression::Load { .. } | Expression::CallResult { .. } => once,
                };
                stable.push(immutable, c.position())?;
            }
        }
        drop(cyclic);

        let mut empty = AdmittedVec::new(memory);
        for _ in instructions.iter() {
            empty.push(reaching::Step::default(), c.position())?;
        }
        let preceding = reaching::search(&reverse, &empty, &reachable, entry, anchor, memory, c)?;
        let mut before = AdmittedVec::new(memory);
        for p in preceding.parents.iter() {
            before.push(p.is_some(), c.position())?;
        }
        drop(preceding);
        drop(empty);
        let mut accesses = AdmittedVec::new(memory);
        for (record, fact) in records.iter().enumerate() {
            c.checkpoint(1)?;
            if let FunctionRecord::MemoryAccess {
                offset,
                access,
                width,
                address,
                ..
            } = fact
            {
                valid_width(*width)?;
                let node = node(&instructions, *offset, c)?;
                let (locations, issue) =
                    locations::identify(address, *width, recipe, facts, &stable, request.abi, c)?;
                let capacity = memory.reserve(
                    (locations.capacity() * std::mem::size_of::<SliceLocation>()) as u64
                        + locations.iter().map(locations::bytes).sum::<u64>(),
                    c.position(),
                )?;
                accesses.push(
                    Access {
                        node,
                        record,
                        kind: *access,
                        locations,
                        issue,
                        _capacity: capacity,
                    },
                    c.position(),
                )?;
            }
        }
        charge_sort(accesses.len(), c)?;
        accesses.sort_unstable_by_key(|a| a.node);
        if accesses.windows(2).any(|w| w[0].node == w[1].node) {
            return Err(integrity(
                "multiple unordered local memory accesses at one instruction",
            ));
        }
        let mut by_record = AdmittedVec::new(memory);
        for i in 0..accesses.len() {
            by_record.push(i, c.position())?;
        }
        charge_sort(by_record.len(), c)?;
        by_record.sort_unstable_by_key(|i| accesses[*i].record);
        Ok(Self {
            instructions,
            edges: reverse,
            accesses,
            by_record,
            reachable,
            before,
            entry,
            anchor,
            closed,
        })
    }
}
