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
    CoexClockHardware, CoexCore, CoexError, CoexEventId, CoexReleaseOutcome, CoexRequest,
    CoexRequestOutcome, CoexStatus, CoexTimerHardware,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoexCommand {
    Enable,
    Disable,
    Request(CoexRequest),
    Release(CoexEventId),
    Status,
    Shutdown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoexOutcome {
    Status(CoexStatus),
    Request(CoexRequestOutcome),
    Release(CoexReleaseOutcome),
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
    /// this epoch is alive. `CoexControl::execute` also keeps at most one
    /// request in flight, so a result can never be consumed by another caller.
    pub fn split(&mut self) -> (CoexControl<'_, M, DEPTH>, CoexOwner<'_, M, DEPTH>) {
        (
            CoexControl {
                commands: self.commands.sender(),
                outcomes: self.outcomes.receiver(),
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
}

impl<M: RawMutex, const DEPTH: usize> CoexControl<'_, M, DEPTH> {
    pub async fn execute(&mut self, command: CoexCommand) -> Result<CoexOutcome, CoexError> {
        self.commands.send(command).await;
        self.outcomes.receive().await
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
                CoexCommand::Request(request) => (
                    core.request(hardware, clock, request)
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
mod tests {
    use embassy_futures::{block_on, join::join};
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use open_esp_radio_esp32s31_coex::{
        CoexClient, CoexClockSelector, CoexPti, CoexPtiTable, CoexTimerClock, CoexTimerIndex,
    };

    use super::*;

    #[derive(Default)]
    struct Hardware {
        enabled: u8,
        disabled: u8,
    }

    impl CoexTimerHardware for Hardware {
        fn configure_request(
            &mut self,
            _index: CoexTimerIndex,
            _client: CoexClient,
            _pti: CoexPti,
        ) -> Result<(), CoexError> {
            Ok(())
        }

        fn set_primary_target(
            &mut self,
            _index: CoexTimerIndex,
            _tick_image: u32,
        ) -> Result<(), CoexError> {
            Ok(())
        }

        fn set_secondary_target(
            &mut self,
            _index: CoexTimerIndex,
            _tick_image: u32,
        ) -> Result<(), CoexError> {
            Ok(())
        }

        fn enable(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
            self.enabled |= 1 << index.value();
            Ok(())
        }

        fn disable(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
            self.disabled |= 1 << index.value();
            self.enabled &= !(1 << index.value());
            Ok(())
        }

        fn force(&mut self, _index: CoexTimerIndex) -> Result<(), CoexError> {
            Ok(())
        }

        fn unforce(&mut self, _index: CoexTimerIndex) -> Result<(), CoexError> {
            Ok(())
        }
    }

    struct Clock(CoexTimerClock);

    impl CoexClockHardware for Clock {
        fn configure(
            &mut self,
            selector: CoexClockSelector,
            divisor: u16,
        ) -> Result<(), CoexError> {
            if !selector.accepts_divisor(divisor) {
                return Err(CoexError::UnsupportedClock);
            }
            self.0.selector = selector;
            self.0.divider_field = divisor - 1;
            Ok(())
        }

        fn sample(&mut self) -> Result<CoexTimerClock, CoexError> {
            Ok(self.0)
        }
    }

    #[test]
    fn single_owner_serializes_request_release_and_shutdown() {
        let mut resources = CoexResources::<NoopRawMutex, 2>::new();
        let (mut control, owner) = resources.split();
        let mut core = CoexCore::new(CoexPtiTable::reviewed_vendor());
        let mut hardware = Hardware::default();
        let mut clock = Clock(CoexTimerClock {
            selector: CoexClockSelector::Selector8,
            divider_field: 0,
            xtal_mhz: 40,
            real_chip: true,
        });
        let request = CoexRequest {
            client: CoexClient::Wifi,
            event: CoexEventId::new(1).unwrap(),
            latency: 1_000,
            duration: 2_000,
        };

        block_on(join(
            async {
                assert_eq!(
                    control.execute(CoexCommand::Enable).await,
                    Ok(CoexOutcome::Status(CoexStatus {
                        enabled: true,
                        active_timers: 0,
                    }))
                );
                assert_eq!(
                    control.execute(CoexCommand::Request(request)).await,
                    Ok(CoexOutcome::Request(CoexRequestOutcome::Armed(
                        CoexTimerIndex::new(0).unwrap()
                    )))
                );
                assert_eq!(
                    control.execute(CoexCommand::Release(request.event)).await,
                    Ok(CoexOutcome::Release(CoexReleaseOutcome::Released(
                        CoexTimerIndex::new(0).unwrap()
                    )))
                );
                assert_eq!(
                    control.execute(CoexCommand::Shutdown).await,
                    Ok(CoexOutcome::Stopped)
                );
            },
            owner.run(&mut core, &mut hardware, &mut clock),
        ))
        .1
        .unwrap();

        assert_eq!(hardware.enabled, 0);
        assert_eq!(hardware.disabled, 1);
    }
}
