//! Aggregate observations of the shared timer, not per-PHY timer attribution.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TimerPhaseTiming {
    pub count: u32,
    pub total_micros: u64,
    pub maximum_micros: u32,
}
impl TimerPhaseTiming {
    fn fits(self, window: u64) -> bool {
        self.total_micros <= window
            && u64::from(self.maximum_micros) <= self.total_micros
            && (self.count != 0 || self == Self::default())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TimerWindowEvidence {
    pub elapsed_micros: u64,
    pub invalid: bool,
    pub registrations: u32,
    pub due_at_registration: u32,
    pub due_at_program_start: u32,
    pub due_at_program_return: u32,
    pub irq_ack: TimerPhaseTiming,
    pub programming: TimerPhaseTiming,
    pub alarm_to_irq: TimerPhaseTiming,
    pub deadline_lateness: TimerPhaseTiming,
    pub irq_to_dispatch: TimerPhaseTiming,
    pub dispatch: TimerPhaseTiming,
    pub interrupts: u32,
    pub replaced: u32,
    pub stopped: u32,
    pub unmatched_interrupts: u32,
    /// IRQ entry precedes completion of an alarm program observed before acknowledgement.
    pub overlapping_interrupts: u32,
    pub unmatched_dispatches: u32,
    pub coalesced_interrupts: u32,
    pub early_interrupts: u32,
    pub armed_at_end: bool,
    pub irq_pending_at_end: bool,
}
impl TimerWindowEvidence {
    pub fn is_valid(&self) -> bool {
        let matched = self
            .interrupts
            .checked_sub(self.unmatched_interrupts)
            .and_then(|count| count.checked_sub(self.overlapping_interrupts));
        !self.invalid
            && [
                self.irq_ack,
                self.programming,
                self.alarm_to_irq,
                self.irq_to_dispatch,
                self.dispatch,
            ]
            .iter()
            .all(|timing| timing.fits(self.elapsed_micros))
            && self.deadline_lateness.fits(u64::MAX)
            && self.due_at_registration <= self.registrations
            && self.due_at_program_start <= self.due_at_program_return
            && self.due_at_program_return <= self.programming.count
            && self.irq_ack.count == self.interrupts
            && matched == Some(self.alarm_to_irq.count)
            && self.deadline_lateness.count == self.alarm_to_irq.count
            && self.early_interrupts <= self.alarm_to_irq.count
            && self
                .alarm_to_irq
                .count
                .checked_add(self.overlapping_interrupts)
                .and_then(|sum| sum.checked_add(self.replaced))
                .and_then(|sum| sum.checked_add(self.stopped))
                .and_then(|sum| sum.checked_add(u32::from(self.armed_at_end)))
                == Some(self.programming.count)
            && self
                .irq_to_dispatch
                .count
                .checked_add(self.unmatched_dispatches)
                == Some(self.dispatch.count)
            && self
                .irq_to_dispatch
                .count
                .checked_add(self.coalesced_interrupts)
                .and_then(|sum| sum.checked_add(u32::from(self.irq_pending_at_end)))
                == Some(self.interrupts)
    }
}

#[cfg(test)]
mod tests;
