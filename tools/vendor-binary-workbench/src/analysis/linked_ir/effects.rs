//! Extraction of context, MMIO, and delay effects from structural traces.

use super::*;
use open_radio_vendor_analysis_model::MemoryObjectLocation;

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

fn collect_memory_object_access_from_event(
    event: &DraftReferenceEvent,
    path: &str,
    read_sources: &BTreeMap<u32, MemoryObjectLocation>,
    output: &mut Vec<MemoryObjectAccess>,
) {
    match event {
        DraftReferenceEvent::Memory {
            access,
            width,
            address,
            region,
            value,
        } => {
            if let Some(location) = address.memory_object_location_with_reads(read_sources) {
                let (write_mask, preserved_mask, forced_zero_mask, forced_one_mask) =
                    context_write_masks(*access, *width, value.as_ref());
                output.push(MemoryObjectAccess {
                    object: LinkedMemoryObject::from_root(location.root, region),
                    offset: location.offset,
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
            collect_memory_object_access_from_flow(
                body,
                &nested_path(path, "bounded-poll"),
                read_sources,
                output,
            );
            if let Some(event) = on_exhausted.as_deref() {
                collect_memory_object_access_from_event(
                    event,
                    &nested_path(path, "poll-exhausted"),
                    read_sources,
                    output,
                );
            }
        }
        DraftReferenceEvent::PollFlow { body, .. } => {
            collect_memory_object_access_from_flow(
                body,
                &nested_path(path, "poll"),
                read_sources,
                output,
            );
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
                collect_memory_object_access_from_flow(
                    flow,
                    &nested_path(path, scope),
                    read_sources,
                    output,
                );
            }
        }
        DraftReferenceEvent::ComposedCall { symbol, flow, .. }
        | DraftReferenceEvent::ComposedCallWithScratch { symbol, flow, .. } => {
            collect_memory_object_access_from_flow(
                flow,
                &nested_path(path, &format!("call {symbol}")),
                read_sources,
                output,
            );
        }
        _ => {}
    }
}

fn collect_memory_object_access_from_flow(
    flow: &DraftReferenceFlow,
    path: &str,
    read_sources: &BTreeMap<u32, MemoryObjectLocation>,
    output: &mut Vec<MemoryObjectAccess>,
) {
    for event in &flow.events {
        collect_memory_object_access_from_event(event, path, read_sources, output);
    }
    if let DraftReferenceTerminator::Branch {
        condition,
        taken,
        not_taken,
    } = &flow.terminator
    {
        let condition = branch_expression(condition);
        collect_memory_object_access_from_flow(
            taken,
            &nested_path(path, &format!("if {condition}")),
            read_sources,
            output,
        );
        collect_memory_object_access_from_flow(
            not_taken,
            &nested_path(path, &format!("if !({condition})")),
            read_sources,
            output,
        );
    }
}

pub(super) fn memory_object_accesses_for_trace(
    trace: &FunctionAnalysis,
) -> Vec<MemoryObjectAccess> {
    let read_sources = memory_read_sources_for_trace(trace);
    let mut output = Vec::new();
    if let Some(flow) = trace.reference_flow.as_ref() {
        collect_memory_object_access_from_flow(flow, "entry", &read_sources, &mut output);
    } else {
        for event in &trace.reference_events {
            collect_memory_object_access_from_event(event, "entry", &read_sources, &mut output);
        }
    }
    output.sort();
    output.dedup();
    output
}

/// Rebase statically known linked-image addresses onto the narrowest sized
/// ELF data symbol. This turns accesses such as `0x1000828c + 0xf8` into
/// reviewable `phy_param + 0xf8` evidence without inferring a nominal type.
pub(super) fn attribute_data_symbols(
    accesses: &mut [MemoryObjectAccess],
    resolver: &ReferenceResolver,
) {
    for access in accesses {
        match &access.object {
            LinkedMemoryObject::Absolute { address, .. } => {
                let Some((member, symbol, offset)) =
                    resolver.data_symbol_location(*address, access.width)
                else {
                    continue;
                };
                access.object = LinkedMemoryObject::Global {
                    member: member.map(str::to_owned),
                    symbol: symbol.to_owned(),
                };
                access.offset = access.offset.wrapping_add(offset);
            }
            // A linked image has already resolved `symbol + argument`, so the
            // structural value can look like `argument + absolute-address`.
            // Promote it only when that apparent offset is contained by a
            // sized ELF data symbol. This removes a false giant context field
            // without guessing from the numerical address alone.
            LinkedMemoryObject::Argument { index } => {
                let Ok(address) = u32::try_from(access.offset) else {
                    continue;
                };
                let Some((member, symbol, offset)) =
                    resolver.data_symbol_location(address, access.width)
                else {
                    continue;
                };
                access.object = LinkedMemoryObject::Indexed {
                    object: Box::new(LinkedMemoryObject::Global {
                        member: member.map(str::to_owned),
                        symbol: symbol.to_owned(),
                    }),
                    argument: *index,
                    stride: 1,
                };
                access.offset = offset;
            }
            _ => {}
        }
    }
}

fn memory_read_sources_for_trace(trace: &FunctionAnalysis) -> BTreeMap<u32, MemoryObjectLocation> {
    fn collect_event(
        event: &DraftReferenceEvent,
        next_token: &mut u32,
        sources: &mut BTreeMap<u32, MemoryObjectLocation>,
    ) {
        match event {
            DraftReferenceEvent::Memory {
                access: MemoryAccess::Read,
                address,
                ..
            } => {
                let token = *next_token;
                *next_token = (*next_token).wrapping_add(1);
                if let Some(location) = address.memory_object_location_with_reads(sources) {
                    sources.insert(token, location);
                }
            }
            DraftReferenceEvent::BoundedPoll {
                body, on_exhausted, ..
            } => {
                collect_flow(body, next_token, sources);
                if let Some(event) = on_exhausted.as_deref() {
                    collect_event(event, next_token, sources);
                }
            }
            DraftReferenceEvent::PollFlow { body, .. } => {
                collect_flow(body, next_token, sources);
            }
            DraftReferenceEvent::SymmetricCalibrationSearch {
                initial_read,
                setup,
                write_candidate,
                sample,
                ..
            } => {
                for flow in [initial_read, setup, write_candidate, sample] {
                    collect_flow(flow, next_token, sources);
                }
            }
            DraftReferenceEvent::ComposedCall { flow, .. }
            | DraftReferenceEvent::ComposedCallWithScratch { flow, .. } => {
                collect_flow(flow, next_token, sources);
            }
            _ => {}
        }
    }

    fn collect_flow(
        flow: &DraftReferenceFlow,
        next_token: &mut u32,
        sources: &mut BTreeMap<u32, MemoryObjectLocation>,
    ) {
        for event in &flow.events {
            collect_event(event, next_token, sources);
        }
        if let DraftReferenceTerminator::Branch {
            taken, not_taken, ..
        } = &flow.terminator
        {
            collect_flow(taken, next_token, sources);
            collect_flow(not_taken, next_token, sources);
        }
    }

    let mut sources = BTreeMap::new();
    let mut next_token = 0;
    if let Some(flow) = trace.reference_flow.as_ref() {
        collect_flow(flow, &mut next_token, &mut sources);
    } else {
        for event in &trace.reference_events {
            collect_event(event, &mut next_token, &mut sources);
        }
    }
    sources
}

pub(super) fn memory_object_fields_for_accesses(
    accesses: &[MemoryObjectAccess],
) -> Vec<MemoryObjectField> {
    let mut fields = BTreeMap::<(LinkedMemoryObject, i64, u8), MemoryObjectField>::new();
    for access in accesses {
        let field = fields
            .entry((access.object.clone(), access.offset, access.width))
            .or_insert_with(|| MemoryObjectField {
                object: access.object.clone(),
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
            _ => unreachable!("memory-object access has a closed access vocabulary"),
        }
        if !field.paths.contains(&access.path) {
            field.paths.push(access.path.clone());
        }
    }
    fields.into_values().collect()
}

pub(super) fn context_accesses_for_memory_objects(
    accesses: &[MemoryObjectAccess],
) -> Vec<ContextAccess> {
    accesses
        .iter()
        .filter_map(|access| {
            let LinkedMemoryObject::Argument { index } = access.object else {
                return None;
            };
            Some(ContextAccess {
                argument: index,
                offset: access.offset.try_into().ok()?,
                access: access.access,
                width: access.width,
                path: access.path.clone(),
                value: access.value.clone(),
                value_pseudo: access.value_pseudo.clone(),
                write_mask: access.write_mask,
                preserved_mask: access.preserved_mask,
                forced_zero_mask: access.forced_zero_mask,
                forced_one_mask: access.forced_one_mask,
            })
        })
        .collect()
}

#[cfg(test)]
pub(super) fn context_accesses_for_trace(trace: &FunctionAnalysis) -> Vec<ContextAccess> {
    context_accesses_for_memory_objects(&memory_object_accesses_for_trace(trace))
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
                .map(|guard| format!("{} <= {}", pseudo_value(&guard.selector), guard.maximum));
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
                        "{} <= {}; value & {mask:#010x} == {expected:#010x}",
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

pub(super) fn collect_delays_from_flow(
    flow: &DraftReferenceFlow,
    path: &str,
    output: &mut Vec<LinkedDelay>,
) {
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
