//! Structural address classes, memory intrinsics and data relocations.

use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum StructuralAddress {
    Absolute(u32),
    PrivateStack(i32),
    ReviewedExternalTableSlot(String, i32),
    FunctionTableSlot(FunctionTableRef, i32),
    CallerMemory(SymbolicValue),
    SymbolMemory(SymbolicValue),
    DereferencedMemory(SymbolicValue),
    IndexedMemory(SymbolicValue),
    DynamicMemory(SymbolicValue),
}

pub(super) fn structural_effective_address(
    values: &[SymbolicValue; 32],
    memory_read_sources: &BTreeMap<u32, MemoryObjectLocation>,
    pointer_context: &StructuralPointerContext,
    base: Reg,
    offset: i32,
) -> Option<StructuralAddress> {
    let base = &values[usize::from(base.0)];
    match base {
        SymbolicValue::Constant(base) => Some(StructuralAddress::Absolute(
            base.wrapping_add(offset as u32),
        )),
        SymbolicValue::StackAddress(base) => {
            Some(StructuralAddress::PrivateStack(base.wrapping_add(offset)))
        }
        SymbolicValue::ReviewedExternalTable(contract) => Some(
            StructuralAddress::ReviewedExternalTableSlot(contract.clone(), offset),
        ),
        SymbolicValue::FunctionTable(table) => {
            Some(StructuralAddress::FunctionTableSlot(*table, offset))
        }
        SymbolicValue::SymbolAddress {
            lo_addend: Some(_), ..
        } => Some(StructuralAddress::SymbolMemory(
            base.clone().add_constant(offset as u32),
        )),
        _ if base.caller_memory_address() => Some(StructuralAddress::CallerMemory(
            base.clone().add_constant(offset as u32),
        )),
        _ if pointer_context.recognizes_reviewed_compressed_pointer(base) => Some(
            StructuralAddress::DynamicMemory(base.clone().add_constant(offset as u32)),
        ),
        _ => {
            let address = base.clone().add_constant(offset as u32);
            if let Some(location) = address.memory_object_location_with_reads(memory_read_sources) {
                match location.root {
                    MemoryObjectRoot::Dereferenced { .. } => {
                        return Some(StructuralAddress::DereferencedMemory(address));
                    }
                    MemoryObjectRoot::Indexed { .. } => {
                        return Some(StructuralAddress::IndexedMemory(address));
                    }
                    MemoryObjectRoot::Allocation { .. }
                    | MemoryObjectRoot::ZeroedAllocation { .. } => {
                        return Some(StructuralAddress::DynamicMemory(address));
                    }
                    MemoryObjectRoot::OpaqueExternalObject { .. } => {
                        return Some(StructuralAddress::DynamicMemory(address));
                    }
                    _ => {}
                }
            }
            (address.is_resolved() && address.has_memory_address_provenance(memory_read_sources))
                .then_some(StructuralAddress::DynamicMemory(address))
        }
    }
}

pub(super) fn structural_value_address_with_reads(
    value: &SymbolicValue,
    memory_read_sources: &BTreeMap<u32, MemoryObjectLocation>,
) -> Option<StructuralAddress> {
    match value {
        SymbolicValue::Constant(address) => Some(StructuralAddress::Absolute(*address)),
        SymbolicValue::StackAddress(offset) => Some(StructuralAddress::PrivateStack(*offset)),
        SymbolicValue::ReviewedExternalTable(contract) => Some(
            StructuralAddress::ReviewedExternalTableSlot(contract.clone(), 0),
        ),
        SymbolicValue::FunctionTable(table) => {
            Some(StructuralAddress::FunctionTableSlot(*table, 0))
        }
        SymbolicValue::SymbolAddress {
            lo_addend: Some(_), ..
        } => Some(StructuralAddress::SymbolMemory(value.clone())),
        _ if value.caller_memory_address() => Some(StructuralAddress::CallerMemory(value.clone())),
        _ => match value
            .memory_object_location_with_reads(memory_read_sources)
            .map(|location| location.root)
        {
            Some(MemoryObjectRoot::Dereferenced { .. }) => {
                Some(StructuralAddress::DereferencedMemory(value.clone()))
            }
            Some(MemoryObjectRoot::Indexed { .. }) => {
                Some(StructuralAddress::IndexedMemory(value.clone()))
            }
            Some(
                MemoryObjectRoot::Allocation { .. }
                | MemoryObjectRoot::ZeroedAllocation { .. }
                | MemoryObjectRoot::OpaqueExternalObject { .. },
            ) => Some(StructuralAddress::DynamicMemory(value.clone())),
            _ => None,
        },
    }
}

