//! Context retained when a required radio continuation cannot be represented.

use oer_bluetooth_ll::dtm::DtmPlanningError;
use oer_bluetooth_radio::{LeInstant, RadioTiming, TimingError};

/// The logical owner whose continuation failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanningRole {
    Advertising,
    Scanning,
    Peripheral,
    DirectTest,
}

/// The operation being prepared, before any new request is published.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanningOperation {
    Event,
    FutureReservation,
    ConnectionIndication,
    ProcedureTimeout,
    Supervision,
}

/// The failing calculation within that operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanningCalculation {
    Admission,
    ChannelSpacing,
    EventDuration,
    LatestAnchor,
    Reservation,
    Recurrence,
    Expiry,
    IntervalEnd,
    MinimumWindow,
    WindowClipping,
    FirstAnchor,
    TransmitWindow,
    WindowGeometry,
    Widening,
    Elapsed,
    SlotAlignment,
    IntervalTransition,
}

/// The accepting owner's original cause.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanningCause {
    Timing(TimingError),
    DirectTest(DtmPlanningError),
    /// The required LL event distance exceeds its bounded wire-counter preview.
    EventDeltaOutsideRange,
}

/// A required continuation failed. The service must cease admission and settle
/// the retained Controller and radio owners through managed teardown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlanningError {
    pub role: PlanningRole,
    pub operation: PlanningOperation,
    pub calculation: PlanningCalculation,
    pub cause: PlanningCause,
}

impl PlanningError {
    pub(crate) const fn timing(
        role: PlanningRole,
        operation: PlanningOperation,
        calculation: PlanningCalculation,
        cause: TimingError,
    ) -> Self {
        Self {
            role,
            operation,
            calculation,
            cause: PlanningCause::Timing(cause),
        }
    }
}

pub(crate) fn earliest(
    now: LeInstant,
    timing: RadioTiming,
    role: PlanningRole,
) -> Result<LeInstant, PlanningError> {
    now.checked_add(timing.preparation_lead)
        .and_then(|at| at.checked_add(timing.admission_guard))
        .and_then(|at| at.checked_add(crate::PLANNING_SLACK))
        .ok_or(PlanningError::timing(
            role,
            PlanningOperation::Event,
            PlanningCalculation::Admission,
            TimingError::BeyondEpoch,
        ))
}
