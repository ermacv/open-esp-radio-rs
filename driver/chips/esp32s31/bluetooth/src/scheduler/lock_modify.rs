//! Event-driven controller model for one scheduler lock/modify transaction.
//!
//! Every transition consumes a fresh cross-owner observation. `Waiting` means
//! the caller should return to its executor and resume only after an interrupt
//! or another controller event supplies a new observation; no path spins.

#![deny(unsafe_code)]

use core::{
    mem,
    sync::atomic::{AtomicU8, Ordering},
};

pub use open_esp_radio_esp32s31_hal::BluetoothSchedulerLockModifyInterruptObservation;
use open_esp_radio_esp32s31_hal::{
    BluetoothControllerHal, BluetoothSchedulerLockModifyObservation,
    BluetoothSchedulerLockModifyPublished, BluetoothSchedulerLockModifyRequest,
    BluetoothSchedulerLockModifyTaskObservation,
};

const EVENT_PENDING: u8 = 1 << 0;
const EVENT_BUSY: u8 = 1 << 1;

/// Result of evaluating one fresh scheduler event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "waiting state or completed transition must remain owned"]
enum BluetoothSchedulerLockModifyProgress<W, R> {
    /// Hardware still has both BUSY and START set.
    Waiting(W),
    /// The current phase can advance without polling.
    Ready(R),
}

/// Result of publishing one scheduler observation from the hard handler.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothSchedulerLockModifyEventPublication {
    /// The cell changed from empty to pending; wake the sole controller worker.
    WakeWorker,
    /// A pending observation was replaced by the newer scheduler state.
    Coalesced,
}

/// One affine scheduler event removed from the ISR-to-worker handoff.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the scheduler event must be processed or explicitly discarded"]
pub struct BluetoothSchedulerLockModifyEvent {
    interrupt: BluetoothSchedulerLockModifyInterruptObservation,
}

impl BluetoothSchedulerLockModifyEvent {
    /// Whether the newest interrupt-side scheduler observation was busy.
    pub const fn is_busy(&self) -> bool {
        self.interrupt.is_busy()
    }
}

/// Lock-free, allocation-free latest-value handoff from ISR to async worker.
///
/// The cell stores no count because every worker step performs fresh
/// task-owned observation and scheduler wait predicates are level-like. A
/// newer BUSY value replaces an older pending value. A publication racing
/// after [`Self::take`] creates a new pending epoch and therefore a new wake
/// edge; there is no RTOS queue or callback execution in interrupt context.
pub struct BluetoothSchedulerLockModifyEventCell {
    state: AtomicU8,
}

impl BluetoothSchedulerLockModifyEventCell {
    /// Construct an empty handoff.
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(0),
        }
    }

    /// Publish one semantic BUSY observation from the unique interrupt owner.
    pub fn publish_from_interrupt(
        &self,
        observation: BluetoothSchedulerLockModifyInterruptObservation,
    ) -> BluetoothSchedulerLockModifyEventPublication {
        let value = EVENT_PENDING | if observation.is_busy() { EVENT_BUSY } else { 0 };
        let previous = self.state.swap(value, Ordering::AcqRel);
        if previous & EVENT_PENDING == 0 {
            BluetoothSchedulerLockModifyEventPublication::WakeWorker
        } else {
            BluetoothSchedulerLockModifyEventPublication::Coalesced
        }
    }

    /// Remove the newest pending observation in one atomic operation.
    pub fn take(&self) -> Option<BluetoothSchedulerLockModifyEvent> {
        let state = self.state.swap(0, Ordering::AcqRel);
        if state & EVENT_PENDING == 0 {
            None
        } else {
            Some(BluetoothSchedulerLockModifyEvent {
                interrupt: BluetoothSchedulerLockModifyInterruptObservation::from_busy(
                    state & EVENT_BUSY != 0,
                ),
            })
        }
    }

    /// Whether an event is currently pending for the sole worker.
    pub fn is_pending(&self) -> bool {
        self.state.load(Ordering::Acquire) & EVENT_PENDING != 0
    }
}