pub(super) fn structural_value_address(value: &SymbolicValue) -> Option<StructuralAddress> {
    structural_value_address_with_reads(value, &BTreeMap::new())
}

pub(super) fn memory_intrinsic_load_byte(
    address: SymbolicValue,
    symbol: &artifact::ArtifactSymbolDefinition,
    stack: &SymbolicStack,
    reference_events: &mut Vec<DraftReferenceEvent>,
    next_memory_read_token: &mut u32,
) -> std::result::Result<SymbolicValue, String> {
    if address.is_resolved() && address.depends_on_call_result() {
        let read_token = *next_memory_read_token;
        *next_memory_read_token += 1;
        reference_events.push(DraftReferenceEvent::Memory {
            access: MemoryAccess::Read,
            width: 8,
            address,
            region: DEFERRED_CALL_RESULT_MEMORY_REGION.to_owned(),
            value: None,
        });
        return Ok(SymbolicValue::memory_read(read_token, 8, false));
    }
    match structural_value_address(&address) {
        Some(StructuralAddress::PrivateStack(offset)) => stack
            .load(offset, 8, false)
            .ok_or_else(|| format!("uninitialized private-stack byte at {offset:+#x}")),
        Some(StructuralAddress::CallerMemory(address)) => {
            let read_token = *next_memory_read_token;
            *next_memory_read_token += 1;
            reference_events.push(DraftReferenceEvent::Memory {
                access: MemoryAccess::Read,
                width: 8,
                address,
                region: "caller-owned ABI argument RAM".to_owned(),
                value: None,
            });
            Ok(SymbolicValue::memory_read(read_token, 8, false))
        }
        Some(StructuralAddress::SymbolMemory(address)) => {
            let read_token = *next_memory_read_token;
            *next_memory_read_token += 1;
            reference_events.push(DraftReferenceEvent::Memory {
                access: MemoryAccess::Read,
                width: 8,
                region: address.canonical(),
                address,
                value: None,
            });
            Ok(SymbolicValue::memory_read(read_token, 8, false))
        }
        Some(StructuralAddress::DereferencedMemory(address)) => {
            let read_token = *next_memory_read_token;
            *next_memory_read_token += 1;
            reference_events.push(DraftReferenceEvent::Memory {
                access: MemoryAccess::Read,
                width: 8,
                address,
                region: "dereferenced known pointer RAM".to_owned(),
                value: None,
            });
            Ok(SymbolicValue::memory_read(read_token, 8, false))
        }
        Some(StructuralAddress::IndexedMemory(address)) => {
            let read_token = *next_memory_read_token;
            *next_memory_read_token += 1;
            reference_events.push(DraftReferenceEvent::Memory {
                access: MemoryAccess::Read,
                width: 8,
                address,
                region: "indexed RAM object".to_owned(),
                value: None,
            });
            Ok(SymbolicValue::memory_read(read_token, 8, false))
        }
        Some(StructuralAddress::DynamicMemory(address)) => {
            let read_token = *next_memory_read_token;
            *next_memory_read_token += 1;
            reference_events.push(DraftReferenceEvent::Memory {
                access: MemoryAccess::Read,
                width: 8,
                address,
                region: "dynamic RAM address".to_owned(),
                value: None,
            });
            Ok(SymbolicValue::memory_read(read_token, 8, false))
        }
        Some(StructuralAddress::Absolute(address)) => {
            let region = symbol
                .memory_region(address, 8)
                .ok_or_else(|| format!("source byte {address:#010x} is not mapped ELF RAM"))?;
            let read_token = *next_memory_read_token;
            *next_memory_read_token += 1;
            reference_events.push(DraftReferenceEvent::Memory {
                access: MemoryAccess::Read,
                width: 8,
                address: SymbolicValue::Constant(address),
                region: region.name.clone(),
                value: None,
            });
            Ok(SymbolicValue::memory_read(read_token, 8, false))
        }
        Some(
            StructuralAddress::ReviewedExternalTableSlot(..)
            | StructuralAddress::FunctionTableSlot(..),
        )
        | None => Err(format!(
            "source address {} has no byte-addressable memory provenance",
            address.canonical()
        )),
    }
}

