use core::sync::atomic::{AtomicU8, Ordering};

use embassy_sync::{blocking_mutex::raw::RawMutex, signal::Signal};

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

    pub(super) fn take_terminal(&self) -> Option<Esp32s31StationCommand> {
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

impl<'resources, M: RawMutex> Esp32s31StationController<'resources, M> {
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

    pub(super) const fn resources(&self) -> &'resources Esp32s31StationControlResources<M> {
        self.resources
    }
}

/// Single-consumer command side supplied to the concrete lifecycle backend.
///
/// A connected backend must observe this future only at a cancellation-safe
/// edge such as [`crate::connected_runner::ConnectedRunner::run_until`]. It records a
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

    #[cfg(test)]
    pub(super) fn take_terminal(&mut self) -> Option<Esp32s31StationCommand> {
        self.resources.take_terminal()
    }
}