impl Default for BluetoothSchedulerLockModifyEventCell {
    fn default() -> Self {
        Self::new()
    }
}

/// A validated transaction waiting for permission to publish.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the scheduler request has not yet been published"]
struct BluetoothSchedulerLockModifyAwaitingPublication {
    request: BluetoothSchedulerLockModifyRequest,
}

impl BluetoothSchedulerLockModifyAwaitingPublication {
    /// Start a pure transaction without touching MMIO.
    const fn new(request: BluetoothSchedulerLockModifyRequest) -> Self {
        Self { request }
    }

    /// Evaluate the pre-publication wait predicate once.
    const fn observe(
        self,
        observation: BluetoothSchedulerLockModifyObservation,
    ) -> BluetoothSchedulerLockModifyProgress<Self, BluetoothSchedulerLockModifyPublication> {
        if observation.wait_active() {
            BluetoothSchedulerLockModifyProgress::Waiting(self)
        } else {
            BluetoothSchedulerLockModifyProgress::Ready(BluetoothSchedulerLockModifyPublication {
                request: self.request,
            })
        }
    }
}

/// Permission for the task owner to publish one request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "publication must be performed or explicitly abandoned"]
struct BluetoothSchedulerLockModifyPublication {
    request: BluetoothSchedulerLockModifyRequest,
}

impl BluetoothSchedulerLockModifyPublication {
    /// Consume the permission through the unique HAL task borrow and begin
    /// awaiting the publication result.
    ///
    /// This is a finite, non-polling operation. The returned state contains
    /// the PAC proof that all ordered MMIO writes and the trailing device fence
    /// completed, so a pure state transition can no longer impersonate live
    /// publication.
    fn publish_with(
        self,
        backend: &mut impl BluetoothSchedulerLockModifyBackend,
    ) -> BluetoothSchedulerLockModifyInFlight {
        BluetoothSchedulerLockModifyInFlight {
            _publication: backend.publish(self.request),
            request: self.request,
        }
    }
}

trait BluetoothSchedulerLockModifyBackend {
    fn capture_task(&mut self) -> BluetoothSchedulerLockModifyTaskObservation;
    fn publish(
        &mut self,
        request: BluetoothSchedulerLockModifyRequest,
    ) -> BluetoothSchedulerLockModifyPublished;
}

impl BluetoothSchedulerLockModifyBackend for BluetoothControllerHal<'_> {
    fn capture_task(&mut self) -> BluetoothSchedulerLockModifyTaskObservation {
        self.capture_scheduler_lock_modify_task()
    }

    #[allow(
        unsafe_code,
        reason = "the worker can publish only after unsafe admission binds the selected scheduler item lifetime"
    )]
    fn publish(
        &mut self,
        request: BluetoothSchedulerLockModifyRequest,
    ) -> BluetoothSchedulerLockModifyPublished {
        // SAFETY: entering Awaiting requires the caller of `begin` to retain
        // the exact merge-selected scheduler item and serialize its list until
        // `take_result` returns it to that caller. The powered runtime retains
        // the sole task and interrupt endpoints for the same scheduler epoch.
        unsafe { self.publish_scheduler_lock_modify(request) }
    }
}

/// One published lock/modify request awaiting its publication-result event.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the in-flight scheduler request still owns its publication result"]
struct BluetoothSchedulerLockModifyInFlight {
    _publication: BluetoothSchedulerLockModifyPublished,
    request: BluetoothSchedulerLockModifyRequest,
}

impl BluetoothSchedulerLockModifyInFlight {
    /// Evaluate the post-publication wait predicate once.
    const fn observe(
        self,
        observation: BluetoothSchedulerLockModifyObservation,
    ) -> BluetoothSchedulerLockModifyProgress<Self, BluetoothSchedulerLockModifyPublicationResult>
    {
        if observation.wait_active() {
            BluetoothSchedulerLockModifyProgress::Waiting(self)
        } else {
            BluetoothSchedulerLockModifyProgress::Ready(
                BluetoothSchedulerLockModifyPublicationResult {
                    code: observation.result_code_after_publication(),
                    request: self.request,
                },
            )
        }
    }
}