pub(super) fn memory_intrinsic_store_byte(
    address: SymbolicValue,
    value: SymbolicValue,
    symbol: &artifact::ArtifactSymbolDefinition,
    stack: &mut SymbolicStack,
    reference_events: &mut Vec<DraftReferenceEvent>,
) -> std::result::Result<(), String> {
    if !value.is_resolved() {
        return Err(format!(
            "destination byte at {} has an unresolved value",
            address.canonical()
        ));
    }
    if address.is_resolved() && address.depends_on_call_result() {
        reference_events.push(DraftReferenceEvent::Memory {
            access: MemoryAccess::Write,
            width: 8,
            address,
            region: DEFERRED_CALL_RESULT_MEMORY_REGION.to_owned(),
            value: Some(value),
        });
        return Ok(());
    }
    match structural_value_address(&address) {
        Some(StructuralAddress::PrivateStack(offset)) => {
            stack.store(offset, 8, &value);
            reference_events.push(DraftReferenceEvent::PrivateStackStore {
                offset,
                width: 8,
                value,
            });
            Ok(())
        }
        Some(StructuralAddress::CallerMemory(address)) => {
            reference_events.push(DraftReferenceEvent::Memory {
                access: MemoryAccess::Write,
                width: 8,
                address,
                region: "caller-owned ABI argument RAM".to_owned(),
                value: Some(value),
            });
            Ok(())
        }
        Some(StructuralAddress::SymbolMemory(address)) => {
            reference_events.push(DraftReferenceEvent::Memory {
                access: MemoryAccess::Write,
                width: 8,
                region: address.canonical(),
                address,
                value: Some(value),
            });
            Ok(())
        }
        Some(StructuralAddress::DereferencedMemory(address)) => {
            reference_events.push(DraftReferenceEvent::Memory {
                access: MemoryAccess::Write,
                width: 8,
                address,
                region: "dereferenced known pointer RAM".to_owned(),
                value: Some(value),
            });
            Ok(())
        }
        Some(StructuralAddress::IndexedMemory(address)) => {
            reference_events.push(DraftReferenceEvent::Memory {
                access: MemoryAccess::Write,
                width: 8,
                address,
                region: "indexed RAM object".to_owned(),
                value: Some(value),
            });
            Ok(())
        }
        Some(StructuralAddress::DynamicMemory(address)) => {
            reference_events.push(DraftReferenceEvent::Memory {
                access: MemoryAccess::Write,
                width: 8,
                address,
                region: "dynamic RAM address".to_owned(),
                value: Some(value),
            });
            Ok(())
        }
        Some(StructuralAddress::Absolute(address)) => {
            let region = symbol
                .memory_region(address, 8)
                .ok_or_else(|| format!("destination byte {address:#010x} is not mapped ELF RAM"))?;
            if !region.writable {
                return Err(format!(
                    "destination byte {address:#010x} is in read-only region {}",
                    region.name
                ));
            }
            reference_events.push(DraftReferenceEvent::Memory {
                access: MemoryAccess::Write,
                width: 8,
                address: SymbolicValue::Constant(address),
                region: region.name.clone(),
                value: Some(value),
            });
            Ok(())
        }
        Some(
            StructuralAddress::ReviewedExternalTableSlot(..)
            | StructuralAddress::FunctionTableSlot(..),
        )
        | None => Err(format!(
            "destination address {} has no writable byte-memory provenance",
            address.canonical()
        )),
    }
}

