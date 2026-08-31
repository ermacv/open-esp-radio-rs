//! DTM context paired with one protocol-neutral scheduler-window reservation.
//!
//! The common timeline owns only raw windows, generations and timing policy.
//! This envelope retains the reviewed DTM scheduler-item transform and the
//! Controller epoch used to project it. It performs no SRAM mutation, list
//! selection, MMIO publication or hardware admission.

#![forbid(unsafe_code)]

use crate::{
    BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample,
    BluetoothDtmSchedulerItemEvent,
    scheduler_timeline::{
        BluetoothSchedulerInitialAdmissionResolved, BluetoothSchedulerRecurringReserved,
        BluetoothSchedulerSequenceAuthorizationError,
        BluetoothSchedulerSequenceAuthorizationFailure, BluetoothSchedulerSequenceReady,
        BluetoothSchedulerTimingPolicy, BluetoothSchedulerWindowReservation,
    },
};

/// DTM event context retaining one exact common scheduler reservation.
#[must_use = "the DTM reservation must be released or retained through completion"]
pub(crate) struct BluetoothDtmSchedulerReservation<State> {
    window: BluetoothSchedulerWindowReservation<State>,
    event: BluetoothDtmSchedulerItemEvent,
    epoch: BluetoothControllerSchedulerEpoch,
}

impl<State> BluetoothDtmSchedulerReservation<State> {
    pub(crate) const fn new(
        window: BluetoothSchedulerWindowReservation<State>,
        event: BluetoothDtmSchedulerItemEvent,
        epoch: BluetoothControllerSchedulerEpoch,
    ) -> Self {
        Self {
            window,
            event,
            epoch,
        }
    }

    pub(crate) const fn window(&self) -> crate::BluetoothSchedulerRawWindow {
        self.window.window()
    }

    pub(crate) const fn event(&self) -> BluetoothDtmSchedulerItemEvent {
        self.event
    }

    pub(crate) const fn epoch(&self) -> BluetoothControllerSchedulerEpoch {
        self.epoch
    }

    pub(crate) const fn timing_policy(&self) -> BluetoothSchedulerTimingPolicy {
        self.window.timing_policy()
    }

    pub(crate) fn into_window(self) -> BluetoothSchedulerWindowReservation<State> {
        self.window
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn into_parts(
        self,
    ) -> (
        BluetoothSchedulerWindowReservation<State>,
        BluetoothDtmSchedulerItemEvent,
        BluetoothControllerSchedulerEpoch,
    ) {
        (self.window, self.event, self.epoch)
    }
}

impl<State> core::fmt::Debug for BluetoothDtmSchedulerReservation<State> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothDtmSchedulerReservation")
            .field("window", &self.window)
            .field("event", &self.event)
            .field("epoch", &self.epoch)
            .finish()
    }
}

/// Rejected sequence deadline retaining the exact DTM context and window owner.
pub(crate) struct BluetoothDtmSchedulerSequenceAuthorizationFailure<State> {
    reservation: BluetoothDtmSchedulerReservation<State>,
    error: BluetoothSchedulerSequenceAuthorizationError,
}

impl<State> BluetoothDtmSchedulerSequenceAuthorizationFailure<State> {
    pub(crate) const fn error(&self) -> BluetoothSchedulerSequenceAuthorizationError {
        self.error
    }

    pub(crate) fn into_reservation(self) -> BluetoothDtmSchedulerReservation<State> {
        self.reservation
    }
}

impl<State> core::fmt::Debug for BluetoothDtmSchedulerSequenceAuthorizationFailure<State> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothDtmSchedulerSequenceAuthorizationFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl BluetoothDtmSchedulerReservation<BluetoothSchedulerInitialAdmissionResolved> {
    pub(crate) fn authorize_sequence(
        self,
        sample: BluetoothControllerTimeSample,
    ) -> Result<
        BluetoothDtmSchedulerReservation<BluetoothSchedulerSequenceReady>,
        BluetoothDtmSchedulerSequenceAuthorizationFailure<
            BluetoothSchedulerInitialAdmissionResolved,
        >,
    > {
        let Self {
            window,
            event,
            epoch,
        } = self;
        match window.authorize_sequence(sample) {
            Ok(window) => Ok(BluetoothDtmSchedulerReservation::new(window, event, epoch)),
            Err(failure) => Err(sequence_failure(failure, event, epoch)),
        }
    }
}

