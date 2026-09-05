use std::collections::VecDeque;

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Phase(&'static str);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RollbackOwner(&'static str);

#[derive(Debug)]
struct ModelController {
    identity: &'static str,
    request:
        Option<Result<BluetoothControllerTimeRequest, BluetoothControllerTimeAcquisitionError>>,
    rechecks: VecDeque<
        Result<BluetoothControllerTimePendingOwnerStep, BluetoothControllerTimeEventError>,
    >,
    cancel: Result<(), BluetoothControllerTimeEventError>,
    drains: VecDeque<
        Result<BluetoothControllerTimePendingOrphanStep, BluetoothControllerTimeEventError>,
    >,
    restored: Option<Phase>,
    rollback_failure: Option<RollbackOwner>,
}

impl ModelController {
    fn ready(identity: &'static str) -> Self {
        Self {
            identity,
            request: Some(Ok(BluetoothControllerTimeRequest::for_validation(1))),
            rechecks: VecDeque::new(),
            cancel: Ok(()),
            drains: VecDeque::new(),
            restored: None,
            rollback_failure: None,
        }
    }
}

impl BluetoothControllerTimePendingOwner for ModelController {
    fn recheck_owned_controller_time(
        &mut self,
        _request: BluetoothControllerTimeRequest,
    ) -> Result<BluetoothControllerTimePendingOwnerStep, BluetoothControllerTimeEventError> {
        self.rechecks
            .pop_front()
            .expect("the test scripts every bounded recheck")
    }

    fn cancel_owned_controller_time(
        &mut self,
        _request: BluetoothControllerTimeRequest,
    ) -> Result<(), BluetoothControllerTimeEventError> {
        self.cancel
    }

    fn drain_orphan_controller_time(
        &mut self,
    ) -> Result<BluetoothControllerTimePendingOrphanStep, BluetoothControllerTimeEventError> {
        self.drains
            .pop_front()
            .expect("the test scripts every bounded orphan drain")
    }
}

impl BluetoothTimedPreparationController for ModelController {
    fn request_timed_preparation_sample(
        &mut self,
    ) -> Result<BluetoothControllerTimeRequest, BluetoothControllerTimeAcquisitionError> {
        self.request
            .take()
            .expect("one preparation publishes exactly one request")
    }
}

fn rollback(
    controller: &mut ModelController,
    phase: Phase,
) -> BluetoothTimedPreparationRollbackOutcome<RollbackOwner> {
    controller.restored = Some(phase);
    match controller.rollback_failure.take() {
        Some(owner) => BluetoothTimedPreparationRollbackOutcome::FailStop(owner),
        None => BluetoothTimedPreparationRollbackOutcome::Restored,
    }
}

#[test]
fn waiting_then_ready_preserves_controller_and_phase_identity() {
    let mut controller = ModelController::ready("controller-a");
    controller
        .rechecks
        .push_back(Ok(BluetoothControllerTimePendingOwnerStep::Waiting));
    controller
        .rechecks
        .push_back(Ok(BluetoothControllerTimePendingOwnerStep::Ready(
            BluetoothControllerTimeSample::for_validation(7),
        )));
    let pending = BluetoothTimedPreparationPending::begin(controller, Phase("admission"), rollback)
        .expect("the request begins");
    let BluetoothTimedPreparationStep::Waiting(pending) = pending.recheck() else {
        panic!("the first bounded observation must wait");
    };
    let BluetoothTimedPreparationStep::Ready {
        controller,
        phase,
        sample,
    } = pending.recheck()
    else {
        panic!("the second bounded observation must complete");
    };
    assert_eq!(controller.identity, "controller-a");
    assert_eq!(phase, Phase("admission"));
    assert_eq!(sample, BluetoothControllerTimeSample::for_validation(7));
    assert_eq!(controller.restored, None);
}

