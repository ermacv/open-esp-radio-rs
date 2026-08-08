//! Mutable state owned by one fail-closed structural trace.

use super::*;

pub(super) struct StructuralTraceState {
    pub(super) values: [SymbolicValue; 32],
    pub(super) events: Vec<ObservableEvent>,
    pub(super) reference_events: Vec<DraftReferenceEvent>,
    pub(super) blockers: Vec<String>,
    pub(super) reference_blockers: Vec<String>,
    pub(super) return_value: SymbolicValue,
    pub(super) unresolved_branch: Option<BranchCondition>,
    pub(super) next_mmio_read_token: u32,
    pub(super) next_memory_read_token: u32,
    pub(super) memory_read_sources: BTreeMap<u32, MemoryObjectLocation>,
    pub(super) next_call_token: u32,
    pub(super) next_external_call_token: u32,
    pub(super) next_private_stack_read_token: u32,
    pub(super) stack: SymbolicStack,
    pub(super) private_stack_may_be_modified_by_call: bool,
}

impl StructuralTraceState {
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
            events: Vec::new(),
            reference_events: Vec::new(),
            blockers: Vec::new(),
            reference_blockers: Vec::new(),
            return_value: SymbolicValue::Unknown,
            unresolved_branch: None,
            next_mmio_read_token: 0,
            next_memory_read_token: 0,
            memory_read_sources: BTreeMap::new(),
            next_call_token: 0,
            next_external_call_token: 0,
            next_private_stack_read_token: 0,
            stack,
            private_stack_may_be_modified_by_call: false,
        }
    }

    pub(super) fn checkpoint(&self) -> StructuralCheckpoint {
        StructuralCheckpoint {
            events_len: self.events.len(),
            reference_events_len: self.reference_events.len(),
            blockers_len: self.blockers.len(),
            reference_blockers_len: self.reference_blockers.len(),
            next_mmio_read_token: self.next_mmio_read_token,
            next_memory_read_token: self.next_memory_read_token,
            memory_read_sources: self.memory_read_sources.clone(),
            next_call_token: self.next_call_token,
            next_external_call_token: self.next_external_call_token,
            stack: self.stack.clone(),
        }
    }

    pub(super) fn restore_checkpoint(&mut self, checkpoint: StructuralCheckpoint) {
        self.events.truncate(checkpoint.events_len);
        self.reference_events
            .truncate(checkpoint.reference_events_len);
        self.blockers.truncate(checkpoint.blockers_len);
        self.reference_blockers
            .truncate(checkpoint.reference_blockers_len);
        self.next_mmio_read_token = checkpoint.next_mmio_read_token;
        self.next_memory_read_token = checkpoint.next_memory_read_token;
        self.memory_read_sources = checkpoint.memory_read_sources;
        self.next_call_token = checkpoint.next_call_token;
        self.next_external_call_token = checkpoint.next_external_call_token;
        self.stack = checkpoint.stack;
    }

    pub(super) fn finish(
        mut self,
        symbol: &artifact::ArtifactSymbolDefinition,
    ) -> FunctionAnalysis {
        let private_stack_crosses_call_boundary =
            self.reference_events.iter().any(|event| match event {
                DraftReferenceEvent::Call { arguments, .. }
                | DraftReferenceEvent::TailCall { arguments, .. } => arguments
                    .iter()
                    .any(|argument| argument.private_stack_offset().is_some()),
                _ => false,
            });
        if !private_stack_crosses_call_boundary
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