pub(super) fn inline_standard_memory_intrinsic(
    function: StandardMemoryFunction,
    arguments: &Rv32CallArguments,
    symbol: &artifact::ArtifactSymbolDefinition,
    stack: &mut SymbolicStack,
    reference_events: &mut Vec<DraftReferenceEvent>,
    next_memory_read_token: &mut u32,
) -> Option<std::result::Result<SymbolicValue, String>> {
    let name = function.contract_id();
    let result = (|| {
        let length = arguments[2]
            .as_constant()
            .ok_or_else(|| format!("{name} length is not constant"))?;
        if length > MAX_INLINE_MEMORY_INTRINSIC_BYTES {
            return Err(format!(
                "{name} length {length} exceeds the reviewed inline limit of {MAX_INLINE_MEMORY_INTRINSIC_BYTES} bytes"
            ));
        }
        let destination = arguments[0].clone();
        match function {
            StandardMemoryFunction::Copy | StandardMemoryFunction::Move => {
                let mut bytes = Vec::with_capacity(length as usize);
                for offset in 0..length {
                    bytes.push(memory_intrinsic_load_byte(
                        arguments[1].clone().add_constant(offset),
                        symbol,
                        stack,
                        reference_events,
                        next_memory_read_token,
                    )?);
                }
                for (offset, value) in bytes.into_iter().enumerate() {
                    memory_intrinsic_store_byte(
                        destination.clone().add_constant(offset as u32),
                        value,
                        symbol,
                        stack,
                        reference_events,
                    )?;
                }
            }
            StandardMemoryFunction::Set => {
                let byte = arguments[1].clone().and(0xff);
                for offset in 0..length {
                    memory_intrinsic_store_byte(
                        destination.clone().add_constant(offset),
                        byte.clone(),
                        symbol,
                        stack,
                        reference_events,
                    )?;
                }
            }
        }
        Ok(destination)
    })();
    Some(result)
}

pub(super) fn structural_indexed_mmio_address(
    values: &[SymbolicValue; 32],
    base: Reg,
    offset: i32,
    svd: &MmioMap,
) -> Option<(SymbolicValue, IndexedMmioDomain)> {
    let address = values[usize::from(base.0)]
        .clone()
        .add_constant(offset as u32);
    let domain = indexed_mmio_domain(&address, svd)?;
    Some((address, domain))
}

pub(super) fn evaluate_branch_for_input(
    condition: &BranchCondition,
    input_index: u8,
    input: u32,
) -> Option<bool> {
    evaluate_branch_values_for_input(
        condition.operation,
        &condition.left,
        &condition.right,
        input_index,
        input,
    )
}

fn evaluate_branch_values_for_input(
    operation: BranchOperation,
    left: &SymbolicValue,
    right: &SymbolicValue,
    input_index: u8,
    input: u32,
) -> Option<bool> {
    let left = evaluate_for_input(left, input_index, input)?;
    let right = evaluate_for_input(right, input_index, input)?;
    Some(match operation {
        BranchOperation::Equal => left == right,
        BranchOperation::NotEqual => left != right,
        BranchOperation::LessSigned => (left as i32) < (right as i32),
        BranchOperation::GreaterEqualSigned => (left as i32) >= (right as i32),
        BranchOperation::LessUnsigned => left < right,
        BranchOperation::GreaterEqualUnsigned => left >= right,
    })
}

