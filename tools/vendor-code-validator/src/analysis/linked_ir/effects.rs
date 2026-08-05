//! Extraction of context, MMIO, and delay effects from structural traces.

use super::*;

fn context_write_masks(
    access: MemoryAccess,
    width: u8,
    value: Option<&SymbolicValue>,
) -> (Option<u32>, Option<u32>, Option<u32>, Option<u32>) {
    if access != MemoryAccess::Write {
        return (None, None, None, None);
    }
    let width_mask = width_mask(width);
    let Some(SymbolicValue::MemoryImage {
        and_mask, or_mask, ..
    }) = value
    else {
        return (Some(width_mask), None, None, None);
    };
    let forced_one = or_mask & width_mask;
    let preserved = and_mask & !forced_one & width_mask;
    let forced_zero = width_mask & !(preserved | forced_one);
    (
        Some(forced_zero | forced_one),
        Some(preserved),
        Some(forced_zero),
        Some(forced_one),
    )
}

fn collect_context_access_from_event(
    event: &DraftReferenceEvent,
    path: &str,
    output: &mut Vec<ContextAccess>,
) {
    match event {
        DraftReferenceEvent::Memory {
            access,
            width,
            address,
            value,
            ..
        } => {
            if let Some((argument, offset)) = address.caller_memory_location() {
                let (write_mask, preserved_mask, forced_zero_mask, forced_one_mask) =
                    context_write_masks(*access, *width, value.as_ref());
                output.push(ContextAccess {
                    argument,
                    offset,
                    access: match access {
                        MemoryAccess::Read => "read",
                        MemoryAccess::Write => "write",
                    },
                    width: *width,
                    path: path.to_owned(),
                    value: value.as_ref().map(SymbolicValue::canonical),
                    value_pseudo: value.as_ref().map(pseudo_value),
                    write_mask,
                    preserved_mask,
                    forced_zero_mask,
                    forced_one_mask,
                });
            }
        }
        DraftReferenceEvent::BoundedPoll {
            body, on_exhausted, ..
        } => {
            collect_context_access_from_flow(body, &nested_path(path, "bounded-poll"), output);
            if let Some(event) = on_exhausted.as_deref() {
                collect_context_access_from_event(
                    event,
                    &nested_path(path, "poll-exhausted"),
                    output,
                );
            }
        }
        DraftReferenceEvent::PollFlow { body, .. } => {
            collect_context_access_from_flow(body, &nested_path(path, "poll"), output);
        }
        DraftReferenceEvent::SymmetricCalibrationSearch {
            initial_read,
            setup,
            write_candidate,
            sample,
            ..
        } => {
            for (scope, flow) in [
                ("calibration-initial-read", initial_read),
                ("calibration-setup", setup),
                ("calibration-write-candidate", write_candidate),
                ("calibration-sample", sample),
            ] {
                collect_context_access_from_flow(flow, &nested_path(path, scope), output);
            }
        }
        DraftReferenceEvent::ComposedCall { symbol, flow, .. }
        | DraftReferenceEvent::ComposedCallWithScratch { symbol, flow, .. } => {
            collect_context_access_from_flow(
                flow,
                &nested_path(path, &format!("call {symbol}")),
                output,
            );
        }
        _ => {}
    }
}

fn collect_context_access_from_flow(
    flow: &DraftReferenceFlow,
    path: &str,
    output: &mut Vec<ContextAccess>,
) {
    for event in &flow.events {
        collect_context_access_from_event(event, path, output);
    }
    if let DraftReferenceTerminator::Branch {
        condition,
        taken,
        not_taken,
    } = &flow.terminator
    {
        let condition = branch_expression(condition);
        collect_context_access_from_flow(
            taken,
            &nested_path(path, &format!("if {condition}")),
            output,
        );
        collect_context_access_from_flow(
            not_taken,
            &nested_path(path, &format!("if !({condition})")),
            output,
        );
    }
}

pub(super) fn context_accesses_for_trace(trace: &FunctionAnalysis) -> Vec<ContextAccess> {
    let mut output = Vec::new();
    if let Some(flow) = trace.reference_flow.as_ref() {
        collect_context_access_from_flow(flow, "entry", &mut output);
    } else {
        for event in &trace.reference_events {
            collect_context_access_from_event(event, "entry", &mut output);
        }
    }
    output.sort();
    output.dedup();
    output
}

pub(super) fn context_fields_for_accesses(accesses: &[ContextAccess]) -> Vec<ContextField> {
    let mut fields = BTreeMap::<(u8, i32, u8), ContextField>::new();
    for access in accesses {
        let field = fields
            .entry((access.argument, access.offset, access.width))
            .or_insert_with(|| ContextField {
                argument: access.argument,
                offset: access.offset,
                width: access.width,
                reads: 0,
                writes: 0,
                write_mask: 0,
                paths: Vec::new(),
                write_values: Vec::new(),
            });
        match access.access {
            "read" => field.reads += 1,
            "write" => {
                field.writes += 1;
                field.write_mask |= access.write_mask.unwrap_or_default();
                if let Some(value) = access.value_pseudo.as_ref()
                    && !field.write_values.contains(value)
                {
                    field.write_values.push(value.clone());
                }
            }
            _ => unreachable!("context access has a closed access vocabulary"),
        }
        if !field.paths.contains(&access.path) {
            field.paths.push(access.path.clone());
        }
    }
    fields.into_values().collect()
}

