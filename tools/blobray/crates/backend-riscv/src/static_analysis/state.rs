//! Mutable state owned by one fail-closed structural trace.

use super::*;

#[derive(Clone)]
pub(super) struct StructuralTraceState {
    pub(super) values: [SymbolicValue; 32],
    pub(super) floating_values: [SymbolicValue; 32],
    pub(super) events: Vec<ObservableEvent>,
    pub(super) located_events: Vec<LocatedObservableEvent>,
    pub(super) located_reference_events: Vec<LocatedReferenceEvent>,
    pub(super) reference_events: Vec<DraftReferenceEvent>,
    pub(super) blockers: Vec<String>,
    pub(super) reference_blockers: Vec<String>,
    pub(super) return_value: SymbolicValue,
    pub(super) unresolved_branch: Option<BranchCondition>,
    pub(super) next_mmio_read_token: u32,
    pub(super) next_memory_read_token: u32,
    pub(super) memory_read_sources: std::sync::Arc<BTreeMap<u32, MemoryObjectLocation>>,
    /// Exact fresh-allocation pointers most recently stored in statically
    /// identified global pointer cells on this one explored path.
    ///
    /// This is deliberately not a generic RAM value cache. Unknown aliases
    /// and calls invalidate it, and only an exact 32-bit load of the same
    /// static cell may consume it.
    allocation_pointer_cells: std::sync::Arc<BTreeMap<MemoryObjectLocation, Option<SymbolicValue>>>,
    pub(super) next_call_token: u32,
    pub(super) next_external_call_token: u32,
    pub(super) next_private_stack_read_token: u32,
    pub(super) stack: std::sync::Arc<SymbolicStack>,
    pub(super) private_stack_may_be_modified_by_call: bool,
}

impl StructuralTraceState {
    /// Preserve the instruction site for directly observed memory effects
    /// before higher-level reference-flow recovery restructures the event
    /// sequence.
    pub(super) fn push_reference_event(&mut self, site: u32, event: DraftReferenceEvent) {
        if matches!(
            event,
            DraftReferenceEvent::Observable(ObservableEvent::Memory { .. })
                | DraftReferenceEvent::Memory { .. }
                | DraftReferenceEvent::IndexedMmio { .. }
                | DraftReferenceEvent::PollMmio { .. }
                | DraftReferenceEvent::PrivateStackLoad { .. }
                | DraftReferenceEvent::PrivateStackStore { .. }
                | DraftReferenceEvent::ModeledDirectCall { .. }
                | DraftReferenceEvent::ReviewedExternalCall { .. }
                | DraftReferenceEvent::Call { .. }
                | DraftReferenceEvent::TailCall { .. }
        ) {
            self.located_reference_events.push(LocatedReferenceEvent {
                site,
                event: event.clone(),
            });
        }
        self.reference_events.push(event);
    }

    pub(super) fn locate_reference_events_since(&mut self, site: u32, start: usize) {
        for event in &self.reference_events[start..] {
            if matches!(
                event,
                DraftReferenceEvent::Observable(ObservableEvent::Memory { .. })
                    | DraftReferenceEvent::Memory { .. }
                    | DraftReferenceEvent::IndexedMmio { .. }
                    | DraftReferenceEvent::PollMmio { .. }
                    | DraftReferenceEvent::PrivateStackLoad { .. }
                    | DraftReferenceEvent::PrivateStackStore { .. }
                    | DraftReferenceEvent::ModeledDirectCall { .. }
                    | DraftReferenceEvent::ReviewedExternalCall { .. }
                    | DraftReferenceEvent::Call { .. }
                    | DraftReferenceEvent::TailCall { .. }
            ) {
                self.located_reference_events.push(LocatedReferenceEvent {
                    site,
                    event: event.clone(),
                });
            }
        }
    }

