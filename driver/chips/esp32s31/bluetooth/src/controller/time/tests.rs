use std::{collections::VecDeque, vec::Vec};

use open_esp_radio_esp32s31_hal::{
    BluetoothControllerTimeLatchBeginError, BluetoothControllerTimeLatchStep,
    BluetoothControllerTimeLatchStepError,
};
use open_esp_radio_esp32s31_pac::{
    BluetoothControllerHalInitConfig, BluetoothControllerLatchedTime, BluetoothHalInitPeriod,
    BluetoothHalInitScale,
};

use super::{
    BluetoothControllerSchedulerEpoch, BluetoothControllerSchedulerNow,
    BluetoothControllerTimeEventError, BluetoothControllerTimeEventStep,
    BluetoothControllerTimeHardware, BluetoothControllerTimePendingCore,
    BluetoothControllerTimePendingCoreStep, BluetoothControllerTimePendingOrphanStep,
    BluetoothControllerTimePendingOwner, BluetoothControllerTimePendingOwnerStep,
    BluetoothControllerTimeRequest, BluetoothControllerTimeRequestError,
    BluetoothControllerTimeSample, BluetoothControllerTimeWorker,
    BluetoothControllerTimeWorkerPhase, drain_controller_time_orphan,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Begin,
    Step,
}

struct ModelHardware {
    in_flight: bool,
    operations: Vec<Operation>,
    steps: VecDeque<BluetoothControllerTimeLatchStep>,
}

impl ModelHardware {
    fn new(steps: impl IntoIterator<Item = BluetoothControllerTimeLatchStep>) -> Self {
        Self {
            in_flight: false,
            operations: Vec::new(),
            steps: steps.into_iter().collect(),
        }
    }
}

impl BluetoothControllerTimeHardware for ModelHardware {
    fn begin_controller_time_latch(
        &mut self,
    ) -> Result<(), BluetoothControllerTimeLatchBeginError> {
        if self.in_flight {
            return Err(BluetoothControllerTimeLatchBeginError::AlreadyInFlight);
        }
        self.in_flight = true;
        self.operations.push(Operation::Begin);
        Ok(())
    }

    fn step_controller_time_latch(
        &mut self,
    ) -> Result<BluetoothControllerTimeLatchStep, BluetoothControllerTimeLatchStepError> {
        if !self.in_flight {
            return Err(BluetoothControllerTimeLatchStepError::NotInFlight);
        }
        let step = self
            .steps
            .pop_front()
            .expect("test hardware needs one scripted event step");
        self.operations.push(Operation::Step);
        if matches!(step, BluetoothControllerTimeLatchStep::Ready(_)) {
            self.in_flight = false;
        }
        Ok(step)
    }
}

#[derive(Debug)]
struct PendingOwnerModel {
    identity: u8,
    rechecks: Vec<BluetoothControllerTimeRequest>,
    cancellations: Vec<BluetoothControllerTimeRequest>,
    recheck_steps: VecDeque<
        Result<BluetoothControllerTimePendingOwnerStep, BluetoothControllerTimeEventError>,
    >,
    cancel_error: Option<BluetoothControllerTimeEventError>,
    orphan_steps: VecDeque<
        Result<BluetoothControllerTimePendingOrphanStep, BluetoothControllerTimeEventError>,
    >,
}

impl PendingOwnerModel {
    fn new(identity: u8) -> Self {
        Self {
            identity,
            rechecks: Vec::new(),
            cancellations: Vec::new(),
            recheck_steps: VecDeque::new(),
            cancel_error: None,
            orphan_steps: VecDeque::new(),
        }
    }
}

impl BluetoothControllerTimePendingOwner for PendingOwnerModel {
    fn recheck_owned_controller_time(
        &mut self,
        request: BluetoothControllerTimeRequest,
    ) -> Result<BluetoothControllerTimePendingOwnerStep, BluetoothControllerTimeEventError> {
        self.rechecks.push(request);
        self.recheck_steps
            .pop_front()
            .expect("pending-owner model needs one recheck step")
    }

