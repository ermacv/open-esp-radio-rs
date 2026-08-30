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
                let Some(object) = LinkedMemoryObject::from_root(location.root, region) else {
                    return;
                };
                output.push(MemoryObjectAccess {
                    object,
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
            let nested_read_sources = memory_read_sources_for_flow(body);
            collect_memory_object_access_from_flow(
                body,
                &nested_path(path, "bounded-poll"),
                &nested_read_sources,
                output,
            );
            if let Some(event) = on_exhausted.as_deref() {
                collect_memory_object_access_from_event(
                    event,
                    &nested_path(path, "poll-exhausted"),
                    &BTreeMap::new(),
                    output,
                );
            }
        }
        DraftReferenceEvent::PollFlow { body, .. } => {
            let nested_read_sources = memory_read_sources_for_flow(body);
            collect_memory_object_access_from_flow(
                body,
                &nested_path(path, "poll"),
                &nested_read_sources,
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
                let nested_read_sources = memory_read_sources_for_flow(flow);
                collect_memory_object_access_from_flow(
                    flow,
                    &nested_path(path, scope),
                    &nested_read_sources,
                    output,
                );
            }
        }
        DraftReferenceEvent::ComposedCall { symbol, flow, .. }
        | DraftReferenceEvent::ComposedCallWithScratch { symbol, flow, .. } => {
            let nested_read_sources = memory_read_sources_for_flow(flow);
            collect_memory_object_access_from_flow(
                flow,
                &nested_path(path, &format!("call {symbol}")),
                &nested_read_sources,
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
        let indexed_absolute = match &access.object {
            LinkedMemoryObject::Indexed { object, .. } => match object.as_ref() {
                LinkedMemoryObject::Absolute { address, .. } => Some(*address),
                _ => None,
            },
            _ => None,
        };
        if let Some(address) = indexed_absolute
            && let Some((member, symbol, offset)) =
                resolver.data_symbol_location(address, access.width)
        {
            let LinkedMemoryObject::Indexed { object, .. } = &mut access.object else {
                unreachable!();
            };
            **object = LinkedMemoryObject::Global {
                member: member.map(str::to_owned),
                symbol: symbol.to_owned(),
            };
            access.offset = access.offset.wrapping_add(offset);
            continue;
        }
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

#[derive(Clone, Default)]
struct PathMemoryReads {
    next_token: u32,
    sources: BTreeMap<u32, MemoryObjectLocation>,
}

fn collect_memory_read_on_path(
    event: &DraftReferenceEvent,
    path: &mut PathMemoryReads,
    candidates: &mut BTreeMap<u32, BTreeSet<Option<MemoryObjectLocation>>>,
) {
    let DraftReferenceEvent::Memory {
        access: MemoryAccess::Read,
        address,
        ..
    } = event
    else {
        // Embedded poll, calibration and composed-call bodies own token
        // namespaces that cannot be joined to the containing flow.
        return;
    };
    let token = path.next_token;
    path.next_token = path.next_token.wrapping_add(1);
    let location = address.memory_object_location_with_reads(&path.sources);
    candidates
        .entry(token)
        .or_default()
        .insert(location.clone());
    if let Some(location) = location {
        path.sources.insert(token, location);
    } else {
        path.sources.remove(&token);
    }
}

fn collect_memory_read_flow_paths(
    flow: &DraftReferenceFlow,
    mut path: PathMemoryReads,
    candidates: &mut BTreeMap<u32, BTreeSet<Option<MemoryObjectLocation>>>,
) {
    for event in &flow.events {
        collect_memory_read_on_path(event, &mut path, candidates);
    }
    if let DraftReferenceTerminator::Branch {
        taken, not_taken, ..
    } = &flow.terminator
    {
        collect_memory_read_flow_paths(taken, path.clone(), candidates);
        collect_memory_read_flow_paths(not_taken, path, candidates);
    }
}

fn unambiguous_memory_read_sources(
    mut candidates: BTreeMap<u32, BTreeSet<Option<MemoryObjectLocation>>>,
) -> BTreeMap<u32, MemoryObjectLocation> {
    candidates
        .iter_mut()
        .filter_map(|(token, locations)| {
            if locations.len() != 1 {
                return None;
            }
            locations
                .pop_first()
                .flatten()
                .map(|location| (*token, location))
        })
        .collect()
}

fn memory_read_sources_for_flow(flow: &DraftReferenceFlow) -> BTreeMap<u32, MemoryObjectLocation> {
    let mut candidates = BTreeMap::new();
    collect_memory_read_flow_paths(flow, PathMemoryReads::default(), &mut candidates);
    unambiguous_memory_read_sources(candidates)
}

pub(super) fn memory_read_sources_for_trace(
    trace: &FunctionAnalysis,
) -> BTreeMap<u32, MemoryObjectLocation> {
    if let Some(flow) = trace.reference_flow.as_ref() {
        memory_read_sources_for_flow(flow)
    } else {
        let mut candidates = BTreeMap::new();
        let mut path = PathMemoryReads::default();
        for event in &trace.reference_events {
            collect_memory_read_on_path(event, &mut path, &mut candidates);
        }
        unambiguous_memory_read_sources(candidates)
    }
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

/// Retain instruction-local memory evidence even when path reconstruction
/// stops before it can emit canonical `MemoryObjectAccess` records.
///
/// A field already recovered from a path is merged instead of counted twice:
/// instruction effects are the lossless site inventory, while path accesses
/// carry the richer path/value presentation.
pub(super) fn merge_instruction_memory_fields(
    fields: &mut Vec<MemoryObjectField>,
    effects: &[LinkedInstructionEffect],
) {
    #[derive(Default)]
    struct InstructionField {
        read_sites: BTreeSet<u32>,
        write_sites: BTreeSet<u32>,
        write_mask: u32,
        paths: BTreeSet<String>,
        write_values: BTreeSet<String>,
    }

    let mut instruction_fields = BTreeMap::<(LinkedMemoryObject, i64, u8), InstructionField>::new();
    for effect in effects {
        let LinkedInstructionEffect::Memory {
            site,
            access,
            width,
            object,
            offset,
            paths,
            value_pseudo,
            write_mask,
            ..
        } = effect
        else {
            continue;
        };
        let field = instruction_fields
            .entry((object.clone(), *offset, *width))
            .or_default();
        match *access {
            "read" => {
                field.read_sites.insert(*site);
            }
            "write" => {
                field.write_sites.insert(*site);
                field.write_mask |= write_mask.unwrap_or_default();
                if let Some(value) = value_pseudo {
                    field.write_values.insert(value.clone());
                }
            }
            _ => unreachable!("instruction memory effect has a closed access vocabulary"),
        }
        field.paths.extend(paths.iter().cloned());
    }

    for ((object, offset, width), instruction) in instruction_fields {
        let field = if let Some(index) = fields.iter().position(|field| {
            field.object == object && field.offset == offset && field.width == width
        }) {
            &mut fields[index]
        } else {
            fields.push(MemoryObjectField {
                object,
                offset,
                width,
                reads: 0,
                writes: 0,
                write_mask: 0,
                paths: Vec::new(),
                write_values: Vec::new(),
            });
            fields
                .last_mut()
                .expect("the missing instruction field was just appended")
        };
        field.reads = field.reads.max(instruction.read_sites.len());
        field.writes = field.writes.max(instruction.write_sites.len());
        field.write_mask |= instruction.write_mask;
        for path in instruction.paths {
            if !field.paths.contains(&path) {
                field.paths.push(path);
            }
        }
        for value in instruction.write_values {
            if !field.write_values.contains(&value) {
                field.write_values.push(value);
            }
        }
    }
    fields.sort_by(|left, right| {
        (&left.object, left.offset, left.width).cmp(&(&right.object, right.offset, right.width))
    });
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

fn access_label(access: MemoryAccess) -> &'static str {
    match access {
        MemoryAccess::Read => "read",
        MemoryAccess::Write => "write",
    }
}

fn matching_mmio_paths(
    accesses: &[LinkedMmioAccess],
    address: u32,
    width: u8,
    access: &str,
) -> (Vec<String>, Vec<String>) {
    let mut paths = BTreeSet::new();
    let mut guards = BTreeSet::new();
    for candidate in accesses.iter().filter(|candidate| {
        candidate.address == address && candidate.width == width && candidate.access == access
    }) {
        paths.insert(candidate.path.clone());
        if let Some(guard) = &candidate.guard {
            guards.insert(guard.clone());
        }
    }
    (paths.into_iter().collect(), guards.into_iter().collect())
}

fn matching_memory_paths(
    accesses: &[MemoryObjectAccess],
    effect: &MemoryObjectAccess,
) -> Vec<String> {
    accesses
        .iter()
        .filter(|candidate| {
            candidate.object == effect.object
                && candidate.offset == effect.offset
                && candidate.access == effect.access
                && candidate.width == effect.width
        })
        .map(|candidate| candidate.path.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(super) fn instruction_effects_for_trace(
    trace: &FunctionAnalysis,
    resolver: &ReferenceResolver,
    mmio_accesses: &[LinkedMmioAccess],
    memory_accesses: &[MemoryObjectAccess],
) -> Vec<LinkedInstructionEffect> {
    let read_sources = memory_read_sources_for_trace(trace);
    let mut output = Vec::new();
    for located in &trace.located_reference_events {
        match &located.event {
            DraftReferenceEvent::Observable(ObservableEvent::Memory {
                access,
                width,
                address,
                register,
                value,
            }) => {
                let access_label = access_label(*access);
                let (paths, guards) =
                    matching_mmio_paths(mmio_accesses, *address, *width, access_label);
                let [
                    modified_mask,
                    preserved_mask,
                    _inverted_mask,
                    forced_zero_mask,
                    forced_one_mask,
                    _read_derived_mask,
                    _dynamic_mask,
                ] = mmio_write_masks(*access, *address, *width, value.as_ref());
                output.push(LinkedInstructionEffect::Mmio {
                    site: located.site,
                    block: None,
                    access: access_label,
                    width: *width,
                    address: *address,
                    register: register.clone(),
                    mode: "static",
                    paths,
                    guards,
                    value: value.as_ref().map(pseudo_value),
                    modified_mask,
                    preserved_mask,
                    forced_zero_mask,
                    forced_one_mask,
                });
            }
            DraftReferenceEvent::IndexedMmio {
                access,
                width,
                registers,
                value,
                ..
            } => {
                let access_label = access_label(*access);
                for register in registers {
                    let (paths, guards) =
                        matching_mmio_paths(mmio_accesses, register.address, *width, access_label);
                    let [
                        modified_mask,
                        preserved_mask,
                        _inverted_mask,
                        forced_zero_mask,
                        forced_one_mask,
                        _read_derived_mask,
                        _dynamic_mask,
                    ] = mmio_write_masks(*access, register.address, *width, value.as_ref());
                    output.push(LinkedInstructionEffect::Mmio {
                        site: located.site,
                        block: None,
                        access: access_label,
                        width: *width,
                        address: register.address,
                        register: register.name.clone(),
                        mode: "indexed-candidate",
                        paths,
                        guards,
                        value: value.as_ref().map(pseudo_value),
                        modified_mask,
                        preserved_mask,
                        forced_zero_mask,
                        forced_one_mask,
                    });
                }
            }
            DraftReferenceEvent::PollMmio {
                width, registers, ..
            } => {
                for register in registers {
                    let (paths, guards) =
                        matching_mmio_paths(mmio_accesses, register.address, *width, "poll");
                    output.push(LinkedInstructionEffect::Mmio {
                        site: located.site,
                        block: None,
                        access: "poll",
                        width: *width,
                        address: register.address,
                        register: register.name.clone(),
                        mode: "structural-poll",
                        paths,
                        guards,
                        value: None,
                        modified_mask: None,
                        preserved_mask: None,
                        forced_zero_mask: None,
                        forced_one_mask: None,
                    });
                }
            }
            event @ DraftReferenceEvent::Memory { .. } => {
                let mut effects = Vec::new();
                collect_memory_object_access_from_event(
                    event,
                    "instruction",
                    &read_sources,
                    &mut effects,
                );
                attribute_data_symbols(&mut effects, resolver);
                for effect in effects {
                    let paths = matching_memory_paths(memory_accesses, &effect);
                    output.push(LinkedInstructionEffect::Memory {
                        site: located.site,
                        block: None,
                        access: effect.access,
                        width: effect.width,
                        object: effect.object,
                        offset: effect.offset,
                        paths,
                        value: effect.value,
                        value_pseudo: effect.value_pseudo,
                        write_mask: effect.write_mask,
                        preserved_mask: effect.preserved_mask,
                        forced_zero_mask: effect.forced_zero_mask,
                        forced_one_mask: effect.forced_one_mask,
                    });
                }
            }
            _ => {}
        }
    }
    output.sort();
    output.dedup();
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
