//! Cancellation-safe Embassy composition for the first legacy LE DTM event.

#![forbid(unsafe_code)]

use core::future::Future;

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_bluetooth_hci::{HciChannelError, InProcessHciControllerEndpoint};
use open_esp_radio_esp32s31_bluetooth::{
    BluetoothDtmFirstActive, BluetoothDtmFirstResponsePending,
    BluetoothDtmFirstResponsePublication, BluetoothDtmFirstRunner, BluetoothDtmFirstRunnerCancel,
    BluetoothDtmFirstRunnerFailure, BluetoothDtmFirstRunnerStep,
    BluetoothSchedulerRunInterruptStorage,
};

/// Executor disposition after driving every immediately available first-event step.
#[must_use = "retain the wait, response, or exact failed Controller owner"]
pub enum EmbassyBluetoothDtmFirstDrive<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Controller time must be rechecked after yielding to the executor.
    Wait(EmbassyBluetoothDtmFirstControllerTimeWait<'runtime, S, SCHEDULER_CAPACITY>),
    /// Hardware reached `RUN`; its exact start response awaits HCI capacity.
    ResponsePending(EmbassyBluetoothDtmFirstResponseTask<'runtime, S, SCHEDULER_CAPACITY>),
    /// A finite core transition failed while retaining its owner.
    Failed(BluetoothDtmFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>),
}

/// Drive all finite non-waiting transitions to the next executor boundary.
///
/// The loop advances only monotonic `Continue` phases. A hardware-owned time
/// request, terminal failure or first scheduler `RUN` always returns control.
pub fn drive_dtm_first_ready<'runtime, S, const SCHEDULER_CAPACITY: usize>(
    mut runner: BluetoothDtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
) -> EmbassyBluetoothDtmFirstDrive<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    loop {
        match runner.step() {
            BluetoothDtmFirstRunnerStep::WaitControllerTime(runner) => {
                return EmbassyBluetoothDtmFirstDrive::Wait(
                    EmbassyBluetoothDtmFirstControllerTimeWait {
                        runner,
                        recheck_ready: false,
                    },
                );
            }
            BluetoothDtmFirstRunnerStep::Continue(next) => runner = next,
            BluetoothDtmFirstRunnerStep::Running(running) => {
                return EmbassyBluetoothDtmFirstDrive::ResponsePending(
                    EmbassyBluetoothDtmFirstResponseTask {
                        pending: running.into_response_pending(),
                    },
                );
            }
            BluetoothDtmFirstRunnerStep::Failed(failure) => {
                return EmbassyBluetoothDtmFirstDrive::Failed(failure);
            }
        }
    }
}

/// First-event runner parked outside any awaited future.
///
/// Keeping the affine runner in this synchronous object makes cancellation of
/// the caller-provided recheck future harmless.
#[must_use = "wait once, resume, or explicitly cancel the retained runner"]
pub struct EmbassyBluetoothDtmFirstControllerTimeWait<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    runner: BluetoothDtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
    recheck_ready: bool,
}

/// Attempt to resume one exact parked controller-time owner.
#[must_use = "retain an owner whose caller-selected recheck future has not completed"]
pub enum EmbassyBluetoothDtmFirstResume<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// This exact wait observed its recheck future and advanced the runner.
    Ready(EmbassyBluetoothDtmFirstDrive<'runtime, S, SCHEDULER_CAPACITY>),
    /// No recheck opportunity completed for this exact wait object.
    NotReady(EmbassyBluetoothDtmFirstControllerTimeWait<'runtime, S, SCHEDULER_CAPACITY>),
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    EmbassyBluetoothDtmFirstControllerTimeWait<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Await one caller-selected cooperative recheck opportunity.
    ///
    /// Completion proves only that another bounded MMIO observation may run;
    /// it does not claim causal association with an unproven interrupt source.
    pub async fn wait_for_recheck<R>(&mut self, recheck: R)
    where
        R: Future<Output = ()>,
    {
        let _retained_owner = &self.runner;
        recheck.await;
        self.recheck_ready = true;
    }

    /// Consume the parked owner and drive from its next bounded observation.
    pub fn resume(self) -> EmbassyBluetoothDtmFirstResume<'runtime, S, SCHEDULER_CAPACITY> {
        if self.recheck_ready {
            EmbassyBluetoothDtmFirstResume::Ready(drive_dtm_first_ready(self.runner))
        } else {
            EmbassyBluetoothDtmFirstResume::NotReady(self)
        }
    }

    /// Explicitly cancel without relying on dropping an affine hardware request.
    pub fn cancel(self) -> BluetoothDtmFirstRunnerCancel<'runtime, S, SCHEDULER_CAPACITY> {
        self.runner.cancel()
    }
}