    fn cancel_owned_controller_time(
        &mut self,
        request: BluetoothControllerTimeRequest,
    ) -> Result<(), BluetoothControllerTimeEventError> {
        self.cancellations.push(request);
        match self.cancel_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn drain_orphan_controller_time(
        &mut self,
    ) -> Result<BluetoothControllerTimePendingOrphanStep, BluetoothControllerTimeEventError> {
        self.orphan_steps
            .pop_front()
            .expect("pending-owner model needs one orphan-drain step")
    }
}

const fn sample(raw_time: u32) -> BluetoothControllerTimeSample {
    BluetoothControllerTimeSample::for_validation(raw_time)
}

#[test]
fn pending_core_drop_cancels_exact_request_then_uses_orphan_drain() {
    let request = BluetoothControllerTimeRequest::for_validation(41);
    let mut owner = PendingOwnerModel::new(1);
    owner.orphan_steps.extend([
        Ok(BluetoothControllerTimePendingOrphanStep::Waiting),
        Ok(BluetoothControllerTimePendingOrphanStep::Drained),
        Ok(BluetoothControllerTimePendingOrphanStep::Idle),
    ]);

    drop(BluetoothControllerTimePendingCore::new(&mut owner, request));
    assert_eq!(owner.cancellations, [request]);
    assert_eq!(
        drain_controller_time_orphan(&mut owner),
        Ok(BluetoothControllerTimePendingOrphanStep::Waiting)
    );
    assert_eq!(
        drain_controller_time_orphan(&mut owner),
        Ok(BluetoothControllerTimePendingOrphanStep::Drained)
    );
    assert_eq!(
        drain_controller_time_orphan(&mut owner),
        Ok(BluetoothControllerTimePendingOrphanStep::Idle)
    );
}

#[test]
fn pending_core_waiting_reconstructs_the_same_owner_and_request() {
    let request = BluetoothControllerTimeRequest::for_validation(42);
    let mut owner = PendingOwnerModel::new(7);
    owner.recheck_steps.extend([
        Ok(BluetoothControllerTimePendingOwnerStep::Waiting),
        Ok(BluetoothControllerTimePendingOwnerStep::Ready(sample(
            0x1234,
        ))),
    ]);

    let pending = BluetoothControllerTimePendingCore::new(&mut owner, request);
    let waiting = match pending.recheck().expect("first recheck waits") {
        BluetoothControllerTimePendingCoreStep::Waiting(waiting) => waiting,
        BluetoothControllerTimePendingCoreStep::Ready { .. } => {
            panic!("first recheck unexpectedly completed")
        }
    };
    let (identity, observed) = match waiting.recheck().expect("second recheck completes") {
        BluetoothControllerTimePendingCoreStep::Ready { owner, sample } => {
            (owner.identity, sample.raw_ticks())
        }
        BluetoothControllerTimePendingCoreStep::Waiting(_) => {
            panic!("second recheck unexpectedly waited")
        }
    };

    assert_eq!(identity, 7);
    assert_eq!(observed, 0x1234);
    assert_eq!(owner.rechecks, [request, request]);
    assert!(owner.cancellations.is_empty());
}

#[test]
fn pending_core_ready_disarms_drop_cancellation() {
    let request = BluetoothControllerTimeRequest::for_validation(43);
    let mut owner = PendingOwnerModel::new(2);
    owner
        .recheck_steps
        .push_back(Ok(BluetoothControllerTimePendingOwnerStep::Ready(sample(
            9,
        ))));

    {
        let ready = BluetoothControllerTimePendingCore::new(&mut owner, request)
            .recheck()
            .expect("recheck completes");
        match ready {
            BluetoothControllerTimePendingCoreStep::Ready {
                owner: returned,
                sample,
            } => {
                assert_eq!(sample.raw_ticks(), 9);
                let _ = returned;
            }
            BluetoothControllerTimePendingCoreStep::Waiting(_) => {
                panic!("recheck unexpectedly waited")
            }
        }
    }

    assert_eq!(owner.rechecks, [request]);
    assert!(owner.cancellations.is_empty());
}

#[test]
fn pending_core_recheck_failure_returns_exact_owner_without_cancellation() {
    let request = BluetoothControllerTimeRequest::for_validation(44);
    let mut owner = PendingOwnerModel::new(9);
    owner
        .recheck_steps
        .push_back(Err(BluetoothControllerTimeEventError::OwnershipLost));

    let failure = BluetoothControllerTimePendingCore::new(owner, request)
        .recheck()
        .expect_err("model rejects the exact recheck");
    let (returned, error) = failure.into_parts();

    assert_eq!(returned.identity, 9);
    assert_eq!(error, BluetoothControllerTimeEventError::OwnershipLost);
    assert_eq!(returned.rechecks, [request]);
    assert!(returned.cancellations.is_empty());
}

#[test]
fn pending_core_explicit_cancel_exposes_mismatch_and_fault() {
    for (generation, expected) in [
        (44, BluetoothControllerTimeEventError::RequestMismatch),
        (45, BluetoothControllerTimeEventError::Faulted),
    ] {
        let request = BluetoothControllerTimeRequest::for_validation(generation);
        let mut owner = PendingOwnerModel::new(3);
        owner.cancel_error = Some(expected);

        let failure = BluetoothControllerTimePendingCore::new(&mut owner, request)
            .cancel()
            .expect_err("model rejects explicit cancellation");
        let (returned, error) = failure.into_parts();
        assert_eq!(error, expected);
        assert_eq!(returned.identity, 3);
        assert_eq!(owner.cancellations, [request]);
    }
}

#[test]
fn request_and_each_recheck_are_separate_bounded_hardware_steps() {
    let mut hardware = ModelHardware::new([
        BluetoothControllerTimeLatchStep::Waiting,
        BluetoothControllerTimeLatchStep::Ready(BluetoothControllerLatchedTime::from_bits(
            0x1234_5678,
        )),
    ]);
    let mut worker = BluetoothControllerTimeWorker::new_idle();

    assert_eq!(worker.phase(), BluetoothControllerTimeWorkerPhase::Idle);
    let request = worker.request(&mut hardware).expect("request is published");
    assert_eq!(hardware.operations, [Operation::Begin]);
    assert!(worker.needs_recheck());

    assert_eq!(
        worker.recheck_owned(request, &mut hardware),
        Ok(BluetoothControllerTimeEventStep::Waiting)
    );
    assert_eq!(hardware.operations, [Operation::Begin, Operation::Step]);
    assert_eq!(
        worker.phase(),
        BluetoothControllerTimeWorkerPhase::Requested
    );

    let sample = match worker
        .recheck_owned(request, &mut hardware)
        .expect("second event completes")
    {
        BluetoothControllerTimeEventStep::Sample {
            request: completed,
            sample,
        } => {
            assert_eq!(completed, request);
            sample
        }
        other => panic!("unexpected event step: {other:?}"),
    };
    assert_eq!(sample.raw_ticks(), 0x1234_5678);
    assert_eq!(
        hardware.operations,
        [Operation::Begin, Operation::Step, Operation::Step]
    );
    assert_eq!(worker.phase(), BluetoothControllerTimeWorkerPhase::Idle);
    assert!(!worker.needs_recheck());
}

#[test]
fn abandoned_request_is_drained_without_becoming_a_new_sample() {
    let mut hardware = ModelHardware::new([
        BluetoothControllerTimeLatchStep::Ready(BluetoothControllerLatchedTime::from_bits(
            0x1111_1111,
        )),
        BluetoothControllerTimeLatchStep::Ready(BluetoothControllerLatchedTime::from_bits(
            0x2222_2222,
        )),
    ]);
    let mut worker = BluetoothControllerTimeWorker::new_idle();

    let abandoned = worker.request(&mut hardware).expect("request is published");
    assert_eq!(worker.cancel_owned(abandoned), Ok(()));
    assert_eq!(
        worker.phase(),
        BluetoothControllerTimeWorkerPhase::DrainingOrphan
    );
    assert_eq!(
        worker.drain_orphan(&mut hardware),
        Ok(BluetoothControllerTimeEventStep::OrphanDrained)
    );
    assert_eq!(worker.phase(), BluetoothControllerTimeWorkerPhase::Idle);

    let fresh = worker
        .request(&mut hardware)
        .expect("fresh request is published");
    let sample = match worker
        .recheck_owned(fresh, &mut hardware)
        .expect("fresh request completes")
    {
        BluetoothControllerTimeEventStep::Sample { request, sample } => {
            assert_eq!(request, fresh);
            sample
        }
        other => panic!("unexpected event step: {other:?}"),
    };
    assert_eq!(sample.raw_ticks(), 0x2222_2222);
    assert_eq!(
        hardware.operations,
        [
            Operation::Begin,
            Operation::Step,
            Operation::Begin,
            Operation::Step
        ]
    );
}

#[test]
fn orphan_waiting_blocks_reuse_until_the_abandoned_request_is_drained() {
    let mut hardware = ModelHardware::new([
        BluetoothControllerTimeLatchStep::Waiting,
        BluetoothControllerTimeLatchStep::Ready(BluetoothControllerLatchedTime::from_bits(
            0x1111_1111,
        )),
        BluetoothControllerTimeLatchStep::Ready(BluetoothControllerLatchedTime::from_bits(
            0x2222_2222,
        )),
    ]);
    let mut worker = BluetoothControllerTimeWorker::new_idle();

    let abandoned = worker.request(&mut hardware).expect("request is published");
    worker
        .cancel_owned(abandoned)
        .expect("the exact request can be abandoned");
    assert_eq!(
        worker.drain_orphan(&mut hardware),
        Ok(BluetoothControllerTimeEventStep::Waiting)
    );
    assert!(worker.needs_recheck());
    assert!(!worker.is_reunitable());

    let operations_before_retry = hardware.operations.clone();
    assert_eq!(
        worker.request(&mut hardware),
        Err(BluetoothControllerTimeRequestError::Busy)
    );
    assert_eq!(hardware.operations, operations_before_retry);

    assert_eq!(
        worker.drain_orphan(&mut hardware),
        Ok(BluetoothControllerTimeEventStep::OrphanDrained)
    );
    assert!(worker.is_reunitable());
    assert!(!worker.needs_recheck());

    let fresh = worker
        .request(&mut hardware)
        .expect("the drained worker accepts a fresh request");
    assert!(matches!(
        worker.recheck_owned(fresh, &mut hardware),
        Ok(BluetoothControllerTimeEventStep::Sample { request, sample })
            if request == fresh && sample.raw_ticks() == 0x2222_2222
    ));
}

#[test]
fn mismatched_cancellation_faults_without_touching_newer_hardware() {
    let mut hardware = ModelHardware::new([
        BluetoothControllerTimeLatchStep::Ready(BluetoothControllerLatchedTime::from_bits(1)),
        BluetoothControllerTimeLatchStep::Ready(BluetoothControllerLatchedTime::from_bits(2)),
    ]);
    let mut worker = BluetoothControllerTimeWorker::new_idle();

    let first = worker.request(&mut hardware).expect("first request");
    assert!(matches!(
        worker.recheck_owned(first, &mut hardware),
        Ok(BluetoothControllerTimeEventStep::Sample {
            request,
            sample: _
        }) if request == first
    ));

    let second = worker.request(&mut hardware).expect("second request");
    assert_ne!(first, second);
    assert_eq!(
        worker.cancel_owned(first),
        Err(BluetoothControllerTimeEventError::RequestMismatch)
    );
    assert_eq!(worker.phase(), BluetoothControllerTimeWorkerPhase::Faulted);
    assert_eq!(
        worker.cancel_owned(second),
        Err(BluetoothControllerTimeEventError::Faulted)
    );
    assert_eq!(
        worker.recheck_owned(second, &mut hardware),
        Err(BluetoothControllerTimeEventError::Faulted)
    );
    assert_eq!(
        hardware.operations,
        [Operation::Begin, Operation::Step, Operation::Begin]
    );
}

#[test]
fn mismatched_recheck_faults_before_hardware_observation() {
    let mut hardware = ModelHardware::new([
        BluetoothControllerTimeLatchStep::Ready(BluetoothControllerLatchedTime::from_bits(1)),
        BluetoothControllerTimeLatchStep::Ready(BluetoothControllerLatchedTime::from_bits(2)),
    ]);
    let mut worker = BluetoothControllerTimeWorker::new_idle();

    let first = worker.request(&mut hardware).expect("first request");
    assert!(matches!(
        worker.recheck_owned(first, &mut hardware),
        Ok(BluetoothControllerTimeEventStep::Sample { request, .. }) if request == first
    ));
    let second = worker.request(&mut hardware).expect("second request");

    assert_eq!(
        worker.recheck_owned(first, &mut hardware),
        Err(BluetoothControllerTimeEventError::RequestMismatch)
    );
    assert_eq!(worker.phase(), BluetoothControllerTimeWorkerPhase::Faulted);
    assert_eq!(
        worker.recheck_owned(second, &mut hardware),
        Err(BluetoothControllerTimeEventError::Faulted)
    );
    assert_eq!(
        hardware.operations,
        [Operation::Begin, Operation::Step, Operation::Begin]
    );
}

#[test]
fn lower_ownership_collision_is_sticky_fail_stop() {
    let mut hardware = ModelHardware::new([BluetoothControllerTimeLatchStep::Waiting]);
    let mut worker = BluetoothControllerTimeWorker::new_idle();
    hardware.in_flight = true;

    assert_eq!(
        worker.request(&mut hardware),
        Err(BluetoothControllerTimeRequestError::OwnershipCollision)
    );
    assert!(hardware.operations.is_empty());
    assert_eq!(worker.phase(), BluetoothControllerTimeWorkerPhase::Faulted);
    assert!(!worker.needs_recheck());
    assert_eq!(
        worker.request(&mut hardware),
        Err(BluetoothControllerTimeRequestError::Faulted)
    );
}

#[test]
fn idle_orphan_drain_and_duplicate_request_do_not_touch_hardware() {
    let mut hardware = ModelHardware::new([BluetoothControllerTimeLatchStep::Waiting]);
    let mut worker = BluetoothControllerTimeWorker::new_idle();

    assert_eq!(
        worker.drain_orphan(&mut hardware),
        Ok(BluetoothControllerTimeEventStep::Idle)
    );
    assert!(hardware.operations.is_empty());

    let _request = worker.request(&mut hardware).expect("request is published");
    assert_eq!(
        worker.request(&mut hardware),
        Err(BluetoothControllerTimeRequestError::Busy)
    );
    assert_eq!(hardware.operations, [Operation::Begin]);
}

#[test]
fn ownership_loss_is_sticky_fail_stop_without_more_hardware_access() {
    let mut hardware = ModelHardware::new([]);
    let mut worker = BluetoothControllerTimeWorker::new_idle();

    let request = worker.request(&mut hardware).expect("request is published");
    hardware.in_flight = false;
    assert_eq!(
        worker.recheck_owned(request, &mut hardware),
        Err(BluetoothControllerTimeEventError::OwnershipLost)
    );
    assert_eq!(worker.phase(), BluetoothControllerTimeWorkerPhase::Faulted);
    assert_eq!(hardware.operations, [Operation::Begin]);

    assert_eq!(
        worker.recheck_owned(request, &mut hardware),
        Err(BluetoothControllerTimeEventError::Faulted)
    );
    assert_eq!(
        worker.request(&mut hardware),
        Err(BluetoothControllerTimeRequestError::Faulted)
    );
    assert_eq!(hardware.operations, [Operation::Begin]);
}

#[test]
fn exhausted_generation_faults_before_hardware_publication() {
    let mut hardware = ModelHardware::new([]);
    let mut worker = BluetoothControllerTimeWorker::new_idle();
    worker.exhaust_generation_for_validation();

    assert_eq!(
        worker.request(&mut hardware),
        Err(BluetoothControllerTimeRequestError::GenerationExhausted)
    );
    assert!(hardware.operations.is_empty());
    assert_eq!(worker.phase(), BluetoothControllerTimeWorkerPhase::Faulted);
    assert!(!worker.is_reunitable());
}

#[test]
fn scheduler_epoch_matches_forward_backward_and_wrapping_branches() {
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    let epoch = BluetoothControllerSchedulerEpoch::new(sample(100), 1_000, scale);

    assert_eq!(epoch.project(sample(103)), 1_012);
    assert_eq!(epoch.project(sample(97)), 988);

    let wrapping_epoch = BluetoothControllerSchedulerEpoch::new(sample(0xffff_fffe), 1_000, scale);
    assert_eq!(wrapping_epoch.project(sample(1)), 1_012);

    let reverse_wrapping_epoch = BluetoothControllerSchedulerEpoch::new(sample(1), 1_000, scale);
    assert_eq!(reverse_wrapping_epoch.project(sample(0xffff_fffe)), 988);

    assert_eq!(epoch.raw_ticks_for_micros(1_012), 103);
    assert_eq!(epoch.raw_ticks_for_micros(1_015), 103);
    assert_eq!(epoch.raw_ticks_for_micros(988), 97);
    assert_eq!(epoch.raw_ticks_for_micros(985), 97);

    let scheduler_wrapping_epoch =
        BluetoothControllerSchedulerEpoch::new(sample(100), 0xffff_fffe, scale);
    assert_eq!(scheduler_wrapping_epoch.raw_ticks_for_micros(10), 103);
}

#[test]
fn first_live_epoch_uses_each_reviewed_forward_scale() {
    let cases = [
        (
            BluetoothHalInitScale::Eight,
            BluetoothHalInitPeriod::Image2000,
        ),
        (
            BluetoothHalInitScale::Eight,
            BluetoothHalInitPeriod::Image1000,
        ),
        (
            BluetoothHalInitScale::Eight,
            BluetoothHalInitPeriod::Image500,
        ),
        (
            BluetoothHalInitScale::Sixteen,
            BluetoothHalInitPeriod::Image2000,
        ),
        (
            BluetoothHalInitScale::Sixteen,
            BluetoothHalInitPeriod::Image1000,
        ),
        (
            BluetoothHalInitScale::Sixteen,
            BluetoothHalInitPeriod::Image500,
        ),
    ];
    let raw_tick_anchor = 0x1234_5678;

    for (hal_scale, period) in cases {
        let scale = BluetoothControllerHalInitConfig::new(hal_scale, 11, 33, period)
            .controller_time_scale();
        let first_sample = sample(raw_tick_anchor);
        let epoch = BluetoothControllerSchedulerEpoch::from_first_live_update(&first_sample, scale);
        let expected = scale.micros_from_raw_ticks(raw_tick_anchor);

        assert_eq!(epoch.project(sample(raw_tick_anchor)), expected);
        assert_eq!(
            epoch.project(sample(raw_tick_anchor.wrapping_add(3))),
            expected.wrapping_add(scale.micros_from_raw_ticks(3))
        );
    }
}

#[test]
fn first_live_epoch_and_affine_now_retain_wrapping_projection() {
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    let first_sample = sample(0x4000_0000);
    let epoch = BluetoothControllerSchedulerEpoch::from_first_live_update(&first_sample, scale);
    let now = BluetoothControllerSchedulerNow::from_retained_epoch(epoch, first_sample);

    assert_eq!(now.epoch(), epoch);
    assert_eq!(now.sample().raw_ticks(), 0x4000_0000);
    assert_eq!(now.micros(), 0);
}

#[test]
fn live_reanchor_preserves_forward_time_and_updates_inverse_wrap_alias() {
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    let first_sample = sample(0);
    let first_epoch =
        BluetoothControllerSchedulerEpoch::from_first_live_update(&first_sample, scale);
    let later_sample = sample(0x4000_0000);
    let later_scheduler_image = first_epoch.project_raw_ticks(later_sample.raw_ticks());
    let reanchored = first_epoch.reanchor(&later_sample);

    assert_eq!(
        reanchored.project_raw_ticks(later_sample.raw_ticks()),
        later_scheduler_image
    );
    assert_eq!(first_epoch.raw_ticks_for_micros(4), 1);
    assert_eq!(reanchored.raw_ticks_for_micros(4), 0x4000_0001);
}

#[test]
fn post_enable_projection_retains_the_existing_scheduler_epoch() {
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    let epoch = BluetoothControllerSchedulerEpoch::new(sample(100), 1_000, scale);
    let post_enable_sample = sample(103);

    assert_eq!(epoch.project_without_reanchor(&post_enable_sample), 1_012);
    assert_eq!(epoch.raw_ticks_for_micros(1_012), 103);
}
