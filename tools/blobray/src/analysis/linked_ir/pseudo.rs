//! Pseudo-Rust value and flow rendering for manual linked-IR analysis.

use std::{collections::BTreeMap, fmt::Write as _, sync::Arc};

use open_radio_vendor_analysis_model::{
    FloatingPointOperation, MemoryObjectLocation, MemoryObjectRoot,
};

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
            Self::ExternalResult(token) => {
                format!("external{}", external_result_call_token(*token))
            }
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

fn pseudo_partial_bits(bits: &[BitSource; 32]) -> String {
    let mut known_mask = 0_u32;
    let mut known_value = 0_u32;
    let mut dynamic_bits = 0_u8;
    let mut unknown_bits = 0_u8;
    for (bit, source) in bits.iter().enumerate() {
        match source {
            BitSource::Constant(value) => {
                known_mask |= 1 << bit;
                if *value {
                    known_value |= 1 << bit;
                }
            }
            BitSource::Unknown => unknown_bits += 1,
            _ => dynamic_bits += 1,
        }
    }
    format!(
        "bit_value(known_mask={known_mask:#010x}, known_value={known_value:#010x}, dynamic_bits={dynamic_bits}, unknown_bits={unknown_bits})"
    )
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
        SymbolicValue::ExternalResult(token) => {
            format!("external{}", external_result_call_token(*token))
        }
        SymbolicValue::ExternalResultHigh(token) => format!("external{token}_high"),
        SymbolicValue::ExternalOutput {
            call_token,
            output_index,
        } => format!("external{call_token}_output{output_index}"),
        SymbolicValue::Expression {
            operation,
            left,
            right,
            ..
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
                ExpressionOperation::CountLeadingZeros => format!("{left}.leading_zeros()"),
                ExpressionOperation::CountTrailingZeros => format!("{left}.trailing_zeros()"),
                ExpressionOperation::PopulationCount => format!("{left}.count_ones()"),
            }
        }
        SymbolicValue::FloatingPoint {
            operation,
            rounding,
            operands,
        } => {
            let operands = operands
                .iter()
                .map(pseudo_value)
                .collect::<Vec<_>>()
                .join(", ");
            let operation = match operation {
                FloatingPointOperation::SignedWordToSingle => "f32_from_i32_bits",
                FloatingPointOperation::SubtractSingle => "f32_sub_bits",
                FloatingPointOperation::DivideSingle => "f32_div_bits",
                FloatingPointOperation::FusedMultiplyAddSingle => "f32_fma_bits",
                FloatingPointOperation::SingleToSignedWord => "i32_from_f32_bits",
            };
            format!("{operation}({operands}, rounding={rounding:?})")
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
            pseudo_masked_bits(bits).unwrap_or_else(|| pseudo_partial_bits(bits))
        }
        SymbolicValue::ReviewedExternalTable(_) => "reviewed_external_table".to_owned(),
        SymbolicValue::ReviewedExternalFunction { .. } => "reviewed_external_function".to_owned(),
        SymbolicValue::FunctionTable(_) => "function_table".to_owned(),
        SymbolicValue::FunctionPointer { .. } => "function_pointer".to_owned(),
    }
}

pub(super) fn pseudo_arguments(arguments: &[SymbolicValue]) -> String {
    fn is_unknown(value: &SymbolicValue) -> bool {
        matches!(value, SymbolicValue::Unknown)
            || matches!(value, SymbolicValue::Bits(bits) if bits.iter().all(|bit| matches!(bit, BitSource::Unknown)))
    }

    let mut rendered = Vec::new();
    let mut index = 0;
    while index < arguments.len() {
        let exact_end = (index + 1..=arguments.len())
            .take_while(|end| arguments[*end - 1].direct_input_index() == Some((*end - 1) as u8))
            .last()
            .unwrap_or(index);
        if exact_end.saturating_sub(index) >= 4 {
            rendered.push(format!("abi_inputs[{index}..{exact_end}]"));
            index = exact_end;
            continue;
        }

        let unknown_end = arguments[index..]
            .iter()
            .take_while(|argument| is_unknown(argument))
            .count()
            + index;
        if unknown_end.saturating_sub(index) >= 4 {
            rendered.push(format!("unknown_abi_inputs[{index}..{unknown_end}]"));
            index = unknown_end;
            continue;
        }

        rendered.push(pseudo_value(&arguments[index]));
        index += 1;
    }
    rendered.join(", ")
}

