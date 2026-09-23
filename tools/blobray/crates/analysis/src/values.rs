//! Finite-height flat lattice: unreachable -> one value -> unknown.
//! Queue membership is bounded by the graph; no expression trees or memory state.
use super::*;
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Value {
    Unknown,
    Expr(u32),
    Constant(u32),
    Image(u32),
    Section(u32, i64),
    Symbol(usize, i64),
    Stack(i64),
    // An incomplete relocation upper is never exported as a resolved address.
    Upper {
        symbol: usize,
        addend: i64,
        site: usize,
        pc: bool,
    },
}
type State = [Value; 32];
fn unknown_state() -> State {
    let mut s = [Value::Unknown; 32];
    s[0] = Value::Constant(0);
    s
}
#[derive(Clone, Copy)]
struct Relocation {
    role: ValueRelocation,
    site: usize,
    symbol: usize,
    addend: i64,
    pair: Option<usize>,
}
#[derive(Clone, Copy)]
struct Operation {
    op: SemanticOp,
    opaque_call: bool,
    relocation: Option<Relocation>,
    gap: Option<SemanticGapReason>,
}

fn prepare(
    input: &FunctionInput<'_>,
    node: &Node,
    isa: &dyn FunctionSemantics,
    control: &mut dyn RunControl,
) -> Result<Operation> {
    let start = (node.offset - input.extent.start) as usize;
    let op = isa.lift(&input.bytes[start..start + node.decoded.length as usize]);
    validate(op)?;
    let mut result = Operation {
        op,
        opaque_call: matches!(
            node.decoded.flow,
            InstructionFlow::Jump { link: true, .. } | InstructionFlow::Indirect { link: true, .. }
        ),
        relocation: None,
        gap: None,
    };
    if node.conflict {
        result.gap = Some(SemanticGapReason::ConflictingBoundary);
        return Ok(result);
    }
    if op == SemanticOp::Unsupported {
        result.gap = Some(SemanticGapReason::UnsupportedInstruction);
    }
    // ET_EXEC bytes already contain link edits. Retained static relocations
    // remain provenance; applying object-file relocation semantics again is wrong.
    if input.image.is_some() {
        return Ok(result);
    }
    for site in input.relocations.first_at(node.offset.saturating_sub(4))
        ..input.relocations.first_at(node.offset.saturating_add(1))
    {
        let raw = &input.relocations[site];
        control.measure(WorkMetric::RelocationLookups, 1);
        control.checkpoint(1)?;
        if raw.offset.checked_add(4) == Some(node.offset)
            && raw.offset >= input.extent.start
            && isa.value_relocation(raw, SemanticOp::Link { dest: 0 }) == ValueRelocation::CallUpper
            && let InstructionFlow::Indirect { base, .. } = node.decoded.flow
        {
            let start = (raw.offset - input.extent.start) as usize;
            if matches!(isa.lift(&input.bytes[start..start + 4]), SemanticOp::Upper { dest, pc_relative: true, .. } if dest == base && dest != 0)
            {
                result.opaque_call = true;
            }
        }
        if raw.offset != node.offset {
            continue;
        }
        let role = isa.value_relocation(raw, op);
        if role == ValueRelocation::Ignore {
            continue;
        }
        if role == ValueRelocation::Unsupported || result.relocation.is_some() {
            result.gap = Some(SemanticGapReason::UnresolvedRelocation);
            continue;
        }
        let normalized = input.relocations.normalized(site);
        let compatible = matches!(
            (role, op),
            (
                ValueRelocation::UpperAbsolute,
                SemanticOp::Upper {
                    pc_relative: false,
                    ..
                }
            ) | (
                ValueRelocation::UpperPcRelative | ValueRelocation::CallUpper,
                SemanticOp::Upper {
                    pc_relative: true,
                    ..
                }
            ) | (
                ValueRelocation::LowerAbsolute | ValueRelocation::LowerPcRelative,
                SemanticOp::Integer {
                    op: IntegerOp::Add,
                    right: Operand::Immediate(_),
                    ..
                } | SemanticOp::Memory { .. }
            )
        );
        if !compatible || !normalized.known {
            result.gap = Some(SemanticGapReason::UnresolvedRelocation);
            continue;
        }
        let symbol = input.relocations.symbol(site);
        let pair = input.relocations.pair(site);
        let (Some(symbol), Some(addend)) = (symbol, normalized.addend) else {
            result.gap = Some(SemanticGapReason::UnresolvedRelocation);
            continue;
        };
        result.relocation = Some(Relocation {
            role,
            site,
            symbol,
            addend,
            pair,
        });
    }
    Ok(result)
}
fn position(control: &mut dyn RunControl, section: u32, offset: u64) {
    let mut p = control.position();
    p.table = Some(u64::from(section));
    p.entry = Some(offset);
    control.set_position(p);
}
fn validate(op: SemanticOp) -> Result<()> {
    let operand = |v| matches!(v, Operand::Immediate(_) | Operand::Register(0..=31));
    let valid = match op {
        SemanticOp::Integer {
            dest, left, right, ..
        } => dest < 32 && operand(left) && operand(right),
        SemanticOp::Upper { dest, .. } | SemanticOp::Link { dest } => dest < 32,
        SemanticOp::Memory {
            kind,
            base,
            dest,
            source,
            width,
            ..
        } => {
            base < 32
                && dest.is_none_or(|r| r < 32)
                && source.is_none_or(|r| r < 32)
                && match kind {
                    MemoryKind::Load => {
                        matches!(width, 1 | 2 | 4) && dest.is_some() && source.is_none()
                    }
                    MemoryKind::Store => {
                        matches!(width, 1 | 2 | 4) && dest.is_none() && source.is_some()
                    }
                    MemoryKind::LoadReserved => width == 4 && dest.is_some() && source.is_none(),
                    MemoryKind::StoreConditional | MemoryKind::Atomic => {
                        width == 4 && dest.is_some() && source.is_some()
                    }
                }
        }
        SemanticOp::None | SemanticOp::Unsupported => true,
    };
    if valid {
        Ok(())
    } else {
        Err(Error::new(
            ErrorCode::Integrity,
            "semantic producer returned invalid operands",
        ))
    }
}
fn operand(s: &State, op: Operand) -> Value {
    match op {
        Operand::Register(r) => s.get(r as usize).copied().unwrap_or(Value::Unknown),
        Operand::Immediate(v) => Value::Constant(v),
    }
}
fn offset(v: Value, delta: i64) -> Value {
    if delta == 0 {
        return v;
    }
    match v {
        Value::Constant(x) => Value::Constant(x.wrapping_add(delta as u32)),
        Value::Image(x) => Value::Image(x.wrapping_add(delta as u32)),
        Value::Section(s, n) => n
            .checked_add(delta)
            .map_or(Value::Unknown, |n| Value::Section(s, n)),
        Value::Symbol(s, n) => n
            .checked_add(delta)
            .map_or(Value::Unknown, |n| Value::Symbol(s, n)),
        Value::Stack(n) => n.checked_add(delta).map_or(Value::Unknown, Value::Stack),
        _ => Value::Unknown,
    }
}
fn integer(op: IntegerOp, a: Value, b: Value) -> Value {
    use IntegerOp::*;
    if let (Value::Constant(a), Value::Constant(b)) = (a, b) {
        let (sa, sb) = (a as i32, b as i32);
        return Value::Constant(match op {
            Add => a.wrapping_add(b),
            Sub => a.wrapping_sub(b),
            And => a & b,
            Or => a | b,
            Xor => a ^ b,
            Shl => a.wrapping_shl(b & 31),
            Shr => a >> (b & 31),
            Sar => (sa >> (b & 31)) as u32,
            Lt => u32::from(sa < sb),
            Ltu => u32::from(a < b),
            Mul => a.wrapping_mul(b),
            Mulh => ((i64::from(sa) * i64::from(sb)) >> 32) as u32,
            Mulhsu => ((i64::from(sa) * i64::from(b)) >> 32) as u32,
            Mulhu => ((u64::from(a) * u64::from(b)) >> 32) as u32,
            Div => {
                if b == 0 {
                    u32::MAX
                } else {
                    sa.wrapping_div(sb) as u32
                }
            }
            Divu => a.checked_div(b).unwrap_or(u32::MAX),
            Rem => {
                if b == 0 {
                    a
                } else {
                    sa.wrapping_rem(sb) as u32
                }
            }
            Remu => {
                if b == 0 {
                    a
                } else {
                    a % b
                }
            }
        });
    }
    match (op, a, b) {
        (Add, v, Value::Constant(n)) | (Add, Value::Constant(n), v) => {
            offset(v, i64::from(n as i32))
        }
        (Sub, v, Value::Constant(n)) => offset(v, -i64::from(n as i32)),
        _ => Value::Unknown,
    }
}
pub(super) fn fold_integer(op: IntegerOp, a: u32, b: u32) -> u32 {
    match integer(op, Value::Constant(a), Value::Constant(b)) {
        Value::Constant(value) => value,
        _ => unreachable!("two RV32 constants always produce a constant"),
    }
}
fn lower(base: Value, r: Relocation) -> Value {
    match base {
        Value::Upper {
            symbol,
            addend,
            site,
            pc,
        } if symbol == r.symbol
            && addend == r.addend
            && ((r.role == ValueRelocation::LowerAbsolute && !pc)
                || (r.role == ValueRelocation::LowerPcRelative && pc && r.pair == Some(site))) =>
        {
            Value::Symbol(symbol, addend)
        }
        _ => Value::Unknown,
    }
}
#[derive(Default)]
struct Effects {
    register: Option<(u8, Value)>,
    memory: Option<(MemoryKind, u8, Value, Option<Value>)>,
    gap: Option<SemanticGapReason>,
}
fn transfer(
    operation: Operation,
    node: &Node,
    input: &FunctionInput<'_>,
    incoming: State,
    control: &mut dyn RunControl,
) -> Result<(State, Effects)> {
    let mut state = incoming;
    let mut effects = Effects::default();
    if let Some(gap) = operation.gap {
        effects.gap = Some(gap);
        if gap == SemanticGapReason::UnresolvedRelocation {
            match operation.op {
                SemanticOp::Memory {
                    kind,
                    width,
                    dest,
                    source,
                    swap,
                    ..
                } => {
                    effects.memory = Some((
                        kind,
                        width,
                        Value::Unknown,
                        source.map(|r| {
                            if kind == MemoryKind::Atomic && !swap {
                                Value::Unknown
                            } else {
                                match operand(&incoming, Operand::Register(r)) {
                                    Value::Constant(n) => Value::Constant(match width {
                                        1 => n & 255,
                                        2 => n & 65535,
                                        _ => n,
                                    }),
                                    v if width == 4 => v,
                                    _ => Value::Unknown,
                                }
                            }
                        }),
                    ));
                    effects.register = dest.map(|r| {
                        (
                            r,
                            if r == 0 {
                                Value::Constant(0)
                            } else {
                                Value::Unknown
                            },
                        )
                    });
                }
                SemanticOp::Integer { dest, .. }
                | SemanticOp::Upper { dest, .. }
                | SemanticOp::Link { dest } => {
                    effects.register = Some((
                        dest,
                        if dest == 0 {
                            Value::Constant(0)
                        } else {
                            Value::Unknown
                        },
                    ));
                }
                _ => {}
            }
        }
        return Ok((unknown_state(), effects));
    }
    let rel = operation.relocation;
    match operation.op {
        SemanticOp::Integer {
            op,
            dest,
            left,
            right,
        } => {
            let value = if let Some(r) = rel {
                lower(operand(&state, left), r)
            } else {
                integer(op, operand(&state, left), operand(&state, right))
            };
            effects.register = Some((dest, value));
            if rel.is_some() && value == Value::Unknown {
                effects.gap = Some(SemanticGapReason::UnresolvedRelocation);
            }
        }
        SemanticOp::Upper {
            dest,
            value,
            pc_relative,
        } => {
            let v = if let Some(r) = rel {
                Value::Upper {
                    symbol: r.symbol,
                    addend: r.addend,
                    site: r.site,
                    pc: pc_relative,
                }
            } else if pc_relative {
                offset(
                    if input.image.is_some() {
                        Value::Image(node.offset as u32)
                    } else {
                        Value::Section(input.section, node.offset as i64)
                    },
                    i64::from(value as i32),
                )
            } else {
                Value::Constant(value)
            };
            effects.register = Some((dest, v));
        }
        SemanticOp::Link { dest } => {
            effects.register = Some((
                dest,
                if input.image.is_some() {
                    Value::Image((node.offset + u64::from(node.decoded.length)) as u32)
                } else {
                    Value::Section(
                        input.section,
                        (node.offset + u64::from(node.decoded.length)) as i64,
                    )
                },
            ))
        }
        SemanticOp::Memory {
            kind,
            base,
            displacement,
            width,
            dest,
            source,
            swap,
            signed,
        } => {
            let address = if let Some(r) = rel {
                lower(operand(&state, Operand::Register(base)), r)
            } else {
                offset(
                    operand(&state, Operand::Register(base)),
                    i64::from(displacement),
                )
            };
            if rel.is_some() && address == Value::Unknown {
                effects.gap = Some(SemanticGapReason::UnresolvedRelocation);
            }
            let value = source.map(|r| {
                let v = operand(&state, Operand::Register(r));
                if kind == MemoryKind::Atomic && !swap {
                    Value::Unknown
                } else if let Value::Constant(n) = v {
                    Value::Constant(match width {
                        1 => n & 255,
                        2 => n & 65535,
                        _ => n,
                    })
                } else if width == 4 {
                    v
                } else {
                    Value::Unknown
                }
            });
            effects.memory = Some((kind, width, address, value));
            let mut loaded = Value::Unknown;
            if kind == MemoryKind::Load
                && let Some(image) = input.image
                && let Value::Constant(address) | Value::Image(address) = address
            {
                let mut bytes = [0; 4];
                if image.read_constant(address, &mut bytes[..width as usize], control)? {
                    let value = u32::from_le_bytes(bytes);
                    loaded = Value::Constant(match (width, signed) {
                        (1, true) => value as i8 as i32 as u32,
                        (2, true) => value as i16 as i32 as u32,
                        _ => value,
                    });
                }
            }
            effects.register = dest.map(|r| (r, loaded));
        }
        SemanticOp::None => {}
        SemanticOp::Unsupported => unreachable!("unsupported operation carries a gap"),
    }
    if let Some((r, v)) = effects.register {
        if r != 0 {
            if let Some(slot) = state.get_mut(r as usize) {
                *slot = v;
            }
        } else {
            effects.register = Some((0, Value::Constant(0)));
        }
    }
    if operation.opaque_call {
        state = unknown_state();
        effects.gap = Some(SemanticGapReason::OpaqueCall);
    }
    Ok((state, effects))
}
fn public(v: Value, input: &FunctionInput<'_>) -> AbstractValue {
    match v {
        Value::Unknown | Value::Upper { .. } => AbstractValue::Unknown,
        Value::Expr(id) => AbstractValue::Expression { id },
        Value::Constant(value) => AbstractValue::Constant { value },
        Value::Image(address) => AbstractValue::ImageAddress { address },
        Value::Section(section, offset) => AbstractValue::Section { section, offset },
        Value::Symbol(i, addend) => AbstractValue::Symbol {
            symbol: input.relocations[i].target.symbol.clone(),
            addend,
        },
        Value::Stack(offset) => AbstractValue::EntryStack { offset },
    }
}
fn known(v: &AbstractValue) -> bool {
    !matches!(v, AbstractValue::Unknown | AbstractValue::Expression { .. })
}

