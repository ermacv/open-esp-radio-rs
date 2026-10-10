//! Bounded local control-flow exploration. No filesystem, project or ISA implementation.
use oer_riscv_model::*;
mod references;
mod value_sets;
mod values;
pub use references::PreparedReferences;
pub use values::fold_integer;

pub struct FunctionInput<'a> {
    pub image: Option<&'a dyn ImageMemory>,
    pub section: u32,
    pub extent: CodeRange,
    pub bytes: &'a [u8],
    pub relocations: &'a PreparedReferences<'a>,
    pub data_ranges: &'a [CodeRange],
    /// Indirect jumps whose targets the caller knows from the build's facts.
    pub jumps: &'a [KnownJump],
}
/// The targets of one indirect jump that the build's facts name, such as
/// the entries a jump table's relocations hold: the control-flow graph
/// follows them as jumps.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KnownJump {
    pub site: u64,
    pub targets: Vec<u64>,
}
#[derive(Default)]
pub struct AnalysisSummary {
    pub coverage: FunctionCoverage,
    pub instructions: u64,
    pub blocks: u64,
    pub edges: u64,
    pub references: u64,
    pub gaps: u64,
    pub semantics: SemanticSummary,
}
struct Node {
    offset: u64,
    decoded: DecodedOp,
    conflict: bool,
}
struct Edge {
    from: u64,
    target: Option<u64>,
    relation: EdgeKind,
    external: bool,
}
fn reserve_vec<T>(capacity: usize) -> Result<Vec<T>> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(capacity)
        .map_err(|_| Error::new(ErrorCode::ResourceLimited, "graph allocation refused"))?;
    Ok(result)
}
fn target(base: u64, delta: i64) -> Option<u64> {
    base.checked_add_signed(delta)
}
/// Every queued halfword is marked before insertion. Cycles cannot grow the queue.
pub fn analyze(
    input: FunctionInput<'_>,
    decoder: &dyn FunctionSemantics,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    sink: &mut dyn FunctionSink,
) -> Result<AnalysisSummary> {
    analyze_with(input, decoder, memory, control, sink, false, None)
}
/// The same CFG/value engine with a bounded expression DAG and explicit ABI assumption.
pub fn research(
    input: FunctionInput<'_>,
    decoder: &dyn FunctionSemantics,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    sink: &mut dyn FunctionSink,
    abi: Option<CallAbi>,
) -> Result<AnalysisSummary> {
    analyze_with(input, decoder, memory, control, sink, true, abi)
}
#[allow(clippy::too_many_arguments)]
fn analyze_with(
    input: FunctionInput<'_>,
    decoder: &dyn FunctionSemantics,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    sink: &mut dyn FunctionSink,
    symbolic: bool,
    abi: Option<CallAbi>,
) -> Result<AnalysisSummary> {
    control.phase(RunPhase::AnalyzeFunction)?;
    let end = input
        .extent
        .start
        .checked_add(input.extent.length)
        .ok_or_else(|| Error::new(ErrorCode::InvalidRequest, "code extent overflow"))?;
    if input.bytes.is_empty()
        || input.bytes.len() as u64 != input.extent.length
        || !input.extent.start.is_multiple_of(2)
    {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "code view and extent disagree",
        ));
    }
    let slots = input.bytes.len().div_ceil(2);
    let capacity = (slots as u64)
        .checked_mul(512)
        .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "graph capacity overflow"))?;
    let _capacity = memory.reserve(capacity, control.position())?;
    let mut states = reserve_vec::<u8>(slots)?;
    states.resize(slots, 0); // 0 unseen, 1 queued, 2 start, 3 interior, 4 gap
    let mut nodes = reserve_vec::<Node>(slots)?;
    let mut work = reserve_vec::<u64>(slots)?;
    // Each instruction adds at most two edges, and a known jump one per
    // target; the known jumps' edges are admitted beyond the graph's own
    // capacity, so pushing them never grows the vector unadmitted.
    let known_edges = input
        .jumps
        .iter()
        .try_fold(0usize, |sum, jump| sum.checked_add(jump.targets.len()))
        .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "edge capacity overflow"))?;
    let _known_capacity = memory.reserve(
        (known_edges as u64)
            .checked_mul(std::mem::size_of::<Edge>() as u64)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "edge capacity overflow"))?,
        control.position(),
    )?;
    let mut edges = reserve_vec::<Edge>(
        slots
            .checked_mul(2)
            .and_then(|own| own.checked_add(known_edges))
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "edge capacity overflow"))?,
    )?;
    let mut summary = AnalysisSummary::default();
    let mut conflicts = reserve_vec::<bool>(slots)?;
    conflicts.resize(slots, false);
    let mut leaders = reserve_vec::<bool>(slots)?;
    leaders.resize(slots, false);
    states[0] = 1;
    leaders[0] = true;
    work.push(input.extent.start);
    let in_scope = |address: u64| address >= input.extent.start && address < end;
    for site in input.relocations.first_at(input.extent.start)..input.relocations.first_at(end) {
        let raw = &input.relocations[site];
        control.checkpoint(1)?;
        if !in_scope(raw.offset) {
            continue;
        }
        let r = input.relocations.normalized(site);
        summary.coverage.references &= r.known;
        emit(
            sink,
            FunctionRecord::Reference {
                raw: Box::new(raw.clone()),
                reference_kind: r.kind,
                target: r.target.clone(),
                addend: r.addend,
                paired: r.paired,
                known: r.known,
            },
            control,
            &mut summary,
        )?;
    }
    while let Some(offset) = work.pop() {
        control.checkpoint(1)?;
        let index = ((offset - input.extent.start) / 2) as usize;
        if states[index] == 2 || states[index] == 4 {
            continue;
        }
        if states[index] == 3 {
            conflicts[index] = true;
            summary.coverage.control_flow = false;
            emit(
                sink,
                FunctionRecord::Gap {
                    start: offset,
                    length: 2.min(end - offset),
                    reason: GapReason::ConflictingBoundary,
                },
                control,
                &mut summary,
            )?;
            continue;
        }
        let mut declared_data = false;
        for range in input.data_ranges {
            control.checkpoint(1)?;
            if offset >= range.start && offset - range.start < range.length {
                declared_data = true;
                break;
            }
        }
        let decoded = if declared_data {
            None
        } else {
            decoder.decode(&input.bytes[(offset - input.extent.start) as usize..])
        };
        let Some(decoded) = decoded else {
            states[index] = 4;
            summary.coverage.decoding = false;
            summary.coverage.control_flow = false;
            emit(
                sink,
                FunctionRecord::Gap {
                    start: offset,
                    length: 2.min(end - offset),
                    reason: if declared_data {
                        GapReason::DeclaredData
                    } else {
                        GapReason::UnsupportedInstruction
                    },
                },
                control,
                &mut summary,
            )?;
            continue;
        };
        let width = u64::from(decoded.length);
        if !matches!(width, 2 | 4) || offset.checked_add(width).is_none_or(|e| e > end) {
            return Err(Error::new(
                ErrorCode::Integrity,
                "decoder returned an invalid instruction width",
            ));
        }
        if width == 4 && matches!(states[index + 1], 2..=4) {
            conflicts[index] = true;
            conflicts[index + 1] = true;
            states[index] = 4;
            summary.coverage.control_flow = false;
            summary.coverage.decoding = false;
            emit(
                sink,
                FunctionRecord::Gap {
                    start: offset,
                    length: width,
                    reason: GapReason::ConflictingBoundary,
                },
                control,
                &mut summary,
            )?;
            continue;
        }
        states[index] = 2;
        if width == 4 {
            if states[index + 1] == 1 {
                conflicts[index + 1] = true;
                summary.coverage.control_flow = false;
                emit(
                    sink,
                    FunctionRecord::Gap {
                        start: offset + 2,
                        length: 2,
                        reason: GapReason::ConflictingBoundary,
                    },
                    control,
                    &mut summary,
                )?;
            }
            states[index + 1] = 3;
        }
        let next = offset + width;
        let mut relocated = None;
        let mut relevant = 0;
        let mut unknown = false;
        for site in input.relocations.first_at(offset.saturating_sub(4))
            ..input.relocations.first_at(offset.saturating_add(1))
        {
            let r = &input.relocations[site];
            control.checkpoint(1)?;
            if input.image.is_some() {
                break;
            }
            let exact = r.offset == offset;
            let call_pair = r.offset.checked_add(4) == Some(offset)
                && r.offset >= input.extent.start
                && matches!(decoded.flow, InstructionFlow::Indirect { .. });
            if !exact && !call_pair {
                continue;
            }
            let normalized = input.relocations.normalized(site);
            if call_pair && normalized.kind == ReferenceKind::Call {
                let position = (r.offset - input.extent.start) as usize;
                let first =
                    u32::from_le_bytes(input.bytes[position..position + 4].try_into().unwrap());
                if first & 0x7f != 0x17
                    || !matches!(decoded.flow,InstructionFlow::Indirect{base,..} if u32::from(base)==(first>>7)&31)
                {
                    unknown = true;
                    continue;
                }
            } else if !exact || normalized.kind != ReferenceKind::Branch {
                if exact && normalized.kind == ReferenceKind::Unknown {
                    unknown = true;
                }
                continue;
            }
            relevant += 1;
            relocated = Some(
                if normalized.known && normalized.target.section == Some(input.section) {
                    normalized
                        .addend
                        .and_then(|add| target(normalized.target.offset, add))
                } else {
                    None
                },
            );
        }
        if relevant > 1 {
            unknown = true;
        }
        let mut outgoing: Vec<(Option<u64>, EdgeKind)> = Vec::with_capacity(2);
        let destination = |displacement: i32| {
            if unknown {
                None
            } else {
                relocated.unwrap_or_else(|| {
                    if input.image.is_some() {
                        Some(u64::from((offset as u32).wrapping_add(displacement as u32)))
                    } else {
                        target(offset, i64::from(displacement))
                    }
                })
            }
        };
        match decoded.flow {
            InstructionFlow::Next => outgoing.push((Some(next), EdgeKind::Fallthrough)),
            InstructionFlow::Branch { displacement } => {
                outgoing.push((destination(displacement), EdgeKind::Taken));
                outgoing.push((Some(next), EdgeKind::Fallthrough));
            }
            InstructionFlow::Jump { displacement, link } => {
                outgoing.push((
                    destination(displacement),
                    if link { EdgeKind::Call } else { EdgeKind::Jump },
                ));
                if link {
                    outgoing.push((Some(next), EdgeKind::PossibleContinuation));
                }
            }
            InstructionFlow::Indirect {
                base,
                offset: imm,
                link,
            } => {
                let known = (!link)
                    .then(|| input.jumps.iter().find(|jump| jump.site == offset))
                    .flatten();
                if let Some(known) = known {
                    for &target in &known.targets {
                        outgoing.push((Some(target), EdgeKind::Jump));
                    }
                } else if relocated.is_none()
                    && !unknown
                    && !link
                    && matches!(base, 1 | 5)
                    && imm == 0
                {
                    outgoing.push((None, EdgeKind::Return));
                } else {
                    if relocated.is_none() || unknown {
                        summary.coverage.control_flow = false;
                    }
                    outgoing.push((
                        if unknown { None } else { relocated.flatten() },
                        if link {
                            EdgeKind::Call
                        } else if relocated.is_some() {
                            EdgeKind::Jump
                        } else {
                            EdgeKind::Indirect
                        },
                    ));
                    if link {
                        outgoing.push((Some(next), EdgeKind::PossibleContinuation));
                    }
                }
            }
            InstructionFlow::Stop => {
                summary.coverage.control_flow = false;
                outgoing.push((None, EdgeKind::Stop));
            }
        }
        if unknown {
            summary.coverage.control_flow = false;
        }
        if matches!(
            decoded.flow,
            InstructionFlow::Branch { .. } | InstructionFlow::Jump { .. }
        ) && relocated.is_none()
            && outgoing.first().is_some_and(|(to, _)| to.is_none())
        {
            summary.coverage.control_flow = false;
        }
        let transfer = !matches!(decoded.flow, InstructionFlow::Next);
        for (destination, mut relation) in outgoing {
            let external = destination.is_none_or(|v| !in_scope(v));
            if let Some(to) = destination.filter(|v| in_scope(*v)) {
                if !to.is_multiple_of(2) {
                    relation = EdgeKind::Conflict;
                    summary.coverage.control_flow = false;
                } else {
                    let i = ((to - input.extent.start) / 2) as usize;
                    if relation != EdgeKind::Call {
                        if transfer {
                            leaders[i] = true;
                        }
                        if states[i] == 0 {
                            states[i] = 1;
                            work.push(to);
                        } else if states[i] == 3 {
                            conflicts[i] = true;
                            relation = EdgeKind::Conflict;
                            summary.coverage.control_flow = false;
                        }
                    }
                }
            }
            edges.push(Edge {
                from: offset,
                target: destination,
                relation,
                external,
            });
        }
        if matches!(decoded.flow, InstructionFlow::Next) && next == end {
            summary.coverage.control_flow = false;
        }
        nodes.push(Node {
            offset,
            decoded,
            conflict: false,
        });
    }
    nodes.sort_unstable_by_key(|n| n.offset);
    for node in &mut nodes {
        let index = ((node.offset - input.extent.start) / 2) as usize;
        node.conflict = conflicts[index] || (node.decoded.length == 4 && conflicts[index + 1]);
    }
    edges.sort_unstable_by_key(|e| (e.from, e.target, e.relation as u8));
    summary.semantics = values::analyze_with(
        &input, &nodes, &edges, decoder, memory, control, sink, symbolic, abi,
    )?;
    summary.semantics.complete &= summary.coverage.decoding && summary.coverage.control_flow;
    summary.instructions = nodes.len() as u64;
    let mut block = None;
    for (index, node) in nodes.iter().enumerate() {
        control.checkpoint(1)?;
        let next = node.offset + u64::from(node.decoded.length);
        if block.is_none() {
            block = Some(node.offset);
        }
        let closes = !matches!(node.decoded.flow, InstructionFlow::Next)
            || nodes.get(index + 1).is_none_or(|n| {
                n.offset != next || leaders[((n.offset - input.extent.start) / 2) as usize]
            });
        if closes {
            let start = block.take().unwrap();
            emit(
                sink,
                FunctionRecord::Block {
                    id: start,
                    start,
                    end: next,
                },
                control,
                &mut summary,
            )?;
        }
    }
    edges.sort_unstable_by_key(|e| (e.from, e.target, e.relation as u8));
    for edge in edges {
        emit(
            sink,
            FunctionRecord::Edge {
                from: edge.from,
                target: edge.target,
                relation: edge.relation,
                external: edge.external,
            },
            control,
            &mut summary,
        )?;
    }
    let mut cursor = input.extent.start;
    for node in &nodes {
        if cursor < node.offset {
            emit(
                sink,
                FunctionRecord::Gap {
                    start: cursor,
                    length: node.offset - cursor,
                    reason: GapReason::Unvisited,
                },
                control,
                &mut summary,
            )?;
        }
        cursor = node.offset + u64::from(node.decoded.length);
    }
    if cursor < end {
        emit(
            sink,
            FunctionRecord::Gap {
                start: cursor,
                length: end - cursor,
                reason: GapReason::Unvisited,
            },
            control,
            &mut summary,
        )?;
    }
    Ok(summary)
}