/// Positional result of the reviewed lock/modify publication path.
///
/// This value only ends the request-publication transaction. Hardware radio
/// completion and descriptor ownership return occur later through the
/// scheduler finished-item and recycle path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerLockModifyPublicationResult {
    code: u8,
    request: BluetoothSchedulerLockModifyRequest,
}

impl BluetoothSchedulerLockModifyPublicationResult {
    /// Return the exact item identity and hardware-list index carried by this
    /// completed publication transaction.
    ///
    /// Retaining the request lets the affine insertion owner recover the exact
    /// merge-selected item/list identity. It does not turn the publication
    /// result into scheduler-item or radio completion.
    pub const fn request(self) -> BluetoothSchedulerLockModifyRequest {
        self.request
    }

    /// Return zero for an idle scheduler or the reviewed request bits 30:27.
    ///
    /// The value remains positional: its higher-level success/error meanings
    /// are not established by the current archive.
    pub const fn code(self) -> u8 {
        self.code
    }
}

/// Why a durable scheduler worker rejected a new transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothSchedulerLockModifyBeginError {
    /// A request is still waiting for publication or its result.
    AlreadyInFlight,
    /// The previous result has not yet been consumed.
    ResultPending,
}

/// Result of one bounded worker event step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the executor must retain the worker outcome"]
pub enum BluetoothSchedulerLockModifyWorkerStep {
    /// No transaction owns this worker.
    Idle,
    /// A completed result is waiting for its consumer; the event was ignored.
    ResultPending,
    /// The current wait predicate remains active; return to the executor.
    Waiting,
    /// This step performed the finite PAC publication and now awaits an event.
    Published,
    /// This step stored the terminal publication result.
    Ready,
}

enum BluetoothSchedulerLockModifyWorkerState {
    Idle,
    Awaiting(BluetoothSchedulerLockModifyAwaitingPublication),
    InFlight(BluetoothSchedulerLockModifyInFlight),
    Complete(BluetoothSchedulerLockModifyPublicationResult),
}

/// Durable single-request worker state owned outside any individual future.
///
/// Dropping a future cannot discard an in-flight hardware request: the worker
/// retains the affine publication proof and continues from the next event.
/// Every [`Self::step`] consumes one value-only ISR event, performs one fresh
/// task observation and returns without polling.
pub struct BluetoothSchedulerLockModifyWorker {
    state: BluetoothSchedulerLockModifyWorkerState,
}

impl BluetoothSchedulerLockModifyWorker {
    /// Construct an idle worker.
    pub const fn new() -> Self {
        Self {
            state: BluetoothSchedulerLockModifyWorkerState::Idle,
        }
    }

    /// Whether this worker owns no admitted or completed request.
    pub const fn is_idle(&self) -> bool {
        matches!(self.state, BluetoothSchedulerLockModifyWorkerState::Idle)
    }

