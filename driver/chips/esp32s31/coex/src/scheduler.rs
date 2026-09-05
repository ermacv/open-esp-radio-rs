//! Executor-neutral internal coexistence schedule state.
//!
//! The vendor implementation stores this state behind an RTOS semaphore.
//! This module owns the state itself; an async/runtime adapter that shares it
//! across tasks must provide its own mutex. Keeping that policy outside the
//! model replaces the semaphore without importing RTOS vocabulary into the
//! generic coexistence core.

/// One opaque four-byte coexistence phase record.
///
/// The vendor scheduler proves the record stride and its ownership, but the
/// individual byte meanings are not yet recovered. Callers can retain and
/// identify a record without manufacturing a semantic interpretation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexPhase {
    image: [u8; 4],
}

impl CoexPhase {
    pub const fn from_reviewed_image(image: [u8; 4]) -> Self {
        Self { image }
    }

    /// Read-only diagnostic image. This cannot be written to MMIO.
    pub const fn image(self) -> [u8; 4] {
        self.image
    }
}

/// One source-owned schedule: a period selector and a bounded phase slice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexSchedule<'phases> {
    period: u8,
    phases: &'phases [CoexPhase],
}

impl<'phases> CoexSchedule<'phases> {
    pub const fn new(period: u8, phases: &'phases [CoexPhase]) -> Self {
        Self { period, phases }
    }

    pub const fn period(self) -> u8 {
        self.period
    }

    pub const fn phases(self) -> &'phases [CoexPhase] {
        self.phases
    }
}

/// Source-owned projection of `coex_schm_env` state used by public scheduler
/// accessors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexScheduler<'schedule> {
    interval: u32,
    current: Option<&'schedule CoexSchedule<'schedule>>,
    phase_index: u8,
}

impl<'schedule> CoexScheduler<'schedule> {
    pub const fn new() -> Self {
        Self {
            interval: 0,
            current: None,
            phase_index: 0,
        }
    }

    pub const fn interval(&self) -> u32 {
        self.interval
    }

    pub fn set_interval(&mut self, interval: u32) {
        self.interval = interval;
    }

    pub fn activate(&mut self, schedule: &'schedule CoexSchedule<'schedule>) {
        self.current = Some(schedule);
        self.phase_index = 0;
    }

    pub fn deactivate(&mut self) {
        self.current = None;
        self.phase_index = 0;
    }

    pub fn set_phase_index(&mut self, index: u8) {
        self.phase_index = index;
    }

    pub const fn phase_index(&self) -> u8 {
        self.phase_index
    }

    /// Return the vendor-compatible default period (`1`) when no schedule is
    /// active, otherwise the active schedule's period byte.
    pub const fn current_period(&self) -> u8 {
        match self.current {
            Some(schedule) => schedule.period,
            None => 1,
        }
    }

    /// Return the current opaque phase only when both a schedule is active and
    /// its phase index is inside the reviewed phase slice.
    pub fn current_phase(&self) -> Option<&'schedule CoexPhase> {
        self.current
            .and_then(|schedule| schedule.phases.get(usize::from(self.phase_index)))
    }
}

impl Default for CoexScheduler<'_> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
