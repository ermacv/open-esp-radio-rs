//! Pseudo-Rust value and flow rendering for manual linked-IR analysis.

use std::fmt::Write as _;

use super::*;

pub(super) fn pseudo_identifier(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || character == '_' {
            output.push(character);
        } else {
            output.push('_');
        }
    }
    if output.is_empty() {
        "unnamed".to_owned()
    } else if output.as_bytes()[0].is_ascii_digit() {
        format!("fn_{output}")
    } else {
        output
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PseudoBitBase {
    Input(u8),
    Register { token: u32, address: u32 },
    IndexedRegister(u32),
    Memory(u32),
    PrivateStack(u32),
    CallResult(u32),
    ExternalResult(u32),
    ExternalResultHigh(u32),
    ExternalOutput { call_token: u32, output_index: u8 },
}

impl PseudoBitBase {
    fn render(&self) -> String {
        match self {
            Self::Input(index) => format!("arg{index}"),
            Self::Register { token, .. } => format!("read{token}"),
            Self::IndexedRegister(token) => format!("indexed_read{token}"),
            Self::Memory(token) => format!("ramread{token}"),
            Self::PrivateStack(token) => format!("private_stack_read{token}"),
            Self::CallResult(token) => format!("call{token}"),
            Self::ExternalResult(token) => format!("external{token}"),
            Self::ExternalResultHigh(token) => format!("external{token}_high"),
            Self::ExternalOutput {
                call_token,
                output_index,
            } => format!("external{call_token}_output{output_index}"),
        }
    }
}

fn pseudo_bit_source(source: &BitSource) -> Option<(PseudoBitBase, u8, bool)> {
    match source {
        BitSource::Input {
            index,
            bit,
            inverted,
        } => Some((PseudoBitBase::Input(*index), *bit, *inverted)),
        BitSource::Register {
            read_token,
            address,
            bit,
            inverted,
        } => Some((
            PseudoBitBase::Register {
                token: *read_token,
                address: *address,
            },
            *bit,
            *inverted,
        )),
        BitSource::IndexedRegister {
            read_token,
            bit,
            inverted,
        } => Some((PseudoBitBase::IndexedRegister(*read_token), *bit, *inverted)),
        BitSource::Memory {
            read_token,
            bit,
            inverted,
        } => Some((PseudoBitBase::Memory(*read_token), *bit, *inverted)),
        BitSource::PrivateStack {
            read_token,
            bit,
            inverted,
        } => Some((PseudoBitBase::PrivateStack(*read_token), *bit, *inverted)),
        BitSource::CallResult {
            call_token,
            bit,
            inverted,
        } => Some((PseudoBitBase::CallResult(*call_token), *bit, *inverted)),
        BitSource::ExternalResult {
            call_token,
            bit,
            inverted,
        } => Some((PseudoBitBase::ExternalResult(*call_token), *bit, *inverted)),
        BitSource::ExternalResultHigh {
            call_token,
            bit,
            inverted,
        } => Some((
            PseudoBitBase::ExternalResultHigh(*call_token),
            *bit,
            *inverted,
        )),
        BitSource::ExternalOutput {
            call_token,
            output_index,
            bit,
            inverted,
        } => Some((
            PseudoBitBase::ExternalOutput {
                call_token: *call_token,
                output_index: *output_index,
            },
            *bit,
            *inverted,
        )),
        BitSource::Unknown | BitSource::Constant(_) => None,
    }
}

fn pseudo_masked_bits(bits: &[BitSource; 32]) -> Option<String> {
    let mut base = None;
    let mut inverted = None;
    let mut mask = 0_u32;
    for (output_bit, source) in bits.iter().enumerate() {
        if matches!(source, BitSource::Constant(false)) {
            continue;
        }
        let (source_base, source_bit, source_inverted) = pseudo_bit_source(source)?;
        if usize::from(source_bit) != output_bit {
            return None;
        }
        if base.as_ref().is_some_and(|base| base != &source_base)
            || inverted.is_some_and(|inverted| inverted != source_inverted)
        {
            return None;
        }
        base = Some(source_base);
        inverted = Some(source_inverted);
        mask |= 1 << output_bit;
    }
    let base = base?.render();
    match (inverted == Some(true), mask) {
        (false, u32::MAX) => Some(base),
        (true, u32::MAX) => Some(format!("(!{base})")),
        (false, mask) => Some(format!("({base} & {mask:#010x})")),
        (true, mask) => Some(format!("((!{base}) & {mask:#010x})")),
    }
}

pub(super) fn pseudo_value(value: &SymbolicValue) -> String {
    if let Some(index) = value.direct_input_index() {
        return format!("arg{index}");
    }
    match value {
        SymbolicValue::Unknown => "unknown".to_owned(),
        SymbolicValue::Input { index } => format!("arg{index}"),
        SymbolicValue::Constant(value) | SymbolicValue::InputConstant { value, .. } => {
            format!("{value:#010x}")
        }
        SymbolicValue::StackAddress(offset) => format!("stack.ptr({offset:+#x})"),
        SymbolicValue::SymbolAddress {
            member,
            symbol,
            hi_addend,
            lo_addend,
            post_offset,
        } => format!(
            "symbol({}::{symbol}, hi={hi_addend:+#x}, lo={}, post={post_offset:+#x})",
            member.as_deref().unwrap_or("linked"),
            lo_addend.map_or_else(|| "?".to_owned(), |value| format!("{value:+#x}"))
        ),
        SymbolicValue::CallResult(token) => format!("call{token}"),
        SymbolicValue::ExternalResult(token) => format!("external{token}"),
        SymbolicValue::ExternalResultHigh(token) => format!("external{token}_high"),
        SymbolicValue::ExternalOutput {
            call_token,
            output_index,
        } => format!("external{call_token}_output{output_index}"),
        SymbolicValue::Expression {
            operation,
            left,
            right,
        } => {
            let left = pseudo_value(left);
            let right = pseudo_value(right);
            match operation {
                ExpressionOperation::Add => format!("{left}.wrapping_add({right})"),
                ExpressionOperation::Subtract => format!("{left}.wrapping_sub({right})"),
                ExpressionOperation::Multiply => format!("{left}.wrapping_mul({right})"),
                ExpressionOperation::DivideSigned => format!("signed_div({left}, {right})"),
                ExpressionOperation::DivideUnsigned => format!("{left} / {right}"),
                ExpressionOperation::RemainderSigned => format!("signed_rem({left}, {right})"),
                ExpressionOperation::RemainderUnsigned => format!("{left} % {right}"),
                ExpressionOperation::BitAnd => format!("({left} & {right})"),
                ExpressionOperation::BitOr => format!("({left} | {right})"),
                ExpressionOperation::BitXor => format!("({left} ^ {right})"),
                ExpressionOperation::ShiftLeft => format!("({left} << ({right} & 31))"),
                ExpressionOperation::ShiftRight => format!("({left} >> ({right} & 31))"),
                ExpressionOperation::ShiftRightArithmetic => {
                    format!("(({left} as i32) >> ({right} & 31)) as u32")
                }
                ExpressionOperation::Equal => format!("u32::from({left} == {right})"),
                ExpressionOperation::LessThanSigned => {
                    format!("u32::from(({left} as i32) < ({right} as i32))")
                }
                ExpressionOperation::LessThanUnsigned => {
                    format!("u32::from({left} < {right})")
                }
            }
        }
        SymbolicValue::WideSignedDivide {
            dividend_low,
            dividend_high,
            divisor_low,
            divisor_high,
            high_word,
        } => format!(
            "sdiv64_{}({}, {}, {}, {})",
            if *high_word { "high" } else { "low" },
            pseudo_value(dividend_low),
            pseudo_value(dividend_high),
            pseudo_value(divisor_low),
            pseudo_value(divisor_high)
        ),
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
        } => format!("((read{read_token} & {and_mask:#010x}) | {or_mask:#010x})"),
        SymbolicValue::MemoryImage {
            read_token,
            and_mask,
            or_mask,
        } => format!("((ramread{read_token} & {and_mask:#010x}) | {or_mask:#010x})"),
        SymbolicValue::Bits(bits) => {
            pseudo_masked_bits(bits).unwrap_or_else(|| format!("symbolic({:?})", value.canonical()))
        }
        SymbolicValue::ReviewedExternalTable(_)
        | SymbolicValue::ReviewedExternalFunction { .. }
        | SymbolicValue::FunctionTable(_)
        | SymbolicValue::FunctionPointer { .. } => {
            format!("symbolic({:?})", value.canonical())
        }
    }
}

