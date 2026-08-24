//! Branch, call and observable-event accounting.

use rv_asm::Reg;

use super::super::{
    AllocationLifecycleEvent, ExecutionEvent, ExecutionProducer, ExecutionTimelineEvent,
    FifoLifecycleEvent, FifoServiceOperation, MemoryOwner, MemoryOwnership, MemoryRange, MmioValue,
    ModeledCallOutput, ModeledCallResponse, OrderedCall, ServiceOutput, ServiceValueSource,
    TableLifecycleEvent, execution_stack_contains,
};
use super::Machine;
use crate::Result;

impl Machine<'_> {
    pub(in crate::execution) fn apply_fifo_service_call(
        &mut self,
        symbol: &str,
        site: u32,
    ) -> Result<bool> {
        let Some(binding) = self.fifo_bindings.get(symbol).cloned() else {
            return Ok(false);
        };
        if binding.handle_argument >= 8 {
            return Err(format!(
                "FIFO binding {symbol} handle argument a{} exceeds RV32 a0..a7",
                binding.handle_argument
            )
            .into());
        }
        let handle = self.registers[usize::from(Reg::A0.0) + usize::from(binding.handle_argument)];
        let service_index = self
            .fifo_services
            .iter()
            .position(|service| service.id == binding.service_id)
            .ok_or_else(|| {
                format!(
                    "FIFO binding {symbol} refers to missing service {}",
                    binding.service_id
                )
            })?;
        if self.fifo_services[service_index].handle != handle {
            return Err(format!(
                "FIFO binding {symbol} expected handle {:#010x} for service {}, received {handle:#010x}",
                self.fifo_services[service_index].handle, binding.service_id
            )
            .into());
        }

        match binding.operation {
            FifoServiceOperation::Enqueue {
                item,
                success_return,
                full_return,
                wake_output,
            } => {
                let value = self.read_service_value(symbol, site, item)?;
                let service = &mut self.fifo_services[service_index];
                validate_fifo_service(service)?;
                validate_service_value(service.item_width, value, symbol, site)?;
                let depth_before = service.items.len();
                if depth_before == service.capacity {
                    self.registers[usize::from(Reg::A0.0)] = full_return;
                    self.fifo_lifecycle.push(FifoLifecycleEvent::Full {
                        service_id: service.id.clone(),
                        site,
                        value,
                        depth: depth_before,
                    });
                    if let Some(output) = wake_output {
                        self.write_service_output(symbol, site, output, 0)?;
                    }
                } else {
                    let woke_receiver = depth_before == 0;
                    service.items.push(value);
                    let service_id = service.id.clone();
                    let depth_after = service.items.len();
                    self.registers[usize::from(Reg::A0.0)] = success_return;
                    self.fifo_lifecycle.push(FifoLifecycleEvent::Enqueued {
                        service_id,
                        site,
                        value,
                        depth_before,
                        depth_after,
                        woke_receiver,
                    });
                    if let Some(output) = wake_output {
                        self.write_service_output(symbol, site, output, u32::from(woke_receiver))?;
                    }
                }
            }
            FifoServiceOperation::Dequeue {
                output,
                success_return,
                empty_return,
            } => {
                let service = &mut self.fifo_services[service_index];
                validate_fifo_service(service)?;
                let depth_before = service.items.len();
                if service.items.is_empty() {
                    self.registers[usize::from(Reg::A0.0)] = empty_return;
                    self.fifo_lifecycle.push(FifoLifecycleEvent::Empty {
                        service_id: service.id.clone(),
                        site,
                    });
                } else {
                    let value = service.items.remove(0);
                    let service_id = service.id.clone();
                    let depth_after = service.items.len();
                    self.write_service_output(symbol, site, output, value)?;
                    self.registers[usize::from(Reg::A0.0)] = success_return;
                    self.fifo_lifecycle.push(FifoLifecycleEvent::Dequeued {
                        service_id,
                        site,
                        value,
                        depth_before,
                        depth_after,
                    });
                }
            }
            FifoServiceOperation::Len => {
                let service = &self.fifo_services[service_index];
                validate_fifo_service(service)?;
                let depth = service.items.len();
                self.registers[usize::from(Reg::A0.0)] =
                    u32::try_from(depth).map_err(|_| "FIFO depth exceeds RV32 return value")?;
                self.fifo_lifecycle.push(FifoLifecycleEvent::Length {
                    service_id: service.id.clone(),
                    site,
                    depth,
                });
            }
        }
        Ok(true)
    }

    fn read_service_value(
        &mut self,
        symbol: &str,
        site: u32,
        source: ServiceValueSource,
    ) -> Result<u32> {
        match source {
            ServiceValueSource::Argument { argument, width } => {
                validate_service_argument(argument, width, symbol, site)?;
                let value = self.registers[usize::from(Reg::A0.0) + usize::from(argument)];
                validate_service_value(width, value, symbol, site)?;
                Ok(value)
            }
            ServiceValueSource::PrivateStackPointer {
                pointer_argument,
                width,
            } => {
                validate_service_argument(pointer_argument, width, symbol, site)?;
                let address =
                    self.registers[usize::from(Reg::A0.0) + usize::from(pointer_argument)];
                if !execution_stack_contains(address) {
                    return Err(format!(
                        "FIFO binding {symbol} at {site:#010x} input a{pointer_argument} points outside private stack: {address:#010x}"
                    )
                    .into());
                }
                self.read(address, width)
            }
        }
    }

    fn write_service_output(
        &mut self,
        symbol: &str,
        site: u32,
        output: ServiceOutput,
        value: u32,
    ) -> Result<()> {
        let ServiceOutput::PrivateStackPointer {
            pointer_argument,
            width,
        } = output;
        validate_service_argument(pointer_argument, width, symbol, site)?;
        validate_service_value(width, value, symbol, site)?;
        let address = self.registers[usize::from(Reg::A0.0) + usize::from(pointer_argument)];
        if !execution_stack_contains(address) {
            return Err(format!(
                "FIFO binding {symbol} at {site:#010x} output a{pointer_argument} points outside private stack: {address:#010x}"
            )
            .into());
        }
        self.write(address, width, value)
    }

    pub(in crate::execution) fn branch(&mut self, taken: bool, offset: i32, width: u32) {
        self.branches.insert((self.pc, taken));
        self.ordered_branches.push((self.pc, taken));
        self.timeline.push(ExecutionTimelineEvent::Branch {
            site: self.pc,
            taken,
        });
        self.pc = if taken {
            self.pc.wrapping_add(offset as u32)
        } else {
            self.pc.wrapping_add(width)
        };
    }

    fn call_arguments(&self, argument_count: u8) -> [u32; 8] {
        core::array::from_fn(|index| {
            if index < usize::from(argument_count) {
                self.registers[usize::from(Reg::A0.0) + index]
            } else {
                0
            }
        })
    }

    fn record_call_with_arguments(&mut self, site: u32, symbol: String, arguments: [u32; 8]) {
        self.calls.insert(symbol.clone());
        let call = OrderedCall {
            site,
            symbol,
            arguments,
        };
        self.ordered_calls.push(call.clone());
        self.timeline.push(ExecutionTimelineEvent::Call(call));
    }

    pub(in crate::execution) fn record_call(&mut self, site: u32, symbol: String) {
        let arguments = self.call_arguments(8);
        self.record_call_with_arguments(site, symbol, arguments);
    }

    pub(in crate::execution) fn record_diagnostic_call(
        &mut self,
        site: u32,
        symbol: String,
        argument_count: u8,
    ) -> [u32; 8] {
        let arguments = self.call_arguments(argument_count);
        self.record_call_with_arguments(site, symbol, arguments);
        arguments
    }

    pub(in crate::execution) fn record_indirect_table_call(
        &mut self,
        site: u32,
        target: u32,
        symbol: &str,
    ) {
        if self.table_layouts.is_empty() {
            return;
        }
        let mut candidates = Vec::new();
        for layout in &self.table_layouts {
            for offset in (0..layout.layout_size).step_by(4) {
                let address = layout.base_address.wrapping_add(offset);
                let value = (0..4).try_fold(0_u32, |value, byte| {
                    self.normal_byte(address.wrapping_add(byte))
                        .ok()
                        .map(|part| value | (u32::from(part) << (byte * 8)))
                });
                if value == Some(target) {
                    candidates.push((layout.layout_id.clone(), offset));
                }
            }
        }
        let (layout_id, slot_offset) = if candidates.len() == 1 {
            let (layout, offset) = candidates.pop().expect("one candidate");
            (Some(layout), Some(offset))
        } else {
            self.table_lifecycle_complete = false;
            (None, None)
        };
        self.table_lifecycle
            .push(TableLifecycleEvent::IndirectCall {
                layout_id,
                slot_offset,
                site,
                target,
                symbol: symbol.to_owned(),
            });
    }

    pub(super) fn modeled_call_response(
        &mut self,
        symbol: &str,
        site: u32,
    ) -> Result<Option<ModeledCallResponse>> {
        let Some(responses) = self.call_responses.get_mut(symbol) else {
            return Ok(None);
        };
        responses.pop_front().map(Some).ok_or_else(|| {
            format!(
                "execution reached modeled call {symbol} at {site:#010x} without a remaining response"
            )
            .into()
        })
    }

    pub(in crate::execution) fn apply_modeled_call_response(
        &mut self,
        symbol: &str,
        site: u32,
        response: ModeledCallResponse,
    ) -> Result<()> {
        if let Some(allocation) = response.allocation {
            const MAX_MODELED_ALLOCATION_BYTES: u32 = 1024 * 1024;
            if allocation.size_argument >= 8 {
                return Err(format!(
                    "modeled call {symbol} at {site:#010x} allocation size argument a{} exceeds RV32 a0..a7",
                    allocation.size_argument
                )
                .into());
            }
            if allocation.capacity == 0
                || allocation.capacity > MAX_MODELED_ALLOCATION_BYTES
                || allocation.address & 3 != 0
            {
                return Err(format!(
                    "modeled call {symbol} at {site:#010x} has invalid allocation arena {:#010x}+{:#x}",
                    allocation.address, allocation.capacity
                )
                .into());
            }
            let requested =
                self.registers[usize::from(Reg::A0.0) + usize::from(allocation.size_argument)];
            if requested == 0 || requested > allocation.capacity {
                return Err(format!(
                    "modeled call {symbol} at {site:#010x} requested {requested:#x} allocation bytes from capacity {:#x}",
                    allocation.capacity
                )
                .into());
            }
            let end = allocation
                .address
                .checked_add(allocation.capacity)
                .ok_or_else(|| {
                    format!(
                        "modeled call {symbol} at {site:#010x} allocation arena overflows address space"
                    )
                })?;
            for address in allocation.address..end {
                if execution_stack_contains(address)
                    || self.image.contains_memory(address)
                    || self.svd.intersects_mmio(address, 8)
                    || self.initial_overlay.contains_key(&address)
                {
                    return Err(format!(
                        "modeled call {symbol} at {site:#010x} allocation arena overlaps existing memory at {address:#010x}"
                    )
                    .into());
                }
            }
            for address in allocation.address..allocation.address + requested {
                self.initial_overlay.insert(address, 0);
                self.overlay.insert(address, 0);
            }
            self.memory_ownership.push(MemoryOwnership {
                range: MemoryRange {
                    start: allocation.address,
                    length: requested,
                },
                owner: MemoryOwner::Cpu,
            });
            self.allocations.push(AllocationLifecycleEvent {
                site,
                symbol: symbol.to_owned(),
                address: allocation.address,
                requested,
                capacity: allocation.capacity,
                zeroed: true,
            });
            if response.return_words[0].is_some() {
                return Err(format!(
                    "modeled call {symbol} at {site:#010x} allocation conflicts with explicit a0 return"
                )
                .into());
            }
            self.registers[usize::from(Reg::A0.0)] = allocation.address;
        }
        let mut output_arguments = std::collections::BTreeSet::new();
        for output in response.outputs {
            match output {
                ModeledCallOutput::PrivateStack {
                    pointer_argument,
                    width,
                    value,
                } => {
                    if pointer_argument >= 8 {
                        return Err(format!(
                            "modeled call {symbol} at {site:#010x} output argument a{pointer_argument} exceeds RV32 a0..a7"
                        )
                        .into());
                    }
                    if !output_arguments.insert(pointer_argument) {
                        return Err(format!(
                            "modeled call {symbol} at {site:#010x} repeats output argument a{pointer_argument}"
                        )
                        .into());
                    }
                    if !matches!(width, 8 | 16 | 32) || value & !MmioValue::mask(width) != 0 {
                        return Err(format!(
                            "modeled call {symbol} at {site:#010x} has invalid {width}-bit output value {value:#010x}"
                        )
                        .into());
                    }
                    let address =
                        self.registers[usize::from(Reg::A0.0) + usize::from(pointer_argument)];
                    if !execution_stack_contains(address) {
                        return Err(format!(
                            "modeled call {symbol} at {site:#010x} output argument a{pointer_argument} points outside private stack: {address:#010x}"
                        )
                        .into());
                    }
                    self.write(address, width, value)?;
                }
            }
        }
        for (index, value) in response.return_words.into_iter().enumerate() {
            if let Some(value) = value {
                self.registers[usize::from(Reg::A0.0) + index] = value;
            }
        }
        Ok(())
    }

    pub(in crate::execution) fn record_event(&mut self, event: ExecutionEvent) {
        let producer = self.image.symbol_containing(self.pc);
        self.event_producers.push(ExecutionProducer {
            pc: self.pc,
            symbol: producer.map(|(_, symbol)| symbol.to_owned()),
            symbol_offset: producer.map(|(start, _)| self.pc.wrapping_sub(start)),
        });
        self.events.push(event.clone());
        self.timeline
            .push(ExecutionTimelineEvent::Observable(event));
    }
}

fn validate_fifo_service(service: &super::super::FifoServiceInstance) -> Result<()> {
    if service.id.trim().is_empty()
        || service.handle == 0
        || service.capacity == 0
        || service.items.len() > service.capacity
        || !matches!(service.item_width, 8 | 16 | 32)
    {
        return Err(format!("invalid FIFO service instance {:?}", service.id).into());
    }
    for value in &service.items {
        validate_service_value(service.item_width, *value, &service.id, 0)?;
    }
    Ok(())
}

fn validate_service_argument(argument: u8, width: u8, symbol: &str, site: u32) -> Result<()> {
    if argument >= 8 || !matches!(width, 8 | 16 | 32) {
        return Err(format!(
            "FIFO binding {symbol} at {site:#010x} has invalid a{argument}/{width}-bit ABI value"
        )
        .into());
    }
    Ok(())
}

fn validate_service_value(width: u8, value: u32, symbol: &str, site: u32) -> Result<()> {
    if !matches!(width, 8 | 16 | 32) || value & !MmioValue::mask(width) != 0 {
        return Err(format!(
            "FIFO binding {symbol} at {site:#010x} has invalid {width}-bit value {value:#010x}"
        )
        .into());
    }
    Ok(())
}
