//! Branch, call and observable-event accounting.

use rv_asm::Reg;

use super::super::{
    ExecutionEvent, ExecutionProducer, ExecutionTimelineEvent, ModeledCallOutput,
    ModeledCallResponse, OrderedCall, TableLifecycleEvent, execution_stack_contains,
};
use super::Machine;
use crate::Result;

impl Machine<'_> {
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

    pub(in crate::execution) fn record_call(&mut self, site: u32, symbol: String) {
        let arguments =
            core::array::from_fn(|index| self.registers[usize::from(Reg::A0.0) + index]);
        self.calls.insert(symbol.clone());
        let call = OrderedCall {
            site,
            symbol,
            arguments,
        };
        self.ordered_calls.push(call.clone());
        self.timeline.push(ExecutionTimelineEvent::Call(call));
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
        let mut output_arguments = std::collections::BTreeSet::new();
        for output in response.outputs {
            match output {
                ModeledCallOutput::PrivateStackU8 {
                    pointer_argument,
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
                    let address =
                        self.registers[usize::from(Reg::A0.0) + usize::from(pointer_argument)];
                    if !execution_stack_contains(address) {
                        return Err(format!(
                            "modeled call {symbol} at {site:#010x} output argument a{pointer_argument} points outside private stack: {address:#010x}"
                        )
                        .into());
                    }
                    self.write(address, 8, u32::from(value))?;
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