pub(super) fn pseudo_arguments(arguments: &[SymbolicValue]) -> String {
    arguments
        .iter()
        .map(pseudo_value)
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Clone, Default)]
pub(super) struct RenderState {
    mmio_reads: u32,
    memory_reads: u32,
}

fn indent(level: usize) -> String {
    "    ".repeat(level)
}

fn render_observable(
    event: &ObservableEvent,
    output: &mut String,
    level: usize,
    state: &mut RenderState,
) {
    let prefix = indent(level);
    match event {
        ObservableEvent::Memory {
            access,
            width,
            address,
            register,
            value,
        } => match access {
            MemoryAccess::Read => {
                writeln!(
                    output,
                    "{prefix}let read{} = mmio.read{width}({address:#010x}); // {register}",
                    state.mmio_reads
                )
                .unwrap();
                state.mmio_reads += 1;
            }
            MemoryAccess::Write => {
                let value = value
                    .as_ref()
                    .map_or_else(|| "unknown".to_owned(), pseudo_value);
                writeln!(
                    output,
                    "{prefix}mmio.write{width}({address:#010x}, {value}); // {register}"
                )
                .unwrap();
            }
        },
        ObservableEvent::Fence {
            fm,
            predecessor,
            successor,
        } => writeln!(
            output,
            "{prefix}fence(fm={fm:#x}, pred={predecessor:#x}, succ={successor:#x});"
        )
        .unwrap(),
    }
}

