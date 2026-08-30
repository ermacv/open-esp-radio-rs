//! Source-owned common scheduler timing policy.

#![forbid(unsafe_code)]

/// Source-owned scheduler timing policy copied by the reviewed Controller init.
///
/// Complete scheduler consumers establish the first value as the late-start
/// guard and the second as the per-item sequence lead. Their physical unit
/// remains intentionally unnamed; both are deltas in the scheduler domain and
/// must pass through the retained Controller time scale before they are mixed
/// with raw item times. Keeping them in a software type prevents the vendor's
/// private eight-byte structure layout from becoming part of the open ABI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerSoftwareConfig {
    late_start_guard_scheduler_delta: u32,
    sequence_lead_scheduler_delta: u32,
    #[cfg(any(target_arch = "riscv32", test))]
    dtm_scheduler_margin: crate::dtm_event_timing::BluetoothDtmSchedulerMargin,
}

impl BluetoothSchedulerSoftwareConfig {
    /// Configuration constructed by the complete ESP32-S31 standalone task.
    pub const fn reviewed_standalone() -> Self {
        Self {
            late_start_guard_scheduler_delta: 40,
            sequence_lead_scheduler_delta: 46,
            #[cfg(any(target_arch = "riscv32", test))]
            dtm_scheduler_margin:
                crate::dtm_event_timing::BluetoothDtmSchedulerMargin::reviewed_standalone(),
        }
    }

    /// Return the source-owned standalone DTM scheduler margin.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn dtm_scheduler_margin(
        self,
    ) -> crate::dtm_event_timing::BluetoothDtmSchedulerMargin {
        self.dtm_scheduler_margin
    }

    /// Scheduler-domain guard used by both insertion deadline checks.
    pub const fn late_start_guard_scheduler_delta(self) -> u32 {
        self.late_start_guard_scheduler_delta
    }

    /// Scheduler-domain lead added to every raw sequence start.
    pub const fn sequence_lead_scheduler_delta(self) -> u32 {
        self.sequence_lead_scheduler_delta
    }
}
