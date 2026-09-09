//! Delay evidence scoped to individual runtime calibration operations.

pub mod tx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    BusBusy,
    Completion,
    Settle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scope {
    Dcode,
    Rfpll,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    Started {
        requested_micros: u64,
    },
    Completed {
        elapsed_micros: u64,
        lateness_micros: u64,
    },
    Unsupported,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Timing {
    pub count: u16,
    pub requested_micros: u32,
    pub elapsed_micros: u32,
    pub maximum_lateness_micros: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Bus {
    /// Subset of waits caused by BusyAtStart; the rest precede completion reads.
    pub bus_busy: u16,
    pub timing: Timing,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Report {
    pub i2c: Bus,
    pub rfpll_i2c: Bus,
    /// Explicit RFPLL settling/lock delays, separate from its I2C transactions.
    pub rfpll_settle: Timing,
    /// Existing analog RFPLL_LOCK_STATUS samples, decoded without extra reads.
    pub pll_locked: u16,
    pub pll_unlocked: u16,
}

#[derive(Default)]
pub(crate) struct Recorder {
    report: Report,
    active: Option<(Scope, Kind, u64)>,
    invalid: bool,
}

impl Recorder {
    pub(crate) fn observe(&mut self, scope: Scope, kind: Kind, event: Event) {
        match event {
            Event::Started { requested_micros } => {
                if self.active.is_some() || (scope == Scope::Dcode && kind == Kind::Settle) {
                    self.invalid = true;
                    return;
                }
                self.active = Some((scope, kind, requested_micros));
            }
            Event::Completed {
                elapsed_micros,
                lateness_micros,
            } => {
                let Some((started_scope, started_kind, requested)) = self.active.take() else {
                    self.invalid = true;
                    return;
                };
                if (started_scope, started_kind) != (scope, kind)
                    || elapsed_micros.checked_sub(requested) != Some(lateness_micros)
                {
                    self.invalid = true;
                    return;
                }
                let (Ok(requested), Ok(elapsed), Ok(lateness)) = (
                    u32::try_from(requested),
                    u32::try_from(elapsed_micros),
                    u32::try_from(lateness_micros),
                ) else {
                    self.invalid = true;
                    return;
                };
                let timing = match kind {
                    Kind::Settle => &mut self.report.rfpll_settle,
                    Kind::BusBusy | Kind::Completion => {
                        let i2c = match scope {
                            Scope::Dcode => &mut self.report.i2c,
                            Scope::Rfpll => &mut self.report.rfpll_i2c,
                        };
                        if kind == Kind::BusBusy {
                            let Some(next) = i2c.bus_busy.checked_add(1) else {
                                self.invalid = true;
                                return;
                            };
                            i2c.bus_busy = next;
                        }
                        &mut i2c.timing
                    }
                };
                let (Some(count), Some(total_requested), Some(total_elapsed)) = (
                    timing.count.checked_add(1),
                    timing.requested_micros.checked_add(requested),
                    timing.elapsed_micros.checked_add(elapsed),
                ) else {
                    self.invalid = true;
                    return;
                };
                *timing = Timing {
                    count,
                    requested_micros: total_requested,
                    elapsed_micros: total_elapsed,
                    maximum_lateness_micros: timing.maximum_lateness_micros.max(lateness),
                };
            }
            Event::Unsupported => self.invalid = true,
        }
    }

    pub(crate) fn pll_lock(&mut self, locked: bool) {
        let count = if locked {
            &mut self.report.pll_locked
        } else {
            &mut self.report.pll_unlocked
        };
        match count.checked_add(1) {
            Some(next) => *count = next,
            None => self.invalid = true,
        }
    }

    pub(crate) fn report(&self) -> Report {
        self.report
    }
    pub(crate) fn is_complete(&self) -> bool {
        !self.invalid && self.active.is_none()
    }
}

#[cfg(test)]
mod tests;
