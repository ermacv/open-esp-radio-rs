//! Application-facing ESP32-S31 station lifecycle facade.
//!
//! The lower integration modules expose precise phase owners. Applications
//! should not have to drive the lower `Esp32s31StaAttempt` transaction or the
//! chip-independent reconnect loop directly, so this module gives that owner
//! graph one configuration, one initial resource bundle, a cooperative
//! controller and a runner with typed terminal outcomes.

use core::{
    marker::PhantomData,
    sync::atomic::{AtomicU8, Ordering},
};

use embassy_sync::{blocking_mutex::raw::RawMutex, signal::Signal};
use open_esp_radio_embassy_net::RawMutex as NetworkRawMutex;
use open_esp_radio_wifi_sta::station::{
    StaAttemptFailure, StaLifecycleBackend, StaLifecycleExit, StaLifecycleProgress,
    StaLifecycleService, StaNextCandidate, StaReconnectPolicy,
};

use crate::runner::{WifiRunner, WifiRunnerBackend, WifiRunnerExit};

pub use crate::station_tasks::{
    Esp32s31ConnectedTaskGroup, Esp32s31ConnectedTaskStopOutcome,
    stop_esp32s31_connected_task_group,
};

const NO_COMMAND: u8 = 0;
const RECONNECT_COMMAND: u8 = 1;
const DISCONNECT_COMMAND: u8 = 2;
const STOP_COMMAND: u8 = 3;

/// Cooperative command accepted by a running station service.
///
/// Commands are ordered by severity. A pending `Stop` cannot be replaced by
/// a later reconnect request, while `Stop` may upgrade either weaker command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Esp32s31StationCommand {
    /// End the current connected epoch and let the lifecycle rescan/reconnect.
    Reconnect = RECONNECT_COMMAND,
    /// Leave the controlled port and return the station owner to the caller.
    Disconnect = DISCONNECT_COMMAND,
    /// Stop the complete station service and return all recoverable owners.
    Stop = STOP_COMMAND,
}

impl Esp32s31StationCommand {
    const fn decode(value: u8) -> Option<Self> {
        match value {
            RECONNECT_COMMAND => Some(Self::Reconnect),
            DISCONNECT_COMMAND => Some(Self::Disconnect),
            STOP_COMMAND => Some(Self::Stop),
            _ => None,
        }
    }
}

/// Static command storage shared by one controller and one station runner.
///
/// This is deliberately separate from DMA and protocol resources. It can be
/// placed in ordinary static storage and reused after the previous runner has
/// returned its owner.
pub struct Esp32s31StationControlResources<M: RawMutex> {
    pending: AtomicU8,
    terminal: AtomicU8,
    wake: Signal<M, ()>,
}

impl<M: RawMutex> Esp32s31StationControlResources<M> {
    pub const fn new() -> Self {
        Self {
            pending: AtomicU8::new(NO_COMMAND),
            terminal: AtomicU8::new(NO_COMMAND),
            wake: Signal::new(),
        }
    }

    /// Start one station session and create its single command consumer.
    ///
    /// The mutable borrow prevents two runners from being constructed from the
    /// same command slot. The returned controller may be copied freely.
    pub fn split(
        &mut self,
    ) -> (
        Esp32s31StationController<'_, M>,
        Esp32s31StationCommandReceiver<'_, M>,
    ) {
        self.pending.store(NO_COMMAND, Ordering::Release);
        self.terminal.store(NO_COMMAND, Ordering::Release);
        self.wake.reset();
        let shared = &*self;
        (
            Esp32s31StationController { resources: shared },
            Esp32s31StationCommandReceiver { resources: shared },
        )
    }

