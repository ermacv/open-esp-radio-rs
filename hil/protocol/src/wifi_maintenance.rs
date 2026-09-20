//! Disjoint software intervals of connected Wi-Fi maintenance.
use serde::{Deserialize, Serialize};

/// Composition-clock microseconds for one completed physical transaction.
///
/// These are software ownership boundaries, not RF-off observations or WCET
/// bounds. Request queuing is excluded. Nested PHY timings overlap this
/// timeline and must not be added to its intervals.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StationPauseTimeline {
    /// Maintenance selected, before watchdog admission and TX drain request.
    pub requested: u64,
    /// Drained worker returned its owner to the supervisor.
    pub drained: u64,
    /// MAC stopped, RX checkpointed, IRQ paused (after optional PM=1 exchange).
    pub quiesced: u64,
    /// Register arena withdrawn and exclusive PHY access admitted.
    pub acquired: u64,
    /// PHY operation or synthetic hold completed, before restoration.
    pub work_completed: u64,
    /// Registers, RX, IRQ and MAC restored.
    pub hardware_restored: u64,
    /// Physical round trip including optional PM=0 exchange returned.
    pub protocol_restored: u64,
    /// Restored owner handed to worker mailbox, not necessarily polled yet.
    pub worker_released: u64,
}

/// Adjacent intervals; their sum is exactly `total_micros`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StationPauseIntervals {
    pub drain_micros: u64,
    pub quiesce_micros: u64,
    pub acquire_micros: u64,
    pub work_micros: u64,
    pub restore_hardware_micros: u64,
    pub restore_protocol_micros: u64,
    pub release_worker_micros: u64,
    pub total_micros: u64,
}

impl StationPauseTimeline {
    /// Reject clock reversal; equal ticks are valid at microsecond resolution.
    pub fn exclusive_intervals(self) -> Option<StationPauseIntervals> {
        Some(StationPauseIntervals {
            drain_micros: self.drained.checked_sub(self.requested)?,
            quiesce_micros: self.quiesced.checked_sub(self.drained)?,
            acquire_micros: self.acquired.checked_sub(self.quiesced)?,
            work_micros: self.work_completed.checked_sub(self.acquired)?,
            restore_hardware_micros: self.hardware_restored.checked_sub(self.work_completed)?,
            restore_protocol_micros: self.protocol_restored.checked_sub(self.hardware_restored)?,
            release_worker_micros: self.worker_released.checked_sub(self.protocol_restored)?,
            total_micros: self.worker_released.checked_sub(self.requested)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timeline(times: [u64; 8]) -> StationPauseTimeline {
        let [
            requested,
            drained,
            quiesced,
            acquired,
            work_completed,
            hardware_restored,
            protocol_restored,
            worker_released,
        ] = times;
        StationPauseTimeline {
            requested,
            drained,
            quiesced,
            acquired,
            work_completed,
            hardware_restored,
            protocol_restored,
            worker_released,
        }
    }

    #[test]
    fn adjacent_intervals_account_for_every_microsecond_once() {
        let report = timeline([10, 21, 35, 39, 107, 153, 156, 172]);
        let parts = report.exclusive_intervals().unwrap();
        assert_eq!(
            parts,
            StationPauseIntervals {
                drain_micros: 11,
                quiesce_micros: 14,
                acquire_micros: 4,
                work_micros: 68,
                restore_hardware_micros: 46,
                restore_protocol_micros: 3,
                release_worker_micros: 16,
                total_micros: 162,
            }
        );
        assert_eq!(
            parts.drain_micros
                + parts.quiesce_micros
                + parts.acquire_micros
                + parts.work_micros
                + parts.restore_hardware_micros
                + parts.restore_protocol_micros
                + parts.release_worker_micros,
            parts.total_micros
        );
    }

    #[test]
    fn every_reversed_boundary_is_rejected() {
        for index in 1..8 {
            let mut times = [100; 8];
            times[index] = 99;
            assert!(timeline(times).exclusive_intervals().is_none());
        }
        assert_eq!(
            timeline([u64::MAX; 8])
                .exclusive_intervals()
                .unwrap()
                .total_micros,
            0
        );
        assert_eq!(
            timeline([0, 0, 0, 0, 0, 0, 0, u64::MAX])
                .exclusive_intervals()
                .unwrap()
                .total_micros,
            u64::MAX
        );
    }
}
