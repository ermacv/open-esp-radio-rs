//! Structural load/store effects over MMIO, ELF memory and ABI-owned RAM.

use super::state::StructuralTraceState;
use super::*;
use rv_asm::Imm;

fn remember_pointer_read_source(
    state: &mut StructuralTraceState,
    read_token: u32,
    width: u8,
    address: &SymbolicValue,
) {
    if width != 32 {
        return;
    }
    if let Some(location) = address.memory_object_location_with_reads(&state.memory_read_sources) {
        std::sync::Arc::make_mut(&mut state.memory_read_sources).insert(read_token, location);
    }
}

fn remember_absolute_pointer_read_source(
    state: &mut StructuralTraceState,
    read_token: u32,
    width: u8,
    address: u32,
) {
    if width == 32 {
        std::sync::Arc::make_mut(&mut state.memory_read_sources).insert(
            read_token,
            MemoryObjectLocation {
                root: MemoryObjectRoot::Absolute { address },
                offset: 0,
            },
        );
    }
}

fn projected_relocation(
    symbol: &artifact::ArtifactSymbolDefinition,
    pointer_context: &StructuralPointerContext,
    pc: u32,
    kind: artifact::RelocationKind,
) -> std::result::Result<Option<StructuralProjectedRelocation>, String> {
    let site = StructuralCallSite::new(symbol, pc);
    let candidates = pointer_context
        .projected_relocations
        .get(&site)
        .into_iter()
        .flatten()
        .filter(|candidate| candidate.kind == kind)
        .cloned()
        .collect::<Vec<_>>();
    match candidates.as_slice() {
        [] => Ok(None),
        [candidate] => Ok(Some(candidate.clone())),
        _ => Err(format!(
            "ambiguous projected {kind:?} relocation at {pc:#x}: {}",
            candidates
                .iter()
                .map(|candidate| format!(
                    "{:?}::{} -> {}{:+#x}",
                    candidate.origin_member,
                    candidate.origin_symbol,
                    candidate.symbol,
                    candidate.addend
                ))
                .collect::<Vec<_>>()
                .join(" | ")
        )),
    }
}

fn projected_symbol_address(relocation: &StructuralProjectedRelocation) -> SymbolicValue {
    SymbolicValue::SymbolAddress {
        member: relocation.origin_member.clone(),
        symbol: relocation.symbol.clone(),
        hi_addend: relocation.addend,
        lo_addend: Some(relocation.addend),
        post_offset: 0,
    }
}

fn projected_relaxed_zero_address(
    relocation: &StructuralProjectedRelocation,
    base: &SymbolicValue,
    offset: i32,
) -> bool {
    // A final linker may relax an origin HI20+LO12 pair into one load/store.
    // When the linked fixture deliberately leaves that data symbol undefined,
    // the encoded address is zero even though the authenticated origin still
    // proves which symbolic cell was referenced. Preserve that identity
    // without inventing the cell's runtime contents.
    // `origin_offsets` describes the origin instructions which survived the
    // correspondence projection, not every instruction consumed by the
    // linker relaxation.  A linker may delete the HI20 instruction entirely
    // and project only the relocated load/store.  Requiring two surviving
    // offsets therefore loses the exact symbol identity and misclassifies the
    // final `0(zero)` encoding as a real null access.
    relocation.correspondence == "linker-relaxation" && base.as_constant() == Some(0) && offset == 0
}

pub(super) fn apply_floating_memory_instruction(
    blocker: artifact::UnsupportedInstruction,
    symbol: &artifact::ArtifactSymbolDefinition,
    pointer_context: &StructuralPointerContext,
    svd: &MmioMap,
    state: &mut StructuralTraceState,
) -> bool {
    let Some(instruction) = artifact::decode_floating_memory_instruction(blocker) else {
        return false;
    };
    let scratch = if instruction.base == Reg::T6 {
        Reg::T5
    } else {
        Reg::T6
    };
    let saved = state.values[usize::from(scratch.0)].clone();
    let integer_instruction = match instruction.access {
        artifact::FloatingMemoryAccess::Load => Inst::Lw {
            offset: Imm::new_i32(instruction.offset),
            dest: scratch,
            base: instruction.base,
        },
        artifact::FloatingMemoryAccess::Store => {
            state.values[usize::from(scratch.0)] =
                state.floating_values[usize::from(instruction.floating_register)].clone();
            Inst::Sw {
                offset: Imm::new_i32(instruction.offset),
                src: scratch,
                base: instruction.base,
            }
        }
    };
    let applied = apply_memory_instruction(
        artifact::DecodedInstruction {
            address: instruction.address,
            width: instruction.instruction_width,
            instruction: integer_instruction,
        },
        symbol,
        pointer_context,
        svd,
        state,
    );
    if instruction.access == artifact::FloatingMemoryAccess::Load {
        state.floating_values[usize::from(instruction.floating_register)] =
            state.values[usize::from(scratch.0)].clone();
    }
    state.values[usize::from(scratch.0)] = saved;
    applied
}