impl BluetoothDtmSchedulerReservation<BluetoothSchedulerRecurringReserved> {
    pub(crate) fn authorize_sequence(
        self,
        sample: BluetoothControllerTimeSample,
    ) -> Result<
        BluetoothDtmSchedulerReservation<BluetoothSchedulerSequenceReady>,
        BluetoothDtmSchedulerSequenceAuthorizationFailure<BluetoothSchedulerRecurringReserved>,
    > {
        let Self {
            window,
            event,
            epoch,
        } = self;
        match window.authorize_sequence(sample) {
            Ok(window) => Ok(BluetoothDtmSchedulerReservation::new(window, event, epoch)),
            Err(failure) => Err(sequence_failure(failure, event, epoch)),
        }
    }
}

fn sequence_failure<State>(
    failure: BluetoothSchedulerSequenceAuthorizationFailure<State>,
    event: BluetoothDtmSchedulerItemEvent,
    epoch: BluetoothControllerSchedulerEpoch,
) -> BluetoothDtmSchedulerSequenceAuthorizationFailure<State> {
    let error = failure.error();
    BluetoothDtmSchedulerSequenceAuthorizationFailure {
        reservation: BluetoothDtmSchedulerReservation::new(
            failure.into_reservation(),
            event,
            epoch,
        ),
        error,
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_pac::BluetoothControllerHalInitConfig;

    use super::BluetoothDtmSchedulerReservation;
    use crate::{
        BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample, BluetoothDtmChannel,
        BluetoothDtmPhy, BluetoothDtmRole, BluetoothDtmRxInitialEventWindow,
        BluetoothDtmSchedulerInstant, BluetoothDtmSchedulerItemEvent,
        BluetoothSchedulerSoftwareConfig,
        scheduler_timeline::{BluetoothSchedulerTimeline, BluetoothSchedulerTimingPolicy},
    };

    fn sample(raw_time: u32) -> BluetoothControllerTimeSample {
        BluetoothControllerTimeSample::for_validation(raw_time)
    }

    #[test]
    fn dtm_envelope_retains_the_event_and_epoch_outside_the_common_timeline() {
        let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
        let epoch = BluetoothControllerSchedulerEpoch::new(sample(100), 1_000, scale);
        let event = BluetoothDtmSchedulerItemEvent::new_initial_receiver(
            BluetoothDtmChannel::new(5).expect("channel is valid"),
            BluetoothDtmPhy::Le1M,
            BluetoothDtmRxInitialEventWindow::new(
                BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
                BluetoothDtmSchedulerInstant::from_image(900),
                BluetoothDtmSchedulerInstant::from_image(1_020),
            ),
        )
        .expect("initial receiver event is valid");
        let policy = BluetoothSchedulerTimingPolicy::from_scheduler_config(
            BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            scale,
        );
        let mut timeline = BluetoothSchedulerTimeline::<1>::new();
        let window = timeline
            .reserve_initial_window(
                event.raw_start(epoch),
                event.raw_end(epoch),
                policy,
                sample(92),
            )
            .expect("the projected event passes its initial deadline");
        let reservation = BluetoothDtmSchedulerReservation::new(window, event, epoch);

        assert_eq!(reservation.window().start(), 310);
        assert_eq!(reservation.window().end(), 586);
        assert_eq!(reservation.event().role(), BluetoothDtmRole::Receiver);
        assert_eq!(reservation.epoch(), epoch);
        assert!(timeline.release(reservation.into_window()).is_ok());
    }
}
