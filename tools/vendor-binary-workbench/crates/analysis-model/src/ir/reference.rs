//! Fully resolved reference IR accepted by Rust code generation.

mod compaction;

use std::collections::BTreeSet;

use compaction::{
    compact_bytes_to_word_memory_loops, compact_cpu_memory_transfers, terminator_uses_call_tokens,
    terminator_uses_memory_tokens, value_uses_call_tokens, value_uses_memory_tokens,
};

use super::{
    BranchCondition, DraftReferenceEvent, DraftReferenceFlow, DraftReferenceTerminator,
    ExpressionOperation, FunctionAnalysis, IndexedMmioGuard, IndexedMmioRegister, MemoryAccess,
    ObservableEvent, SymbolicValue, collect_value_inputs,
};
use open_radio_vendor_contracts::{ExternalFunctionRef, ExternalTableRef};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolvedReferenceEvent {
    Observable(ObservableEvent),
    IndexedMmio {
        access: MemoryAccess,
        width: u8,
        address: SymbolicValue,
        registers: Vec<IndexedMmioRegister>,
        guard: Option<IndexedMmioGuard>,
        value: Option<SymbolicValue>,
    },
    PollMmio {
        width: u8,
        address: SymbolicValue,
        registers: Vec<IndexedMmioRegister>,
        guard: Option<IndexedMmioGuard>,
        mask: u32,
        expected: u32,
    },
    BoundedPoll {
        maximum_attempts: u16,
        body: Box<ResolvedReferenceFlow>,
        repeat_while_mask: u32,
        repeat_while_expected: u32,
        on_exhausted: Option<Box<ResolvedReferenceEvent>>,
    },
    PollFlow {
        body: Box<ResolvedReferenceFlow>,
        exit_when_mask: u32,
        exit_when_expected: u32,
    },
    SymmetricCalibrationSearch {
        token: u32,
        attempts_per_direction: u16,
        settle_micros: u32,
        sample_shift: u8,
        sample_mask: u32,
        accepted_sample: u32,
        initial_read: Box<ResolvedReferenceFlow>,
        setup: Box<ResolvedReferenceFlow>,
        write_candidate: Box<ResolvedReferenceFlow>,
        sample: Box<ResolvedReferenceFlow>,
    },
    DelayMicros {
        micros: SymbolicValue,
    },
    Memory {
        access: MemoryAccess,
        width: u8,
        address: SymbolicValue,
        region: String,
        value: Option<SymbolicValue>,
    },
    /// Proven counted loop that reloads one 32-bit CPU-RAM word before
    /// writing each of its four little-endian bytes.
    ///
    /// This deliberately records the vendor access shape, rather than lowering
    /// it to `memcpy`: repeated reads are observable when ranges alias. The
    /// event is recovered only from complete unrolled non-MMIO memory events.
    WordToBytesMemoryLoop {
        source: SymbolicValue,
        source_region: String,
        destination: SymbolicValue,
        destination_region: String,
        length: u32,
    },
    /// Proven counted loop that calls a pure four-byte little-endian loader
    /// and writes each resulting 32-bit word to CPU RAM.
    BytesToWordMemoryLoop {
        first_call_token: u32,
        source: SymbolicValue,
        source_region: String,
        destination: SymbolicValue,
        destination_region: String,
        length: u32,
    },
    ExternalCall {
        token: u32,
        site: u32,
        table: ExternalTableRef,
        function: ExternalFunctionRef,
        arguments: Box<[SymbolicValue]>,
    },
    DiagnosticCall {
        function: String,
        argument_count: u8,
        arguments: Box<[SymbolicValue]>,
    },
    ComposedCall {
        token: u32,
        symbol: String,
        arguments: Box<[SymbolicValue]>,
        flow: Box<ResolvedReferenceFlow>,
        result_modeled: bool,
    },
    ComposedCallWithScratch {
        token: u32,
        symbol: String,
        arguments: Box<[SymbolicValue]>,
        flow: Box<ResolvedReferenceFlow>,
        result_modeled: bool,
        scratch_argument: u8,
        scratch_size: u16,
    },
    WideSignedDivide {
        token: u32,
        dividend_low: SymbolicValue,
        dividend_high: SymbolicValue,
        divisor_low: SymbolicValue,
        divisor_high: SymbolicValue,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolvedReferenceTerminator {
    Return(SymbolicValue),
    Branch {
        condition: BranchCondition,
        taken: Box<ResolvedReferenceFlow>,
        not_taken: Box<ResolvedReferenceFlow>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedReferenceFlow {
    pub events: Vec<ResolvedReferenceEvent>,
    pub terminator: ResolvedReferenceTerminator,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolvedReferenceBody {
    Linear {
        events: Vec<ResolvedReferenceEvent>,
        return_value: SymbolicValue,
    },
    Flow(ResolvedReferenceFlow),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedReferenceProgram {
    pub symbol: String,
    pub dependencies: Vec<String>,
    pub body: ResolvedReferenceBody,
    pub exit_return_modeled: bool,
}

fn collect_resolved_flow_inputs(flow: &ResolvedReferenceFlow, output: &mut BTreeSet<u8>) {
    for event in &flow.events {
        match event {
            ResolvedReferenceEvent::Observable(ObservableEvent::Memory {
                value: Some(value),
                ..
            }) => collect_value_inputs(value, output),
            ResolvedReferenceEvent::IndexedMmio {
                address,
                guard,
                value,
                ..
            } => {
                collect_value_inputs(address, output);
                if let Some(guard) = guard {
                    collect_value_inputs(&guard.selector, output);
                }
                if let Some(value) = value {
                    collect_value_inputs(value, output);
                }
            }
            ResolvedReferenceEvent::PollMmio { address, guard, .. } => {
                collect_value_inputs(address, output);
                if let Some(guard) = guard {
                    collect_value_inputs(&guard.selector, output);
                }
            }
            ResolvedReferenceEvent::BoundedPoll {
                body, on_exhausted, ..
            } => {
                collect_resolved_flow_inputs(body, output);
                if let Some(event) = on_exhausted {
                    collect_resolved_event_inputs(event, output);
                }
            }
            ResolvedReferenceEvent::PollFlow { body, .. } => {
                collect_resolved_flow_inputs(body, output);
            }
            ResolvedReferenceEvent::SymmetricCalibrationSearch {
                initial_read,
                setup,
                sample,
                ..
            } => {
                collect_resolved_flow_inputs(initial_read, output);
                collect_resolved_flow_inputs(setup, output);
                collect_resolved_flow_inputs(sample, output);
            }
            ResolvedReferenceEvent::Memory { address, value, .. } => {
                collect_value_inputs(address, output);
                if let Some(value) = value {
                    collect_value_inputs(value, output);
                }
            }
            ResolvedReferenceEvent::WordToBytesMemoryLoop {
                source,
                destination,
                ..
            }
            | ResolvedReferenceEvent::BytesToWordMemoryLoop {
                source,
                destination,
                ..
            } => {
                collect_value_inputs(source, output);
                collect_value_inputs(destination, output);
            }
            ResolvedReferenceEvent::DelayMicros { micros } => collect_value_inputs(micros, output),
            ResolvedReferenceEvent::ExternalCall {
                table: _,
                function,
                arguments,
                ..
            } => {
                let argument_count = function.spec().argument_count;
                for value in arguments.iter().take(usize::from(argument_count)) {
                    collect_value_inputs(value, output);
                }
            }
            ResolvedReferenceEvent::DiagnosticCall {
                argument_count,
                arguments,
                ..
            } => {
                for value in arguments.iter().take(usize::from(*argument_count)) {
                    collect_value_inputs(value, output);
                }
            }
            ResolvedReferenceEvent::ComposedCall {
                arguments, flow, ..
            } => {
                for index in resolved_reference_flow_input_indices(flow) {
                    collect_value_inputs(&arguments[usize::from(index)], output);
                }
            }
            ResolvedReferenceEvent::ComposedCallWithScratch {
                arguments,
                flow,
                scratch_argument,
                ..
            } => {
                for index in resolved_reference_flow_input_indices(flow) {
                    if index != *scratch_argument {
                        collect_value_inputs(&arguments[usize::from(index)], output);
                    }
                }
            }
            ResolvedReferenceEvent::WideSignedDivide {
                dividend_low,
                dividend_high,
                divisor_low,
                divisor_high,
                ..
            } => {
                collect_value_inputs(dividend_low, output);
                collect_value_inputs(dividend_high, output);
                collect_value_inputs(divisor_low, output);
                collect_value_inputs(divisor_high, output);
            }
            ResolvedReferenceEvent::Observable(_) => {}
        }
    }
    match &flow.terminator {
        ResolvedReferenceTerminator::Return(value) => collect_value_inputs(value, output),
        ResolvedReferenceTerminator::Branch {
            condition,
            taken,
            not_taken,
        } => {
            collect_value_inputs(&condition.left, output);
            collect_value_inputs(&condition.right, output);
            collect_resolved_flow_inputs(taken, output);
            collect_resolved_flow_inputs(not_taken, output);
        }
    }
}

fn collect_resolved_event_inputs(event: &ResolvedReferenceEvent, output: &mut BTreeSet<u8>) {
    let flow = ResolvedReferenceFlow {
        events: vec![event.clone()],
        terminator: ResolvedReferenceTerminator::Return(SymbolicValue::Unknown),
    };
    collect_resolved_flow_inputs(&flow, output);
}

pub fn resolved_reference_flow_input_indices(flow: &ResolvedReferenceFlow) -> BTreeSet<u8> {
    let mut output = BTreeSet::new();
    collect_resolved_flow_inputs(flow, &mut output);
    output
}

impl ResolvedReferenceEvent {
    fn from_draft(event: &DraftReferenceEvent) -> Result<Self, String> {
        Ok(match event {
            DraftReferenceEvent::Observable(event) => Self::Observable(event.clone()),
            DraftReferenceEvent::IndexedMmio {
                access,
                width,
                address,
                registers,
                guard,
                value,
            } => Self::IndexedMmio {
                access: *access,
                width: *width,
                address: address.clone(),
                registers: registers.clone(),
                guard: guard.clone(),
                value: value.clone(),
            },
            DraftReferenceEvent::PollMmio {
                width,
                address,
                registers,
                guard,
                mask,
                expected,
            } => Self::PollMmio {
                width: *width,
                address: address.clone(),
                registers: registers.clone(),
                guard: guard.clone(),
                mask: *mask,
                expected: *expected,
            },
            DraftReferenceEvent::BoundedPoll {
                maximum_attempts,
                body,
                repeat_while_mask,
                repeat_while_expected,
                on_exhausted,
            } => Self::BoundedPoll {
                maximum_attempts: *maximum_attempts,
                body: Box::new(ResolvedReferenceFlow::from_draft(body)?),
                repeat_while_mask: *repeat_while_mask,
                repeat_while_expected: *repeat_while_expected,
                on_exhausted: on_exhausted
                    .as_deref()
                    .map(Self::from_draft)
                    .transpose()?
                    .map(Box::new),
            },
            DraftReferenceEvent::PollFlow {
                body,
                exit_when_mask,
                exit_when_expected,
            } => Self::PollFlow {
                body: Box::new(ResolvedReferenceFlow::from_draft(body)?),
                exit_when_mask: *exit_when_mask,
                exit_when_expected: *exit_when_expected,
            },
            DraftReferenceEvent::SymmetricCalibrationSearch {
                token,
                attempts_per_direction,
                settle_micros,
                sample_shift,
                sample_mask,
                accepted_sample,
                initial_read,
                setup,
                write_candidate,
                sample,
            } => Self::SymmetricCalibrationSearch {
                token: *token,
                attempts_per_direction: *attempts_per_direction,
                settle_micros: *settle_micros,
                sample_shift: *sample_shift,
                sample_mask: *sample_mask,
                accepted_sample: *accepted_sample,
                initial_read: Box::new(ResolvedReferenceFlow::from_draft(initial_read)?),
                setup: Box::new(ResolvedReferenceFlow::from_draft(setup)?),
                write_candidate: Box::new(ResolvedReferenceFlow::from_draft(write_candidate)?),
                sample: Box::new(ResolvedReferenceFlow::from_draft(sample)?),
            },
            DraftReferenceEvent::DelayMicros { micros } => Self::DelayMicros {
                micros: micros.clone(),
            },
            DraftReferenceEvent::Memory {
                access,
                width,
                address,
                region,
                value,
            } => Self::Memory {
                access: *access,
                width: *width,
                address: address.clone(),
                region: region.clone(),
                value: value.clone(),
            },
            DraftReferenceEvent::PrivateStackLoad { token, .. } => {
                return Err(format!(
                    "private-stack read token {token} escaped reference composition"
                ));
            }
            DraftReferenceEvent::PrivateStackStore { offset, .. } => {
                return Err(format!(
                    "private-stack store at {offset:+#x} escaped reference composition"
                ));
            }
            DraftReferenceEvent::ExternalCall {
                token,
                site,
                table,
                function,
                arguments,
            } => Self::ExternalCall {
                token: *token,
                site: *site,
                table: *table,
                function: *function,
                arguments: arguments.clone(),
            },
            DraftReferenceEvent::DiagnosticCall {
                function,
                argument_count,
                arguments,
            } => Self::DiagnosticCall {
                function: function.clone(),
                argument_count: *argument_count,
                arguments: arguments.clone(),
            },
            DraftReferenceEvent::ComposedCall {
                token,
                symbol,
                arguments,
                flow,
                result_modeled,
            } => Self::ComposedCall {
                token: *token,
                symbol: symbol.clone(),
                arguments: arguments.clone(),
                flow: Box::new(ResolvedReferenceFlow::from_draft(flow)?),
                result_modeled: *result_modeled,
            },
            DraftReferenceEvent::ComposedCallWithScratch {
                token,
                symbol,
                arguments,
                flow,
                result_modeled,
                scratch_argument,
                scratch_size,
            } => Self::ComposedCallWithScratch {
                token: *token,
                symbol: symbol.clone(),
                arguments: arguments.clone(),
                flow: Box::new(ResolvedReferenceFlow::from_draft(flow)?),
                result_modeled: *result_modeled,
                scratch_argument: *scratch_argument,
                scratch_size: *scratch_size,
            },
            DraftReferenceEvent::WideSignedDivide {
                token,
                dividend_low,
                dividend_high,
                divisor_low,
                divisor_high,
            } => Self::WideSignedDivide {
                token: *token,
                dividend_low: dividend_low.clone(),
                dividend_high: dividend_high.clone(),
                divisor_low: divisor_low.clone(),
                divisor_high: divisor_high.clone(),
            },
            DraftReferenceEvent::Call { site, target, .. } => {
                return Err(format!(
                    "unresolved call at {site:#010x} to {target:#010x} escaped reference resolution"
                ));
            }
            DraftReferenceEvent::TailCall { site, target, .. } => {
                return Err(format!(
                    "unresolved tail call at {site:#010x} to {target:#010x} escaped reference resolution"
                ));
            }
            DraftReferenceEvent::ScratchCall { site, target, .. } => {
                return Err(format!(
                    "unresolved scratch call at {site:#010x} to {target:#010x} escaped reference resolution"
                ));
            }
            DraftReferenceEvent::BranchDecision { condition, .. } => {
                return Err(format!(
                    "branch decision at {:#010x} escaped structured reference flow",
                    condition.site
                ));
            }
        })
    }
}

impl ResolvedReferenceFlow {
    fn from_draft(flow: &DraftReferenceFlow) -> Result<Self, String> {
        let events = flow
            .events
            .iter()
            .map(ResolvedReferenceEvent::from_draft)
            .collect::<Result<Vec<_>, _>>()?;
        let terminator = match &flow.terminator {
            DraftReferenceTerminator::Return(value) => {
                ResolvedReferenceTerminator::Return(value.clone())
            }
            DraftReferenceTerminator::Branch {
                condition,
                taken,
                not_taken,
            } => ResolvedReferenceTerminator::Branch {
                condition: condition.clone(),
                taken: Box::new(Self::from_draft(taken)?),
                not_taken: Box::new(Self::from_draft(not_taken)?),
            },
        };
        let events = compact_cpu_memory_transfers(events, |start, end| {
            terminator_uses_memory_tokens(&terminator, start, end)
        });
        let events = compact_bytes_to_word_memory_loops(events, |start, end| {
            terminator_uses_call_tokens(&terminator, start, end)
        });
        Ok(Self { events, terminator })
    }
}

impl TryFrom<&FunctionAnalysis> for ResolvedReferenceProgram {
    type Error = String;

    fn try_from(trace: &FunctionAnalysis) -> Result<Self, Self::Error> {
        if !trace.is_reference_eligible() {
            return Err(format!(
                "{} is not eligible for reference generation: {}",
                trace.symbol,
                trace.reference_failure_reasons().join("; ")
            ));
        }

        let body = if let Some(flow) = &trace.reference_flow {
            ResolvedReferenceBody::Flow(ResolvedReferenceFlow::from_draft(flow)?)
        } else {
            ResolvedReferenceBody::Linear {
                events: {
                    let events = compact_cpu_memory_transfers(
                        trace
                            .reference_events
                            .iter()
                            .map(ResolvedReferenceEvent::from_draft)
                            .collect::<Result<Vec<_>, _>>()?,
                        |start, end| value_uses_memory_tokens(&trace.return_value, start, end),
                    );
                    compact_bytes_to_word_memory_loops(events, |start, end| {
                        value_uses_call_tokens(&trace.return_value, start, end)
                    })
                },
                return_value: trace.return_value.clone(),
            }
        };
        Ok(Self {
            symbol: trace.symbol.clone(),
            dependencies: trace
                .reference_dependencies
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            body,
            exit_return_modeled: trace.reference_exit_return_modeled(),
        })
    }
}
