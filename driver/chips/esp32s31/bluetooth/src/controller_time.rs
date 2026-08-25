//! Event-driven controller-time latch and scheduler-epoch projection.
//!
//! Every in-flight observation is a single decision. `Waiting` tells the
//! caller to return to its executor and arrange a later interrupt or bounded
//! timer recheck; no state here spins, allocates, stores a waker or depends on
//! an RTOS.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_pac::{
    BluetoothControllerLatchedTime, BluetoothControllerTimeLatchObservation,
    BluetoothControllerTimeLatchRequest, BluetoothControllerTimeScale,
};

/// Result of evaluating one fresh controller-time event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "waiting state or ready transition must remain owned"]
pub enum BluetoothControllerTimeLatchProgress<W, R> {
    /// Hardware still owns the request; yield until another event.
    Waiting(W),
    /// The current phase can advance without polling.
    Ready(R),
}

/// Permission and exact image for publishing one latch request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the controller-time latch request must be published or abandoned"]
pub struct BluetoothControllerTimeLatchPublication {
    request: BluetoothControllerTimeLatchRequest,
}

impl BluetoothControllerTimeLatchPublication {
    /// Begin one pure request without touching MMIO.
    pub const fn new(request: BluetoothControllerTimeLatchRequest) -> Self {
        Self { request }
    }

    /// Exact fresh-read OR image for `SLEEP_TIMER_CONTROL`.
    pub const fn control_image(self, fresh_control_read: u32) -> u32 {
        self.request.publication_image(fresh_control_read)
    }

    /// Record that the request write completed.
    ///
    /// A future live owner will perform the RMW and consume this token
    /// internally before exposing the in-flight phase.
    pub const fn published(self) -> BluetoothControllerTimeLatchInFlight {
        BluetoothControllerTimeLatchInFlight { _private: () }
    }
}

/// One published latch request awaiting hardware's self-clear edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the in-flight controller-time request must be observed again"]
pub struct BluetoothControllerTimeLatchInFlight {
    _private: (),
}

impl BluetoothControllerTimeLatchInFlight {
    /// Evaluate exactly one fresh control-register observation.
    pub const fn observe(
        self,
        observation: BluetoothControllerTimeLatchObservation,
    ) -> BluetoothControllerTimeLatchProgress<Self, BluetoothControllerTimeLatchReadReady> {
        if observation.pending() {
            BluetoothControllerTimeLatchProgress::Waiting(self)
        } else {
            BluetoothControllerTimeLatchProgress::Ready(BluetoothControllerTimeLatchReadReady {
                _private: (),
            })
        }
    }
}

/// Proof that hardware cleared the request and the latched word may be read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the ready latched-time word must be read or explicitly abandoned"]
pub struct BluetoothControllerTimeLatchReadReady {
    _private: (),
}

impl BluetoothControllerTimeLatchReadReady {
    /// Complete the ordered read with one positional latched-time image.
    pub const fn complete(
        self,
        latched_time: BluetoothControllerLatchedTime,
    ) -> BluetoothControllerTimeSample {
        BluetoothControllerTimeSample { latched_time }
    }
}

/// One ordered controller-time sample from the always-awake latch path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothControllerTimeSample {
    latched_time: BluetoothControllerLatchedTime,
}

impl BluetoothControllerTimeSample {
    /// Return the complete wrapping raw-time image.
    pub const fn raw_time(self) -> u32 {
        self.latched_time.bits()
    }
}

/// Raw-time anchor paired with the BLE scheduler's positional epoch.
///
/// The projection exactly retains the current `r_sched_timer_convertTimeToUs`
/// branch geometry. Every reviewed S31 HAL configuration has a positive scale
/// image, so the helper's optional negative-side remainder is always zero.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothControllerSchedulerEpoch {
    raw_anchor: u32,
    scheduler_anchor: u32,
    scale: BluetoothControllerTimeScale,
}

impl BluetoothControllerSchedulerEpoch {
    /// Bind one ordered controller sample to one scheduler-domain anchor.
    pub const fn new(
        sample: BluetoothControllerTimeSample,
        scheduler_anchor: u32,
        scale: BluetoothControllerTimeScale,
    ) -> Self {
        Self {
            raw_anchor: sample.raw_time(),
            scheduler_anchor,
            scale,
        }
    }