fn mmio_write_masks(
    access: MemoryAccess,
    address: u32,
    width: u8,
    value: Option<&SymbolicValue>,
) -> [Option<u32>; 7] {
    if access != MemoryAccess::Write {
        return [None; 7];
    }
    let pattern = super::super::mmio_discovery::classify_write_bits(value, address, width);
    [
        Some(pattern.modified_mask(width)),
        Some(pattern.preserved_mask),
        Some(pattern.inverted_mask),
        Some(pattern.forced_zero_mask),
        Some(pattern.forced_one_mask),
        Some(pattern.read_derived_mask),
        Some(pattern.dynamic_mask),
    ]
}

struct MmioAccessDraft<'a> {
    address: u32,
    width: u8,
    register: &'a str,
    access: MemoryAccess,
    mode: &'static str,
    path: &'a str,
    address_expression: Option<String>,
    guard: Option<String>,
    value: Option<&'a SymbolicValue>,
}

fn push_mmio_access(output: &mut Vec<LinkedMmioAccess>, draft: MmioAccessDraft<'_>) {
    let MmioAccessDraft {
        address,
        width,
        register,
        access,
        mode,
        path,
        address_expression,
        guard,
        value,
    } = draft;
    let [
        modified_mask,
        preserved_mask,
        inverted_mask,
        forced_zero_mask,
        forced_one_mask,
        read_derived_mask,
        dynamic_mask,
    ] = mmio_write_masks(access, address, width, value);
    output.push(LinkedMmioAccess {
        ordinal: output.len(),
        address,
        width,
        register: register.to_owned(),
        access: match access {
            MemoryAccess::Read => "read",
            MemoryAccess::Write => "write",
        },
        mode,
        path: path.to_owned(),
        address_expression,
        guard,
        predicate_mask: None,
        predicate_expected: None,
        value: value.map(pseudo_value),
        modified_mask,
        preserved_mask,
        inverted_mask,
        forced_zero_mask,
        forced_one_mask,
        read_derived_mask,
        dynamic_mask,
    });
}

fn collect_mmio_access_from_event(
    event: &DraftReferenceEvent,
    path: &str,
    output: &mut Vec<LinkedMmioAccess>,
) {
    match event {
        DraftReferenceEvent::Observable(ObservableEvent::Memory {
            access,
            width,
            address,
            register,
            value,
        }) => push_mmio_access(
            output,
            MmioAccessDraft {
                address: *address,
                width: *width,
                register,
                access: *access,
                mode: "static",
                path,
                address_expression: None,
                guard: None,
                value: value.as_ref(),
            },
        ),
        DraftReferenceEvent::IndexedMmio {
            access,
            width,
            address,
            registers,
            guard,
            value,
        } => {
            let address_expression = Some(pseudo_value(address));
            let guard = guard
                .as_ref()
                .map(|guard| format!("{} < {}", pseudo_value(&guard.selector), guard.maximum));
            for register in registers {
                push_mmio_access(
                    output,
                    MmioAccessDraft {
                        address: register.address,
                        width: *width,
                        register: &register.name,
                        access: *access,
                        mode: "indexed-candidate",
                        path,
                        address_expression: address_expression.clone(),
                        guard: guard.clone(),
                        value: value.as_ref(),
                    },
                );
            }
        }
        DraftReferenceEvent::PollMmio {
            width,
            address,
            registers,
            guard,
            mask,
            expected,
        } => {
            let address_expression = Some(pseudo_value(address));
            let guard = guard.as_ref().map_or_else(
                || format!("value & {mask:#010x} == {expected:#010x}"),
                |guard| {
                    format!(
                        "{} < {}; value & {mask:#010x} == {expected:#010x}",
                        pseudo_value(&guard.selector),
                        guard.maximum
                    )
                },
            );
            for register in registers {
                let mut access = LinkedMmioAccess {
                    ordinal: output.len(),
                    address: register.address,
                    width: *width,
                    register: register.name.clone(),
                    access: "poll",
                    mode: "indexed-candidate",
                    path: path.to_owned(),
                    address_expression: address_expression.clone(),
                    guard: Some(guard.clone()),
                    predicate_mask: Some(*mask),
                    predicate_expected: Some(*expected),
                    value: None,
                    modified_mask: None,
                    preserved_mask: None,
                    inverted_mask: None,
                    forced_zero_mask: None,
                    forced_one_mask: None,
                    read_derived_mask: None,
                    dynamic_mask: None,
                };
                if registers.len() == 1 && address.as_constant() == Some(register.address) {
                    access.mode = "static";
                    access.address_expression = None;
                }
                output.push(access);
            }
        }
        DraftReferenceEvent::BoundedPoll {
            body, on_exhausted, ..
        } => {
            collect_mmio_access_from_flow(body, &nested_path(path, "bounded-poll"), output);
            if let Some(event) = on_exhausted.as_deref() {
                collect_mmio_access_from_event(event, &nested_path(path, "poll-exhausted"), output);
            }
        }
        DraftReferenceEvent::PollFlow { body, .. } => {
            collect_mmio_access_from_flow(body, &nested_path(path, "poll"), output);
        }
        DraftReferenceEvent::SymmetricCalibrationSearch {
            initial_read,
            setup,
            write_candidate,
            sample,
            ..
        } => {
            for (scope, flow) in [
                ("calibration-initial-read", initial_read),
                ("calibration-setup", setup),
                ("calibration-write-candidate", write_candidate),
                ("calibration-sample", sample),
            ] {
                collect_mmio_access_from_flow(flow, &nested_path(path, scope), output);
            }
        }
        DraftReferenceEvent::ComposedCall { symbol, flow, .. }
        | DraftReferenceEvent::ComposedCallWithScratch { symbol, flow, .. } => {
            collect_mmio_access_from_flow(
                flow,
                &nested_path(path, &format!("call {symbol}")),
                output,
            );
        }
        _ => {}
    }
}

