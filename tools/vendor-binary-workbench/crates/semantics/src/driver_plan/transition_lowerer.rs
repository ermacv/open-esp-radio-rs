//! Executor-neutral `Action`/`Completion` state-machine skeleton lowering.

use std::{collections::BTreeSet, fmt::Write as _};

use crate::{
    BitSource, BranchCondition, BranchOperation, DriverAction, DriverFlow, DriverPlan,
    DriverTerminator, EffectDisposition, ExpressionOperation, MemoryAccess, Result, SymbolicValue,
    Timeout, collect_value_inputs,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransitionSkeletonOutput {
    pub transition_type: String,
    pub source: String,
}

#[derive(Clone, Debug)]
enum FlatNode {
    Action {
        action: DriverAction,
        read_token: Option<usize>,
        next: usize,
    },
    Branch {
        condition: BranchCondition,
        taken: usize,
        not_taken: usize,
    },
    Return(SymbolicValue),
}

fn type_identifier(value: &str) -> String {
    let mut output = String::new();
    let mut capitalize = true;
    for character in value.chars() {
        if !character.is_ascii_alphanumeric() {
            capitalize = true;
        } else if capitalize {
            output.push(character.to_ascii_uppercase());
            capitalize = false;
        } else {
            output.push(character.to_ascii_lowercase());
        }
    }
    if output.as_bytes().first().is_some_and(u8::is_ascii_digit) {
        output.insert(0, 'N');
    }
    output
}

fn leaf_variant(action: &DriverAction) -> Result<String> {
    let binding = match action {
        DriverAction::Mmio { binding, .. } => binding,
        DriverAction::IndexedMmio { .. } => {
            return Err("indexed MMIO is a finite PAC transaction, not one transition leaf".into());
        }
        DriverAction::Delay { .. } => {
            return Err("non-MMIO transition action has no PAC leaf".into());
        }
    };
    Ok(type_identifier(&binding.identity))
}

fn omitted(action: &DriverAction) -> bool {
    matches!(action.disposition(), EffectDisposition::AllowedOmission(_))
}

fn flatten_flow(
    flow: &DriverFlow,
    start_reads: usize,
    nodes: &mut Vec<FlatNode>,
) -> Result<(usize, usize)> {
    let mut read_count = start_reads;
    let mut read_tokens = Vec::with_capacity(flow.actions.len());
    for action in &flow.actions {
        let read_token = matches!(
            action,
            DriverAction::Mmio {
                access: MemoryAccess::Read,
                ..
            } | DriverAction::IndexedMmio {
                access: MemoryAccess::Read,
                ..
            }
        )
        .then(|| {
            let token = read_count;
            read_count += 1;
            token
        });
        read_tokens.push(read_token);
    }

    let (mut next, mut maximum_reads) = match &flow.terminator {
        DriverTerminator::Return(value) => {
            let node = nodes.len();
            nodes.push(FlatNode::Return(value.clone()));
            (node, read_count)
        }
        DriverTerminator::Branch {
            condition,
            taken,
            not_taken,
        } => {
            let (taken, taken_reads) = flatten_flow(taken, read_count, nodes)?;
            let (not_taken, not_taken_reads) = flatten_flow(not_taken, read_count, nodes)?;
            let node = nodes.len();
            nodes.push(FlatNode::Branch {
                condition: condition.clone(),
                taken,
                not_taken,
            });
            (node, taken_reads.max(not_taken_reads))
        }
    };
    for (action, read_token) in flow.actions.iter().zip(read_tokens).rev() {
        if omitted(action) {
            continue;
        }
        let node = nodes.len();
        nodes.push(FlatNode::Action {
            action: action.clone(),
            read_token,
            next,
        });
        next = node;
    }
    maximum_reads = maximum_reads.max(read_count);
    Ok((next, maximum_reads))
}

fn render_bit_source(source: BitSource) -> Result<String> {
    match source {
        BitSource::Constant(false) => Ok("0_u32".to_owned()),
        BitSource::Constant(true) => Ok("1_u32".to_owned()),
        BitSource::Input {
            index,
            bit,
            inverted,
        } => {
            let source = if inverted {
                format!("!self.arg{index}")
            } else {
                format!("self.arg{index}")
            };
            Ok(format!("(({source} >> {bit}) & 1_u32)"))
        }
        BitSource::Register {
            read_token,
            bit,
            inverted,
            ..
        }
        | BitSource::IndexedRegister {
            read_token,
            bit,
            inverted,
        } => {
            let source = if inverted {
                format!("!self.reads[{read_token}]")
            } else {
                format!("self.reads[{read_token}]")
            };
            Ok(format!("(({source} >> {bit}) & 1_u32)"))
        }
        BitSource::Unknown
        | BitSource::Memory { .. }
        | BitSource::PrivateStack { .. }
        | BitSource::CallResult { .. }
        | BitSource::ExternalResult { .. } => {
            Err(format!("unsupported transition bit source: {source:?}").into())
        }
    }
}

fn render_value(value: &SymbolicValue) -> Result<String> {
    if let Some(index) = value.direct_input_index() {
        return Ok(format!("self.arg{index}"));
    }
    match value {
        SymbolicValue::Constant(value) => Ok(format!("{value:#010x}_u32")),
        SymbolicValue::Input { index } | SymbolicValue::InputConstant { index, .. } => {
            Ok(format!("self.arg{index}"))
        }
        SymbolicValue::Expression {
            operation,
            left,
            right,
        } => {
            let left = render_value(left)?;
            let right = render_value(right)?;
            Ok(match operation {
                ExpressionOperation::Add => format!("({left}).wrapping_add({right})"),
                ExpressionOperation::Subtract => format!("({left}).wrapping_sub({right})"),
                ExpressionOperation::Multiply => format!("({left}).wrapping_mul({right})"),
                ExpressionOperation::BitAnd => format!("({left}) & ({right})"),
                ExpressionOperation::BitOr => format!("({left}) | ({right})"),
                ExpressionOperation::BitXor => format!("({left}) ^ ({right})"),
                ExpressionOperation::ShiftLeft => {
                    format!("({left}).wrapping_shl(({right}) & 31)")
                }
                ExpressionOperation::ShiftRight => {
                    format!("({left}).wrapping_shr(({right}) & 31)")
                }
                ExpressionOperation::ShiftRightArithmetic => {
                    format!("(({left}) as i32).wrapping_shr(({right}) & 31) as u32")
                }
                ExpressionOperation::Equal => {
                    format!("u32::from(({left}) == ({right}))")
                }
                ExpressionOperation::LessThanSigned => {
                    format!("u32::from((({left}) as i32) < (({right}) as i32))")
                }
                ExpressionOperation::LessThanUnsigned => {
                    format!("u32::from(({left}) < ({right}))")
                }
                ExpressionOperation::DivideSigned
                | ExpressionOperation::DivideUnsigned
                | ExpressionOperation::RemainderSigned
                | ExpressionOperation::RemainderUnsigned => {
                    return Err(format!(
                        "division/remainder has no executor-neutral transition lowering: {operation:?}"
                    )
                    .into());
                }
            })
        }
        SymbolicValue::RegisterImage {
            read_token,
            and_mask,
            or_mask,
            ..
        }
        | SymbolicValue::IndexedRegisterImage {
            read_token,
            and_mask,
            or_mask,
        }
        | SymbolicValue::MemoryImage {
            read_token,
            and_mask,
            or_mask,
        } => Ok(format!(
            "(self.reads[{read_token}] & {and_mask:#010x}_u32) | {or_mask:#010x}_u32"
        )),
        SymbolicValue::Bits(bits) => {
            let mut terms = Vec::new();
            for (destination, source) in bits.iter().copied().enumerate() {
                if source == BitSource::Constant(false) {
                    continue;
                }
                let source = render_bit_source(source)?;
                terms.push(if destination == 0 {
                    source
                } else {
                    format!("({source} << {destination})")
                });
            }
            Ok(if terms.is_empty() {
                "0_u32".to_owned()
            } else {
                terms.join(" | ")
            })
        }
        SymbolicValue::Unknown
        | SymbolicValue::StackAddress(_)
        | SymbolicValue::SymbolAddress { .. }
        | SymbolicValue::CallResult(_)
        | SymbolicValue::ExternalTable(_)
        | SymbolicValue::ExternalFunction { .. }
        | SymbolicValue::ReviewedExternalFunction { .. }
        | SymbolicValue::FunctionTable(_)
        | SymbolicValue::FunctionPointer { .. }
        | SymbolicValue::ExternalResult(_)
        | SymbolicValue::WideSignedDivide { .. } => {
            Err(format!("value has no executor-neutral transition lowering: {value:?}").into())
        }
    }
}

fn render_condition(condition: &BranchCondition) -> Result<String> {
    let left = render_value(&condition.left)?;
    let right = render_value(&condition.right)?;
    Ok(match condition.operation {
        BranchOperation::Equal => format!("({left}) == ({right})"),
        BranchOperation::NotEqual => format!("({left}) != ({right})"),
        BranchOperation::LessSigned => format!("(({left}) as i32) < (({right}) as i32)"),
        BranchOperation::GreaterEqualSigned => {
            format!("(({left}) as i32) >= (({right}) as i32)")
        }
        BranchOperation::LessUnsigned => format!("({left}) < ({right})"),
        BranchOperation::GreaterEqualUnsigned => format!("({left}) >= ({right})"),
    })
}

fn timeout_source(timeout: Timeout) -> String {
    match timeout {
        Timeout::Attempts(attempts) => format!("TransitionTimeout::Attempts({attempts})"),
        Timeout::DeadlineMicros(micros) => {
            format!("TransitionTimeout::DeadlineMicros({micros})")
        }
    }
}

fn action_source(action: &DriverAction) -> Result<String> {
    Ok(match (action, action.disposition()) {
        (
            DriverAction::Mmio {
                access: MemoryAccess::Read,
                ..
            },
            EffectDisposition::Required,
        ) => format!("TransitionAction::Read(PacLeaf::{})", leaf_variant(action)?),
        (
            DriverAction::Mmio {
                access: MemoryAccess::Write,
                value: Some(value),
                ..
            },
            EffectDisposition::Required,
        ) => format!(
            "TransitionAction::Write {{ leaf: PacLeaf::{}, value: {} }}",
            leaf_variant(action)?,
            render_value(value)?
        ),
        (DriverAction::IndexedMmio { .. }, EffectDisposition::Required) => {
            return Err(
                "indexed MMIO cannot be flattened into one executor-neutral PacLeaf".into(),
            );
        }
        (DriverAction::Delay { micros, .. }, EffectDisposition::Required) => {
            format!("TransitionAction::DelayMicros({})", render_value(micros)?)
        }
        (_, EffectDisposition::ReplacedByAsync { condition, timeout }) => format!(
            "TransitionAction::AwaitReady {{ condition: {condition:?}, timeout: {} }}",
            timeout_source(*timeout)
        ),
        (_, EffectDisposition::PlatformProvidedInput { input }) => {
            format!("TransitionAction::PlatformInput({input:?})")
        }
        (_, EffectDisposition::PlatformProvidedService { service }) => {
            format!("TransitionAction::PlatformService({service:?})")
        }
        (_, EffectDisposition::PublishedEvent { event }) => {
            format!("TransitionAction::PublishEvent({event:?})")
        }
        (_, EffectDisposition::InitializationPrerequisite { prerequisite }) => {
            format!("TransitionAction::RequireInitialization({prerequisite:?})")
        }
        (_, EffectDisposition::AllowedOmission(_)) => {
            return Err("omitted transition action reached source rendering".into());
        }
        (_, EffectDisposition::PlatformOwned) => {
            return Err("platform-owned action needs a named service before lowering".into());
        }
        (_, EffectDisposition::Forbidden) => {
            return Err("forbidden action reached transition lowering".into());
        }
        (DriverAction::Mmio { .. }, EffectDisposition::Required) => {
            return Err("malformed required MMIO action".into());
        }
    })
}

fn action_expects_value(action: &DriverAction) -> bool {
    matches!(
        action,
        DriverAction::Mmio {
            access: MemoryAccess::Read,
            ..
        } | DriverAction::IndexedMmio {
            access: MemoryAccess::Read,
            ..
        }
    )
}

fn collect_inputs(flow: &DriverFlow, output: &mut BTreeSet<u8>) {
    for action in &flow.actions {
        match action {
            DriverAction::Mmio {
                value: Some(value), ..
            }
            | DriverAction::Delay { micros: value, .. } => collect_value_inputs(value, output),
            DriverAction::Mmio { value: None, .. } => {}
            DriverAction::IndexedMmio {
                input_index, value, ..
            } => {
                output.insert(*input_index);
                if let Some(value) = value {
                    collect_value_inputs(value, output);
                }
            }
        }
    }
    match &flow.terminator {
        DriverTerminator::Return(value) => collect_value_inputs(value, output),
        DriverTerminator::Branch {
            condition,
            taken,
            not_taken,
        } => {
            collect_value_inputs(&condition.left, output);
            collect_value_inputs(&condition.right, output);
            collect_inputs(taken, output);
            collect_inputs(not_taken, output);
        }
    }
}

fn collect_leaves(flow: &DriverFlow, output: &mut BTreeSet<String>) -> Result<()> {
    for action in &flow.actions {
        if matches!(
            action,
            DriverAction::Mmio { .. } | DriverAction::IndexedMmio { .. }
        ) {
            output.insert(leaf_variant(action)?);
        }
    }
    if let DriverTerminator::Branch {
        taken, not_taken, ..
    } = &flow.terminator
    {
        collect_leaves(taken, output)?;
        collect_leaves(not_taken, output)?;
    }
    Ok(())
}

pub fn lower_transition_skeleton(plan: &DriverPlan) -> Result<TransitionSkeletonOutput> {
    let transition_type = format!("{}Transition", type_identifier(&plan.symbol));
    let mut inputs = BTreeSet::new();
    collect_inputs(&plan.flow, &mut inputs);
    let mut leaves = BTreeSet::new();
    collect_leaves(&plan.flow, &mut leaves)?;
    let mut nodes = Vec::new();
    let (entry, read_count) = flatten_flow(&plan.flow, 0, &mut nodes)?;

    let mut source = String::new();
    writeln!(
        source,
        "// @generated executor-neutral transition skeleton; review names and domain types before integration."
    )?;
    writeln!(source, "// Source vendor symbol: {}", plan.symbol)?;
    writeln!(source, "#[derive(Clone, Copy, Debug, Eq, PartialEq)]")?;
    writeln!(source, "pub enum PacLeaf {{")?;
    for leaf in &leaves {
        writeln!(source, "    {leaf},")?;
    }
    writeln!(source, "}}")?;
    writeln!(source, "#[derive(Clone, Copy, Debug, Eq, PartialEq)]")?;
    writeln!(
        source,
        "pub enum TransitionTimeout {{ Attempts(u32), DeadlineMicros(u32) }}"
    )?;
    writeln!(source, "#[derive(Clone, Copy, Debug, Eq, PartialEq)]")?;
    writeln!(source, "pub enum TransitionAction {{")?;
    writeln!(source, "    Read(PacLeaf),")?;
    writeln!(source, "    Write {{ leaf: PacLeaf, value: u32 }},")?;
    writeln!(source, "    DelayMicros(u32),")?;
    writeln!(
        source,
        "    AwaitReady {{ condition: &'static str, timeout: TransitionTimeout }},"
    )?;
    writeln!(source, "    PlatformInput(&'static str),")?;
    writeln!(source, "    PlatformService(&'static str),")?;
    writeln!(source, "    PublishEvent(&'static str),")?;
    writeln!(source, "    RequireInitialization(&'static str),")?;
    writeln!(source, "    Complete(Option<u32>),")?;
    writeln!(source, "}}")?;
    writeln!(source, "#[derive(Clone, Copy, Debug, Eq, PartialEq)]")?;
    writeln!(
        source,
        "pub enum TransitionCompletion {{ Value(u32), Complete }}"
    )?;
    writeln!(source, "#[derive(Clone, Copy, Debug, Eq, PartialEq)]")?;
    writeln!(source, "pub struct UnexpectedCompletion;")?;
    writeln!(source, "#[derive(Clone, Copy, Debug, Eq, PartialEq)]")?;
    writeln!(source, "enum Phase {{")?;
    for node in 0..nodes.len() {
        writeln!(source, "    Node{node},")?;
    }
    writeln!(source, "    Done,")?;
    writeln!(source, "}}")?;
    writeln!(source, "pub struct {transition_type} {{")?;
    writeln!(source, "    phase: Phase,")?;
    writeln!(source, "    reads: [u32; {read_count}],")?;
    for input in &inputs {
        writeln!(source, "    arg{input}: u32,")?;
    }
    writeln!(source, "}}")?;
    writeln!(source, "impl {transition_type} {{")?;
    write!(source, "    pub const fn new(")?;
    for (position, input) in inputs.iter().enumerate() {
        if position != 0 {
            write!(source, ", ")?;
        }
        write!(source, "arg{input}: u32")?;
    }
    writeln!(source, ") -> Self {{")?;
    writeln!(
        source,
        "        Self {{ phase: Phase::Node{entry}, reads: [0; {read_count}],"
    )?;
    for input in &inputs {
        writeln!(source, "            arg{input},")?;
    }
    writeln!(source, "        }}")?;
    writeln!(source, "    }}")?;
    writeln!(
        source,
        "    pub fn action(&mut self) -> Option<TransitionAction> {{"
    )?;
    writeln!(source, "        loop {{")?;
    writeln!(source, "            match self.phase {{")?;
    for (node_index, node) in nodes.iter().enumerate() {
        match node {
            FlatNode::Action { action, .. } => {
                writeln!(
                    source,
                    "                Phase::Node{node_index} => return Some({}),",
                    action_source(action)?
                )?;
            }
            FlatNode::Branch {
                condition,
                taken,
                not_taken,
            } => {
                writeln!(source, "                Phase::Node{node_index} => {{")?;
                writeln!(
                    source,
                    "                    self.phase = if {} {{ Phase::Node{taken} }} else {{ Phase::Node{not_taken} }};",
                    render_condition(condition)?
                )?;
                writeln!(source, "                }}")?;
            }
            FlatNode::Return(value) => {
                let value = if plan.exit_return_modeled {
                    format!("Some({})", render_value(value)?)
                } else {
                    "None".to_owned()
                };
                writeln!(
                    source,
                    "                Phase::Node{node_index} => return Some(TransitionAction::Complete({value})),"
                )?;
            }
        }
    }
    writeln!(source, "                Phase::Done => return None,")?;
    writeln!(source, "            }}")?;
    writeln!(source, "        }}")?;
    writeln!(source, "    }}")?;
    writeln!(
        source,
        "    pub fn advance(&mut self, completion: TransitionCompletion) -> Result<(), UnexpectedCompletion> {{"
    )?;
    writeln!(source, "        match self.phase {{")?;
    for (node_index, node) in nodes.iter().enumerate() {
        match node {
            FlatNode::Action {
                action,
                read_token,
                next,
            } if action_expects_value(action) => {
                let token = read_token.expect("read action has an assigned token");
                writeln!(
                    source,
                    "            Phase::Node{node_index} => match completion {{"
                )?;
                writeln!(
                    source,
                    "                TransitionCompletion::Value(value) => {{ self.reads[{token}] = value; self.phase = Phase::Node{next}; Ok(()) }}"
                )?;
                writeln!(
                    source,
                    "                TransitionCompletion::Complete => Err(UnexpectedCompletion),"
                )?;
                writeln!(source, "            }},")?;
            }
            FlatNode::Action { next, .. } => {
                writeln!(
                    source,
                    "            Phase::Node{node_index} => match completion {{"
                )?;
                writeln!(
                    source,
                    "                TransitionCompletion::Complete => {{ self.phase = Phase::Node{next}; Ok(()) }}"
                )?;
                writeln!(
                    source,
                    "                TransitionCompletion::Value(_) => Err(UnexpectedCompletion),"
                )?;
                writeln!(source, "            }},")?;
            }
            FlatNode::Return(_) => {
                writeln!(
                    source,
                    "            Phase::Node{node_index} => match completion {{"
                )?;
                writeln!(
                    source,
                    "                TransitionCompletion::Complete => {{ self.phase = Phase::Done; Ok(()) }}"
                )?;
                writeln!(
                    source,
                    "                TransitionCompletion::Value(_) => Err(UnexpectedCompletion),"
                )?;
                writeln!(source, "            }},")?;
            }
            FlatNode::Branch { .. } => {
                writeln!(
                    source,
                    "            Phase::Node{node_index} => Err(UnexpectedCompletion),"
                )?;
            }
        }
    }
    writeln!(
        source,
        "            Phase::Done => Err(UnexpectedCompletion),"
    )?;
    writeln!(source, "        }}")?;
    writeln!(source, "    }}")?;
    writeln!(source, "}}")?;

    Ok(TransitionSkeletonOutput {
        transition_type,
        source,
    })
}
