use core::marker::PhantomData;

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_wifi_sta::station::{
    StaAttemptFailure, StaLifecycleExit, StaLifecycleProgress, StaLifecycleService,
    StaNextCandidate, StaReconnectPolicy,
};

use super::{
    Esp32s31StationAttemptRunner, Esp32s31StationCommand, Esp32s31StationCompletion,
    Esp32s31StationControlError, Esp32s31StationControlResources, Esp32s31StationController,
    backend::Esp32s31StationLifecycleBackend,
};

/// Stable application policy for one station service.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31StationConfig {
    reconnect: StaReconnectPolicy,
    initial_candidate: StaNextCandidate,
}

impl Esp32s31StationConfig {
    /// A normal application starts without a selected candidate and scans.
    pub const fn new(reconnect: StaReconnectPolicy) -> Self {
        Self {
            reconnect,
            initial_candidate: StaNextCandidate::Refresh,
        }
    }

    /// Start from a caller-proven candidate, for example after a cold scan.
    pub const fn with_initial_candidate(mut self, candidate: StaNextCandidate) -> Self {
        self.initial_candidate = candidate;
        self
    }

    pub const fn reconnect_policy(self) -> StaReconnectPolicy {
        self.reconnect
    }

    pub const fn initial_candidate(self) -> StaNextCandidate {
        self.initial_candidate
    }
}

/// Exact role owner consumed when one station task is materialized.
pub struct Esp32s31StationStartResources<O> {
    owner: O,
}

impl<O> Esp32s31StationStartResources<O> {
    pub const fn new(owner: O) -> Self {
        Self { owner }
    }

    pub fn into_owner(self) -> O {
        self.owner
    }
}

/// Complete ownership frontier returned by a finite station task.
///
/// `owner` contains the exact phase/DMA/network resources returned by the
/// attempt. `runner` contains the platform integration resources which lived
/// beside that owner for the whole service, including an interrupt epoch when
/// the concrete composition owns one. Both are required before the physical
/// Wi-Fi owner can be reconstructed.
pub struct Esp32s31StationReturnedResources<O, R> {
    owner: O,
    runner: R,
}

impl<O, R> Esp32s31StationReturnedResources<O, R> {
    const fn new(owner: O, runner: R) -> Self {
        Self { owner, runner }
    }

    pub fn into_parts(self) -> (O, R) {
        (self.owner, self.runner)
    }
}

/// Why a station runner returned through its normal stopped edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StationStopReason {
    Requested(Esp32s31StationCommand),
    /// The backend stopped for a platform-specific reason without consuming a
    /// public controller command.
    Backend,
}

/// Complete application-visible result with the exact lifecycle owner.
pub enum Esp32s31StationExit<O, R, E> {
    Stopped {
        resources: Esp32s31StationReturnedResources<O, R>,
        progress: StaLifecycleProgress,
        reason: Esp32s31StationStopReason,
    },
    RetryExhausted {
        resources: Esp32s31StationReturnedResources<O, R>,
        progress: StaLifecycleProgress,
        failure: StaAttemptFailure<E>,
    },
    Terminal {
        resources: Esp32s31StationReturnedResources<O, R>,
        progress: StaLifecycleProgress,
        failure: StaAttemptFailure<E>,
    },
}

/// Materialize one task-owned station lifecycle and its hardware-free
/// application controller in a single transaction.
pub fn prepare_esp32s31_station_task<'control, M, R>(
    config: Esp32s31StationConfig,
    resources: Esp32s31StationStartResources<R::Owner>,
    control: &'control mut Esp32s31StationControlResources<M>,
    runner: R,
) -> Result<
    (
        Esp32s31StationController<'control, M>,
        Esp32s31StationTask<'control, M, R>,
    ),
    Esp32s31StationControlError,