    fn request(&self, command: Esp32s31StationCommand) -> bool {
        let requested = command as u8;
        let mut observed = self.pending.load(Ordering::Acquire);
        loop {
            if observed >= requested {
                return false;
            }
            match self.pending.compare_exchange_weak(
                observed,
                requested,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.wake.signal(());
                    return true;
                }
                Err(actual) => observed = actual,
            }
        }
    }

    fn take_pending(&self) -> Option<Esp32s31StationCommand> {
        Esp32s31StationCommand::decode(self.pending.swap(NO_COMMAND, Ordering::AcqRel))
    }

    fn record_terminal(&self, command: Esp32s31StationCommand) {
        self.terminal.store(command as u8, Ordering::Release);
    }

    fn take_terminal(&self) -> Option<Esp32s31StationCommand> {
        Esp32s31StationCommand::decode(self.terminal.swap(NO_COMMAND, Ordering::AcqRel))
    }
}

impl<M: RawMutex> Default for Esp32s31StationControlResources<M> {
    fn default() -> Self {
        Self::new()
    }
}

/// Copyable application handle for cooperative station control.
pub struct Esp32s31StationController<'resources, M: RawMutex> {
    resources: &'resources Esp32s31StationControlResources<M>,
}

impl<M: RawMutex> Clone for Esp32s31StationController<'_, M> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex> Copy for Esp32s31StationController<'_, M> {}

impl<M: RawMutex> Esp32s31StationController<'_, M> {
    /// Request a fresh scan and association without destroying the service.
    pub fn request_reconnect(&self) -> bool {
        self.resources.request(Esp32s31StationCommand::Reconnect)
    }

    /// Request a finite disconnect and return from the station runner.
    pub fn request_disconnect(&self) -> bool {
        self.resources.request(Esp32s31StationCommand::Disconnect)
    }

    /// Request complete finite station shutdown.
    pub fn request_stop(&self) -> bool {
        self.resources.request(Esp32s31StationCommand::Stop)
    }
}

/// Single-consumer command side supplied to the concrete lifecycle backend.
///
/// A connected backend must observe this future only at a cancellation-safe
/// edge such as [`crate::runner::WifiRunner::run_until`]. It records a
/// terminal command only when it returns `StaAttemptOutcome::Stopped`;
/// `Reconnect` normally becomes a disconnected epoch instead.
pub struct Esp32s31StationCommandReceiver<'resources, M: RawMutex> {
    resources: &'resources Esp32s31StationControlResources<M>,
}

impl<M: RawMutex> Esp32s31StationCommandReceiver<'_, M> {
    pub fn try_take(&mut self) -> Option<Esp32s31StationCommand> {
        self.resources.take_pending()
    }

    pub async fn wait(&mut self) -> Esp32s31StationCommand {
        loop {
            if let Some(command) = self.try_take() {
                return command;
            }
            self.resources.wake.wait().await;
        }
    }

    /// Return a command which arrived before the backend reached a safe edge.
    ///
    /// This is primarily useful for `Reconnect`: an early request must remain
    /// pending until the connected runner can end its epoch safely. A stronger
    /// command queued concurrently retains priority.
    pub fn defer(&mut self, command: Esp32s31StationCommand) -> bool {
        self.resources.request(command)
    }

    /// Preserve the reason for a terminal `Stopped` lifecycle edge.
    pub fn record_terminal(&mut self, command: Esp32s31StationCommand) {
        self.resources.record_terminal(command);
    }
}

/// How a reconnect command reached the connected runner's safe stop edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StationReconnectSource {
    /// The command future won while the runner was still connected.
    Controller,
    /// Peer loss won the same scheduling edge; the pending reconnect command
    /// was consumed after the runner had already returned link-down owners.
    CoalescedDisconnect,
}

/// Production result of one finite connected station epoch.
///
/// This keeps application control semantics out of HIL and prevents a queued
/// terminal command from leaking into a replacement epoch when peer loss wins
/// the same scheduler turn.
pub enum Esp32s31ConnectedStationExit<E> {
    Disconnected,
    ReconnectRequested {
        source: Esp32s31StationReconnectSource,
    },
    StationStopped(Esp32s31StationCommand),
    HardwareFailure(E),
}