fn emit(
    sink: &mut dyn FunctionSink,
    record: FunctionRecord,
    control: &mut dyn RunControl,
    summary: &mut AnalysisSummary,
) -> Result<()> {
    control.checkpoint(1)?;
    match &record {
        FunctionRecord::Instruction { .. } => summary.instructions += 1,
        FunctionRecord::Block { .. } => summary.blocks += 1,
        FunctionRecord::Edge { .. } => summary.edges += 1,
        FunctionRecord::Reference { .. } => summary.references += 1,
        FunctionRecord::Gap { .. } => summary.gaps += 1,
        _ => {}
    }
    sink.record(&record, control)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A decoder the analysis must never reach.
    struct Unreached;
    impl FunctionDecoder for Unreached {
        fn identity(&self) -> &'static str {
            "unreached"
        }
        fn decode(&self, _: &[u8]) -> Option<DecodedOp> {
            unreachable!("the graph's capacity is admitted before decoding")
        }
        fn reference(
            &self,
            _: &FunctionRelocation,
            _: &[FunctionRelocation],
            _: u32,
            _: &mut dyn RunControl,
        ) -> Result<NormalizedReference> {
            unreachable!()
        }
    }
    impl FunctionSemantics for Unreached {
        fn semantic_identity(&self) -> &'static str {
            "unreached"
        }
        fn lift(&self, _: &[u8]) -> SemanticOp {
            unreachable!()
        }
        fn value_relocation(&self, _: &FunctionRelocation, _: SemanticOp) -> ValueRelocation {
            unreachable!()
        }
    }
    struct Discard;
    impl FunctionSink for Discard {
        fn record(&mut self, _: &FunctionRecord, _: &mut dyn RunControl) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn known_jump_edges_are_admitted_before_the_graph_is_built() {
        let bytes = [0; 4];
        // Far more targets than two edges per instruction leave room for.
        let jumps = [KnownJump {
            site: 0,
            targets: (0..4096).map(|i| i * 2).collect(),
        }];
        let input = FunctionInput {
            image: None,
            section: 1,
            extent: CodeRange {
                start: 0,
                length: 4,
            },
            bytes: &bytes,
            relocations: &PreparedReferences::empty(),
            data_ranges: &[],
            jumps: &jumps,
        };
        // The graph's own capacity fits; the known jumps' edges do not.
        let memory = WorkingMemory::new(8 * 1024).unwrap();
        let error = research(
            input,
            &Unreached,
            &memory,
            &mut || Ok(()),
            &mut Discard,
            None,
        )
        .err()
        .unwrap();
        assert_eq!(error.code, ErrorCode::ResourceLimited, "{error:?}");
    }
}