    pub(super) fn new(specialized_arguments: Option<&Rv32CallArguments>) -> Self {
        let mut values = core::array::from_fn(|_| SymbolicValue::Unknown);
        values[0] = SymbolicValue::Constant(0);
        values[usize::from(Reg::SP.0)] = SymbolicValue::StackAddress(0);
        for index in 0..RV32_REGISTER_ARGUMENT_COUNT {
            values[10 + index] = specialized_arguments
                .and_then(|arguments| arguments[index].as_constant())
                .map_or_else(
                    || SymbolicValue::input(index as u8),
                    |value| SymbolicValue::InputConstant {
                        index: index as u8,
                        value,
                    },
                );
        }

        let mut stack = SymbolicStack::default();
        for index in 0..RV32_STACK_ARGUMENT_COUNT {
            let argument_index = RV32_REGISTER_ARGUMENT_COUNT + index;
            let value = specialized_arguments
                .and_then(|arguments| arguments[argument_index].as_constant())
                .map_or_else(
                    || SymbolicValue::input(argument_index as u8),
                    SymbolicValue::Constant,
                );
            stack.store((index * 4) as i32, 32, &value);
        }

        Self {
            values,
            floating_values: core::array::from_fn(|_| SymbolicValue::Unknown),
            events: Vec::new(),
            located_events: Vec::new(),
            located_reference_events: Vec::new(),
            reference_events: Vec::new(),
            blockers: Vec::new(),
            reference_blockers: Vec::new(),
            return_value: SymbolicValue::Unknown,
            unresolved_branch: None,
            next_mmio_read_token: 0,
            next_memory_read_token: 0,
            memory_read_sources: std::sync::Arc::default(),
            allocation_pointer_cells: std::sync::Arc::default(),
            next_call_token: 0,
            next_external_call_token: 0,
            next_private_stack_read_token: 0,
            stack: std::sync::Arc::new(stack),
            private_stack_may_be_modified_by_call: false,
        }
    }

    pub(super) fn invalidate_floating_call_clobbers(&mut self) {
        for register in (0..=7).chain(10..=17).chain(28..=31) {
            self.floating_values[register] = SymbolicValue::Unknown;
        }
    }

    fn static_pointer_cell(address: &SymbolicValue) -> Option<MemoryObjectLocation> {
        let location = address.memory_object_location_with_reads(&BTreeMap::new())?;
        matches!(
            location.root,
            MemoryObjectRoot::RelocatedSymbol { .. } | MemoryObjectRoot::Absolute { .. }
        )
        .then_some(location)
    }

    fn fresh_allocation_pointer(value: &SymbolicValue) -> bool {
        matches!(
            value.memory_object_location_with_reads(&BTreeMap::new()),
            Some(MemoryObjectLocation {
                root: MemoryObjectRoot::Allocation { .. }
                    | MemoryObjectRoot::ZeroedAllocation { .. },
                offset: 0,
            })
        )
    }

    pub(super) fn forwarded_allocation_pointer(
        &self,
        address: &SymbolicValue,
        width: u8,
    ) -> Option<SymbolicValue> {
        if width != 32 {
            return None;
        }
        let cell = Self::static_pointer_cell(address)?;
        self.allocation_pointer_cells.get(&cell).cloned().flatten()
    }

    pub(super) fn allocation_pointer_cell_was_written(
        &self,
        address: &SymbolicValue,
        width: u8,
    ) -> bool {
        if width != 32 {
            return false;
        }
        Self::static_pointer_cell(address)
            .is_some_and(|cell| self.allocation_pointer_cells.contains_key(&cell))
    }

    /// Observe one exact write to static RAM. Any prior cell whose physical
    /// relationship to this write is not provably disjoint is discarded.
    /// The written value is retained only when this is an exact pointer-sized
    /// store of a reviewed fresh allocation base.
    pub(super) fn observe_static_pointer_cell_write(
        &mut self,
        address: &SymbolicValue,
        width: u8,
        value: &SymbolicValue,
    ) {
        let Some(cell) = Self::static_pointer_cell(address) else {
            self.invalidate_allocation_pointer_cells();
            return;
        };
        let cells = std::sync::Arc::make_mut(&mut self.allocation_pointer_cells);
        for (existing, forwarded) in cells.iter_mut() {
            if !static_cells_provably_disjoint(existing, &cell, width) {
                *forwarded = None;
            }
        }
        let forwarded =
            (width == 32 && Self::fresh_allocation_pointer(value)).then(|| value.clone());
        cells.insert(cell, forwarded);
    }

    pub(super) fn observe_nonstatic_memory_write(&mut self, address: &SymbolicValue) {
        let fresh_allocation_object = address
            .memory_object_location_with_reads(&self.memory_read_sources)
            .is_some_and(|location| {
                matches!(
                    location.root,
                    MemoryObjectRoot::Allocation { .. } | MemoryObjectRoot::ZeroedAllocation { .. }
                )
            });
        if !fresh_allocation_object {
            self.invalidate_allocation_pointer_cells();
        }
    }

    pub(super) fn observe_memory_write(
        &mut self,
        address: &SymbolicValue,
        width: u8,
        value: &SymbolicValue,
    ) {
        if Self::static_pointer_cell(address).is_some() {
            self.observe_static_pointer_cell_write(address, width, value);
        } else {
            self.observe_nonstatic_memory_write(address);
        }
    }