/// Running first event retained outside the HCI-capacity wait future.
#[must_use = "wait for capacity and retry the consuming response publication"]
pub struct EmbassyBluetoothDtmFirstResponseTask<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pending: BluetoothDtmFirstResponsePending<'runtime, S, SCHEDULER_CAPACITY>,
}

/// Result of one consuming response-publication attempt.
#[must_use = "retain backpressured ownership or advance the active DTM session"]
pub enum EmbassyBluetoothDtmFirstResponseTaskPublication<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// The exact Command Complete entered its matching HCI epoch.
    Published(BluetoothDtmFirstActive<'runtime, S, SCHEDULER_CAPACITY>),
    /// The queue is still full; the exact response owner remains parked.
    Pending(EmbassyBluetoothDtmFirstResponseTask<'runtime, S, SCHEDULER_CAPACITY>),
    /// The endpoint belongs to another powered Controller epoch.
    EndpointMismatch(EmbassyBluetoothDtmFirstResponseTask<'runtime, S, SCHEDULER_CAPACITY>),
    /// A non-capacity HCI boundary fault retained the unchanged response owner.
    Fault {
        /// Exact running event and response authority.
        task: EmbassyBluetoothDtmFirstResponseTask<'runtime, S, SCHEDULER_CAPACITY>,
        /// Exact validation or transport boundary failure.
        error: HciChannelError,
    },
}

/// Result of waiting for capacity on the supplied HCI endpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "endpoint mismatch must not be treated as matching queue capacity"]
pub enum EmbassyBluetoothDtmFirstResponseWait {
    /// The matching queue reported a non-reserving capacity hint.
    Ready,
    /// The endpoint belongs to another powered Controller epoch.
    EndpointMismatch,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    EmbassyBluetoothDtmFirstResponseTask<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Wait for a cancellation-safe Controller-to-Host capacity hint.
    ///
    /// The affine response remains in `self`, outside this borrowed future.
    /// Capacity is not reserved; callers must still invoke [`Self::try_publish`].
    pub async fn wait_publish_ready<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &InProcessHciControllerEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> EmbassyBluetoothDtmFirstResponseWait {
        let _retained_owner = &self.pending;
        if !self.pending.matches_hci_endpoint(controller) {
            return EmbassyBluetoothDtmFirstResponseWait::EndpointMismatch;
        }
        controller.wait_publish_ready().await;
        EmbassyBluetoothDtmFirstResponseWait::Ready
    }

    /// Attempt exact-once publication through the matching HCI epoch.
    pub fn try_publish<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        self,
        controller: &InProcessHciControllerEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> EmbassyBluetoothDtmFirstResponseTaskPublication<'runtime, S, SCHEDULER_CAPACITY> {
        match self.pending.try_publish(controller) {
            BluetoothDtmFirstResponsePublication::Published(active) => {
                EmbassyBluetoothDtmFirstResponseTaskPublication::Published(active)
            }
            BluetoothDtmFirstResponsePublication::Pending(pending) => {
                EmbassyBluetoothDtmFirstResponseTaskPublication::Pending(Self { pending })
            }
            BluetoothDtmFirstResponsePublication::EndpointMismatch(pending) => {
                EmbassyBluetoothDtmFirstResponseTaskPublication::EndpointMismatch(Self { pending })
            }
            BluetoothDtmFirstResponsePublication::Fault { pending, error } => {
                EmbassyBluetoothDtmFirstResponseTaskPublication::Fault {
                    task: Self { pending },
                    error,
                }
            }
        }
    }
}