    /// Admit one common-scheduler merge result without touching hardware.
    ///
    /// # Safety
    ///
    /// `request.address()` must identify the exact scheduler item selected by
    /// the insertion merge stage, not merely the item originally submitted for
    /// insertion. The caller must retain that pinned item and exclusive access
    /// to its hardware list until [`Self::take_result`] returns the matching
    /// request. Dropping the logical owner while the worker remains active is
    /// a contract violation even before the request crosses the MMIO boundary.
    #[allow(
        unsafe_code,
        reason = "the merge-selected SRAM item lifetime is not yet representable by this low-level worker"
    )]
    pub unsafe fn begin(
        &mut self,
        request: BluetoothSchedulerLockModifyRequest,
    ) -> Result<(), BluetoothSchedulerLockModifyBeginError> {
        self.begin_inner(request)
    }

    fn begin_inner(
        &mut self,
        request: BluetoothSchedulerLockModifyRequest,
    ) -> Result<(), BluetoothSchedulerLockModifyBeginError> {
        match self.state {
            BluetoothSchedulerLockModifyWorkerState::Idle => {
                self.state = BluetoothSchedulerLockModifyWorkerState::Awaiting(
                    BluetoothSchedulerLockModifyAwaitingPublication::new(request),
                );
                Ok(())
            }
            BluetoothSchedulerLockModifyWorkerState::Complete(_) => {
                Err(BluetoothSchedulerLockModifyBeginError::ResultPending)
            }
            BluetoothSchedulerLockModifyWorkerState::Awaiting(_)
            | BluetoothSchedulerLockModifyWorkerState::InFlight(_) => {
                Err(BluetoothSchedulerLockModifyBeginError::AlreadyInFlight)
            }
        }
    }

    /// Process exactly one interrupt event through the production HAL.
    pub fn step(
        &mut self,
        event: BluetoothSchedulerLockModifyEvent,
        controller: &mut BluetoothControllerHal<'_>,
    ) -> BluetoothSchedulerLockModifyWorkerStep {
        self.step_with(event, controller)
    }

    fn step_with(
        &mut self,
        event: BluetoothSchedulerLockModifyEvent,
        backend: &mut impl BluetoothSchedulerLockModifyBackend,
    ) -> BluetoothSchedulerLockModifyWorkerStep {
        let state = mem::replace(
            &mut self.state,
            BluetoothSchedulerLockModifyWorkerState::Idle,
        );
        match state {
            BluetoothSchedulerLockModifyWorkerState::Idle => {
                BluetoothSchedulerLockModifyWorkerStep::Idle
            }
            BluetoothSchedulerLockModifyWorkerState::Complete(result) => {
                self.state = BluetoothSchedulerLockModifyWorkerState::Complete(result);
                BluetoothSchedulerLockModifyWorkerStep::ResultPending
            }
            BluetoothSchedulerLockModifyWorkerState::Awaiting(awaiting) => {
                let observation = BluetoothSchedulerLockModifyObservation::from_split(
                    event.interrupt,
                    backend.capture_task(),
                );
                match awaiting.observe(observation) {
                    BluetoothSchedulerLockModifyProgress::Waiting(awaiting) => {
                        self.state = BluetoothSchedulerLockModifyWorkerState::Awaiting(awaiting);
                        BluetoothSchedulerLockModifyWorkerStep::Waiting
                    }
                    BluetoothSchedulerLockModifyProgress::Ready(publication) => {
                        self.state = BluetoothSchedulerLockModifyWorkerState::InFlight(
                            publication.publish_with(backend),
                        );
                        BluetoothSchedulerLockModifyWorkerStep::Published
                    }
                }
            }
            BluetoothSchedulerLockModifyWorkerState::InFlight(in_flight) => {
                let observation = BluetoothSchedulerLockModifyObservation::from_split(
                    event.interrupt,
                    backend.capture_task(),
                );
                match in_flight.observe(observation) {
                    BluetoothSchedulerLockModifyProgress::Waiting(in_flight) => {
                        self.state = BluetoothSchedulerLockModifyWorkerState::InFlight(in_flight);
                        BluetoothSchedulerLockModifyWorkerStep::Waiting
                    }
                    BluetoothSchedulerLockModifyProgress::Ready(result) => {
                        self.state = BluetoothSchedulerLockModifyWorkerState::Complete(result);
                        BluetoothSchedulerLockModifyWorkerStep::Ready
                    }
                }
            }
        }
    }

    /// Consume the stored result and make the worker idle again.
    pub fn take_result(&mut self) -> Option<BluetoothSchedulerLockModifyPublicationResult> {
        let state = mem::replace(
            &mut self.state,
            BluetoothSchedulerLockModifyWorkerState::Idle,
        );
        match state {
            BluetoothSchedulerLockModifyWorkerState::Complete(result) => Some(result),
            other => {
                self.state = other;
                None
            }
        }
    }

    /// Whether publication or its result remains owned by the worker.
    pub const fn is_active(&self) -> bool {
        !matches!(self.state, BluetoothSchedulerLockModifyWorkerState::Idle)
    }
}

impl Default for BluetoothSchedulerLockModifyWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