#[cfg(test)]
pub(super) fn analyze(
    input: &FunctionInput<'_>,
    nodes: &[Node],
    edges: &[Edge],
    isa: &dyn FunctionSemantics,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    sink: &mut dyn FunctionSink,
) -> Result<SemanticSummary> {
    analyze_with(input, nodes, edges, isa, memory, control, sink, false, None)
}
#[allow(clippy::too_many_arguments)]
pub(super) fn analyze_with(
    input: &FunctionInput<'_>,
    nodes: &[Node],
    edges: &[Edge],
    isa: &dyn FunctionSemantics,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    sink: &mut dyn FunctionSink,
    symbolic: bool,
    abi: Option<CallAbi>,
) -> Result<SemanticSummary> {
    control.phase(RunPhase::AnalyzeValues)?;
    let bytes_per_node = std::mem::size_of::<Option<State>>()
        + std::mem::size_of::<Operation>()
        + std::mem::size_of::<usize>()
        + 1;
    let bytes = nodes
        .len()
        .checked_mul(bytes_per_node)
        .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "value state capacity overflow"))?;
    let _reservation = memory.reserve(bytes as u64, control.position())?;
    let mut symbols = Symbols::new(if symbolic { nodes.len() } else { 0 }, memory, control)?;
    let mut states = reserve_vec::<Option<State>>(nodes.len())?;
    states.resize(nodes.len(), None);
    let mut queued = reserve_vec::<bool>(nodes.len())?;
    queued.resize(nodes.len(), false);
    let mut operations = reserve_vec::<Operation>(nodes.len())?;
    for node in nodes {
        position(control, input.section, node.offset);
        control.checkpoint(1)?;
        operations.push(prepare(input, node, isa, control)?);
    }
    let mut queue = VecDeque::new();
    queue
        .try_reserve_exact(nodes.len())
        .map_err(|_| Error::new(ErrorCode::ResourceLimited, "value queue allocation refused"))?;
    if let Ok(entry) = nodes.binary_search_by_key(&input.extent.start, |n| n.offset) {
        let mut state = unknown_state();
        if symbolic {
            for (r, slot) in state.iter_mut().enumerate().skip(1) {
                *slot = symbols.expression(
                    u64::MAX,
                    Expression::EntryRegister { register: r as u8 },
                    control,
                )?;
            }
        }
        state[2] = Value::Stack(0);
        states[entry] = Some(state);
        queued[entry] = true;
        queue.push_back(entry);
    }
    while let Some(index) = queue.pop_front() {
        queued[index] = false;
        let node = &nodes[index];
        position(control, input.section, node.offset);
        control.checkpoint(1)?;
        let (out, _) = evaluate(
            operations[index],
            node,
            input,
            states[index].unwrap(),
            control,
            &mut symbols,
            symbolic,
            abi,
        )?;
        let first = edges.partition_point(|e| e.from < node.offset);
        for edge in edges[first..].iter().take_while(|e| e.from == node.offset) {
            control.checkpoint(1)?;
            if !matches!(
                edge.relation,
                EdgeKind::Fallthrough
                    | EdgeKind::Taken
                    | EdgeKind::Jump
                    | EdgeKind::PossibleContinuation
            ) {
                continue;
            }
            let Some(target) = edge.target else { continue };
            let Ok(to) = nodes.binary_search_by_key(&target, |n| n.offset) else {
                continue;
            };
            let changed = match &mut states[to] {
                None => {
                    states[to] = Some(out);
                    true
                }
                Some(old) => {
                    let mut changed = false;
                    for (a, b) in old.iter_mut().zip(out) {
                        if *a != b && *a != Value::Unknown {
                            *a = Value::Unknown;
                            changed = true;
                        }
                    }
                    changed
                }
            };
            if changed && !queued[to] {
                queued[to] = true;
                queue.push_back(to);
            }
        }
    }
    // Emit after convergence. Re-evaluation below only reuses stable DAG nodes.
    let mut emitted = 0;
    let mut summary = SemanticSummary {
        complete: true,
        ..Default::default()
    };
    for (i, node) in nodes.iter().enumerate() {
        position(control, input.section, node.offset);
        control.checkpoint(1)?;
        let start = (node.offset - input.extent.start) as usize;
        sink.record(
            &FunctionRecord::Instruction {
                offset: node.offset,
                bytes: input.bytes[start..start + node.decoded.length as usize].into(),
                decoded: node.decoded.clone(),
            },
            control,
        )?;
        let Some(state) = states[i] else { continue };
        let (_, mut effects) = evaluate(
            operations[i],
            node,
            input,
            state,
            control,
            &mut symbols,
            symbolic,
            abi,
        )?;
        if symbolic {
            if let Some((test, left, right)) =
                isa.branch(&input.bytes[start..start + node.decoded.length as usize])
            {
                sink.record(
                    &FunctionRecord::Condition {
                        offset: node.offset,
                        test,
                        left: public(operand(&state, left), input),
                        right: public(operand(&state, right), input),
                    },
                    control,
                )?;
            }
            if operations[i].opaque_call {
                sink.record(
                    &FunctionRecord::CallInputs {
                        offset: node.offset,
                        registers: state.iter().map(|v| public(*v, input)).collect(),
                    },
                    control,
                )?;
            }
            if matches!(
                node.decoded.flow,
                InstructionFlow::Indirect {
                    base: 1 | 5,
                    offset: 0,
                    link: false
                }
            ) {
                sink.record(
                    &FunctionRecord::ReturnValue {
                        offset: node.offset,
                        low: public(state[10], input),
                        high: public(state[11], input),
                    },
                    control,
                )?;
            }
        }
        while emitted < symbols.records.len() {
            let (offset, expression) = &symbols.records[emitted];
            sink.record(
                &FunctionRecord::Expression {
                    id: emitted as u32,
                    offset: *offset,
                    origin: None,
                    expression: expression.clone(),
                },
                control,
            )?;
            emitted += 1;
        }
        if input.image.is_some() {
            let transfer = match node.decoded.flow {
                InstructionFlow::Jump { displacement, link } => {
                    let address = (node.offset as u32).wrapping_add(displacement as u32);
                    (link
                        || u64::from(address) < input.extent.start
                        || u64::from(address) >= input.extent.start + input.extent.length)
                        .then_some((Value::Image(address), link))
                }
                InstructionFlow::Indirect {
                    base,
                    offset: displacement,
                    link,
                } => {
                    let target = match offset(state[base as usize], i64::from(displacement)) {
                        Value::Constant(n) | Value::Image(n) => Value::Image(n & !1),
                        _ => Value::Unknown,
                    };
                    let return_pattern =
                        matches!(base, 1 | 5) && displacement == 0 && target == Value::Unknown;
                    let local = matches!(target, Value::Image(a) if u64::from(a) >= input.extent.start && u64::from(a) < input.extent.start + input.extent.length);
                    if !link && local {
                        effects
                            .gap
                            .get_or_insert(SemanticGapReason::UnexpandedControlFlow);
                    }
                    if !link && (return_pattern || local) {
                        None
                    } else {
                        Some((target, link))
                    }
                }
                _ => None,
            };
            if let Some((target, call)) = transfer {
                effects.gap.get_or_insert(SemanticGapReason::OpaqueCall);
                sink.record(
                    &FunctionRecord::Transfer {
                        offset: node.offset,
                        target: if node.conflict {
                            AbstractValue::Unknown
                        } else {
                            public(target, input)
                        },
                        call,
                    },
                    control,
                )?;
            }
        }
        let relocation = operations[i].relocation.map(|r| {
            let raw = &input.relocations[r.site];
            RelocationSite {
                section: raw.section,
                index: raw.index,
            }
        });
        if let Some((register, v)) = effects.register {
            let value = public(v, input);
            summary.values += 1;
            summary.known_values += u64::from(known(&value));
            sink.record(
                &FunctionRecord::Value {
                    offset: node.offset,
                    register,
                    value,
                    relocation,
                },
                control,
            )?;
        }
        if let Some((access, width, addr, v)) = effects.memory {
            let address = public(addr, input);
            summary.accesses += 1;
            summary.known_addresses += u64::from(known(&address));
            sink.record(
                &FunctionRecord::MemoryAccess {
                    offset: node.offset,
                    access,
                    width,
                    address,
                    value: v.map(|v| public(v, input)),
                    relocation,
                },
                control,
            )?;
        }
        if let Some(reason) = effects.gap {
            summary.complete = false;
            summary.gaps += 1;
            sink.record(
                &FunctionRecord::SemanticGap {
                    offset: node.offset,
                    reason,
                },
                control,
            )?;
        }
    }
    Ok(summary)
}

