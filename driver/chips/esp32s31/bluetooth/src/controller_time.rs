//! Event-driven controller-time latch and scheduler-epoch projection.
//!
//! Every in-flight observation is a single decision. `Waiting` tells the
//! caller to return to its executor and arrange a later interrupt or bounded
//! timer recheck; no state here spins, allocates, stores a waker or depends on
//! an RTOS.

#![forbid(unsafe_code)]

#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_esp32s31_pac::{BluetoothControllerLatchedTime, BluetoothControllerTimeScale};

/// One ordered controller-time sample from the always-awake latch path.
#[derive(Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct BluetoothControllerTimeSample {
    latched_time: BluetoothControllerLatchedTime,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothControllerTimeSample {
    #[cfg(any(target_arch = "riscv32", test, feature = "validation-probes"))]
    const fn from_live_latch(latched_time: BluetoothControllerLatchedTime) -> Self {
        Self { latched_time }
    }

    #[cfg(test)]
    pub(crate) const fn for_validation(raw_ticks: u32) -> Self {
        Self::from_live_latch(BluetoothControllerLatchedTime::from_bits(raw_ticks))
    }

    /// Return the complete wrapping raw controller-tick image.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn raw_ticks(&self) -> u32 {
        self.latched_time.bits()
    }

    /// Borrow the typed hardware sample for one private descriptor update.
    ///
    /// This does not expose an integer image or duplicate scheduler-time
    /// authority. The returned PAC value can only enter a lower typed codec.
    #[cfg(target_arch = "riscv32")]
    pub(crate) const fn latched_time(&self) -> BluetoothControllerLatchedTime {
        self.latched_time
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
    /// A late identity cannot silently abandon a newer request: it faults the
    /// private worker before any additional hardware observation.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    #[must_use = "the request identity must be completed or abandoned"]
    pub(crate) struct BluetoothControllerTimeRequest(u64);

    impl BluetoothControllerTimeRequest {
        #[cfg(test)]
        pub(crate) const fn for_validation(generation: u64) -> Self {
            Self(generation)
        }
    }

    /// Durable logical phase retained between executor events.
    #[cfg(test)]
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub(crate) enum BluetoothControllerTimeWorkerPhase {
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
    pub(crate) enum BluetoothControllerTimeRequestError {
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
    pub(crate) enum BluetoothControllerTimeEventStep {
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
    pub(crate) enum BluetoothControllerTimeEventError {
        /// The caller did not present the identity owned by the active request.
        RequestMismatch,
        /// The lower sticky owner disappeared while the worker was active.
        OwnershipLost,
        /// A previous ownership mismatch already stopped this worker.
        Faulted,
    }

    /// Executor-neutral logical owner of controller-time sampling.
    ///
    /// The sole controller runner must retain this state beside its task resources,
    /// while every method receives only a short HAL borrow. Each recheck performs
    /// at most one hardware observation and contains no loop, await, waker, timer,
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
        #[cfg(test)]
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
        #[cfg(test)]
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
        pub(crate) fn cancel_owned(
            &mut self,
            request: BluetoothControllerTimeRequest,
        ) -> Result<(), BluetoothControllerTimeEventError> {
            match self.state {
                BluetoothControllerTimeWorkerState::Requested(owned) if owned == request => {
                    self.state = BluetoothControllerTimeWorkerState::DrainingOrphan(request);
                    Ok(())
                }
                BluetoothControllerTimeWorkerState::Faulted => {
                    Err(BluetoothControllerTimeEventError::Faulted)
                }
                BluetoothControllerTimeWorkerState::Idle
                | BluetoothControllerTimeWorkerState::Requested(_)
                | BluetoothControllerTimeWorkerState::DrainingOrphan(_) => {
                    self.state = BluetoothControllerTimeWorkerState::Faulted;
                    Err(BluetoothControllerTimeEventError::RequestMismatch)
                }
            }
        }

        /// Recheck exactly the request whose affine owner is presented.
        pub(crate) fn recheck_owned(
            &mut self,
            request: BluetoothControllerTimeRequest,
            hardware: &mut impl BluetoothControllerTimeHardware,
        ) -> Result<BluetoothControllerTimeEventStep, BluetoothControllerTimeEventError> {
            match self.state {
                BluetoothControllerTimeWorkerState::Requested(owned) if owned == request => {}
                BluetoothControllerTimeWorkerState::Faulted => {
                    return Err(BluetoothControllerTimeEventError::Faulted);
                }
                BluetoothControllerTimeWorkerState::Idle
                | BluetoothControllerTimeWorkerState::Requested(_)
                | BluetoothControllerTimeWorkerState::DrainingOrphan(_) => {
                    self.state = BluetoothControllerTimeWorkerState::Faulted;
                    return Err(BluetoothControllerTimeEventError::RequestMismatch);
                }
            }

            match hardware.step_controller_time_latch() {
                Ok(BluetoothControllerTimeLatchStep::Waiting) => {
                    Ok(BluetoothControllerTimeEventStep::Waiting)
                }
                Ok(BluetoothControllerTimeLatchStep::Ready(latched_time)) => {
                    self.state = BluetoothControllerTimeWorkerState::Idle;
                    Ok(BluetoothControllerTimeEventStep::Sample {
                        request,
                        sample: BluetoothControllerTimeSample::from_live_latch(latched_time),
                    })
                }
                Err(BluetoothControllerTimeLatchStepError::NotInFlight) => {
                    self.state = BluetoothControllerTimeWorkerState::Faulted;
                    Err(BluetoothControllerTimeEventError::OwnershipLost)
                }
            }
        }

        /// Drain one abandoned hardware request without creating a sample.
        pub(crate) fn drain_orphan(
            &mut self,
            hardware: &mut impl BluetoothControllerTimeHardware,
        ) -> Result<BluetoothControllerTimeEventStep, BluetoothControllerTimeEventError> {
            match self.state {
                BluetoothControllerTimeWorkerState::Idle => {
                    return Ok(BluetoothControllerTimeEventStep::Idle);
                }
                BluetoothControllerTimeWorkerState::DrainingOrphan(_) => {}
                BluetoothControllerTimeWorkerState::Faulted => {
                    return Err(BluetoothControllerTimeEventError::Faulted);
                }
                BluetoothControllerTimeWorkerState::Requested(_) => {
                    self.state = BluetoothControllerTimeWorkerState::Faulted;
                    return Err(BluetoothControllerTimeEventError::RequestMismatch);
                }
            }

            match hardware.step_controller_time_latch() {
                Ok(BluetoothControllerTimeLatchStep::Waiting) => {
                    Ok(BluetoothControllerTimeEventStep::Waiting)
                }
                Ok(BluetoothControllerTimeLatchStep::Ready(_)) => {
                    self.state = BluetoothControllerTimeWorkerState::Idle;
                    Ok(BluetoothControllerTimeEventStep::OrphanDrained)
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
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) use worker::BluetoothControllerTimeRequest;
#[cfg(test)]
pub(crate) use worker::BluetoothControllerTimeWorkerPhase;
#[cfg(any(target_arch = "riscv32", test, feature = "validation-probes"))]
pub(crate) use worker::{
    BluetoothControllerTimeEventError, BluetoothControllerTimeEventStep,
    BluetoothControllerTimeRequestError, BluetoothControllerTimeWorker,
};

#[cfg(any(target_arch = "riscv32", test))]
pub(crate) trait BluetoothControllerTimePendingOwner {
    fn recheck_owned_controller_time(
        &mut self,
        request: BluetoothControllerTimeRequest,
    ) -> Result<BluetoothControllerTimePendingOwnerStep, BluetoothControllerTimeEventError>;

    fn cancel_owned_controller_time(
        &mut self,
        request: BluetoothControllerTimeRequest,
    ) -> Result<(), BluetoothControllerTimeEventError>;

    fn drain_orphan_controller_time(
        &mut self,
    ) -> Result<BluetoothControllerTimePendingOrphanStep, BluetoothControllerTimeEventError>;
}

#[cfg(any(target_arch = "riscv32", test))]
impl<T> BluetoothControllerTimePendingOwner for &mut T
where
    T: BluetoothControllerTimePendingOwner + ?Sized,
{
    fn recheck_owned_controller_time(
        &mut self,
        request: BluetoothControllerTimeRequest,
    ) -> Result<BluetoothControllerTimePendingOwnerStep, BluetoothControllerTimeEventError> {
        T::recheck_owned_controller_time(self, request)
    }

    fn cancel_owned_controller_time(
        &mut self,
        request: BluetoothControllerTimeRequest,
    ) -> Result<(), BluetoothControllerTimeEventError> {
        T::cancel_owned_controller_time(self, request)
    }

    fn drain_orphan_controller_time(
        &mut self,
    ) -> Result<BluetoothControllerTimePendingOrphanStep, BluetoothControllerTimeEventError> {
        T::drain_orphan_controller_time(self)
    }
}

#[cfg(any(target_arch = "riscv32", test))]
#[derive(Debug)]
pub(crate) enum BluetoothControllerTimePendingOwnerStep {
    Waiting,
    Ready(BluetoothControllerTimeSample),
}

#[cfg(any(target_arch = "riscv32", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothControllerTimePendingOrphanStep {
    Idle,
    Waiting,
    Drained,
}

#[cfg(any(target_arch = "riscv32", test))]
#[derive(Debug)]
pub(crate) struct BluetoothControllerTimePendingCore<O>
where
    O: BluetoothControllerTimePendingOwner,
{
    owner: Option<O>,
    request: Option<BluetoothControllerTimeRequest>,
}

#[cfg(any(target_arch = "riscv32", test))]
#[derive(Debug)]
pub(crate) enum BluetoothControllerTimePendingCoreStep<O>
where
    O: BluetoothControllerTimePendingOwner,
{
    Waiting(BluetoothControllerTimePendingCore<O>),
    Ready {
        owner: O,
        sample: BluetoothControllerTimeSample,
    },
}

#[cfg(any(target_arch = "riscv32", test))]
#[derive(Debug)]
pub(crate) struct BluetoothControllerTimePendingCoreFailure<O>
where
    O: BluetoothControllerTimePendingOwner,
{
    owner: O,
    error: BluetoothControllerTimeEventError,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<O> BluetoothControllerTimePendingCoreFailure<O>
where
    O: BluetoothControllerTimePendingOwner,
{
    pub(crate) fn into_parts(self) -> (O, BluetoothControllerTimeEventError) {
        (self.owner, self.error)
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl<O> BluetoothControllerTimePendingCore<O>
where
    O: BluetoothControllerTimePendingOwner,
{
    pub(crate) const fn new(owner: O, request: BluetoothControllerTimeRequest) -> Self {
        Self {
            owner: Some(owner),
            request: Some(request),
        }
    }

    pub(crate) fn recheck(
        mut self,
    ) -> Result<
        BluetoothControllerTimePendingCoreStep<O>,
        BluetoothControllerTimePendingCoreFailure<O>,
    > {
        let mut owner = self
            .owner
            .take()
            .expect("private pending-time core retains the exact owner");
        let request = self
            .request
            .take()
            .expect("private pending-time core retains the exact request");

        match owner.recheck_owned_controller_time(request) {
            Ok(BluetoothControllerTimePendingOwnerStep::Waiting) => Ok(
                BluetoothControllerTimePendingCoreStep::Waiting(Self::new(owner, request)),
            ),
            Ok(BluetoothControllerTimePendingOwnerStep::Ready(sample)) => {
                Ok(BluetoothControllerTimePendingCoreStep::Ready { owner, sample })
            }
            Err(error) => Err(BluetoothControllerTimePendingCoreFailure { owner, error }),
        }
    }

    pub(crate) fn cancel(mut self) -> Result<O, BluetoothControllerTimePendingCoreFailure<O>> {
        let mut owner = self
            .owner
            .take()
            .expect("private pending-time core retains the exact owner");
        let request = self
            .request
            .take()
            .expect("private pending-time core retains the exact request");

        match owner.cancel_owned_controller_time(request) {
            Ok(()) => Ok(owner),
            Err(error) => Err(BluetoothControllerTimePendingCoreFailure { owner, error }),
        }
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl<O> Drop for BluetoothControllerTimePendingCore<O>
where
    O: BluetoothControllerTimePendingOwner,
{
    fn drop(&mut self) {
        if let (Some(mut owner), Some(request)) = (self.owner.take(), self.request.take()) {
            let _result = owner.cancel_owned_controller_time(request);
        }
    }
}

#[cfg(any(target_arch = "riscv32", test))]
pub(crate) fn drain_controller_time_orphan(
    owner: &mut impl BluetoothControllerTimePendingOwner,
) -> Result<BluetoothControllerTimePendingOrphanStep, BluetoothControllerTimeEventError> {
    owner.drain_orphan_controller_time()
}

/// Raw-tick anchor paired with the BLE scheduler's microsecond epoch.
///
/// The projection exactly retains the current `r_sched_timer_convertTimeToUs`
/// branch geometry. Every reviewed S31 HAL configuration has a positive scale
/// image, so the helper's optional negative-side remainder is always zero.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct BluetoothControllerSchedulerEpoch {
    raw_tick_anchor: u32,
    micros_anchor: u32,
    scale: BluetoothControllerTimeScale,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothControllerSchedulerEpoch {
    /// Establish the source-owned epoch from its first live raw-tick update.
    ///
    /// Scheduler initialization starts both reference images at zero. Every
    /// reviewed ESP32-S31 time scale is positive, so the forward conversion's
    /// optional remainder is zero: the raw anchor is the exact sample and the
    /// scheduler anchor is its wrapping scale projection. The constructor only
    /// borrows the sample so the caller can then consume that same affine value
    /// into [`BluetoothControllerSchedulerNow`].
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn from_first_live_update(
        sample: &BluetoothControllerTimeSample,
        scale: BluetoothControllerTimeScale,
    ) -> Self {
        let raw_tick_anchor = sample.raw_ticks();
        Self {
            raw_tick_anchor,
            micros_anchor: scale.micros_from_raw_ticks(raw_tick_anchor),
            scale,
        }
    }

    /// Bind arbitrary validation anchors without publishing that authority in
    /// production code.
    #[cfg(test)]
    pub(crate) const fn new(
        sample: BluetoothControllerTimeSample,
        micros_anchor: u32,
        scale: BluetoothControllerTimeScale,
    ) -> Self {
        Self {
            raw_tick_anchor: sample.raw_ticks(),
            micros_anchor,
            scale,
        }
    }

    /// Project a later or earlier wrapping raw sample into the scheduler epoch.
    #[cfg(test)]
    pub(crate) const fn project(self, sample: BluetoothControllerTimeSample) -> u32 {
        self.project_without_reanchor(&sample)
    }

    /// Project one completed live sample while retaining the exact epoch.
    ///
    /// The post-enable timing observation is a separate scheduler-time read in
    /// the reviewed standalone flow. It must use the retained epoch, but unlike
    /// a task-run current-time update it does not advance either epoch anchor.
    pub(crate) const fn project_without_reanchor(
        self,
        sample: &BluetoothControllerTimeSample,
    ) -> u32 {
        self.project_raw_ticks(sample.raw_ticks())
    }

    /// Advance the raw anchor while preserving this sample's scheduler image.
    ///
    /// The Controller does this after every live task-run reference update.
    /// Re-anchoring is required even when the forward image is unchanged:
    /// shifting scales have wrapping aliases that the inverse projection can
    /// distinguish only through the latest raw anchor.
    pub(crate) const fn reanchor(self, sample: &BluetoothControllerTimeSample) -> Self {
        let raw_tick_anchor = sample.raw_ticks();
        Self {
            raw_tick_anchor,
            micros_anchor: self.project_raw_ticks(raw_tick_anchor),
            scale: self.scale,
        }
    }

    const fn project_raw_ticks(self, raw_ticks: u32) -> u32 {
        let delta = raw_ticks.wrapping_sub(self.raw_tick_anchor);
        if delta as i32 >= 0 {
            self.micros_anchor
                .wrapping_add(self.scale.micros_from_raw_ticks(delta))
        } else {
            self.micros_anchor
                .wrapping_sub(self.scale.micros_from_raw_ticks(delta.wrapping_neg()))
        }
    }

    /// Project one scheduler microsecond position back into raw controller ticks.
    ///
    /// This retains the complete `r_sched_timer_convertTimeToTicks` branch
    /// geometry. Discarded low scheduler bits are truncated toward the epoch
    /// anchor on both sides, matching the reviewed inverse helper.
    pub const fn raw_ticks_for_micros(self, micros: u32) -> u32 {
        let delta = micros.wrapping_sub(self.micros_anchor);
        if delta as i32 >= 0 {
            self.raw_tick_anchor
                .wrapping_add(self.scale.raw_ticks_from_micros(delta).whole_ticks)
        } else {
            self.raw_tick_anchor.wrapping_sub(
                self.scale
                    .raw_ticks_from_micros(delta.wrapping_neg())
                    .whole_ticks,
            )
        }
    }
}

/// One exact live sample bound to the retained Controller scheduler epoch.
///
/// This aggregate is deliberately affine: it cannot duplicate or detach the
/// sample from the epoch used to project it. Hardware ownership is not implied,
/// and the projected image is not RF-ready authority.
#[must_use = "the epoch-bound live scheduler sample must be consumed by scheduling"]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct BluetoothControllerSchedulerNow {
    epoch: BluetoothControllerSchedulerEpoch,
    sample: BluetoothControllerTimeSample,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothControllerSchedulerNow {
    /// Bind one exact sample to an already retained source-owned epoch.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn from_retained_epoch(
        epoch: BluetoothControllerSchedulerEpoch,
        sample: BluetoothControllerTimeSample,
    ) -> Self {
        Self { epoch, sample }
    }

    /// Retained epoch used for this exact projection.
    pub(crate) const fn epoch(&self) -> BluetoothControllerSchedulerEpoch {
        self.epoch
    }

    /// Exact raw sample retained by this projection.
    #[cfg(test)]
    pub(crate) const fn sample(&self) -> &BluetoothControllerTimeSample {
        &self.sample
    }

    /// Wrapping microsecond image projected from the retained sample and epoch.
    pub(crate) const fn micros(&self) -> u32 {
        self.epoch.project_raw_ticks(self.sample.raw_ticks())
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
        ) -> Result<BluetoothControllerTimePendingOwnerStep, BluetoothControllerTimeEventError>
        {
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
        ) -> Result<BluetoothControllerTimePendingOrphanStep, BluetoothControllerTimeEventError>
        {
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

        let wrapping_epoch =
            BluetoothControllerSchedulerEpoch::new(sample(0xffff_fffe), 1_000, scale);
        assert_eq!(wrapping_epoch.project(sample(1)), 1_012);

        let reverse_wrapping_epoch =
            BluetoothControllerSchedulerEpoch::new(sample(1), 1_000, scale);
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
            let epoch =
                BluetoothControllerSchedulerEpoch::from_first_live_update(&first_sample, scale);
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
}
