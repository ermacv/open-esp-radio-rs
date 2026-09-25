//! Timing policy for an ACL PHY handoff; values alone never grant RF access.
//!
//! All budgets are explicit microseconds. They need measured hardware bounds;
//! there is no default thermal deferral limit. Controller time is wrapping and
//! distinct from the monotonic execution clock. The acquisition's earliest
//! monotonic sample bounds their mapping conservatively, including time spent
//! waiting for the actual Controller sample.

use core::num::NonZeroU32;
#[cfg(any(target_arch = "riscv32", test))]
use core::num::NonZeroU64;
#[cfg(any(target_arch = "riscv32", test))]
use oer_esp32s31_phy::tracking::deadline::TrackingDeadline;

/// Observation emitted only after the guarded successor is published to RUN.
/// This does not prove reception of a peer packet or RF quality.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralMaintenanceRun {
    pub admitted_at_micros: u64,
    pub run_at_micros: u64,
    pub restoration_deadline_micros: u64,
    pub event_counter: u16,
}

/// Caller-selected execution, restoration and protocol margin budgets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralMaintenanceBudget {
    execution: NonZeroU32,
    restoration: NonZeroU32,
    protocol_margin: NonZeroU32,
}

impl PeripheralMaintenanceBudget {
    /// Maximum PHY executor occupancy, excluding restoration.
    pub const fn execution_micros(self) -> NonZeroU32 {
        self.execution
    }

    /// Separate reserve for restoring IRQ, timer and protocol execution.
    pub const fn restoration_micros(self) -> NonZeroU32 {
        self.restoration
    }

    /// Reject intervals which cannot be compared within a Controller half-range.
    pub const fn new(
        execution_micros: NonZeroU32,
        restoration_micros: NonZeroU32,
        protocol_margin_micros: NonZeroU32,
    ) -> Option<Self> {
        let total = execution_micros.get() as u64
            + restoration_micros.get() as u64
            + protocol_margin_micros.get() as u64;
        if total >= i32::MAX as u64 {
            return None;
        }
        Some(Self {
            execution: execution_micros,
            restoration: restoration_micros,
            protocol_margin: protocol_margin_micros,
        })
    }
}

/// A missed admission keeps the connection runnable without granting PHY access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeripheralMaintenanceBlocked {
    NotDue,
    Cancelled,
    /// The session is not at an unreserved, CPU-owned successor candidate.
    RadioPhase,
    /// Already restoring an earlier maintenance transaction.
    RestorationPending,
    Clock,
    WindowTooShort,
    /// The original event already needs ordinary latency recovery; it cannot
    /// be counted as an intentional maintenance omission.
    EventAlreadyLate,
    ProtocolDeadline,
    /// The completed event is already being recovered after unrelated RF loss.
    RecoveryInProgress,
    LinkLayer(oer_bluetooth_ll::connection::maintenance::SkipBlocked),
}

#[cfg(any(target_arch = "riscv32", test))]
#[derive(Clone, Copy, Debug)]
pub(crate) struct Window {
    pub(crate) execution: TrackingDeadline,
    pub(crate) restoration: TrackingDeadline,
}

#[cfg(any(target_arch = "riscv32", test))]
impl PeripheralMaintenanceBudget {
    #[allow(
        clippy::too_many_arguments,
        reason = "one sampled mapping and the actual reserved event bounds"
    )]
    pub(crate) fn admit(
        self,
        controller_now: oer_esp32s31_bluetooth::SchedulerInstant,
        event_start: oer_esp32s31_bluetooth::SchedulerInstant,
        event_end: oer_esp32s31_bluetooth::SchedulerInstant,
        acquisition_started: u64,
        monotonic_now: u64,
        late_guard_micros: u32,
        deadlines: super::deadlines::Deadlines,
    ) -> Result<Window, PeripheralMaintenanceBlocked> {
        use PeripheralMaintenanceBlocked as Blocked;
        if monotonic_now < acquisition_started {
            return Err(Blocked::Clock);
        }
        if !controller_now
            .wrapping_add(late_guard_micros)
            .is_before(event_start)
        {
            return Err(Blocked::EventAlreadyLate);
        }
        if !event_start.is_before(event_end) {
            return Err(Blocked::WindowTooShort);
        }
        let protected_end = event_end.wrapping_add(self.protocol_margin.get());
        if !event_end.is_before(protected_end)
            || !controller_now.is_before(protected_end)
            || deadlines.decide(controller_now, protected_end) != super::deadlines::Decision::Run
        {
            return Err(Blocked::ProtocolDeadline);
        }
        let available = event_start.image().wrapping_sub(controller_now.image());
        let available = available
            .checked_sub(late_guard_micros)
            .ok_or(Blocked::WindowTooShort)?;
        let must_restore_before = acquisition_started
            .checked_add(u64::from(available))
            .ok_or(Blocked::Clock)?;
        if monotonic_now >= must_restore_before {
            return Err(Blocked::EventAlreadyLate);
        }
        let execution = TrackingDeadline::new(monotonic_now, NonZeroU64::from(self.execution))
            .ok_or(Blocked::Clock)?;
        let total = u64::from(self.execution.get()) + u64::from(self.restoration.get());
        let restoration = TrackingDeadline::new(monotonic_now, NonZeroU64::new(total).unwrap())
            .ok_or(Blocked::Clock)?;
        if restoration.expires_at_micros() >= must_restore_before {
            return Err(Blocked::WindowTooShort);
        }
        Ok(Window {
            execution,
            restoration,
        })
    }
}

#[cfg(test)]
mod tests;
