#![no_std]
#![forbid(unsafe_code)]

//! Embassy mailbox around the executor-neutral ESP32-S31 coexistence core.
//!
//! Exactly one [`CoexOwner`] mutates the hardware and core state. Wi-Fi and
//! later Bluetooth integrations submit typed commands through [`CoexControl`]
//! instead of sharing registers or reproducing the vendor RTOS callback table.

#[cfg(test)]
extern crate std;

use embassy_sync::{
    blocking_mutex::raw::RawMutex,
    channel::{Channel, Receiver, Sender},
};
use open_esp_radio_esp32s31_coex::{
    CoexClientRequest, CoexClockHardware, CoexCore, CoexError, CoexEventId, CoexStatus,
    CoexTimerHardware, CoexTimerIndex,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoexCommand {
    Enable,
    Disable,
    WifiRequest(CoexClientRequest),
    BluetoothRequest(CoexClientRequest),
    Release(CoexEventId),
    Status,
    Shutdown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoexOutcome {
    Status(CoexStatus),
    Request(CoexTimerIndex),
    Release(CoexTimerIndex),
    Stopped,
}

/// Statically allocated command/result channels for one coexistence epoch.
pub struct CoexResources<M: RawMutex, const DEPTH: usize> {
    commands: Channel<M, CoexCommand, DEPTH>,
    outcomes: Channel<M, Result<CoexOutcome, CoexError>, 1>,
}

impl<M: RawMutex, const DEPTH: usize> CoexResources<M, DEPTH> {
    pub const fn new() -> Self {
        Self {
            commands: Channel::new(),
            outcomes: Channel::new(),
        }
    }

    /// Bind the sole request/reply control endpoint to the sole hardware owner.
    ///
    /// The mutable borrow prevents a second split while either endpoint from
    /// this epoch is alive. Ending both endpoints abandons any queued work;
    /// a new epoch starts with empty command and response channels. Hardware
    /// state remains owned by the caller's `CoexCore` across this boundary.
    pub fn split(&mut self) -> (CoexControl<'_, M, DEPTH>, CoexOwner<'_, M, DEPTH>) {
        self.commands.clear();
        self.outcomes.clear();
        (
            CoexControl {
                commands: self.commands.sender(),
                outcomes: self.outcomes.receiver(),
                state: ControlState::Ready,
            },
            CoexOwner {
                commands: self.commands.receiver(),
                outcomes: self.outcomes.sender(),
            },
        )
    }
}

impl<M: RawMutex, const DEPTH: usize> Default for CoexResources<M, DEPTH> {
    fn default() -> Self {
        Self::new()
    }
}

pub struct CoexControl<'resources, M: RawMutex, const DEPTH: usize> {
    commands: Sender<'resources, M, CoexCommand, DEPTH>,
    outcomes: Receiver<'resources, M, Result<CoexOutcome, CoexError>, 1>,
    // Lives outside the borrowed execute future, including cancellation.
    state: ControlState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoexControlError {
    /// Shutdown completed; a new owner/resource epoch is required.
    Stopped,
    Operation(CoexError),
}

#[derive(Clone, Copy)]
enum ControlState {
    Ready,
    Pending,
    Stopped,
}

impl<M: RawMutex, const DEPTH: usize> CoexControl<'_, M, DEPTH> {
    /// Execute one command. Cancellation retains its transaction until the
    /// next call settles it. Successful Shutdown, including a cancelled call,
    /// ends this endpoint epoch and subsequent calls return `Stopped`.
    pub async fn execute(&mut self, command: CoexCommand) -> Result<CoexOutcome, CoexControlError> {
        // Settle the preceding cancelled call before publishing another
        // command. Strictly one outstanding transaction makes its response
        // unambiguous without a second request-id namespace.
        if matches!(self.state, ControlState::Pending) {
            let outcome = self.outcomes.receive().await;
            let _ = self.settle(outcome);
        }
        if matches!(self.state, ControlState::Stopped) {
            return Err(CoexControlError::Stopped);
        }
        self.commands.send(command).await;
        self.state = ControlState::Pending;
        let outcome = self.outcomes.receive().await;
        self.settle(outcome)
    }

    fn settle(
        &mut self,
        outcome: Result<CoexOutcome, CoexError>,
    ) -> Result<CoexOutcome, CoexControlError> {
        self.state = if matches!(outcome, Ok(CoexOutcome::Stopped)) {
            ControlState::Stopped
        } else {
            ControlState::Ready
        };
        outcome.map_err(CoexControlError::Operation)
    }
}

pub struct CoexOwner<'resources, M: RawMutex, const DEPTH: usize> {
    commands: Receiver<'resources, M, CoexCommand, DEPTH>,
    outcomes: Sender<'resources, M, Result<CoexOutcome, CoexError>, 1>,
}

impl<M: RawMutex, const DEPTH: usize> CoexOwner<'_, M, DEPTH> {
    /// Run the only task allowed to mutate coexistence state and MMIO.
    pub async fn run<H: CoexTimerHardware, C: CoexClockHardware>(
        self,
        core: &mut CoexCore,
        hardware: &mut H,
        clock: &mut C,
    ) -> Result<(), CoexError> {
        loop {
            let command = self.commands.receive().await;
            let (result, stop) = match command {
                CoexCommand::Enable => {
                    core.enable();
                    (Ok(CoexOutcome::Status(core.status())), false)
                }
                CoexCommand::Disable => (
                    core.disable(hardware)
                        .map(|()| CoexOutcome::Status(core.status())),
                    false,
                ),
                CoexCommand::WifiRequest(request) => (
                    core.request_wifi(hardware, clock, request)
                        .map(CoexOutcome::Request),
                    false,
                ),
                CoexCommand::BluetoothRequest(request) => (
                    core.request_bluetooth(hardware, clock, request)
                        .map(CoexOutcome::Request),
                    false,
                ),
                CoexCommand::Release(event) => (
                    core.release(hardware, event).map(CoexOutcome::Release),
                    false,
                ),
                CoexCommand::Status => (Ok(CoexOutcome::Status(core.status())), false),
                CoexCommand::Shutdown => match core.disable(hardware) {
                    Ok(()) => (Ok(CoexOutcome::Stopped), true),
                    Err(error) => (Err(error), false),
                },
            };
            self.outcomes.send(result).await;
            if stop {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests;