#[derive(Clone, Default)]
pub(super) struct RenderState {
    mmio_reads: u32,
    memory_reads: u32,
    data_symbols: Arc<Vec<artifact::ArtifactDataSymbolDefinition>>,
    call_names: Arc<BTreeMap<u32, Option<String>>>,
}

impl RenderState {
    pub(super) fn with_context(resolver: Option<&ReferenceResolver>, calls: &[LinkedCall]) -> Self {
        let mut call_names = BTreeMap::new();
        for call in calls.iter().filter(|call| {
            !call.kind.contains("unresolved")
                && !call.kind.contains("ambiguous")
                && !call.target.contains(" | ")
        }) {
            let Some(site) = call.site else {
                continue;
            };
            match call_names.entry(site) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(Some(
                        call.semantic_operation
                            .clone()
                            .unwrap_or_else(|| call.target.clone()),
                    ));
                }
                std::collections::btree_map::Entry::Occupied(mut entry)
                    if entry.get().as_deref()
                        != Some(
                            call.semantic_operation
                                .as_deref()
                                .unwrap_or(call.target.as_str()),
                        ) =>
                {
                    entry.insert(None);
                }
                std::collections::btree_map::Entry::Occupied(_) => {}
            }
        }
        Self {
            data_symbols: Arc::new(
                resolver
                    .map(|resolver| resolver.data_symbols.clone())
                    .unwrap_or_default(),
            ),
            call_names: Arc::new(call_names),
            ..Self::default()
        }
    }

    fn call_name(&self, site: u32, target: u32) -> String {
        self.call_names
            .get(&site)
            .and_then(|name| name.as_deref())
            .map(pseudo_identifier)
            .unwrap_or_else(|| format!("unresolved_call_{target:08x}"))
    }

    fn indexed_global(
        &self,
        address: &SymbolicValue,
        width: u8,
    ) -> Option<(u8, i64, &artifact::ArtifactDataSymbolDefinition, u32)> {
        let affine = address.memory_object_location_with_reads(&BTreeMap::new());
        let (argument, stride, address) = match affine {
            Some(MemoryObjectLocation {
                root:
                    MemoryObjectRoot::Indexed {
                        root,
                        argument,
                        stride,
                    },
                offset,
            }) => {
                let MemoryObjectRoot::Absolute { address } = root.as_ref() else {
                    return None;
                };
                let address = i64::from(*address).checked_add(offset)?;
                (argument, stride, u32::try_from(address).ok()?)
            }
            _ => {
                // Keep accepting the historical `arg + linked-address` form.
                // It predates explicit indexed-object provenance in the IR.
                let (argument, offset) = address.caller_memory_location()?;
                (argument, 1, u32::try_from(offset).ok()?)
            }
        };
        let bytes = u32::from(width.checked_div(8)?);
        let end = address.checked_add(bytes)?;
        let symbol = self.data_symbols.iter().find(|symbol| {
            address >= symbol.address && end <= symbol.address.saturating_add(symbol.size)
        })?;
        Some((argument, stride, symbol, address - symbol.address))
    }
}