>
where
    M: RawMutex,
    R: Esp32s31StationAttemptRunner<M>,
{
    let (controller, receiver) = control.split()?;
    let backend = Esp32s31StationLifecycleBackend::new(receiver, runner);
    let task = Esp32s31StationTask {
        lifecycle: Some(StaLifecycleService::new(backend, config.reconnect)),
        resources: Some(resources),
        initial_candidate: config.initial_candidate,
        control: controller.resources(),
        completion_pending: true,
        _mutex: PhantomData,
    };
    Ok((controller, task))
}

/// Owned station service. Running it consumes the initial owner and always
/// returns both the lifecycle's exact final owner and its platform runner in
/// [`Esp32s31StationExit`].
///
/// Dropping this task or its in-flight [`run`](Self::run) future is not a
/// shutdown operation. It publishes [`Esp32s31StationCompletion::Faulted`]
/// and permanently prevents another task from splitting the same control
/// resources. Live lower-level owners independently retain or poison their
/// DMA/IRQ resources, so cancellation cannot be mistaken for quiescence.
pub struct Esp32s31StationTask<'control, M: RawMutex, R: Esp32s31StationAttemptRunner<M>> {
    lifecycle: Option<StaLifecycleService<Esp32s31StationLifecycleBackend<'control, M, R>>>,
    resources: Option<Esp32s31StationStartResources<R::Owner>>,
    initial_candidate: StaNextCandidate,
    control: &'control Esp32s31StationControlResources<M>,
    completion_pending: bool,
    _mutex: PhantomData<M>,
}

impl<M, R> Esp32s31StationTask<'_, M, R>
where
    M: RawMutex,
    R: Esp32s31StationAttemptRunner<M>,
{
    pub async fn run(mut self) -> Esp32s31StationExit<R::Owner, R, R::Error> {
        let owner = self
            .resources
            .take()
            .expect("station task starts with exactly one owner")
            .into_owner();
        let mut lifecycle = self
            .lifecycle
            .take()
            .expect("station task starts with exactly one lifecycle");
        let outcome = lifecycle
            .run_with_candidate(owner, self.initial_candidate)
            .await;
        let (receiver, runner) = lifecycle.into_backend().into_parts();
        let (exit, completion) = match outcome {
            StaLifecycleExit::Stopped { owner, progress } => (
                Esp32s31StationExit::Stopped {
                    resources: Esp32s31StationReturnedResources::new(owner, runner),
                    progress,
                    reason: self
                        .control
                        .take_terminal()
                        .map_or(Esp32s31StationStopReason::Backend, |command| {
                            Esp32s31StationStopReason::Requested(command)
                        }),
                },
                Esp32s31StationCompletion::Stopped,
            ),
            StaLifecycleExit::Exhausted {
                owner,
                progress,
                failure,
            } => (
                Esp32s31StationExit::RetryExhausted {
                    resources: Esp32s31StationReturnedResources::new(owner, runner),
                    progress,
                    failure,
                },
                Esp32s31StationCompletion::Ended,
            ),
            StaLifecycleExit::Terminal {
                owner,
                progress,
                failure,
            } => (
                Esp32s31StationExit::Terminal {
                    resources: Esp32s31StationReturnedResources::new(owner, runner),
                    progress,
                    failure,
                },
                Esp32s31StationCompletion::Ended,
            ),
        };
        self.control.publish_completion(completion);
        self.completion_pending = false;
        // The receiver's fail-closed Drop observes the completion published
        // above. Keep it alive until this edge so extracting the runner cannot
        // transiently classify a clean stop as cancellation.
        drop(receiver);
        exit
    }
}

impl<M, R> Drop for Esp32s31StationTask<'_, M, R>
where
    M: RawMutex,
    R: Esp32s31StationAttemptRunner<M>,
{
    fn drop(&mut self) {
        if self.completion_pending {
            // Active phase owners have their own fail-closed Drop contracts.
            // This acknowledgement never claims that IRQ or DMA stopped; it
            // only prevents the application from waiting forever and makes
            // the required reset explicit.
            self.control
                .publish_completion(Esp32s31StationCompletion::Faulted);
        }
    }
}