fn render_embedded_flow(
    label: &str,
    flow: &DraftReferenceFlow,
    output: &mut String,
    level: usize,
    state: &mut RenderState,
) {
    let prefix = indent(level);
    writeln!(output, "{prefix}// {label}").unwrap();
    for event in &flow.events {
        render_event(event, output, level, state);
    }
}

pub(super) fn render_event(
    event: &DraftReferenceEvent,
    output: &mut String,
    level: usize,
    state: &mut RenderState,
) {
    let prefix = indent(level);
    match event {
        DraftReferenceEvent::Observable(event) => {
            render_observable(event, output, level, state);
        }
        DraftReferenceEvent::IndexedMmio {
            access,
            width,
            address,
            registers,
            guard,
            value,
        } => {
            let candidates = registers
                .iter()
                .map(|register| format!("{}@{:#010x}", register.name, register.address))
                .collect::<Vec<_>>()
                .join(", ");
            if let Some(guard) = guard {
                writeln!(
                    output,
                    "{prefix}assert!({} < {});",
                    pseudo_value(&guard.selector),
                    guard.maximum
                )
                .unwrap();
            }
            match access {
                MemoryAccess::Read => {
                    writeln!(
                        output,
                        "{prefix}let read{} = mmio.read{width}({}); // indexed: {candidates}",
                        state.mmio_reads,
                        pseudo_value(address)
                    )
                    .unwrap();
                    state.mmio_reads += 1;
                }
                MemoryAccess::Write => {
                    let value = value
                        .as_ref()
                        .map_or_else(|| "unknown".to_owned(), pseudo_value);
                    writeln!(
                        output,
                        "{prefix}mmio.write{width}({}, {value}); // indexed: {candidates}",
                        pseudo_value(address)
                    )
                    .unwrap();
                }
            }
        }
        DraftReferenceEvent::PollMmio {
            width,
            address,
            mask,
            expected,
            ..
        } => writeln!(
            output,
            "{prefix}while (mmio.read{width}({}) & {mask:#010x}) != {expected:#010x} {{ spin(); }}",
            pseudo_value(address)
        )
        .unwrap(),
        DraftReferenceEvent::BoundedPoll {
            maximum_attempts,
            body,
            repeat_while_mask,
            repeat_while_expected,
            on_exhausted,
        } => {
            writeln!(
                output,
                "{prefix}for attempt in 0..{maximum_attempts} {{ // repeat while result & {repeat_while_mask:#010x} == {repeat_while_expected:#010x}"
            )
            .unwrap();
            render_embedded_flow("poll body", body, output, level + 1, state);
            writeln!(output, "{prefix}}}").unwrap();
            if let Some(event) = on_exhausted.as_deref() {
                writeln!(output, "{prefix}if exhausted {{").unwrap();
                render_event(event, output, level + 1, state);
                writeln!(output, "{prefix}}}").unwrap();
            }
        }
        DraftReferenceEvent::PollFlow {
            body,
            exit_when_mask,
            exit_when_expected,
        } => {
            writeln!(
                output,
                "{prefix}loop {{ // exit when result & {exit_when_mask:#010x} == {exit_when_expected:#010x}"
            )
            .unwrap();
            render_embedded_flow("poll flow", body, output, level + 1, state);
            writeln!(output, "{prefix}}}").unwrap();
        }
        DraftReferenceEvent::SymmetricCalibrationSearch {
            attempts_per_direction,
            settle_micros,
            sample_shift,
            sample_mask,
            accepted_sample,
            initial_read,
            setup,
            write_candidate,
            sample,
            ..
        } => {
            writeln!(
                output,
                "{prefix}calibration_search(attempts={attempts_per_direction}, settle_us={settle_micros}, sample=(_ >> {sample_shift}) & {sample_mask:#x}, accepted={accepted_sample:#x}) {{"
            )
            .unwrap();
            for (label, flow) in [
                ("initial read", initial_read),
                ("setup", setup),
                ("write candidate", write_candidate),
                ("sample", sample),
            ] {
                render_embedded_flow(label, flow, output, level + 1, state);
            }
            writeln!(output, "{prefix}}}").unwrap();
        }
        DraftReferenceEvent::DelayMicros { micros } => {
            writeln!(output, "{prefix}delay_us({});", pseudo_value(micros)).unwrap();
        }
        DraftReferenceEvent::Memory {
            access,
            width,
            address,
            region,
            value,
        } => match access {
            MemoryAccess::Read => {
                if let Some((argument, offset)) = address.caller_memory_location() {
                    writeln!(
                        output,
                        "{prefix}let ramread{} = ctx{argument}.read{width}({offset:+#x}); // {region}",
                        state.memory_reads,
                    )
                    .unwrap();
                } else {
                    writeln!(
                        output,
                        "{prefix}let ramread{} = memory.read{width}({}); // {region}",
                        state.memory_reads,
                        pseudo_value(address)
                    )
                    .unwrap();
                }
                state.memory_reads += 1;
            }
            MemoryAccess::Write => {
                let value = value.as_ref().map_or_else(|| "unknown".to_owned(), pseudo_value);
                if let Some((argument, offset)) = address.caller_memory_location() {
                    writeln!(
                        output,
                        "{prefix}ctx{argument}.write{width}({offset:+#x}, {value}); // {region}"
                    )
                    .unwrap();
                } else {
                    writeln!(
                        output,
                        "{prefix}memory.write{width}({}, {value}); // {region}",
                        pseudo_value(address)
                    )
                    .unwrap();
                }
            }
        },
        DraftReferenceEvent::PrivateStackLoad {
            token,
            offset,
            width,
            signed,
        } => writeln!(
            output,
            "{prefix}let private_stack_read{token} = stack.load{width}({offset:+#x}, signed={signed});"
        )
        .unwrap(),
        DraftReferenceEvent::PrivateStackStore {
            offset,
            width,
            value,
        } => writeln!(
            output,
            "{prefix}stack.store{width}({offset:+#x}, {});",
            pseudo_value(value)
        )
        .unwrap(),
        DraftReferenceEvent::ReviewedExternalCall {
            token,
            site,
            candidates,
            arguments,
        } => {
            let model = match candidates.as_slice() {
                [candidate] => candidate.execution_model.as_ref(),
                _ => None,
            };
            writeln!(
                output,
                "{prefix}let external{token} = reviewed_abi.{}({}); // site {site:#010x}; model={}; {}",
                pseudo_identifier(
                    &candidates
                        .iter()
                        .map(|candidate| candidate.name.as_str())
                        .collect::<Vec<_>>()
                        .join("_or_")
                ),
                pseudo_arguments(arguments),
                model.map_or("none", |model| model.id.as_str()),
                candidates
                    .iter()
                    .map(|candidate| format!(
                        "{}::{}({}) -> {}",
                        candidate.contract,
                        candidate.name,
                        candidate.argument_types.join(", "),
                        candidate.return_type
                    ))
                    .collect::<Vec<_>>()
                    .join(" | ")
            )
            .unwrap();
        }
        DraftReferenceEvent::ModeledDirectCall {
            token,
            site,
            function,
            arguments,
        } => writeln!(
            output,
            "{prefix}let external_result{token} = platform.{}({}); // site {site:#010x}; operation={}; return={}; model={}; replacement={} ",
            pseudo_identifier(&function.name),
            pseudo_arguments(arguments),
            function.operation,
            function.return_type,
            external_return_model(function.return_model),
            function.replacement_hint.as_deref().unwrap_or("none"),
        )
        .unwrap(),
        DraftReferenceEvent::DiagnosticCall {
            function,
            arguments,
            ..
        } => writeln!(
            output,
            "{prefix}diagnostic.{function}({});",
            pseudo_arguments(arguments)
        )
        .unwrap(),
        DraftReferenceEvent::Call {
            token,
            target,
            arguments,
            ..
        } => writeln!(
            output,
            "{prefix}let call{token} = sub_{target:08x}({});",
            pseudo_arguments(arguments)
        )
        .unwrap(),
        DraftReferenceEvent::TailCall {
            target, arguments, ..
        } => writeln!(
            output,
            "{prefix}return sub_{target:08x}({}); // tail call",
            pseudo_arguments(arguments)
        )
        .unwrap(),
        DraftReferenceEvent::ComposedCall {
            token,
            symbol,
            arguments,
            result_modeled,
            ..
        } => {
            let callee = pseudo_identifier(symbol);
            if *result_modeled {
                writeln!(
                    output,
                    "{prefix}let call{token} = {callee}({});",
                    pseudo_arguments(arguments)
                )
                .unwrap();
            } else {
                writeln!(
                    output,
                    "{prefix}{callee}({}); // return value not modeled",
                    pseudo_arguments(arguments)
                )
                .unwrap();
            }
        }
        DraftReferenceEvent::ScratchCall {
            token,
            target,
            arguments,
            scratch_argument,
            scratch_size,
            ..
        } => writeln!(
            output,
            "{prefix}let call{token} = sub_{target:08x}_with_scratch(arg={scratch_argument}, size={scratch_size}, [{}]);",
            pseudo_arguments(arguments)
        )
        .unwrap(),
        DraftReferenceEvent::ComposedCallWithScratch {
            token,
            symbol,
            arguments,
            result_modeled,
            scratch_argument,
            scratch_size,
            ..
        } => writeln!(
            output,
            "{prefix}{}{}({}); // scratch arg={scratch_argument} size={scratch_size}, result-modeled={result_modeled}",
            if *result_modeled {
                format!("let call{token} = ")
            } else {
                String::new()
            },
            pseudo_identifier(symbol),
            pseudo_arguments(arguments)
        )
        .unwrap(),
        DraftReferenceEvent::WideSignedDivide {
            token,
            dividend_low,
            dividend_high,
            divisor_low,
            divisor_high,
        } => writeln!(
            output,
            "{prefix}let wide_div{token} = sdiv64(low={}, high={}, divisor_low={}, divisor_high={});",
            pseudo_value(dividend_low),
            pseudo_value(dividend_high),
            pseudo_value(divisor_low),
            pseudo_value(divisor_high)
        )
        .unwrap(),
        DraftReferenceEvent::BranchDecision { condition, taken } => writeln!(
            output,
            "{prefix}// forced branch at {:#010x}: {} => {taken}",
            condition.site,
            branch_expression(condition)
        )
        .unwrap(),
    }
}

