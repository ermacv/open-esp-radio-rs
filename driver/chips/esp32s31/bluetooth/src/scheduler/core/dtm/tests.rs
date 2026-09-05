use super::{
    BluetoothControllerTimeAcquisitionError as TimeError,
    BluetoothDtmControllerEventPreparationError as PreparationError,
    BluetoothDtmFirstPreparationCompletionClass, BluetoothDtmRole,
    BluetoothSchedulerEmptyListMergeError, BluetoothSchedulerReservationError as ReservationError,
    BluetoothSchedulerSequenceAuthorizationError as SequenceError,
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
            BluetoothDtmFirstPreparationCompletionClass::HardwareFailure,
        );
    }

    for error in [
        PreparationError::LinkStateRoleMismatch {
            expected: BluetoothDtmRole::Receiver,
            observed: BluetoothDtmRole::Transmitter,
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
        PreparationError::EmptyList(BluetoothSchedulerEmptyListMergeError::ListNotEmpty),
    ] {
        assert_eq!(
            classify_dtm_first_preparation_completion(error),
            BluetoothDtmFirstPreparationCompletionClass::FailStop,
        );
    }
}