#[test]
fn failed_recheck_rolls_back_the_exact_phase_before_fail_stop() {
    let mut controller = ModelController::ready("controller-f");
    controller
        .rechecks
        .push_back(Err(BluetoothControllerTimeEventError::OwnershipLost));
    let pending = BluetoothTimedPreparationPending::begin(controller, Phase("sequence"), rollback)
        .expect("the request begins");

    let BluetoothTimedPreparationStep::FailStop(failure) = pending.recheck() else {
        panic!("a lost controller-time owner must fail closed");
    };
    assert_eq!(
        failure.cause(),
        BluetoothTimedPreparationFailStopCause::ControllerTime(
            BluetoothControllerTimeAcquisitionError::OwnershipLost
        )
    );
    let (controller, rollback_owner) = failure.into_parts();
    assert_eq!(controller.identity, "controller-f");
    assert_eq!(controller.restored, Some(Phase("sequence")));
    assert!(rollback_owner.is_none());
}

#[test]
fn begin_failure_rolls_back_but_seals_the_non_idle_controller() {
    let mut controller = ModelController::ready("controller-b");
    controller.request = Some(Err(BluetoothControllerTimeAcquisitionError::Busy));
    let failure = BluetoothTimedPreparationPending::begin(controller, Phase("sequence"), rollback)
        .expect_err("a busy worker cannot accept another preparation");
    assert_eq!(
        failure.cause(),
        BluetoothTimedPreparationFailStopCause::ControllerTime(
            BluetoothControllerTimeAcquisitionError::Busy
        )
    );
    let (controller, rollback_owner) = failure.into_parts();
    assert_eq!(controller.identity, "controller-b");
    assert_eq!(controller.restored, Some(Phase("sequence")));
    assert!(rollback_owner.is_none());
}

#[test]
fn cancellation_waits_for_exact_orphan_drain_before_recovery() {
    let mut controller = ModelController::ready("controller-c");
    controller
        .drains
        .push_back(Ok(BluetoothControllerTimePendingOrphanStep::Waiting));
    controller
        .drains
        .push_back(Ok(BluetoothControllerTimePendingOrphanStep::Drained));
    let pending = BluetoothTimedPreparationPending::begin(controller, Phase("timing"), rollback)
        .expect("the request begins");
    let cancellation = pending.cancel().expect("rollback succeeds");
    let BluetoothTimedPreparationCancellationStep::Waiting(cancellation) =
        cancellation.recheck::<RollbackOwner>()
    else {
        panic!("the hardware request remains abandoned");
    };
    let BluetoothTimedPreparationCancellationStep::Recovered(controller) =
        cancellation.recheck::<RollbackOwner>()
    else {
        panic!("only the drained observation recovers the controller");
    };
    assert_eq!(controller.identity, "controller-c");
    assert_eq!(controller.restored, Some(Phase("timing")));
}

#[test]
fn rollback_failure_retains_the_exact_unrestored_owner() {
    let mut controller = ModelController::ready("controller-d");
    controller.rollback_failure = Some(RollbackOwner("graph-d"));
    let pending = BluetoothTimedPreparationPending::begin(controller, Phase("admission"), rollback)
        .expect("the request begins");
    let failure = pending
        .cancel()
        .expect_err("an unrestored role graph is permanently sealed");
    assert_eq!(
        failure.cause(),
        BluetoothTimedPreparationFailStopCause::Rollback
    );
    let (controller, rollback_owner) = failure.into_parts();
    assert_eq!(controller.identity, "controller-d");
    assert_eq!(rollback_owner, Some(RollbackOwner("graph-d")));
}

#[test]
fn idle_during_cancel_drain_is_phase_ownership_fail_stop() {
    let mut controller = ModelController::ready("controller-e");
    controller
        .drains
        .push_back(Ok(BluetoothControllerTimePendingOrphanStep::Idle));
    let pending = BluetoothTimedPreparationPending::begin(controller, Phase("sequence"), rollback)
        .expect("the request begins");
    let cancellation = pending.cancel().expect("rollback succeeds");
    let BluetoothTimedPreparationCancellationStep::FailStop(failure) =
        cancellation.recheck::<RollbackOwner>()
    else {
        panic!("Idle cannot substitute for the exact Drained observation");
    };
    assert_eq!(
        failure.cause(),
        BluetoothTimedPreparationFailStopCause::PhaseOwnership
    );
    assert_eq!(failure.into_parts().0.identity, "controller-e");
}