fn coalesce_disconnected_station_command<E, M: RawMutex>(
    control: &mut Esp32s31StationCommandReceiver<'_, M>,
) -> Esp32s31ConnectedStationExit<E> {
    match control.try_take() {
        Some(Esp32s31StationCommand::Reconnect) => {
            Esp32s31ConnectedStationExit::ReconnectRequested {
                source: Esp32s31StationReconnectSource::CoalescedDisconnect,
            }
        }
        Some(command @ (Esp32s31StationCommand::Disconnect | Esp32s31StationCommand::Stop)) => {
            control.record_terminal(command);
            Esp32s31ConnectedStationExit::StationStopped(command)
        }
        None => Esp32s31ConnectedStationExit::Disconnected,
    }
}

fn complete_connected_station_command<E, M: RawMutex>(
    command: Esp32s31StationCommand,
    control: &mut Esp32s31StationCommandReceiver<'_, M>,
) -> Esp32s31ConnectedStationExit<E> {
    match command {
        Esp32s31StationCommand::Reconnect => Esp32s31ConnectedStationExit::ReconnectRequested {
            source: Esp32s31StationReconnectSource::Controller,
        },
        Esp32s31StationCommand::Disconnect | Esp32s31StationCommand::Stop => {
            control.record_terminal(command);
            Esp32s31ConnectedStationExit::StationStopped(command)
        }
    }
}

/// Run one connected hardware owner until peer loss or a station command.
///
/// `WifiRunner` observes the stop future only at a transaction-safe boundary.
/// A simultaneous peer disconnect is then coalesced with any still-pending
/// application command before ownership is handed back to the outer lifecycle.
pub async fn run_esp32s31_connected_station_epoch<
    'resources,
    'irq,
    RM,
    CM,
    B,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
>(
    runner: &mut WifiRunner<
        'resources,
        'irq,
        RM,
        B,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
    >,
    control: &mut Esp32s31StationCommandReceiver<'_, CM>,
) -> Esp32s31ConnectedStationExit<B::Error>
where
    RM: NetworkRawMutex,
    CM: RawMutex,
    B: WifiRunnerBackend<'resources, RM, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
{
    let requested_command = core::cell::Cell::new(None);
    let station_stop = async {
        requested_command.set(Some(control.wait().await));
    };
    match runner.run_until(station_stop).await {
        Ok(WifiRunnerExit::Disconnected) => coalesce_disconnected_station_command(control),
        Ok(WifiRunnerExit::Stopped) => {
            let command = requested_command
                .get()
                .expect("a stopped station runner consumed one controller command");
            complete_connected_station_command(command, control)
        }
        Err(error) => Esp32s31ConnectedStationExit::HardwareFailure(error),
    }
}

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
            control: controller.resources,
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

#[cfg(test)]
mod tests {
    use core::future::{Future, ready};

    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use open_esp_radio_wifi_sta::station::{
        StaAttemptContext, StaAttemptOutcome, StaBackoffOutcome, StaBackoffReason,
        StaFailureDisposition, StaLifecycleStage,
    };

    use super::*;

