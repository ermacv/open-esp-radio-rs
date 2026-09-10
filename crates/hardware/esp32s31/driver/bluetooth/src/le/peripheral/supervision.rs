//! Elapsed-time supervision, independent of anchor capture and RX delivery.

use crate::SchedulerInstant;

/// Deadline derived from a hardware receive timestamp (or the creation seed).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PeripheralSupervisionDeadline(SchedulerInstant);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PeripheralSupervisionDecision {
    Run,
    Wait,
    Expired,
}

impl PeripheralSupervisionDeadline {
    pub(crate) const fn new(reference: SchedulerInstant, timeout_micros: u32) -> Self {
        Self(reference.wrapping_add(timeout_micros))
    }

    /// Check a fresh current sample before sequence publication. A future
    /// reservation beyond the deadline cannot hide expiry while waiting for RUN.
    /// If the event has already started before expiry, completion supplies the
    /// next valid-RX reference before another outside-event decision is made.
    pub(crate) const fn decide(
        self,
        now: SchedulerInstant,
        event_start: SchedulerInstant,
    ) -> PeripheralSupervisionDecision {
        if !now.is_before(self.0) {
            PeripheralSupervisionDecision::Expired
        } else if !event_start.is_before(self.0) {
            PeripheralSupervisionDecision::Wait
        } else {
            PeripheralSupervisionDecision::Run
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn at(micros: u32) -> SchedulerInstant {
        SchedulerInstant::from_image(micros)
    }

    #[test]
    fn receive_timestamp_projects_through_a_nonzero_epoch_and_raw_wrap() {
        use crate::{ControllerSchedulerEpoch, ControllerTimeSample};
        use oer_esp32s31_bluetooth_memory::PeripheralConnectionReceiveTime;
        let scale = oer_esp32s31_pac::BluetoothControllerHalInitConfig::reviewed_standalone()
            .controller_time_scale();
        let epoch = ControllerSchedulerEpoch::new(
            ControllerTimeSample::for_validation(u32::MAX - 99),
            50_000,
            scale,
        );
        let reference = epoch.project_peripheral_receive_time(
            PeripheralConnectionReceiveTime::from_controller_ticks(100),
        );
        assert_eq!(reference, 50_100);
    }

    #[test]
    fn expiration_uses_elapsed_time_including_between_events() {
        let deadline = PeripheralSupervisionDeadline::new(at(10_000), 2_000_000);
        assert_eq!(
            deadline.decide(at(1_900_000), at(2_000_000)),
            PeripheralSupervisionDecision::Run
        );
        assert_eq!(
            deadline.decide(at(2_009_999), at(2_010_000)),
            PeripheralSupervisionDecision::Wait
        );
        assert_eq!(
            deadline.decide(at(2_010_000), at(2_020_000)),
            PeripheralSupervisionDecision::Expired
        );
    }

    #[test]
    fn unchanged_receive_time_does_not_extend_deadline_on_rechecks() {
        for now in [100, 500_000, 1_999_999, 2_000_000] {
            let deadline = PeripheralSupervisionDeadline::new(at(0), 2_000_000);
            assert_eq!(
                deadline.decide(at(now), at(2_100_000)),
                if now < 2_000_000 {
                    PeripheralSupervisionDecision::Wait
                } else {
                    PeripheralSupervisionDecision::Expired
                }
            );
        }
    }

    #[test]
    fn valid_receive_update_extends_deadline_without_payload_or_anchor_inputs() {
        let before = PeripheralSupervisionDeadline::new(at(100), 2_000_000);
        let after = PeripheralSupervisionDeadline::new(at(1_900_000), 2_000_000);
        assert_eq!(
            before.decide(at(2_000_100), at(2_100_000)),
            PeripheralSupervisionDecision::Expired
        );
        assert_eq!(
            after.decide(at(2_000_100), at(2_100_000)),
            PeripheralSupervisionDecision::Run
        );
    }

    #[test]
    fn deadline_and_current_wrap_without_early_or_late_expiry() {
        let deadline = PeripheralSupervisionDeadline::new(at(u32::MAX - 99), 100_000);
        assert_eq!(
            deadline.decide(at(u32::MAX), at(0)),
            PeripheralSupervisionDecision::Run
        );
        assert_eq!(
            deadline.decide(at(99_899), at(99_900)),
            PeripheralSupervisionDecision::Wait
        );
        assert_eq!(
            deadline.decide(at(99_900), at(100_000)),
            PeripheralSupervisionDecision::Expired
        );
    }
}
