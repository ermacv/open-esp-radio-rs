//! Event-driven controller-time latch and scheduler-epoch projection.
//!
//! Every in-flight observation is a single decision. `Waiting` tells the
//! caller to return to its executor and arrange a later interrupt or bounded
//! timer recheck; no state here spins, allocates, stores a waker or depends on
//! an RTOS.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_pac::{BluetoothControllerLatchedTime, BluetoothControllerTimeScale};

/// One ordered controller-time sample from the always-awake latch path.
#[derive(Debug, Eq, PartialEq)]
pub struct BluetoothControllerTimeSample {
    latched_time: BluetoothControllerLatchedTime,
}

impl BluetoothControllerTimeSample {
    #[cfg(any(target_arch = "riscv32", test, feature = "validation-probes"))]
    const fn from_live_latch(latched_time: BluetoothControllerLatchedTime) -> Self {
        Self { latched_time }
    }

    #[cfg(test)]
    pub(crate) const fn for_validation(raw_time: u32) -> Self {
        Self::from_live_latch(BluetoothControllerLatchedTime::from_bits(raw_time))
    }

    /// Return the complete wrapping raw-time image.
    pub const fn raw_time(&self) -> u32 {
        self.latched_time.bits()
    }
}

#[cfg(any(target_arch = "riscv32", test, feature = "validation-probes"))]
mod worker {
    use open_esp_radio_esp32s31_hal::{
        BluetoothControllerHal, BluetoothControllerTimeLatchBeginError,
        BluetoothControllerTimeLatchStep, BluetoothControllerTimeLatchStepError,
    };

    use super::BluetoothControllerTimeSample;

    /// Crate-private hardware seam required by the controller-time worker.
    ///
    /// Keeping this boundary private prevents downstream code from manufacturing
    /// a supposedly live sample. Production has exactly one implementation: the
    /// finite borrow of the unique controller HAL owner.
    pub(crate) trait BluetoothControllerTimeHardware {
        /// Publish one fresh request without waiting for hardware.
        fn begin_controller_time_latch(
            &mut self,
        ) -> Result<(), BluetoothControllerTimeLatchBeginError>;

        /// Perform exactly one hardware observation and return immediately.
        fn step_controller_time_latch(
            &mut self,
        ) -> Result<BluetoothControllerTimeLatchStep, BluetoothControllerTimeLatchStepError>;
    }

    impl BluetoothControllerTimeHardware for BluetoothControllerHal<'_> {
        fn begin_controller_time_latch(
            &mut self,
        ) -> Result<(), BluetoothControllerTimeLatchBeginError> {
            BluetoothControllerHal::begin_controller_time_latch(self)
        }

