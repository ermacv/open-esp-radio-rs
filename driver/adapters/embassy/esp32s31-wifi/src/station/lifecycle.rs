use core::marker::PhantomData;

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_wifi_sta::station::{
    StaAttemptFailure, StaLifecycleBackend, StaLifecycleExit, StaLifecycleProgress,
    StaLifecycleService, StaNextCandidate, StaReconnectPolicy,
};

use super::{
    Esp32s31StationCommand, Esp32s31StationCommandReceiver, Esp32s31StationControlResources,
    Esp32s31StationController,
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

/// Exact initial owner consumed by a station runner.
pub struct Esp32s31StationResources<O> {
    owner: O,
}

impl<O> Esp32s31StationResources<O> {
    pub const fn new(owner: O) -> Self {
        Self { owner }
    }

    pub fn into_owner(self) -> O {
        self.owner
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
pub enum Esp32s31StationExit<O, E> {
    Stopped {
        resources: Esp32s31StationResources<O>,
        progress: StaLifecycleProgress,
        reason: Esp32s31StationStopReason,
    },
    RetryExhausted {
        resources: Esp32s31StationResources<O>,
        progress: StaLifecycleProgress,
        failure: StaAttemptFailure<E>,
    },
    Terminal {
        resources: Esp32s31StationResources<O>,
        progress: StaLifecycleProgress,
        failure: StaAttemptFailure<E>,
    },
}

/// Namespace used to construct a controller/runner pair without exposing the
/// chip-independent lifecycle service to application code.
pub struct Esp32s31Station;

impl Esp32s31Station {
    pub fn new<'control, M, O, B, F>(
        config: Esp32s31StationConfig,
        resources: Esp32s31StationResources<O>,
        control: &'control mut Esp32s31StationControlResources<M>,
        backend: F,
    ) -> (
        Esp32s31StationController<'control, M>,
        Esp32s31StationRunner<'control, M, B>,
    )
    where
        M: RawMutex,
        B: StaLifecycleBackend<Owner = O>,
        F: FnOnce(Esp32s31StationCommandReceiver<'control, M>) -> B,
    {
        let (controller, receiver) = control.split();
        let runner = Esp32s31StationRunner {
            lifecycle: StaLifecycleService::new(backend(receiver), config.reconnect),
            resources,
            initial_candidate: config.initial_candidate,
            control: controller.resources(),
            _mutex: PhantomData,
        };
        (controller, runner)
    }
}

/// Owned station service. Running it consumes the initial owner and always
/// returns the lifecycle's exact final owner in [`Esp32s31StationExit`].
pub struct Esp32s31StationRunner<'control, M: RawMutex, B: StaLifecycleBackend> {
    lifecycle: StaLifecycleService<B>,
    resources: Esp32s31StationResources<B::Owner>,
    initial_candidate: StaNextCandidate,
    control: &'control Esp32s31StationControlResources<M>,
    _mutex: PhantomData<M>,
}

impl<M, B> Esp32s31StationRunner<'_, M, B>
where
    M: RawMutex,
    B: StaLifecycleBackend,
{
    pub async fn run(mut self) -> Esp32s31StationExit<B::Owner, B::Error> {
        let owner = self.resources.into_owner();
        match self
            .lifecycle
            .run_with_candidate(owner, self.initial_candidate)
            .await
        {
            StaLifecycleExit::Stopped { owner, progress } => Esp32s31StationExit::Stopped {
                resources: Esp32s31StationResources::new(owner),
                progress,
                reason: self
                    .control
                    .take_terminal()
                    .map_or(Esp32s31StationStopReason::Backend, |command| {
                        Esp32s31StationStopReason::Requested(command)
                    }),
            },
            StaLifecycleExit::Exhausted {
                owner,
                progress,
                failure,
            } => Esp32s31StationExit::RetryExhausted {
                resources: Esp32s31StationResources::new(owner),
                progress,
                failure,
            },
            StaLifecycleExit::Terminal {
                owner,
                progress,
                failure,
            } => Esp32s31StationExit::Terminal {
                resources: Esp32s31StationResources::new(owner),
                progress,
                failure,
            },
        }
    }
}