fn structural_indexed_read_only_memory_address_for_input(
    values: &[SymbolicValue; 32],
    base: Reg,
    offset: i32,
    width: u8,
    symbol: &artifact::ArtifactSymbolDefinition,
    reference_events: &[DraftReferenceEvent],
) -> Option<(SymbolicValue, String)> {
    const MAX_EXHAUSTIVE_INPUT_BITS: usize = 8;

    let address = values[usize::from(base.0)]
        .clone()
        .add_constant(offset as u32);
    let mut input_index = None;
    let mut input_bits = BTreeSet::new();
    if !collect_evaluable_input_bits(&address, &mut input_index, &mut input_bits) {
        return None;
    }
    let input_index = input_index?;

    let mut relevant_decisions = Vec::new();
    for event in reference_events {
        let DraftReferenceEvent::BranchDecision { condition, taken } = event else {
            continue;
        };
        let mut condition_index = None;
        let mut condition_bits = BTreeSet::new();
        if !collect_evaluable_input_bits(&condition.left, &mut condition_index, &mut condition_bits)
            || !collect_evaluable_input_bits(
                &condition.right,
                &mut condition_index,
                &mut condition_bits,
            )
        {
            return None;
        }
        if condition_index == Some(input_index) {
            input_bits.extend(condition_bits);
            relevant_decisions.push((condition, *taken));
        }
    }
    if input_bits.len() > MAX_EXHAUSTIVE_INPUT_BITS {
        return None;
    }

    let input_bits = input_bits.into_iter().collect::<Vec<_>>();
    let mut selected_region = None::<(u32, u32, String)>;
    let mut feasible_inputs = 0usize;
    for combination in 0..(1_u32 << input_bits.len()) {
        let input = input_bits
            .iter()
            .enumerate()
            .fold(0_u32, |value, (source, destination)| {
                value | (((combination >> source) & 1) << destination)
            });
        if relevant_decisions.iter().any(|(condition, taken)| {
            evaluate_branch_for_input(condition, input_index, input) != Some(*taken)
        }) {
            continue;
        }
        let candidate = evaluate_for_input(&address, input_index, input)?;
        let region = symbol
            .memory_region(candidate, width)
            .filter(|region| !region.writable)?;
        let identity = (region.start, region.length, region.name.clone());
        if selected_region
            .as_ref()
            .is_some_and(|selected| selected != &identity)
        {
            return None;
        }
        selected_region = Some(identity);
        feasible_inputs += 1;
    }
    let (_, _, region) = selected_region?;
    (feasible_inputs != 0).then_some((address, format!("bounded read-only ELF {region}")))
}

const REVIEWED_MEMORY_PROJECTION_INPUT: u8 = u8::MAX;

fn depends_on_memory_read(value: &SymbolicValue, read_token: u32) -> bool {
    value.tree().any(|value| {
        value.bits().iter().any(|source| {
            matches!(
                source,
                BitSource::Memory {
                    read_token: source_token,
                    ..
                } if *source_token == read_token
            )
        })
    })
}

fn project_memory_read_as_input(value: &SymbolicValue, read_token: u32) -> SymbolicValue {
    if let SymbolicValue::Expression {
        operation,
        left,
        right,
        ..
    } = value
    {
        return SymbolicValue::expression(
            *operation,
            project_memory_read_as_input(left, read_token),
            project_memory_read_as_input(right, read_token),
        );
    }
    SymbolicValue::from_bits(value.bits().map(|source| match source {
        BitSource::Memory {
            read_token: source_token,
            bit,
            inverted,
        } if source_token == read_token => BitSource::Input {
            index: REVIEWED_MEMORY_PROJECTION_INPUT,
            bit,
            inverted,
        },
        source => source,
    }))
}