pub(super) fn apply_memory_instruction(
    decoded: artifact::DecodedInstruction,
    symbol: &artifact::ArtifactSymbolDefinition,
    pointer_context: &StructuralPointerContext,
    svd: &MmioMap,
    state: &mut StructuralTraceState,
) -> bool {
    let pc = decoded.address;
    let instruction = decoded.instruction;
    match instruction {
        Inst::Lb { offset, dest, base }
        | Inst::Lbu { offset, dest, base }
        | Inst::Lh { offset, dest, base }
        | Inst::Lhu { offset, dest, base }
        | Inst::Lw { offset, dest, base } => {
            let width = match instruction {
                Inst::Lb { .. } | Inst::Lbu { .. } => 8,
                Inst::Lh { .. } | Inst::Lhu { .. } => 16,
                _ => 32,
            };
            let signed = matches!(instruction, Inst::Lb { .. } | Inst::Lh { .. });
            let projected_relocation = match projected_relocation(
                symbol,
                pointer_context,
                pc as u32,
                artifact::RelocationKind::Lo12I,
            ) {
                Ok(relocation) => relocation,
                Err(error) => {
                    state
                        .reference_blockers
                        .push(format!("projected-data-relocation at {pc:#x}: {error}"));
                    structural_set(&mut state.values, dest, SymbolicValue::Unknown);
                    return true;
                }
            };
            let relocated_pointer = projected_relocation
                .as_ref()
                .and_then(|relocation| {
                    (relocation.addend == 0 && offset.as_i32() == 0)
                        .then(|| {
                            pointer_context
                                .relocated_pointer_symbols
                                .get(&relocation.symbol)
                                .cloned()
                        })
                        .flatten()
                })
                .or_else(|| {
                    symbol
                        .relocation(pc as u32, artifact::RelocationKind::Lo12I)
                        .and_then(|relocation| {
                            (relocation.addend == 0 && offset.as_i32() == 0)
                                .then(|| {
                                    pointer_context
                                        .relocated_pointer_symbols
                                        .get(&relocation.symbol)
                                        .cloned()
                                })
                                .flatten()
                        })
                });
            let address = if relocated_pointer.is_some() {
                None
            } else if let Some(relocation) = projected_relocation.as_ref()
                && projected_relaxed_zero_address(
                    relocation,
                    &state.values[usize::from(base.0)],
                    offset.as_i32(),
                )
            {
                Some(StructuralAddress::SymbolMemory(projected_symbol_address(
                    relocation,
                )))
            } else if symbol.addresses_resolved
                && let Some(address) = structural_effective_address(
                    &state.values,
                    &state.memory_read_sources,
                    base,
                    offset.as_i32(),
                )
            {
                // A projected archive relocation describes provenance, not
                // the immediate encoding after final linking. When the linked
                // instruction and its base recover an exact address, that
                // address is linker truth; comparing its low immediate with
                // the relocatable addend rejects ordinary absolute HI/LO
                // materialization (for example LUI + a negative LO12).
                Some(address)
            } else if let Some(relocation) = projected_relocation.as_ref() {
                let expected_offset = ((relocation.addend as u32) << 20) as i32 >> 20;
                if offset.as_i32() != expected_offset {
                    state.reference_blockers.push(format!(
                        "malformed-projected-data-relocation at {pc:#x}: Lo12I encodes {:+#x}, expected {expected_offset:+#x}",
                        offset.as_i32()
                    ));
                    structural_set(&mut state.values, dest, SymbolicValue::Unknown);
                    return true;
                }
                Some(StructuralAddress::SymbolMemory(projected_symbol_address(
                    relocation,
                )))
            } else {
                match complete_low_relocation(
                    symbol,
                    pc as u32,
                    artifact::RelocationKind::Lo12I,
                    &state.values[usize::from(base.0)],
                    offset.as_i32(),
                ) {
                    Ok(Some(address)) => Some(StructuralAddress::SymbolMemory(address)),
                    Ok(None) => structural_effective_address(
                        &state.values,
                        &state.memory_read_sources,
                        base,
                        offset.as_i32(),
                    ),
                    Err(error) => {
                        state
                            .reference_blockers
                            .push(format!("malformed-data-relocation at {pc:#x}: {error}"));
                        structural_set(&mut state.values, dest, SymbolicValue::Unknown);
                        return true;
                    }
                }
            };
            let value = match (relocated_pointer, address) {
                (Some(value), _) if width == 32 => value,
                (_, Some(StructuralAddress::Absolute(address)))
                    if width == 32
                        && pointer_context
                            .reviewed_external_pointer_cells
                            .contains_key(&address) =>
                {
                    SymbolicValue::ReviewedExternalTable(
                        pointer_context.reviewed_external_pointer_cells[&address].clone(),
                    )
                }
                (_, Some(StructuralAddress::Absolute(address)))
                    if width == 32
                        && pointer_context
                            .function_pointer_cells
                            .contains_key(&address) =>
                {
                    SymbolicValue::FunctionTable(pointer_context.function_pointer_cells[&address])
                }
                (_, Some(StructuralAddress::Absolute(address)))
                    if width == 32 && pointer_context.data_pointer_cells.contains_key(&address) =>
                {
                    pointer_context.data_pointer_cells[&address].clone()
                }
                (_, Some(StructuralAddress::ReviewedExternalTableSlot(contract, offset)))
                    if width == 32 =>
                {
                    let Ok(offset) = u32::try_from(offset) else {
                        state.reference_blockers.push(format!(
                            "negative-external-abi-slot at {pc:#x}: {instruction}"
                        ));
                        structural_set(&mut state.values, dest, SymbolicValue::Unknown);
                        return true;
                    };
                    if pointer_context
                        .reviewed_external_slots
                        .contains_key(&(contract.clone(), offset))
                    {
                        SymbolicValue::ReviewedExternalFunction { contract, offset }
                    } else {
                        state.reference_blockers.push(format!(
                            "unregistered-external-abi-slot at {pc:#x}: {}+{offset:#x}",
                            contract
                        ));
                        SymbolicValue::Unknown
                    }
                }
                (_, Some(StructuralAddress::FunctionTableSlot(table, offset))) if width == 32 => {
                    let Ok(offset) = u32::try_from(offset) else {
                        state.reference_blockers.push(format!(
                            "negative-function-table-slot at {pc:#x}: {instruction}"
                        ));
                        structural_set(&mut state.values, dest, SymbolicValue::Unknown);
                        return true;
                    };
                    match pointer_context.function_table_slots.get(&(table, offset)) {
                        Some(target) => SymbolicValue::FunctionPointer {
                            table,
                            target: *target,
                        },
                        None => {
                            state.reference_blockers.push(format!(
                                "unregistered-function-table-slot at {pc:#x}: {}+{offset:#x}",
                                table.id()
                            ));
                            SymbolicValue::Unknown
                        }
                    }
                }
                (_, Some(StructuralAddress::PrivateStack(offset))) => {
                    if state.private_stack_may_be_modified_by_call {
                        let token = state.next_private_stack_read_token;
                        state.next_private_stack_read_token += 1;
                        state.push_reference_event(
                            pc as u32,
                            DraftReferenceEvent::PrivateStackLoad {
                                token,
                                offset,
                                width,
                                signed,
                            },
                        );
                        SymbolicValue::private_stack_read(token, width, signed)
                    } else {
                        state.stack.load(offset, width, signed).unwrap_or_else(|| {
                            state.reference_blockers.push(format!(
                                "uninitialized-private-stack-load at {pc:#x}: {instruction}"
                            ));
                            SymbolicValue::Unknown
                        })
                    }
                }
                (_, Some(StructuralAddress::CallerMemory(address))) => {
                    let read_token = state.next_memory_read_token;
                    state.next_memory_read_token += 1;
                    remember_pointer_read_source(state, read_token, width, &address);
                    state.push_reference_event(
                        pc as u32,
                        DraftReferenceEvent::Memory {
                            access: MemoryAccess::Read,
                            width,
                            address,
                            region: "caller-owned ABI argument RAM".to_owned(),
                            value: None,
                        },
                    );
                    SymbolicValue::memory_read(read_token, width, signed)
                }
                (_, Some(StructuralAddress::SymbolMemory(address))) => {
                    let read_token = state.next_memory_read_token;
                    state.next_memory_read_token += 1;
                    remember_pointer_read_source(state, read_token, width, &address);
                    state.push_reference_event(
                        pc as u32,
                        DraftReferenceEvent::Memory {
                            access: MemoryAccess::Read,
                            width,
                            region: address.canonical(),
                            address,
                            value: None,
                        },
                    );
                    SymbolicValue::memory_read(read_token, width, signed)
                }
                (_, Some(StructuralAddress::DereferencedMemory(address))) => {
                    let read_token = state.next_memory_read_token;
                    state.next_memory_read_token += 1;
                    remember_pointer_read_source(state, read_token, width, &address);
                    state.push_reference_event(
                        pc as u32,
                        DraftReferenceEvent::Memory {
                            access: MemoryAccess::Read,
                            width,
                            address,
                            region: "dereferenced known pointer RAM".to_owned(),
                            value: None,
                        },
                    );
                    SymbolicValue::memory_read(read_token, width, signed)
                }
                (_, Some(StructuralAddress::IndexedMemory(address))) => {
                    let read_token = state.next_memory_read_token;
                    state.next_memory_read_token += 1;
                    remember_pointer_read_source(state, read_token, width, &address);
                    state.push_reference_event(
                        pc as u32,
                        DraftReferenceEvent::Memory {
                            access: MemoryAccess::Read,
                            width,
                            address,
                            region: "indexed RAM object".to_owned(),
                            value: None,
                        },
                    );
                    SymbolicValue::memory_read(read_token, width, signed)
                }
                (_, Some(StructuralAddress::DynamicMemory(address))) => {
                    let read_token = state.next_memory_read_token;
                    state.next_memory_read_token += 1;
                    state.push_reference_event(
                        pc as u32,
                        DraftReferenceEvent::Memory {
                            access: MemoryAccess::Read,
                            width,
                            address,
                            region: "dynamic RAM address".to_owned(),
                            value: None,
                        },
                    );
                    SymbolicValue::memory_read(read_token, width, signed)
                }
                (_, Some(StructuralAddress::Absolute(address))) if svd.contains_mmio(address) => {
                    let read_token = state.next_mmio_read_token;
                    state.next_mmio_read_token += 1;
                    let event = ObservableEvent::Memory {
                        access: MemoryAccess::Read,
                        width,
                        address,
                        register: svd.display_register_name(address),
                        value: None,
                    };
                    state.events.push(event.clone());
                    state.located_events.push(LocatedObservableEvent {
                        site: pc as u32,
                        event: event.clone(),
                    });
                    state.push_reference_event(pc as u32, DraftReferenceEvent::Observable(event));
                    SymbolicValue::register_read(read_token, address, width, signed)
                }
                (_, Some(StructuralAddress::Absolute(address)))
                    if symbol.memory_region(address, width).is_some() =>
                {
                    let region = symbol.memory_region(address, width).unwrap();
                    let read_token = state.next_memory_read_token;
                    state.next_memory_read_token += 1;
                    remember_absolute_pointer_read_source(state, read_token, width, address);
                    state.push_reference_event(
                        pc as u32,
                        DraftReferenceEvent::Memory {
                            access: MemoryAccess::Read,
                            width,
                            address: SymbolicValue::Constant(address),
                            region: region.name.clone(),
                            value: None,
                        },
                    );
                    SymbolicValue::memory_read(read_token, width, signed)
                }
                _ => {
                    if let Some((address, domain)) =
                        structural_indexed_mmio_address(&state.values, base, offset.as_i32(), svd)
                    {
                        let read_token = state.next_mmio_read_token;
                        state.next_mmio_read_token += 1;
                        state.push_reference_event(
                            pc as u32,
                            DraftReferenceEvent::IndexedMmio {
                                access: MemoryAccess::Read,
                                width,
                                address,
                                registers: domain.registers,
                                guard: domain.guard,
                                value: None,
                            },
                        );
                        SymbolicValue::indexed_register_read(read_token, width, signed)
                    } else if let Some((address, region)) =
                        structural_indexed_read_only_memory_address(
                            &state.values,
                            base,
                            offset.as_i32(),
                            width,
                            symbol,
                            &state.reference_events,
                        )
                    {
                        let read_token = state.next_memory_read_token;
                        state.next_memory_read_token += 1;
                        remember_pointer_read_source(state, read_token, width, &address);
                        state.push_reference_event(
                            pc as u32,
                            DraftReferenceEvent::Memory {
                                access: MemoryAccess::Read,
                                width,
                                address,
                                region,
                                value: None,
                            },
                        );
                        SymbolicValue::memory_read(read_token, width, signed)
                    } else if state.values[usize::from(base.0)].is_resolved()
                        && state.values[usize::from(base.0)].depends_on_private_stack_read()
                    {
                        let read_token = state.next_memory_read_token;
                        state.next_memory_read_token += 1;
                        let address = state.values[usize::from(base.0)]
                            .clone()
                            .add_constant(offset.as_u32());
                        remember_pointer_read_source(state, read_token, width, &address);
                        state.push_reference_event(
                            pc as u32,
                            DraftReferenceEvent::Memory {
                                access: MemoryAccess::Read,
                                width,
                                address,
                                region: DEFERRED_CALLER_MEMORY_REGION.to_owned(),
                                value: None,
                            },
                        );
                        SymbolicValue::memory_read(read_token, width, signed)
                    } else {
                        state.reference_blockers.push(format!(
                            "unmodeled-memory-load at {pc:#x}: {instruction}{}; base {} = {}",
                            if symbol.addresses_resolved {
                                ""
                            } else {
                                " (relocatable addresses)"
                            },
                            base,
                            state.values[usize::from(base.0)].canonical(),
                        ));
                        SymbolicValue::Unknown
                    }
                }
            };
            structural_set(&mut state.values, dest, value);
        }
        Inst::Sb { offset, src, base }
        | Inst::Sh { offset, src, base }
        | Inst::Sw { offset, src, base } => {
            let width = match instruction {
                Inst::Sb { .. } => 8,
                Inst::Sh { .. } => 16,
                _ => 32,
            };
            let value = state.values[usize::from(src.0)].clone();
            let projected_relocation = match projected_relocation(
                symbol,
                pointer_context,
                pc as u32,
                artifact::RelocationKind::Lo12S,
            ) {
                Ok(relocation) => relocation,
                Err(error) => {
                    state
                        .reference_blockers
                        .push(format!("projected-data-relocation at {pc:#x}: {error}"));
                    return true;
                }
            };
            let address = if let Some(relocation) = projected_relocation.as_ref()
                && projected_relaxed_zero_address(
                    relocation,
                    &state.values[usize::from(base.0)],
                    offset.as_i32(),
                ) {
                Some(StructuralAddress::SymbolMemory(projected_symbol_address(
                    relocation,
                )))
            } else {
                match complete_low_relocation(
                    symbol,
                    pc as u32,
                    artifact::RelocationKind::Lo12S,
                    &state.values[usize::from(base.0)],
                    offset.as_i32(),
                ) {
                    Ok(Some(address)) => Some(StructuralAddress::SymbolMemory(address)),
                    Ok(None) => structural_effective_address(
                        &state.values,
                        &state.memory_read_sources,
                        base,
                        offset.as_i32(),
                    ),
                    Err(error) => {
                        state
                            .reference_blockers
                            .push(format!("malformed-data-relocation at {pc:#x}: {error}"));
                        return true;
                    }
                }
            };
            match address {
                Some(StructuralAddress::PrivateStack(offset)) => {
                    std::sync::Arc::make_mut(&mut state.stack).store(offset, width, &value);
                    state.push_reference_event(
                        pc as u32,
                        DraftReferenceEvent::PrivateStackStore {
                            offset,
                            width,
                            value,
                        },
                    );
                }
                Some(StructuralAddress::CallerMemory(address)) => {
                    if !value.is_resolved() {
                        state
                            .reference_blockers
                            .push(format!("unresolved-memory-write at {pc:#x}: {instruction}"));
                    }
                    state.push_reference_event(
                        pc as u32,
                        DraftReferenceEvent::Memory {
                            access: MemoryAccess::Write,
                            width,
                            address,
                            region: "caller-owned ABI argument RAM".to_owned(),
                            value: Some(value),
                        },
                    );
                }
                Some(StructuralAddress::SymbolMemory(address)) => {
                    if !value.is_resolved() {
                        state
                            .reference_blockers
                            .push(format!("unresolved-memory-write at {pc:#x}: {instruction}"));
                    }
                    state.push_reference_event(
                        pc as u32,
                        DraftReferenceEvent::Memory {
                            access: MemoryAccess::Write,
                            width,
                            region: address.canonical(),
                            address,
                            value: Some(value),
                        },
                    );
                }
                Some(StructuralAddress::DereferencedMemory(address)) => {
                    if !value.is_resolved() {
                        state
                            .reference_blockers
                            .push(format!("unresolved-memory-write at {pc:#x}: {instruction}"));
                    }
                    state.push_reference_event(
                        pc as u32,
                        DraftReferenceEvent::Memory {
                            access: MemoryAccess::Write,
                            width,
                            address,
                            region: "dereferenced known pointer RAM".to_owned(),
                            value: Some(value),
                        },
                    );
                }
                Some(StructuralAddress::IndexedMemory(address)) => {
                    if !value.is_resolved() {
                        state
                            .reference_blockers
                            .push(format!("unresolved-memory-write at {pc:#x}: {instruction}"));
                    }
                    state.push_reference_event(
                        pc as u32,
                        DraftReferenceEvent::Memory {
                            access: MemoryAccess::Write,
                            width,
                            address,
                            region: "indexed RAM object".to_owned(),
                            value: Some(value),
                        },
                    );
                }
                Some(StructuralAddress::DynamicMemory(address)) => {
                    if !value.is_resolved() {
                        state
                            .reference_blockers
                            .push(format!("unresolved-memory-write at {pc:#x}: {instruction}"));
                    }
                    state.push_reference_event(
                        pc as u32,
                        DraftReferenceEvent::Memory {
                            access: MemoryAccess::Write,
                            width,
                            address,
                            region: "dynamic RAM address".to_owned(),
                            value: Some(value),
                        },
                    );
                }
                Some(StructuralAddress::Absolute(address)) if svd.contains_mmio(address) => {
                    if !value.is_resolved() {
                        state.blockers.push(format!(
                            "unresolved MMIO write value at {pc:#x}: {instruction}"
                        ));
                    }
                    let event = ObservableEvent::Memory {
                        access: MemoryAccess::Write,
                        width,
                        address,
                        register: svd.display_register_name(address),
                        value: Some(value),
                    };
                    state.events.push(event.clone());
                    state.located_events.push(LocatedObservableEvent {
                        site: pc as u32,
                        event: event.clone(),
                    });
                    state.push_reference_event(pc as u32, DraftReferenceEvent::Observable(event));
                }
                Some(StructuralAddress::Absolute(address))
                    if symbol.memory_region(address, width).is_some() =>
                {
                    let region = symbol.memory_region(address, width).unwrap();
                    if !region.writable {
                        state.reference_blockers.push(format!(
                            "read-only-memory-store at {pc:#x}: {instruction} ({})",
                            region.name
                        ));
                    }
                    if !value.is_resolved() {
                        state
                            .reference_blockers
                            .push(format!("unresolved-memory-write at {pc:#x}: {instruction}"));
                    }
                    state.push_reference_event(
                        pc as u32,
                        DraftReferenceEvent::Memory {
                            access: MemoryAccess::Write,
                            width,
                            address: SymbolicValue::Constant(address),
                            region: region.name.clone(),
                            value: Some(value),
                        },
                    );
                }
                _ => {
                    if let Some((address, domain)) =
                        structural_indexed_mmio_address(&state.values, base, offset.as_i32(), svd)
                    {
                        if !value.is_resolved() {
                            state.reference_blockers.push(format!(
                                "unresolved indexed MMIO write value at {pc:#x}: {instruction}"
                            ));
                        }
                        state.push_reference_event(
                            pc as u32,
                            DraftReferenceEvent::IndexedMmio {
                                access: MemoryAccess::Write,
                                width,
                                address,
                                registers: domain.registers,
                                guard: domain.guard,
                                value: Some(value),
                            },
                        );
                    } else if state.values[usize::from(base.0)].is_resolved()
                        && state.values[usize::from(base.0)].depends_on_private_stack_read()
                    {
                        state.push_reference_event(
                            pc as u32,
                            DraftReferenceEvent::Memory {
                                access: MemoryAccess::Write,
                                width,
                                address: state.values[usize::from(base.0)]
                                    .clone()
                                    .add_constant(offset.as_u32()),
                                region: DEFERRED_CALLER_MEMORY_REGION.to_owned(),
                                value: Some(value),
                            },
                        );
                    } else {
                        state.reference_blockers.push(format!(
                            "unmodeled-memory-store at {pc:#x}: {instruction}{}; base {} = {}",
                            if symbol.addresses_resolved {
                                ""
                            } else {
                                " (relocatable addresses)"
                            },
                            base,
                            state.values[usize::from(base.0)].canonical(),
                        ));
                    }
                }
            }
        }
        _ => return false,
    }
    true
}
