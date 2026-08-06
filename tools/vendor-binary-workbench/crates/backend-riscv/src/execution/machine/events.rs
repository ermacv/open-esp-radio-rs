//! Branch, call and observable-event accounting.

use rv_asm::Reg;

use super::super::{ExecutionEvent, ExecutionTimelineEvent, OrderedCall};
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

    pub(super) fn modeled_call_result(&mut self, symbol: &str, site: u32) -> Result<Option<u32>> {
        let Some(responses) = self.call_returns.get_mut(symbol) else {
            return Ok(None);
        };
        responses.pop_front().map(Some).ok_or_else(|| {
            format!(
                "execution reached modeled call {symbol} at {site:#010x} without a remaining response"
            )
            .into()
        })
    }

    pub(in crate::execution) fn record_event(&mut self, event: ExecutionEvent) {
        self.events.push(event.clone());
        self.timeline
            .push(ExecutionTimelineEvent::Observable(event));
    }
}
