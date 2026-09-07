use super::{
    ControllerTimeAcquisitionError as TimeError,
    DtmControllerEventPreparationError as PreparationError, DtmFirstPreparationCompletionClass,
    DtmRole, SchedulerEmptyListMergeError, SchedulerReservationError as ReservationError,
    SchedulerSequenceAuthorizationError as SequenceError,
    classify_dtm_first_preparation_completion,
};

#[test]
fn first_preparation_completion_classifies_every_portable_error_branch() {
    for error in [
        PreparationError::Reservation(ReservationError::InitialDeadlineExpired),
        PreparationError::Reservation(ReservationError::TimelineFull),
        PreparationError::SequenceAuthorization(SequenceError::DeadlineExpired),
        PreparationError::ControllerTime(TimeError::Busy),
    ] {
        assert_eq!(
            classify_dtm_first_preparation_completion(error),
            DtmFirstPreparationCompletionClass::HardwareFailure,
        );
    }

    for error in [
        PreparationError::LinkStateRoleMismatch {
            expected: DtmRole::Receiver,
            observed: DtmRole::Transmitter,
        },
        PreparationError::Reservation(ReservationError::WindowOutsideForwardHalfRange),
        PreparationError::Reservation(ReservationError::OverlapResolutionOutsideForwardHalfRange),
        PreparationError::Reservation(ReservationError::RecurringOverlapUnsupported),
        PreparationError::Reservation(ReservationError::GenerationExhausted),
        PreparationError::ControllerTime(TimeError::OwnershipCollision),
        PreparationError::ControllerTime(TimeError::GenerationExhausted),
        PreparationError::ControllerTime(TimeError::RequestMismatch),
        PreparationError::ControllerTime(TimeError::OwnershipLost),
        PreparationError::ControllerTime(TimeError::Faulted),
        PreparationError::ControllerTime(TimeError::Cancelled),
        PreparationError::EmptyList(SchedulerEmptyListMergeError::ListNotEmpty),
    ] {
        assert_eq!(
            classify_dtm_first_preparation_completion(error),
            DtmFirstPreparationCompletionClass::FailStop,
        );
    }
}