    pub(super) fn invalidate_allocation_pointer_cells(&mut self) {
        for forwarded in std::sync::Arc::make_mut(&mut self.allocation_pointer_cells).values_mut() {
            *forwarded = None;
        }
    }

    pub(super) fn checkpoint(&self) -> StructuralCheckpoint {
        StructuralCheckpoint {
            events_len: self.events.len(),
            located_events_len: self.located_events.len(),
            located_reference_events_len: self.located_reference_events.len(),
            reference_events_len: self.reference_events.len(),
            blockers_len: self.blockers.len(),
            reference_blockers_len: self.reference_blockers.len(),
            next_mmio_read_token: self.next_mmio_read_token,
            next_memory_read_token: self.next_memory_read_token,
            memory_read_sources: self.memory_read_sources.clone(),
            allocation_pointer_cells: self.allocation_pointer_cells.clone(),
            next_call_token: self.next_call_token,
            next_external_call_token: self.next_external_call_token,
            stack: self.stack.clone(),
        }
    }

    pub(super) fn restore_checkpoint(&mut self, checkpoint: StructuralCheckpoint) {
        self.events.truncate(checkpoint.events_len);
        self.located_events.truncate(checkpoint.located_events_len);
        self.located_reference_events
            .truncate(checkpoint.located_reference_events_len);
        self.reference_events
            .truncate(checkpoint.reference_events_len);
        self.blockers.truncate(checkpoint.blockers_len);
        self.reference_blockers
            .truncate(checkpoint.reference_blockers_len);
        self.next_mmio_read_token = checkpoint.next_mmio_read_token;
        self.next_memory_read_token = checkpoint.next_memory_read_token;
        self.memory_read_sources = checkpoint.memory_read_sources;
        self.allocation_pointer_cells = checkpoint.allocation_pointer_cells;
        self.next_call_token = checkpoint.next_call_token;
        self.next_external_call_token = checkpoint.next_external_call_token;
        self.stack = checkpoint.stack;
    }

    pub(super) fn finish(
        mut self,
        symbol: &artifact::ArtifactSymbolDefinition,
        preserve_private_stack_stores: bool,
    ) -> FunctionAnalysis {
        let private_stack_crosses_call_boundary =
            self.reference_events.iter().any(|event| match event {
                DraftReferenceEvent::Call { arguments, .. }
                | DraftReferenceEvent::TailCall { arguments, .. } => arguments
                    .iter()
                    .any(|argument| argument.private_stack_offset().is_some()),
                _ => false,
            });
        if !preserve_private_stack_stores
            && !private_stack_crosses_call_boundary
            && !self
                .reference_events
                .iter()
                .any(|event| matches!(event, DraftReferenceEvent::PrivateStackLoad { .. }))
        {
            self.reference_events
                .retain(|event| !matches!(event, DraftReferenceEvent::PrivateStackStore { .. }));
        }

        FunctionAnalysis {
            symbol: symbol.name.clone(),
            events: self.events,
            located_events: self.located_events,
            located_reference_events: self.located_reference_events,
            reference_events: self.reference_events,
            reference_dependencies: Vec::new(),
            blockers: self.blockers,
            reference_blockers: self.reference_blockers,
            return_value: self.return_value,
            reference_flow: None,
            unresolved_branch: self.unresolved_branch,
        }
    }
}

fn static_cells_provably_disjoint(
    existing: &MemoryObjectLocation,
    write: &MemoryObjectLocation,
    write_width: u8,
) -> bool {
    let width_bytes = i128::from(write_width / 8);
    match (&existing.root, &write.root) {
        (
            MemoryObjectRoot::Absolute {
                address: existing_address,
            },
            MemoryObjectRoot::Absolute {
                address: write_address,
            },
        ) => {
            let existing_start = i128::from(*existing_address) + i128::from(existing.offset);
            let write_start = i128::from(*write_address) + i128::from(write.offset);
            existing_start + 4 <= write_start || write_start + width_bytes <= existing_start
        }
        (existing_root, write_root) if existing_root == write_root => {
            let existing_start = i128::from(existing.offset);
            let write_start = i128::from(write.offset);
            existing_start + 4 <= write_start || write_start + width_bytes <= existing_start
        }
        // Distinct symbolic roots and symbolic-vs-absolute addresses may be
        // linker aliases. Without an exact address proof their relationship is
        // ambiguous, so the earlier forwarding fact must not survive.
        _ => false,
    }
}