        fn step_controller_time_latch(
            &mut self,
        ) -> Result<BluetoothControllerTimeLatchStep, BluetoothControllerTimeLatchStepError>
        {
            BluetoothControllerHal::step_controller_time_latch(self)
        }
    }

    /// Opaque identity of one logical controller-time request.
    ///
    /// A cancellation path must return the identity it received from `request`.
    /// This prevents a late cancellation from abandoning a newer request.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    #[must_use = "the request identity must be completed or abandoned"]
    pub struct BluetoothControllerTimeRequest(u64);

    /// Durable logical phase retained between executor events.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum BluetoothControllerTimeWorkerPhase {
        /// No logical or hardware request belongs to this worker.
        Idle,
        /// This worker published the request and may return its completed sample.
        Requested,
        /// An abandoned request must be drained without becoming a sample for a
        /// later logical caller.
        DrainingOrphan,
        /// Hardware and logical ownership disagreed; no further MMIO is allowed.
        Faulted,
    }

    /// Why a fresh logical controller-time request was not published.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum BluetoothControllerTimeRequestError {
        /// Another logical request or orphan drain is active; no MMIO occurred.
        Busy,
        /// The lower owner already held a request although the durable worker was
        /// idle. The worker entered fail-stop instead of stealing that request.
        OwnershipCollision,
        /// The non-repeating request generation space was exhausted before MMIO.
        GenerationExhausted,
        /// A prior ownership mismatch put the worker into fail-stop.
        Faulted,
    }

    /// Result of exactly one controller-time recheck event.
    #[derive(Debug, Eq, PartialEq)]
    #[must_use = "the worker event outcome must drive the next controller action"]
    pub enum BluetoothControllerTimeEventStep {
        /// No transaction was active; no MMIO occurred.
        Idle,
        /// Hardware still owns the request; arrange one later recheck event.
        Waiting,
        /// The request owned by this worker produced one ordered live sample.
        Sample {
            /// Identity of the logical request which owns this result.
            request: BluetoothControllerTimeRequest,
            /// Ordered sample read by the live HAL path.
            sample: BluetoothControllerTimeSample,
        },
        /// An abandoned request was drained without relabelling its latched word
        /// as a new logical sample.
        OrphanDrained,
    }

    /// Fail-stop result of a controller-time recheck event.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum BluetoothControllerTimeEventError {
        /// The lower sticky owner disappeared while the worker was active.
        OwnershipLost,
        /// A previous ownership mismatch already stopped this worker.
        Faulted,
    }

    /// Executor-neutral logical owner of controller-time sampling.
    ///
    /// The sole controller runner must retain this state beside its task resources,
    /// while every method receives only a short HAL borrow. `on_recheck_event` performs at
    /// most one hardware observation and contains no loop, await, waker, timer,
    /// allocator or RTOS binding.
    pub(crate) struct BluetoothControllerTimeWorker {
        state: BluetoothControllerTimeWorkerState,
        last_generation: u64,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum BluetoothControllerTimeWorkerState {
        Idle,
        Requested(BluetoothControllerTimeRequest),
        DrainingOrphan(BluetoothControllerTimeRequest),
        Faulted,
    }

    impl BluetoothControllerTimeWorker {
        /// Construct the sole worker while splitting a proven-cold task owner.
        ///
        /// This constructor is intentionally private to the crate ownership
        /// transition. Moving later lifecycle typestates carries this value; they
        /// never reconstruct it from a borrowed HAL or clear a fault by dropping
        /// the worker.
        pub(crate) const fn new_idle() -> Self {
            Self {
                state: BluetoothControllerTimeWorkerState::Idle,
                last_generation: 0,
            }
        }

        /// Current durable logical phase.
        pub(crate) const fn phase(&self) -> BluetoothControllerTimeWorkerPhase {
            match self.state {
                BluetoothControllerTimeWorkerState::Idle => {
                    BluetoothControllerTimeWorkerPhase::Idle
                }
                BluetoothControllerTimeWorkerState::Requested(_) => {
                    BluetoothControllerTimeWorkerPhase::Requested
                }
                BluetoothControllerTimeWorkerState::DrainingOrphan(_) => {
                    BluetoothControllerTimeWorkerPhase::DrainingOrphan
                }
                BluetoothControllerTimeWorkerState::Faulted => {
                    BluetoothControllerTimeWorkerPhase::Faulted
                }
            }
        }

        /// Whether an external durable wake or bounded timer must recheck hardware.
        pub(crate) const fn needs_recheck(&self) -> bool {
            matches!(
                self.state,
                BluetoothControllerTimeWorkerState::Requested(_)
                    | BluetoothControllerTimeWorkerState::DrainingOrphan(_)
            )
        }

        /// Whether the complete task owner may return to the cold ownership state.
        #[cfg(test)]
        pub(crate) const fn is_reunitable(&self) -> bool {
            matches!(self.state, BluetoothControllerTimeWorkerState::Idle)
        }

        #[cfg(test)]
        pub(crate) fn exhaust_generation_for_validation(&mut self) {
            self.last_generation = u64::MAX;
        }

        /// Publish one fresh logical sample request.
        ///
        /// Non-idle phases return without MMIO. A lower-layer collision proves
        /// desynchronized ownership and permanently faults this worker.
        pub(crate) fn request(
            &mut self,
            hardware: &mut impl BluetoothControllerTimeHardware,
        ) -> Result<BluetoothControllerTimeRequest, BluetoothControllerTimeRequestError> {
            match self.state {
                BluetoothControllerTimeWorkerState::Idle => {}
                BluetoothControllerTimeWorkerState::Requested(_)
                | BluetoothControllerTimeWorkerState::DrainingOrphan(_) => {
                    return Err(BluetoothControllerTimeRequestError::Busy);
                }
                BluetoothControllerTimeWorkerState::Faulted => {
                    return Err(BluetoothControllerTimeRequestError::Faulted);
                }
            }

            let Some(generation) = self.last_generation.checked_add(1) else {
                self.state = BluetoothControllerTimeWorkerState::Faulted;
                return Err(BluetoothControllerTimeRequestError::GenerationExhausted);
            };
            let request = BluetoothControllerTimeRequest(generation);

            match hardware.begin_controller_time_latch() {
                Ok(()) => {
                    self.last_generation = generation;
                    self.state = BluetoothControllerTimeWorkerState::Requested(request);
                    Ok(request)
                }
                Err(BluetoothControllerTimeLatchBeginError::AlreadyInFlight) => {
                    self.state = BluetoothControllerTimeWorkerState::Faulted;
                    Err(BluetoothControllerTimeRequestError::OwnershipCollision)
                }
            }
        }

        /// Abandon the current logical caller without clearing hardware ownership.
        ///
        /// A later event must still drain the request, but its latched value is no
        /// longer allowed to satisfy any logical sample request.
        pub(crate) fn abandon(&mut self, request: BluetoothControllerTimeRequest) -> bool {
            if self.state == BluetoothControllerTimeWorkerState::Requested(request) {
                self.state = BluetoothControllerTimeWorkerState::DrainingOrphan(request);
                true
            } else {
                false
            }
        }

        /// Handle exactly one durable controller event or bounded timer recheck.
        pub(crate) fn on_recheck_event(
            &mut self,
            hardware: &mut impl BluetoothControllerTimeHardware,
        ) -> Result<BluetoothControllerTimeEventStep, BluetoothControllerTimeEventError> {
            let request = match self.state {
                BluetoothControllerTimeWorkerState::Idle => {
                    return Ok(BluetoothControllerTimeEventStep::Idle);
                }
                BluetoothControllerTimeWorkerState::Requested(request) => Some(request),
                BluetoothControllerTimeWorkerState::DrainingOrphan(_) => None,
                BluetoothControllerTimeWorkerState::Faulted => {
                    return Err(BluetoothControllerTimeEventError::Faulted);
                }
            };

            match hardware.step_controller_time_latch() {
                Ok(BluetoothControllerTimeLatchStep::Waiting) => {
                    Ok(BluetoothControllerTimeEventStep::Waiting)
                }
                Ok(BluetoothControllerTimeLatchStep::Ready(latched_time)) => {
                    self.state = BluetoothControllerTimeWorkerState::Idle;
                    if let Some(request) = request {
                        Ok(BluetoothControllerTimeEventStep::Sample {
                            request,
                            sample: BluetoothControllerTimeSample::from_live_latch(latched_time),
                        })
                    } else {
                        Ok(BluetoothControllerTimeEventStep::OrphanDrained)
                    }
                }
                Err(BluetoothControllerTimeLatchStepError::NotInFlight) => {
                    self.state = BluetoothControllerTimeWorkerState::Faulted;
                    Err(BluetoothControllerTimeEventError::OwnershipLost)
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) use worker::BluetoothControllerTimeHardware;
#[cfg(target_arch = "riscv32")]
pub use worker::BluetoothControllerTimeRequest;
#[cfg(any(target_arch = "riscv32", test, feature = "validation-probes"))]
pub(crate) use worker::BluetoothControllerTimeWorker;
#[cfg(any(target_arch = "riscv32", test, feature = "validation-probes"))]
pub use worker::{
    BluetoothControllerTimeEventError, BluetoothControllerTimeEventStep,
    BluetoothControllerTimeRequestError, BluetoothControllerTimeWorkerPhase,
};

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
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn new(
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
    use std::{collections::VecDeque, vec::Vec};

    use open_esp_radio_esp32s31_hal::{
        BluetoothControllerTimeLatchBeginError, BluetoothControllerTimeLatchStep,
        BluetoothControllerTimeLatchStepError,
    };
    use open_esp_radio_esp32s31_pac::{
        BluetoothControllerHalInitConfig, BluetoothControllerLatchedTime,
    };

    use super::{
        BluetoothControllerSchedulerEpoch, BluetoothControllerTimeEventError,
        BluetoothControllerTimeEventStep, BluetoothControllerTimeHardware,
        BluetoothControllerTimeRequestError, BluetoothControllerTimeSample,
        BluetoothControllerTimeWorker, BluetoothControllerTimeWorkerPhase,
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
        ) -> Result<BluetoothControllerTimeLatchStep, BluetoothControllerTimeLatchStepError>
        {
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

    const fn sample(raw_time: u32) -> BluetoothControllerTimeSample {
        BluetoothControllerTimeSample::for_validation(raw_time)
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
            worker.on_recheck_event(&mut hardware),
            Ok(BluetoothControllerTimeEventStep::Waiting)
        );
        assert_eq!(hardware.operations, [Operation::Begin, Operation::Step]);
        assert_eq!(
            worker.phase(),
            BluetoothControllerTimeWorkerPhase::Requested
        );

        let sample = match worker
            .on_recheck_event(&mut hardware)
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
        assert_eq!(sample.raw_time(), 0x1234_5678);
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
        assert!(worker.abandon(abandoned));
        assert_eq!(
            worker.phase(),
            BluetoothControllerTimeWorkerPhase::DrainingOrphan
        );
        assert_eq!(
            worker.on_recheck_event(&mut hardware),
            Ok(BluetoothControllerTimeEventStep::OrphanDrained)
        );
        assert_eq!(worker.phase(), BluetoothControllerTimeWorkerPhase::Idle);

        let fresh = worker
            .request(&mut hardware)
            .expect("fresh request is published");
        let sample = match worker
            .on_recheck_event(&mut hardware)
            .expect("fresh request completes")
        {
            BluetoothControllerTimeEventStep::Sample { request, sample } => {
                assert_eq!(request, fresh);
                sample
            }
            other => panic!("unexpected event step: {other:?}"),
        };
        assert_eq!(sample.raw_time(), 0x2222_2222);
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
    fn late_cancellation_cannot_abandon_a_newer_request() {
        let mut hardware = ModelHardware::new([
            BluetoothControllerTimeLatchStep::Ready(BluetoothControllerLatchedTime::from_bits(1)),
            BluetoothControllerTimeLatchStep::Ready(BluetoothControllerLatchedTime::from_bits(2)),
        ]);
        let mut worker = BluetoothControllerTimeWorker::new_idle();

        let first = worker.request(&mut hardware).expect("first request");
        assert!(matches!(
            worker.on_recheck_event(&mut hardware),
            Ok(BluetoothControllerTimeEventStep::Sample {
                request,
                sample: _
            }) if request == first
        ));

        let second = worker.request(&mut hardware).expect("second request");
        assert_ne!(first, second);
        assert!(!worker.abandon(first));
        assert_eq!(
            worker.phase(),
            BluetoothControllerTimeWorkerPhase::Requested
        );
        assert!(matches!(
            worker.on_recheck_event(&mut hardware),
            Ok(BluetoothControllerTimeEventStep::Sample { request, sample })
                if request == second && sample.raw_time() == 2
        ));
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
    fn idle_event_and_duplicate_request_do_not_touch_hardware() {
        let mut hardware = ModelHardware::new([BluetoothControllerTimeLatchStep::Waiting]);
        let mut worker = BluetoothControllerTimeWorker::new_idle();

        assert_eq!(
            worker.on_recheck_event(&mut hardware),
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

        let _request = worker.request(&mut hardware).expect("request is published");
        hardware.in_flight = false;
        assert_eq!(
            worker.on_recheck_event(&mut hardware),
            Err(BluetoothControllerTimeEventError::OwnershipLost)
        );
        assert_eq!(worker.phase(), BluetoothControllerTimeWorkerPhase::Faulted);
        assert_eq!(hardware.operations, [Operation::Begin]);

        assert_eq!(
            worker.on_recheck_event(&mut hardware),
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