fn indexed_global_element(argument: u8, stride: i64, offset: u32) -> String {
    if stride == 1 {
        format!("arg{argument} + {offset:#x}")
    } else {
        format!("arg{argument} * {stride:#x} + {offset:#x}")
    }
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
                    "{prefix}assert!({} <= {});",
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
                if let Some((argument, stride, symbol, offset)) =
                    state.indexed_global(address, *width)
                {
                    let symbol = symbol.member.as_deref().map_or_else(
                        || symbol.name.clone(),
                        |member| format!("{member}::{}", symbol.name),
                    );
                    let element = indexed_global_element(argument, stride, offset);
                    writeln!(
                        output,
                        "{prefix}let ramread{} = {symbol}[{element}].read{width}(); // {region}",
                        state.memory_reads,
                    )
                    .unwrap();
                } else if let Some((argument, offset)) = address.caller_memory_location() {
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
                if let Some((argument, stride, symbol, offset)) =
                    state.indexed_global(address, *width)
                {
                    let symbol = symbol.member.as_deref().map_or_else(
                        || symbol.name.clone(),
                        |member| format!("{member}::{}", symbol.name),
                    );
                    let element = indexed_global_element(argument, stride, offset);
                    writeln!(
                        output,
                        "{prefix}{symbol}[{element}].write{width}({value}); // {region}"
                    )
                    .unwrap();
                } else if let Some((argument, offset)) = address.caller_memory_location() {
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
            site,
            function,
            arguments,
            ..
        } => writeln!(
            output,
            "{prefix}diagnostic.{function}({}); // site {site:#010x}",
            pseudo_arguments(arguments),
        )
        .unwrap(),
        DraftReferenceEvent::Call {
            token,
            site,
            target,
            arguments,
            ..
        } => {
            let callee = state.call_name(*site, *target);
            writeln!(
                output,
                "{prefix}let call{token} = {callee}({});",
                pseudo_arguments(arguments)
            )
            .unwrap();
        }
        DraftReferenceEvent::TailCall {
            site,
            target,
            arguments,
            ..
        } => {
            let callee = state.call_name(*site, *target);
            writeln!(
                output,
                "{prefix}return {callee}({}); // tail call",
                pseudo_arguments(arguments)
            )
            .unwrap();
        }
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
            site,
            target,
            arguments,
            scratch_argument,
            scratch_size,
            ..
        } => {
            let callee = state.call_name(*site, *target);
            writeln!(
                output,
                "{prefix}let call{token} = {callee}_with_scratch(arg={scratch_argument}, size={scratch_size}, [{}]);",
                pseudo_arguments(arguments)
            )
            .unwrap();
        }
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
    if matches!(
        flow.events.last(),
        Some(DraftReferenceEvent::TailCall { .. })
    ) {
        return;
    }
    let prefix = indent(level);
    match &flow.terminator {
        DraftReferenceTerminator::Return(value) => {
            writeln!(output, "{prefix}return {};", pseudo_value(value)).unwrap();
        }
        DraftReferenceTerminator::FailStop {
            site,
            function,
            argument_count,
            arguments,
        } => {
            let arguments = arguments
                .iter()
                .take(usize::from(*argument_count))
                .map(pseudo_value)
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(
                output,
                "{prefix}fail_stop {function}({arguments}); // site {site:#010x}"
            )
            .unwrap();
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
    resolver: Option<&ReferenceResolver>,
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
        render_flow(
            flow,
            &mut output,
            1,
            RenderState::with_context(resolver, calls),
        );
    } else {
        let mut state = RenderState::with_context(resolver, calls);
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
        if !matches!(
            trace.reference_events.last(),
            Some(DraftReferenceEvent::TailCall { .. })
        ) {
            writeln!(output, "    return {};", pseudo_value(&trace.return_value)).unwrap();
        }
    }
    output.push_str("}\n");
    output
}

/// Replace a bounded-unrolling presentation with a compact CFG-derived loop
/// outline. The full instruction stream, blocks, calls and effects remain
/// stored independently; this function changes presentation only.
pub(super) fn render_structural_loop_pseudo(
    identity: &str,
    body: &artifact::FunctionBody,
    calls: &[LinkedCall],
    effects: &[LinkedInstructionEffect],
) -> Option<String> {
    if body.loops.is_empty() {
        return None;
    }

    fn block_for_site(body: &artifact::FunctionBody, site: u32) -> Option<usize> {
        let offset = u64::from(site).checked_sub(body.address)?;
        body.basic_blocks
            .iter()
            .find(|block| offset >= block.start_offset && offset < block.end_offset)
            .map(|block| block.id)
    }

    fn deepest_region_for_block(regions: &[artifact::FunctionLoop], block: usize) -> Option<usize> {
        regions
            .iter()
            .filter(|region| region.body_blocks.contains(&block))
            .max_by_key(|region| region.depth)
            .map(|region| region.id)
    }

    fn structural_floating_instruction(
        instruction: &artifact::FunctionInstruction,
    ) -> Option<artifact::UnsupportedInstruction> {
        if instruction.blocker_class.as_deref() != Some("floating-point") {
            return None;
        }
        Some(artifact::UnsupportedInstruction {
            address: instruction.address,
            width: instruction.width,
            raw: u32::from_str_radix(instruction.raw.trim_start_matches("0x"), 16).ok()?,
            class: artifact::UnsupportedInstructionClass::FloatingPoint,
            integer_destination: None,
            linear_control_flow: true,
        })
    }

    fn render_region(
        region: &artifact::FunctionLoop,
        regions: &[artifact::FunctionLoop],
        calls_by_region: &BTreeMap<usize, Vec<&LinkedCall>>,
        effects_by_region: &BTreeMap<usize, Vec<&LinkedInstructionEffect>>,
        blockers_by_region: &BTreeMap<usize, Vec<&artifact::FunctionInstruction>>,
        output: &mut String,
        level: usize,
    ) {
        let prefix = indent(level);
        match (region.kind, region.counted.as_ref()) {
            (artifact::FunctionLoopKind::Natural, Some(counted)) if counted.step > 0 => {
                writeln!(
                    output,
                    "{prefix}for {} in ({}..{}).step_by({}) {{ // {} iterations; structural candidate, not execution proof",
                    counted.induction_register,
                    counted.initial,
                    counted.bound,
                    counted.step,
                    counted.trip_count,
                )
                .unwrap();
            }
            (artifact::FunctionLoopKind::Natural, Some(counted)) => {
                writeln!(
                    output,
                    "{prefix}counted_loop({}, initial={}, bound={}, step={}) {{ // {} iterations; structural candidate, not execution proof",
                    counted.induction_register,
                    counted.initial,
                    counted.bound,
                    counted.step,
                    counted.trip_count,
                )
                .unwrap();
            }
            (artifact::FunctionLoopKind::Natural, None) => {
                writeln!(
                    output,
                    "{prefix}loop_at(bb{}) {{ // trip count unknown",
                    region.header_block.expect("natural loop has a header")
                )
                .unwrap();
            }
            (artifact::FunctionLoopKind::Irreducible, _) => {
                writeln!(
                    output,
                    "{prefix}irreducible_cfg_region({:?}) {{ // multiple entry blocks; no structured-loop claim",
                    region.body_blocks
                )
                .unwrap();
            }
        }
        writeln!(
            output,
            "{prefix}    // loop{} header={:?} latches={:?} body={:?} exits={:?}",
            region.id,
            region.header_block,
            region.latch_blocks,
            region.body_blocks,
            region.exit_blocks,
        )
        .unwrap();

        for call in calls_by_region.get(&region.id).into_iter().flatten() {
            let site = call
                .site
                .map_or_else(|| "unknown".to_owned(), |site| format!("{site:#010x}"));
            let operation = call
                .semantic_operation
                .as_deref()
                .unwrap_or("internal-code");
            writeln!(
                output,
                "{prefix}    call {}(); // site={site}, kind={}, operation={operation}, argument-shapes={}",
                pseudo_identifier(&call.target),
                call.kind,
                call.argument_shapes,
            )
            .unwrap();
        }
        if let Some(effects) = effects_by_region.get(&region.id) {
            let mmio = effects
                .iter()
                .filter(|effect| matches!(effect, LinkedInstructionEffect::Mmio { .. }))
                .count();
            let memory = effects.len() - mmio;
            writeln!(
                output,
                "{prefix}    loop_effects(mmio={mmio}, memory={memory}); // exact effects remain in linked IR"
            )
            .unwrap();
        }
        for instruction in blockers_by_region.get(&region.id).into_iter().flatten() {
            let floating = structural_floating_instruction(instruction);
            if let Some(decoded) = floating.and_then(artifact::decode_floating_data_instruction) {
                writeln!(
                    output,
                    "{prefix}    floating_value_flow(site={:#010x}, operation={:?}, rounding={:?}); // structural bits only; no executable FP claim",
                    instruction.address,
                    decoded.operation,
                    decoded.rounding,
                )
                .unwrap();
            } else if let Some(decoded) =
                floating.and_then(artifact::decode_floating_memory_instruction)
            {
                writeln!(
                    output,
                    "{prefix}    floating_memory(site={:#010x}, access={:?}, width=32, register=f{}); // structural raw-bit transfer",
                    instruction.address,
                    decoded.access,
                    decoded.floating_register,
                )
                .unwrap();
            } else {
                writeln!(
                    output,
                    "{prefix}    unknown_instruction(site={:#010x}, raw={}, class={});",
                    instruction.address,
                    instruction.raw,
                    instruction.blocker_class.as_deref().unwrap_or("unknown"),
                )
                .unwrap();
            }
        }
        for child in regions
            .iter()
            .filter(|candidate| candidate.parent == Some(region.id))
        {
            render_region(
                child,
                regions,
                calls_by_region,
                effects_by_region,
                blockers_by_region,
                output,
                level + 1,
            );
        }
        writeln!(output, "{prefix}}}").unwrap();
    }

    let mut calls_by_region = BTreeMap::<usize, Vec<&LinkedCall>>::new();
    let mut outside_calls = Vec::new();
    for call in calls {
        let region = call
            .site
            .and_then(|site| block_for_site(body, site))
            .and_then(|block| deepest_region_for_block(&body.loops, block));
        if let Some(region) = region {
            calls_by_region.entry(region).or_default().push(call);
        } else {
            outside_calls.push(call);
        }
    }
    let mut effects_by_region = BTreeMap::<usize, Vec<&LinkedInstructionEffect>>::new();
    for effect in effects {
        let Some(block) = block_for_site(body, effect.site()) else {
            continue;
        };
        if let Some(region) = deepest_region_for_block(&body.loops, block) {
            effects_by_region.entry(region).or_default().push(effect);
        }
    }
    let mut blockers_by_region = BTreeMap::<usize, Vec<&artifact::FunctionInstruction>>::new();
    for instruction in body
        .instructions
        .iter()
        .filter(|instruction| !instruction.supported)
    {
        let Some(block) = block_for_site(body, instruction.address as u32) else {
            continue;
        };
        if let Some(region) = deepest_region_for_block(&body.loops, block) {
            blockers_by_region
                .entry(region)
                .or_default()
                .push(instruction);
        }
    }

    let mut output = String::new();
    writeln!(output, "// vendor symbol: {identity}").unwrap();
    writeln!(
        output,
        "// STRUCTURAL-LOOP-VIEW: derived from the conservative CFG; use --full for every instruction and block."
    )
    .unwrap();
    writeln!(
        output,
        "fn {}(args: [u32; 16]) -> u32 {{",
        pseudo_identifier(identity)
    )
    .unwrap();
    writeln!(
        output,
        "    // Non-loop branch ordering is not reconstructed in this compact view."
    )
    .unwrap();
    outside_calls.sort_by_key(|call| call.site);
    for call in outside_calls {
        let site = call
            .site
            .map_or_else(|| "unknown".to_owned(), |site| format!("{site:#010x}"));
        let operation = call
            .semantic_operation
            .as_deref()
            .unwrap_or("internal-code");
        writeln!(
            output,
            "    {}(); // outside recognized loops; site={site}, kind={}, operation={operation}",
            pseudo_identifier(&call.target),
            call.kind,
        )
        .unwrap();
    }
    for region in body.loops.iter().filter(|region| region.parent.is_none()) {
        render_region(
            region,
            &body.loops,
            &calls_by_region,
            &effects_by_region,
            &blockers_by_region,
            &mut output,
            1,
        );
    }
    writeln!(
        output,
        "    return unknown; // exact return and non-loop flow remain in the lossless IR"
    )
    .unwrap();
    output.push_str("}\n");
    Some(output)
}