fn collect_mmio_access_from_flow(
    flow: &DraftReferenceFlow,
    path: &str,
    output: &mut Vec<LinkedMmioAccess>,
) {
    for event in &flow.events {
        collect_mmio_access_from_event(event, path, output);
    }
    if let DraftReferenceTerminator::Branch {
        condition,
        taken,
        not_taken,
    } = &flow.terminator
    {
        let condition = branch_expression(condition);
        collect_mmio_access_from_flow(
            taken,
            &nested_path(path, &format!("if {condition}")),
            output,
        );
        collect_mmio_access_from_flow(
            not_taken,
            &nested_path(path, &format!("if !({condition})")),
            output,
        );
    }
}

pub(super) fn mmio_accesses_for_trace(trace: &FunctionAnalysis) -> Vec<LinkedMmioAccess> {
    let mut output = Vec::new();
    if let Some(flow) = trace.reference_flow.as_ref() {
        collect_mmio_access_from_flow(flow, "entry", &mut output);
    } else {
        for event in &trace.reference_events {
            collect_mmio_access_from_event(event, "entry", &mut output);
        }
    }
    output
}

fn collect_delay_from_event(
    event: &DraftReferenceEvent,
    path: &str,
    output: &mut Vec<LinkedDelay>,
) {
    match event {
        DraftReferenceEvent::DelayMicros { micros } => output.push(LinkedDelay {
            ordinal: output.len(),
            path: path.to_owned(),
            micros: micros.canonical(),
            constant_micros: micros.as_constant(),
        }),
        DraftReferenceEvent::BoundedPoll {
            body, on_exhausted, ..
        } => {
            collect_delays_from_flow(body, &nested_path(path, "bounded-poll"), output);
            if let Some(event) = on_exhausted.as_deref() {
                collect_delay_from_event(event, &nested_path(path, "poll-exhausted"), output);
            }
        }
        DraftReferenceEvent::PollFlow { body, .. } => {
            collect_delays_from_flow(body, &nested_path(path, "poll"), output);
        }
        DraftReferenceEvent::SymmetricCalibrationSearch {
            settle_micros,
            initial_read,
            setup,
            write_candidate,
            sample,
            ..
        } => {
            output.push(LinkedDelay {
                ordinal: output.len(),
                path: nested_path(path, "calibration-settle"),
                micros: SymbolicValue::Constant(*settle_micros).canonical(),
                constant_micros: Some(*settle_micros),
            });
            for (scope, flow) in [
                ("calibration-initial-read", initial_read),
                ("calibration-setup", setup),
                ("calibration-write-candidate", write_candidate),
                ("calibration-sample", sample),
            ] {
                collect_delays_from_flow(flow, &nested_path(path, scope), output);
            }
        }
        DraftReferenceEvent::ComposedCall { symbol, flow, .. }
        | DraftReferenceEvent::ComposedCallWithScratch { symbol, flow, .. } => {
            collect_delays_from_flow(flow, &nested_path(path, &format!("call {symbol}")), output);
        }
        _ => {}
    }
}

fn collect_delays_from_flow(flow: &DraftReferenceFlow, path: &str, output: &mut Vec<LinkedDelay>) {
    for event in &flow.events {
        collect_delay_from_event(event, path, output);
    }
    if let DraftReferenceTerminator::Branch {
        condition,
        taken,
        not_taken,
    } = &flow.terminator
    {
        let condition = branch_expression(condition);
        collect_delays_from_flow(
            taken,
            &nested_path(path, &format!("if {condition}")),
            output,
        );
        collect_delays_from_flow(
            not_taken,
            &nested_path(path, &format!("if !({condition})")),
            output,
        );
    }
}

pub(super) fn delays_for_trace(trace: &FunctionAnalysis) -> Vec<LinkedDelay> {
    let mut output = Vec::new();
    if let Some(flow) = trace.reference_flow.as_ref() {
        collect_delays_from_flow(flow, "entry", &mut output);
    } else {
        for event in &trace.reference_events {
            collect_delay_from_event(event, "entry", &mut output);
        }
    }
    output
}