struct Symbols<'a> {
    records: Vec<(u64, Expression)>,
    memory: &'a WorkingMemory,
    reservations: Vec<MemoryReservation<'a>>,
    _index: MemoryReservation<'a>,
    limit: usize,
}
impl<'a> Symbols<'a> {
    fn new(nodes: usize, memory: &'a WorkingMemory, c: &mut dyn RunControl) -> Result<Self> {
        let limit = if nodes == 0 {
            0
        } else {
            nodes
                .checked_mul(8)
                .and_then(|n| n.checked_add(32))
                .ok_or_else(|| {
                    Error::new(ErrorCode::ResourceLimited, "expression capacity overflow")
                })?
        };
        let chunks = limit.div_ceil(64);
        let index = memory.reserve(
            (chunks as u64) * std::mem::size_of::<MemoryReservation<'_>>() as u64,
            c.position(),
        )?;
        Ok(Self {
            records: Vec::new(),
            memory,
            reservations: reserve_vec(chunks)?,
            _index: index,
            limit,
        })
    }
    fn expression(
        &mut self,
        offset: u64,
        expr: Expression,
        c: &mut dyn RunControl,
    ) -> Result<Value> {
        for (id, (site, old)) in self.records.iter().enumerate().rev() {
            c.checkpoint(1)?;
            if *site == offset && old == &expr {
                return Ok(Value::Expr(id as u32));
            }
        }
        if self.records.len() == self.limit {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "expression capacity exhausted",
            ));
        }
        if self.records.len() == self.records.capacity() {
            let count = (self.limit - self.records.len()).min(64);
            let capacity = self.memory.reserve(count as u64 * 1024, c.position())?;
            self.records.try_reserve_exact(count).map_err(|_| {
                Error::new(ErrorCode::ResourceLimited, "expression allocation refused")
            })?;
            self.reservations.push(capacity);
        }
        let id = u32::try_from(self.records.len())
            .map_err(|_| Error::new(ErrorCode::ResourceLimited, "expression ID overflow"))?;
        self.records.push((offset, expr));
        Ok(Value::Expr(id))
    }
    fn binary(
        &mut self,
        site: u64,
        op: IntegerOp,
        a: Value,
        b: Value,
        input: &FunctionInput<'_>,
        c: &mut dyn RunControl,
    ) -> Result<Value> {
        let simple = integer(op, a, b);
        if simple != Value::Unknown {
            return Ok(simple);
        }
        if a == Value::Unknown || b == Value::Unknown {
            return Ok(Value::Unknown);
        }
        self.expression(
            site,
            Expression::Integer {
                op,
                left: public(a, input),
                right: public(b, input),
            },
            c,
        )
    }
}
#[allow(clippy::too_many_arguments)]
fn evaluate(
    op: Operation,
    node: &Node,
    input: &FunctionInput<'_>,
    incoming: State,
    c: &mut dyn RunControl,
    symbols: &mut Symbols<'_>,
    enabled: bool,
    abi: Option<CallAbi>,
) -> Result<(State, Effects)> {
    let (mut state, mut effects) = transfer(op, node, input, incoming, c)?;
    if !enabled || op.gap.is_some() {
        return Ok((state, effects));
    }
    match op.op {
        SemanticOp::Integer {
            op: int,
            dest,
            left,
            right,
        } if op.relocation.is_none() => {
            let v = symbols.binary(
                node.offset,
                int,
                operand(&incoming, left),
                operand(&incoming, right),
                input,
                c,
            )?;
            effects.register = Some((dest, v));
            if dest != 0 {
                state[dest as usize] = v;
            }
        }
        SemanticOp::Memory {
            kind,
            base,
            displacement,
            width,
            dest,
            source,
            signed,
            ..
        } if op.relocation.is_none() => {
            let address = symbols.binary(
                node.offset,
                IntegerOp::Add,
                incoming[base as usize],
                Value::Constant(displacement as u32),
                input,
                c,
            )?;
            let value = match source {
                Some(r) if kind == MemoryKind::Store => Some(symbols.binary(
                    node.offset,
                    IntegerOp::And,
                    incoming[r as usize],
                    Value::Constant(match width {
                        1 => 255,
                        2 => 65535,
                        _ => u32::MAX,
                    }),
                    input,
                    c,
                )?),
                _ => effects.memory.and_then(|m| m.3),
            };
            effects.memory = Some((kind, width, address, value));
            if kind == MemoryKind::Load
                && let Some(dest) = dest
                && dest != 0
                && state[dest as usize] == Value::Unknown
            {
                let v = symbols.expression(
                    node.offset,
                    Expression::Load {
                        address: public(address, input),
                        width,
                        signed,
                    },
                    c,
                )?;
                state[dest as usize] = v;
                effects.register = Some((dest, v));
            }
        }
        _ => (),
    }
    if op.opaque_call {
        state = unknown_state();
        if abi.is_some() {
            for r in [2, 3, 4, 8, 9, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27] {
                state[r] = incoming[r];
            }
        }
        for r in [10, 11] {
            state[r] = symbols.expression(
                node.offset,
                Expression::CallResult {
                    callsite: node.offset,
                    register: r as u8,
                },
                c,
            )?;
        }
    }
    state[0] = Value::Constant(0);
    if matches!(effects.register, Some((0, _))) {
        effects.register = Some((0, Value::Constant(0)));
    }
    Ok((state, effects))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rv32_arithmetic_handles_wrap_shifts_and_division_boundaries() {
        use IntegerOp::*;
        for (op, a, b, expected) in [
            (Add, u32::MAX, 1, 0),
            (Sub, 0, 1, u32::MAX),
            (Shl, 1, 33, 2),
            (Shr, 0x80000000, 32, 0x80000000),
            (Sar, 0x80000000, 31, u32::MAX),
            (Lt, u32::MAX, 0, 1),
            (Ltu, u32::MAX, 0, 0),
            (Mul, u32::MAX, 2, 0xfffffffe),
            (Mulh, u32::MAX, 2, u32::MAX),
            (Mulhsu, 0x80000000, u32::MAX, 0x80000000),
            (Mulhu, u32::MAX, u32::MAX, 0xfffffffe),
            (Div, 7, 0, u32::MAX),
            (Div, 0x80000000, u32::MAX, 0x80000000),
            (Rem, 0x80000000, u32::MAX, 0),
            (Rem, 0xfffffff9, 3, u32::MAX),
            (Divu, u32::MAX, 2, 0x7fffffff),
            (Remu, 8, 0, 8),
        ] {
            assert_eq!(
                integer(op, Value::Constant(a), Value::Constant(b)),
                Value::Constant(expected),
                "{op:?}"
            );
        }
        assert_eq!(offset(Value::Stack(i64::MAX), 1), Value::Unknown);
    }
    struct Isa(Vec<SemanticOp>);
    impl PointerDecoder for Isa {}
    impl FunctionDecoder for Isa {
        fn identity(&self) -> &'static str {
            "test"
        }
        fn decode(&self, _: &[u8]) -> Option<DecodedOp> {
            unreachable!()
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
    impl FunctionSemantics for Isa {
        fn semantic_identity(&self) -> &'static str {
            "test-values"
        }
        fn lift(&self, b: &[u8]) -> SemanticOp {
            self.0[b[0] as usize]
        }
        fn value_relocation(&self, _: &FunctionRelocation, _: SemanticOp) -> ValueRelocation {
            unreachable!()
        }
    }
    #[derive(Default)]
    struct Records(Vec<FunctionRecord>);
    impl FunctionSink for Records {
        fn record(&mut self, r: &FunctionRecord, _: &mut dyn RunControl) -> Result<()> {
            self.0.push(r.clone());
            Ok(())
        }
    }
    fn calculate(
        ops: Vec<SemanticOp>,
        links: &[(usize, usize)],
        reverse: bool,
    ) -> Vec<FunctionRecord> {
        let bytes: Vec<u8> = (0..ops.len()).flat_map(|i| [i as u8, 0]).collect();
        let nodes: Vec<_> = (0..ops.len())
            .map(|i| Node {
                offset: i as u64 * 2,
                decoded: DecodedOp {
                    length: 2,
                    text: String::new(),
                    flow: InstructionFlow::Next,
                },
                conflict: false,
            })
            .collect();
        let mut edges: Vec<_> = links
            .iter()
            .map(|&(from, to)| Edge {
                from: from as u64 * 2,
                target: Some(to as u64 * 2),
                relation: EdgeKind::Jump,
                external: false,
            })
            .collect();
        edges.sort_by_key(|e| {
            (
                e.from,
                if reverse {
                    u64::MAX - e.target.unwrap()
                } else {
                    e.target.unwrap()
                },
            )
        });
        let input = FunctionInput {
            image: None,
            section: 1,
            extent: CodeRange {
                start: 0,
                length: bytes.len() as u64,
            },
            bytes: &bytes,
            relocations: &PreparedReferences::empty(),
            data_ranges: &[],
        };
        let memory = WorkingMemory::new(1024 * 1024).unwrap();
        let mut sink = Records::default();
        analyze(
            &input,
            &nodes,
            &edges,
            &Isa(ops),
            &memory,
            &mut || Ok(()),
            &mut sink,
        )
        .unwrap();
        sink.0
    }
    fn set(dest: u8, n: u32) -> SemanticOp {
        SemanticOp::Integer {
            op: IntegerOp::Add,
            dest,
            left: Operand::Register(0),
            right: Operand::Immediate(n),
        }
    }
    fn read(base: u8) -> SemanticOp {
        SemanticOp::Memory {
            kind: MemoryKind::Load,
            base,
            displacement: 0,
            width: 4,
            dest: Some(10),
            source: None,
            swap: false,
            signed: false,
        }
    }
    #[test]
    fn joins_and_loops_converge_independently_of_visit_order() {
        for (right, expected) in [
            (7, AbstractValue::Constant { value: 7 }),
            (8, AbstractValue::Unknown),
        ] {
            let ops = vec![SemanticOp::None, set(5, 7), set(5, right), read(5)];
            let links = [(0, 1), (0, 2), (1, 3), (2, 3)];
            let records = calculate(ops.clone(), &links, false);
            assert_eq!(records, calculate(ops, &links, true));
            assert!(records.iter().any(
                |r| matches!(r,FunctionRecord::MemoryAccess{address,..} if address==&expected)
            ));
        }
        let ops = vec![
            set(5, 1),
            SemanticOp::Integer {
                op: IntegerOp::Add,
                dest: 5,
                left: Operand::Register(5),
                right: Operand::Immediate(1),
            },
            read(5),
        ];
        let records = calculate(ops, &[(0, 1), (1, 1), (1, 2)], false);
        assert!(records.iter().any(|r| matches!(
            r,
            FunctionRecord::MemoryAccess {
                address: AbstractValue::Unknown,
                ..
            }
        )));
    }
    #[test]
    fn conflicts_and_calls_cannot_propagate_stale_values() {
        let input = FunctionInput {
            image: None,
            section: 1,
            extent: CodeRange {
                start: 0,
                length: 4,
            },
            bytes: &[0; 4],
            relocations: &PreparedReferences::empty(),
            data_ranges: &[],
        };
        let mut node = Node {
            offset: 0,
            decoded: DecodedOp {
                length: 4,
                text: String::new(),
                flow: InstructionFlow::Jump {
                    displacement: 128,
                    link: true,
                },
            },
            conflict: false,
        };
        let mut state = unknown_state();
        state[2] = Value::Stack(-16);
        state[10] = Value::Constant(42);
        let (out, effects) = transfer(
            Operation {
                op: SemanticOp::Link { dest: 1 },
                opaque_call: true,
                relocation: None,
                gap: None,
            },
            &node,
            &input,
            state,
            &mut || Ok(()),
        )
        .unwrap();
        assert_eq!(out, unknown_state());
        assert_eq!(effects.gap, Some(SemanticGapReason::OpaqueCall));
        node.decoded.flow = InstructionFlow::Next;
        let (out, effects) = transfer(
            Operation {
                op: read(10),
                opaque_call: false,
                relocation: None,
                gap: Some(SemanticGapReason::ConflictingBoundary),
            },
            &node,
            &input,
            state,
            &mut || Ok(()),
        )
        .unwrap();
        assert_eq!(out, unknown_state());
        assert!(effects.memory.is_none());
    }
    #[test]
    fn interrupted_solver_releases_admitted_state_before_emitting_records() {
        let bytes = [0, 0, 1, 0];
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
        };
        let nodes: Vec<_> = [0, 2]
            .into_iter()
            .map(|offset| Node {
                offset,
                decoded: DecodedOp {
                    length: 2,
                    text: String::new(),
                    flow: InstructionFlow::Next,
                },
                conflict: false,
            })
            .collect();
        let edges = [Edge {
            from: 0,
            target: Some(2),
            relation: EdgeKind::Fallthrough,
            external: false,
        }];
        for code in [
            ErrorCode::Cancelled,
            ErrorCode::ResourceLimited,
            ErrorCode::TimedOut,
        ] {
            let memory = WorkingMemory::new(65536).unwrap();
            let mut records = Records::default();
            // Admission + two lifts + first transfer, then interruption on its edge.
            let mut calls = 0;
            let result = analyze(
                &input,
                &nodes,
                &edges,
                &Isa(vec![set(5, 1), read(5)]),
                &memory,
                &mut || {
                    calls += 1;
                    if calls == 5 {
                        Err(Error::new(code, "injected control stop"))
                    } else {
                        Ok(())
                    }
                },
                &mut records,
            );
            assert_eq!(result.unwrap_err().code, code);
            assert!(records.0.is_empty());
            assert_eq!(memory.used(), 0);
        }
        assert_eq!(
            validate(SemanticOp::Link { dest: 32 }).unwrap_err().code,
            ErrorCode::Integrity
        );
    }

    #[test]
    fn explicit_integer_abi_preserves_only_declared_registers() {
        let input = FunctionInput {
            image: None,
            section: 1,
            extent: CodeRange {
                start: 0,
                length: 4,
            },
            bytes: &[0; 4],
            relocations: &PreparedReferences::empty(),
            data_ranges: &[],
        };
        let node = Node {
            offset: 0,
            decoded: DecodedOp {
                length: 4,
                text: String::new(),
                flow: InstructionFlow::Jump {
                    displacement: 128,
                    link: true,
                },
            },
            conflict: false,
        };
        let mut incoming = unknown_state();
        incoming[2] = Value::Stack(-16);
        incoming[8] = Value::Constant(0x60000000);
        incoming[5] = Value::Constant(42);
        let operation = Operation {
            op: SemanticOp::Link { dest: 1 },
            opaque_call: true,
            relocation: None,
            gap: None,
        };
        let memory = WorkingMemory::new(1024 * 1024).unwrap();
        let mut symbols = Symbols::new(1, &memory, &mut || Ok(())).unwrap();
        let (plain, _) = evaluate(
            operation,
            &node,
            &input,
            incoming,
            &mut || Ok(()),
            &mut symbols,
            true,
            None,
        )
        .unwrap();
        assert_eq!(plain[8], Value::Unknown);
        assert_eq!(plain[2], Value::Unknown);
        let (declared, effects) = evaluate(
            operation,
            &node,
            &input,
            incoming,
            &mut || Ok(()),
            &mut symbols,
            true,
            Some(CallAbi::RiscvInteger),
        )
        .unwrap();
        assert_eq!(declared[8], incoming[8]);
        assert_eq!(declared[2], incoming[2]);
        assert_eq!(declared[5], Value::Unknown);
        assert_eq!(effects.gap, Some(SemanticGapReason::OpaqueCall));
        assert!(matches!(declared[10], Value::Expr(_)));
        drop(symbols);
        assert_eq!(memory.used(), 0);
    }

    #[test]
    fn upper_address_requires_flowing_identity_and_exact_pcrel_pair() {
        let upper = Value::Upper {
            symbol: 3,
            addend: 8,
            site: 7,
            pc: true,
        };
        let mut r = Relocation {
            role: ValueRelocation::LowerPcRelative,
            site: 9,
            symbol: 3,
            addend: 8,
            pair: Some(7),
        };
        assert_eq!(lower(offset(upper, 0), r), Value::Symbol(3, 8));
        assert_eq!(offset(upper, 1), Value::Unknown);
        r.pair = Some(6);
        assert_eq!(lower(upper, r), Value::Unknown);
        r.pair = Some(7);
        r.symbol = 4;
        assert_eq!(lower(upper, r), Value::Unknown);
        assert_eq!(lower(Value::Constant(0), r), Value::Unknown);
    }
}