fn structural_indexed_read_only_memory_address_for_reviewed_memory(
    address: &SymbolicValue,
    width: u8,
    symbol: &artifact::ArtifactSymbolDefinition,
    reference_events: &[DraftReferenceEvent],
    read_token: u32,
    domain: ReviewedMemoryValueDomain,
) -> Option<(SymbolicValue, String)> {
    if !depends_on_memory_read(address, read_token) {
        return None;
    }
    let projected_address = project_memory_read_as_input(address, read_token);
    let mut input_index = Some(REVIEWED_MEMORY_PROJECTION_INPUT);
    let mut input_bits = BTreeSet::new();
    if !collect_evaluable_input_bits(&projected_address, &mut input_index, &mut input_bits)
        || input_index != Some(REVIEWED_MEMORY_PROJECTION_INPUT)
    {
        return None;
    }

    let mut relevant_decisions = Vec::new();
    for event in reference_events {
        let DraftReferenceEvent::BranchDecision { condition, taken } = event else {
            continue;
        };
        if !depends_on_memory_read(&condition.left, read_token)
            && !depends_on_memory_read(&condition.right, read_token)
        {
            continue;
        }
        let left = project_memory_read_as_input(&condition.left, read_token);
        let right = project_memory_read_as_input(&condition.right, read_token);
        let mut condition_index = Some(REVIEWED_MEMORY_PROJECTION_INPUT);
        let mut condition_bits = BTreeSet::new();
        if !collect_evaluable_input_bits(&left, &mut condition_index, &mut condition_bits)
            || !collect_evaluable_input_bits(&right, &mut condition_index, &mut condition_bits)
            || condition_index != Some(REVIEWED_MEMORY_PROJECTION_INPUT)
        {
            return None;
        }
        relevant_decisions.push((condition.operation, left, right, *taken));
    }

    let mut selected_region = None::<(u32, u32, String)>;
    let mut feasible_inputs = 0usize;
    for input in domain.minimum()..=domain.maximum() {
        if relevant_decisions
            .iter()
            .any(|(operation, left, right, taken)| {
                evaluate_branch_values_for_input(
                    *operation,
                    left,
                    right,
                    REVIEWED_MEMORY_PROJECTION_INPUT,
                    input,
                ) != Some(*taken)
            })
        {
            continue;
        }
        let candidate =
            evaluate_for_input(&projected_address, REVIEWED_MEMORY_PROJECTION_INPUT, input)?;
        let region = symbol
            .memory_region(candidate, width)
            .filter(|region| !region.writable)?;
        let identity = (region.start, region.length, region.name.clone());
        if selected_region
            .as_ref()
            .is_some_and(|selected| selected != &identity)
        {
            return None;
        }
        selected_region = Some(identity);
        feasible_inputs += 1;
    }
    let (_, _, region) = selected_region?;
    (feasible_inputs != 0).then_some((
        address.clone(),
        format!(
            "bounded read-only ELF {region} via reviewed domain {}",
            domain.id()
        ),
    ))
}

pub(super) fn structural_indexed_read_only_memory_address(
    values: &[SymbolicValue; 32],
    base: Reg,
    offset: i32,
    width: u8,
    symbol: &artifact::ArtifactSymbolDefinition,
    reference_events: &[DraftReferenceEvent],
    reviewed_memory_domains: &BTreeMap<u32, ReviewedMemoryValueDomain>,
) -> Option<(SymbolicValue, String)> {
    if let Some(result) = structural_indexed_read_only_memory_address_for_input(
        values,
        base,
        offset,
        width,
        symbol,
        reference_events,
    ) {
        return Some(result);
    }

    let address = values[usize::from(base.0)]
        .clone()
        .add_constant(offset as u32);
    reviewed_memory_domains
        .iter()
        .find_map(|(read_token, domain)| {
            structural_indexed_read_only_memory_address_for_reviewed_memory(
                &address,
                width,
                symbol,
                reference_events,
                *read_token,
                *domain,
            )
        })
}

pub(super) fn relocation_symbol_address(
    owner: &artifact::ArtifactSymbolDefinition,
    relocation: &artifact::SymbolRelocation,
) -> SymbolicValue {
    SymbolicValue::SymbolAddress {
        member: owner.member.clone(),
        symbol: relocation.symbol.clone(),
        hi_addend: relocation.addend,
        lo_addend: None,
        post_offset: 0,
    }
}

