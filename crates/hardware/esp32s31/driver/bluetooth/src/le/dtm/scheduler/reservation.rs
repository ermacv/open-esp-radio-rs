//! DTM context paired with one protocol-neutral scheduler-window reservation.
//!
//! The common timeline owns only raw windows, generations and timing policy.
//! This envelope retains the reviewed DTM scheduler-item transform and the
//! Controller epoch used to project it. It performs no SRAM mutation, list
//! selection, MMIO publication or hardware admission.

#![forbid(unsafe_code)]

use crate::{
    ControllerSchedulerEpoch, ControllerTimeSample,
    le::dtm::DtmSchedulerItemEvent,
    scheduler::timeline::{
        SchedulerInitialAdmissionResolved, SchedulerRecurringReserved,
        SchedulerSequenceAuthorizationError, SchedulerSequenceAuthorizationFailure,
        SchedulerSequenceReady, SchedulerTimingPolicy, SchedulerWindowReservation,
    },
};

/// DTM event context retaining one exact common scheduler reservation.
#[must_use = "the DTM reservation must be released or retained through completion"]
pub(crate) struct DtmSchedulerReservation<State> {
    window: SchedulerWindowReservation<State>,
    event: DtmSchedulerItemEvent,
    epoch: ControllerSchedulerEpoch,
}

impl<State> DtmSchedulerReservation<State> {
    pub(crate) const fn new(
        window: SchedulerWindowReservation<State>,
        event: DtmSchedulerItemEvent,
        epoch: ControllerSchedulerEpoch,
    ) -> Self {
        Self {
            window,
            event,
            epoch,
        }
    }

    pub(crate) const fn window(&self) -> crate::scheduler::SchedulerRawWindow {
        self.window.window()
    }

    pub(crate) const fn event(&self) -> DtmSchedulerItemEvent {
        self.event
    }

    pub(crate) const fn epoch(&self) -> ControllerSchedulerEpoch {
        self.epoch
    }

    pub(crate) const fn timing_policy(&self) -> SchedulerTimingPolicy {
        self.window.timing_policy()
    }

    pub(crate) fn into_window(self) -> SchedulerWindowReservation<State> {
        self.window
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn into_parts(
        self,
    ) -> (
        SchedulerWindowReservation<State>,
        DtmSchedulerItemEvent,
        ControllerSchedulerEpoch,
    ) {
        (self.window, self.event, self.epoch)
    }
}

impl<State> core::fmt::Debug for DtmSchedulerReservation<State> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("DtmSchedulerReservation")
            .field("window", &self.window)
            .field("event", &self.event)
            .field("epoch", &self.epoch)
            .finish()
    }
}

/// Rejected sequence deadline retaining the exact DTM context and window owner.
pub(crate) struct DtmSchedulerSequenceAuthorizationFailure<State> {
    reservation: DtmSchedulerReservation<State>,
    error: SchedulerSequenceAuthorizationError,
}

impl<State> DtmSchedulerSequenceAuthorizationFailure<State> {
    pub(crate) const fn error(&self) -> SchedulerSequenceAuthorizationError {
        self.error
    }

    pub(crate) fn into_reservation(self) -> DtmSchedulerReservation<State> {
        self.reservation
    }
}

impl<State> core::fmt::Debug for DtmSchedulerSequenceAuthorizationFailure<State> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("DtmSchedulerSequenceAuthorizationFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl DtmSchedulerReservation<SchedulerInitialAdmissionResolved> {
    pub(crate) fn authorize_sequence(
        self,
        sample: ControllerTimeSample,
    ) -> Result<
        DtmSchedulerReservation<SchedulerSequenceReady>,
        DtmSchedulerSequenceAuthorizationFailure<SchedulerInitialAdmissionResolved>,
    > {
        let Self {
            window,
            event,
            epoch,
        } = self;
        match window.authorize_sequence(sample) {
            Ok(window) => Ok(DtmSchedulerReservation::new(window, event, epoch)),
            Err(failure) => Err(sequence_failure(failure, event, epoch)),
        }
    }
}

impl DtmSchedulerReservation<SchedulerRecurringReserved> {
    pub(crate) fn authorize_sequence(
        self,
        sample: ControllerTimeSample,
    ) -> Result<
        DtmSchedulerReservation<SchedulerSequenceReady>,
        DtmSchedulerSequenceAuthorizationFailure<SchedulerRecurringReserved>,
    > {
        let Self {
            window,
            event,
            epoch,
        } = self;
        match window.authorize_sequence(sample) {
            Ok(window) => Ok(DtmSchedulerReservation::new(window, event, epoch)),
            Err(failure) => Err(sequence_failure(failure, event, epoch)),
        }
    }
}

fn sequence_failure<State>(
    failure: SchedulerSequenceAuthorizationFailure<State>,
    event: DtmSchedulerItemEvent,
    epoch: ControllerSchedulerEpoch,
) -> DtmSchedulerSequenceAuthorizationFailure<State> {
    let error = failure.error();
    DtmSchedulerSequenceAuthorizationFailure {
        reservation: DtmSchedulerReservation::new(failure.into_reservation(), event, epoch),
        error,
    }
}

#[cfg(test)]
mod tests;