    struct Backend<'control> {
        control: Esp32s31StationCommandReceiver<'control, NoopRawMutex>,
        fail: bool,
    }

    impl StaLifecycleBackend for Backend<'_> {
        type Owner = u32;
        type Error = u8;

        fn run_attempt(
            &mut self,
            owner: Self::Owner,
            _context: StaAttemptContext,
        ) -> impl Future<Output = StaAttemptOutcome<Self::Owner, Self::Error>> + '_ {
            let outcome = if let Some(command) = self.control.try_take() {
                self.control.record_terminal(command);
                StaAttemptOutcome::Stopped { owner }
            } else if self.fail {
                StaAttemptOutcome::Failed {
                    owner,
                    failure: StaAttemptFailure::new(
                        StaLifecycleStage::Authentication,
                        StaFailureDisposition::RetryCurrentCandidate,
                        9,
                    ),
                }
            } else {
                StaAttemptOutcome::Stopped { owner }
            };
            ready(outcome)
        }

        fn wait_backoff(
            &mut self,
            owner: Self::Owner,
            _delay_millis: u32,
            _reason: StaBackoffReason,
        ) -> impl Future<Output = StaBackoffOutcome<Self::Owner>> + '_ {
            ready(StaBackoffOutcome::Elapsed { owner })
        }
    }

    fn policy(attempt_limit: u16) -> StaReconnectPolicy {
        StaReconnectPolicy::new(attempt_limit, 1, 1, 1).unwrap()
    }

    #[test]
    fn command_priority_never_downgrades_a_pending_stop() {
        let mut resources = Esp32s31StationControlResources::<NoopRawMutex>::new();
        let (controller, mut receiver) = resources.split();
        assert!(controller.request_reconnect());
        assert!(controller.request_disconnect());
        assert!(!controller.request_reconnect());
        assert!(controller.request_stop());
        assert!(!controller.request_disconnect());
        assert_eq!(block_on(receiver.wait()), Esp32s31StationCommand::Stop);
    }

    #[test]
    fn peer_disconnect_coalesces_a_pending_reconnect_without_leaking_it() {
        let mut resources = Esp32s31StationControlResources::<NoopRawMutex>::new();
        let (controller, mut receiver) = resources.split();
        assert!(controller.request_reconnect());
        assert!(matches!(
            coalesce_disconnected_station_command::<(), _>(&mut receiver),
            Esp32s31ConnectedStationExit::ReconnectRequested {
                source: Esp32s31StationReconnectSource::CoalescedDisconnect,
            }
        ));
        assert_eq!(receiver.try_take(), None);
    }

    #[test]
    fn terminal_connected_command_records_the_public_stop_reason() {
        let mut resources = Esp32s31StationControlResources::<NoopRawMutex>::new();
        let (_controller, mut receiver) = resources.split();
        assert!(matches!(
            complete_connected_station_command::<(), _>(
                Esp32s31StationCommand::Stop,
                &mut receiver,
            ),
            Esp32s31ConnectedStationExit::StationStopped(Esp32s31StationCommand::Stop)
        ));
        assert_eq!(
            receiver.resources.take_terminal(),
            Some(Esp32s31StationCommand::Stop)
        );
    }

    #[test]
    fn controller_stop_returns_the_exact_owner_and_reason() {
        let mut control = Esp32s31StationControlResources::<NoopRawMutex>::new();
        let (controller, runner) = Esp32s31Station::new(
            Esp32s31StationConfig::new(policy(2)),
            Esp32s31StationResources::new(41),
            &mut control,
            |control| Backend {
                control,
                fail: false,
            },
        );
        assert!(controller.request_stop());
        let Esp32s31StationExit::Stopped {
            resources,
            progress,
            reason,
        } = block_on(runner.run())
        else {
            panic!("station did not stop");
        };
        assert_eq!(resources.into_owner(), 41);
        assert_eq!(progress.attempts_started, 1);
        assert_eq!(
            reason,
            Esp32s31StationStopReason::Requested(Esp32s31StationCommand::Stop)
        );
    }

    #[test]
    fn retry_exhaustion_preserves_failure_and_owner() {
        let mut control = Esp32s31StationControlResources::<NoopRawMutex>::new();
        let (_controller, runner) = Esp32s31Station::new(
            Esp32s31StationConfig::new(policy(1)),
            Esp32s31StationResources::new(77),
            &mut control,
            |control| Backend {
                control,
                fail: true,
            },
        );
        let Esp32s31StationExit::RetryExhausted {
            resources,
            progress,
            failure,
        } = block_on(runner.run())
        else {
            panic!("station did not exhaust its bounded retry policy");
        };
        assert_eq!(resources.into_owner(), 77);
        assert_eq!(progress.attempts_started, 1);
        assert_eq!(failure.stage, StaLifecycleStage::Authentication);
        assert_eq!(failure.error, 9);
    }
}