pub(super) fn complete_low_relocation(
    owner: &artifact::ArtifactSymbolDefinition,
    pc: u32,
    kind: artifact::RelocationKind,
    base: &SymbolicValue,
    encoded_offset: i32,
) -> std::result::Result<Option<SymbolicValue>, String> {
    if owner.addresses_resolved {
        return Ok(None);
    }
    let Some(relocation) = owner.relocation(pc, kind) else {
        return Ok(None);
    };
    let expected_offset = ((relocation.addend as u32) << 20) as i32 >> 20;
    if encoded_offset != expected_offset {
        return Err(format!(
            "relocation {kind:?} at {pc:#x} encodes {encoded_offset:+#x}, expected low addend {expected_offset:+#x}"
        ));
    }
    let SymbolicValue::SymbolAddress {
        member,
        symbol,
        hi_addend,
        lo_addend: None,
        post_offset: 0,
    } = base
    else {
        return Err(format!(
            "relocation {kind:?} at {pc:#x} has no matching incomplete HI20 base"
        ));
    };
    if member != &owner.member || symbol != &relocation.symbol {
        return Err(format!(
            "relocation {kind:?} at {pc:#x} does not match its HI20 base: low={:?}::{}{:+#x}, high={member:?}::{symbol}{hi_addend:+#x}",
            owner.member, relocation.symbol, relocation.addend
        ));
    }
    Ok(Some(SymbolicValue::SymbolAddress {
        member: member.clone(),
        symbol: symbol.clone(),
        hi_addend: *hi_addend,
        lo_addend: Some(relocation.addend),
        post_offset: 0,
    }))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn table_symbol() -> artifact::ArtifactSymbolDefinition {
        artifact::ArtifactSymbolDefinition {
            member: None,
            name: "bounded_table_consumer".to_owned(),
            address: 0x1000,
            bytes: Vec::new(),
            addresses_resolved: true,
            memory_regions: Arc::from([artifact::MemoryRegion {
                start: 0x2000,
                length: 40,
                writable: false,
                name: ".rodata.channel-map".to_owned(),
            }]),
            relocations: Vec::new(),
        }
    }

    fn values_with_reviewed_index() -> [SymbolicValue; 32] {
        let mut values = core::array::from_fn(|_| SymbolicValue::Unknown);
        values[usize::from(Reg::A5.0)] =
            SymbolicValue::memory_read(3, 8, false).add_constant(0x2000);
        values
    }

    #[test]
    fn reviewed_memory_domain_proves_the_complete_read_only_table_access() {
        let values = values_with_reviewed_index();
        let unrelated_branch = DraftReferenceEvent::BranchDecision {
            condition: BranchCondition {
                site: 0x1004,
                operation: BranchOperation::Equal,
                left: SymbolicValue::memory_read(1, 32, false),
                right: SymbolicValue::Constant(0),
            },
            taken: false,
        };
        let domains = BTreeMap::from([(
            3,
            ReviewedMemoryValueDomain::inclusive("channel-0-through-39", 0, 39),
        )]);

        let (address, region) = structural_indexed_read_only_memory_address(
            &values,
            Reg::A5,
            0,
            8,
            &table_symbol(),
            &[unrelated_branch],
            &domains,
        )
        .expect("every reviewed input selects the same read-only table");

        assert_eq!(address, values[usize::from(Reg::A5.0)]);
        assert!(region.contains("channel-0-through-39"));
    }

    #[test]
    fn reviewed_memory_domain_fails_closed_when_one_value_escapes_the_table() {
        let domains = BTreeMap::from([(
            3,
            ReviewedMemoryValueDomain::inclusive("channel-0-through-40", 0, 40),
        )]);

        assert!(
            structural_indexed_read_only_memory_address(
                &values_with_reviewed_index(),
                Reg::A5,
                0,
                8,
                &table_symbol(),
                &[],
                &domains,
            )
            .is_none()
        );
    }
}