    /// Project a later or earlier wrapping raw sample into the scheduler epoch.
    pub const fn project(self, sample: BluetoothControllerTimeSample) -> u32 {
        let delta = sample.raw_time().wrapping_sub(self.raw_anchor);
        if delta as i32 >= 0 {
            self.scheduler_anchor
                .wrapping_add(self.scale.scheduler_delta_from_raw(delta))
        } else {
            self.scheduler_anchor
                .wrapping_sub(self.scale.scheduler_delta_from_raw(delta.wrapping_neg()))
        }
    }

    /// Project one scheduler-domain position back into raw controller time.
    ///
    /// This retains the complete `r_sched_timer_convertTimeToTicks` branch
    /// geometry. Discarded low scheduler bits are truncated toward the epoch
    /// anchor on both sides, matching the reviewed inverse helper.
    pub const fn raw_time_for_scheduler_time(self, scheduler_time: u32) -> u32 {
        let delta = scheduler_time.wrapping_sub(self.scheduler_anchor);
        if delta as i32 >= 0 {
            self.raw_anchor
                .wrapping_add(self.scale.raw_delta_from_scheduler(delta).whole)
        } else {
            self.raw_anchor.wrapping_sub(
                self.scale
                    .raw_delta_from_scheduler(delta.wrapping_neg())
                    .whole,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_pac::{
        BluetoothControllerHalInitConfig, BluetoothControllerLatchedTime,
        BluetoothControllerTimeLatchObservation, BluetoothControllerTimeLatchRequest,
    };

    use super::{
        BluetoothControllerSchedulerEpoch, BluetoothControllerTimeLatchProgress,
        BluetoothControllerTimeLatchPublication,
    };

    fn sample(raw_time: u32) -> super::BluetoothControllerTimeSample {
        let ready = match BluetoothControllerTimeLatchPublication::new(
            BluetoothControllerTimeLatchRequest::new(),
        )
        .published()
        .observe(BluetoothControllerTimeLatchObservation::from_control_bits(
            0,
        )) {
            BluetoothControllerTimeLatchProgress::Ready(ready) => ready,
            BluetoothControllerTimeLatchProgress::Waiting(_) => panic!("clear request stalled"),
        };
        ready.complete(BluetoothControllerLatchedTime::from_bits(raw_time))
    }

    #[test]
    fn pending_observation_returns_control_without_reading_time() {
        let publication = BluetoothControllerTimeLatchPublication::new(
            BluetoothControllerTimeLatchRequest::new(),
        );
        assert_eq!(publication.control_image(0x8000_0007), 0x8400_0007);

        let in_flight = publication.published();
        let in_flight = match in_flight.observe(
            BluetoothControllerTimeLatchObservation::from_control_bits(1 << 26),
        ) {
            BluetoothControllerTimeLatchProgress::Waiting(in_flight) => in_flight,
            BluetoothControllerTimeLatchProgress::Ready(_) => panic!("pending latch advanced"),
        };
        let ready = match in_flight.observe(
            BluetoothControllerTimeLatchObservation::from_control_bits(0x8000_0007),
        ) {
            BluetoothControllerTimeLatchProgress::Ready(ready) => ready,
            BluetoothControllerTimeLatchProgress::Waiting(_) => panic!("cleared latch stalled"),
        };

        assert_eq!(
            ready
                .complete(BluetoothControllerLatchedTime::from_bits(0x1234_5678))
                .raw_time(),
            0x1234_5678
        );
    }

    #[test]
    fn scheduler_epoch_matches_forward_backward_and_wrapping_branches() {
        let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
        let epoch = BluetoothControllerSchedulerEpoch::new(sample(100), 1_000, scale);

        assert_eq!(epoch.project(sample(103)), 1_012);
        assert_eq!(epoch.project(sample(97)), 988);

        let wrapping_epoch =
            BluetoothControllerSchedulerEpoch::new(sample(0xffff_fffe), 1_000, scale);
        assert_eq!(wrapping_epoch.project(sample(1)), 1_012);

        let reverse_wrapping_epoch =
            BluetoothControllerSchedulerEpoch::new(sample(1), 1_000, scale);
        assert_eq!(reverse_wrapping_epoch.project(sample(0xffff_fffe)), 988);

        assert_eq!(epoch.raw_time_for_scheduler_time(1_012), 103);
        assert_eq!(epoch.raw_time_for_scheduler_time(1_015), 103);
        assert_eq!(epoch.raw_time_for_scheduler_time(988), 97);
        assert_eq!(epoch.raw_time_for_scheduler_time(985), 97);

        let scheduler_wrapping_epoch =
            BluetoothControllerSchedulerEpoch::new(sample(100), 0xffff_fffe, scale);
        assert_eq!(
            scheduler_wrapping_epoch.raw_time_for_scheduler_time(10),
            103
        );
    }
}