fn render_flow(
    flow: &DraftReferenceFlow,
    output: &mut String,
    level: usize,
    mut state: RenderState,
) {
    for event in &flow.events {
        render_event(event, output, level, &mut state);
    }
    let prefix = indent(level);
    match &flow.terminator {
        DraftReferenceTerminator::Return(value) => {
            writeln!(output, "{prefix}return {};", pseudo_value(value)).unwrap();
        }
        DraftReferenceTerminator::Branch {
            condition,
            taken,
            not_taken,
        } => {
            writeln!(
                output,
                "{prefix}if {} {{ // site {:#010x}",
                branch_expression(condition),
                condition.site
            )
            .unwrap();
            render_flow(taken, output, level + 1, state.clone());
            writeln!(output, "{prefix}}} else {{").unwrap();
            render_flow(not_taken, output, level + 1, state);
            writeln!(output, "{prefix}}}").unwrap();
        }
    }
}

pub(super) fn render_pseudo(
    identity: &str,
    trace: &FunctionAnalysis,
    calls: &[LinkedCall],
    direct_blockers: &[LinkedDiagnostic],
    reference_blockers: &[LinkedDiagnostic],
    call_graph_blockers: &[LinkedDiagnostic],
) -> String {
    let mut output = String::new();
    writeln!(output, "// vendor symbol: {identity}").unwrap();
    for blocker in direct_blockers {
        writeln!(
            output,
            "// DIRECT-BLOCKER [{}]: {}",
            blocker.kind, blocker.rendered
        )
        .unwrap();
    }
    for blocker in reference_blockers {
        writeln!(
            output,
            "// REFERENCE-BLOCKER [{}]: {}",
            blocker.kind, blocker.rendered
        )
        .unwrap();
    }
    for blocker in call_graph_blockers {
        writeln!(
            output,
            "// CALL-GRAPH-BLOCKER [{}]: {}",
            blocker.kind, blocker.rendered
        )
        .unwrap();
    }
    for call in calls {
        let site = call
            .site
            .map_or_else(|| "unknown-site".to_owned(), |site| format!("{site:#010x}"));
        let semantic = call.semantic_operation.as_deref().unwrap_or("-");
        let contract = call
            .semantic_contract
            .as_ref()
            .map_or("-", |contract| contract.id.as_str());
        let guard_paths = call
            .guard_paths
            .as_ref()
            .map_or_else(|| "unknown".to_owned(), |paths| paths.len().to_string());
        writeln!(
            output,
            "// DIRECT-CALL {site}: {} {}{} [argument-shapes={}] [cfg-guard-paths={guard_paths}] [semantic={semantic}] [contract={contract}]",
            call.kind,
            call.target,
            if call.tail { " [tail]" } else { "" },
            call.argument_shapes,
        )
        .unwrap();
    }
    writeln!(
        output,
        "fn {}(args: [u32; 16]) -> u32 {{",
        pseudo_identifier(identity)
    )
    .unwrap();
    writeln!(
        output,
        "    // argN denotes args[N]; ctxN denotes memory rooted at pointer argument N."
    )
    .unwrap();
    if let Some(flow) = trace.reference_flow.as_ref() {
        render_flow(flow, &mut output, 1, RenderState::default());
    } else {
        let mut state = RenderState::default();
        for event in &trace.reference_events {
            render_event(event, &mut output, 1, &mut state);
        }
        if trace.unresolved_branch.is_some() {
            writeln!(
                output,
                "    // control flow continues beyond the recovered prefix"
            )
            .unwrap();
        }
        writeln!(output, "    return {};", pseudo_value(&trace.return_value)).unwrap();
    }
    output.push_str("}\n");
    output
}
