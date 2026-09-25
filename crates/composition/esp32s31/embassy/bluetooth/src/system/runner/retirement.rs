//! Reversible timer extraction and its join with idle HCI retirement.

use super::*;
use crate::{BluetoothInterruptBindError, BluetoothInterruptDisabled};
use oer_esp32s31_bluetooth_runtime::controller::{
    ControllerCommandPhase, ControllerCommandRetirementError,
};
use oer_esp32s31_radio_esp_hal::{
    EspHalBluetoothInterruptRouteError, EspHalBluetoothModemLpTimerRetirementError,
    EspHalBluetoothModemLpTimerStorageError,
};
use {
    oer_esp32s31_bluetooth::{
        modem_timer::ControllerModemTimerRetired,
        modem_timer_retirement::ControllerModemTimerRetirementError,
    },
    oer_esp32s31_bluetooth_controller::controller::ControllerTaskHciRetired,
};

type RetiredTimer<const MT: usize> = ControllerModemTimerRetired<'static, PublishedStorage, MT>;

/// Exact rejection while moving or restoring the disjoint timer owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothHardwareTimerError {
    /// Radio or command work must return the actor to idle first.
    NotIdle(ControllerCommandPhase),
    /// An ISR fault prevents normal ownership retirement.
    InterruptFault(BluetoothInterruptFault),
    /// The complete route epoch could not be disabled on this core.
    Disable(EspHalBluetoothInterruptRouteError),
    /// Timer work or stable storage still owns an outstanding obligation.
    Retire(ControllerModemTimerRetirementError<EspHalBluetoothModemLpTimerRetirementError>),
    /// The exact ready owner could not be restored to its slot.
    Restore(EspHalBluetoothModemLpTimerStorageError),
    /// Restored owners could not reactivate the complete IRQ service.
    Bind(BluetoothInterruptBindError),
}

enum TimerTransition<const MT: usize> {
    Unchanged,
    Disabled(BluetoothInterruptDisabled),
    Retired {
        interrupt: BluetoothInterruptDisabled,
        timer: RetiredTimer<MT>,
    },
}

/// Complete failed transition, retryable through `resume` without losing owners.
#[must_use = "resume or retain the exact hardware transition"]
pub struct BluetoothHardwareTimerFailure<
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> {
    error: BluetoothHardwareTimerError,
    runner: BluetoothHardwareRunner<MT, SC, H2C, C2H, PC>,
    transition: TimerTransition<MT>,
}

/// Idle hardware runner with its timer outside ISR storage and all IRQs disabled.
///
/// HCI stays open until `try_retire_hci` succeeds. `resume` restores the exact
/// timer before reactivation. The started counter and powered radio owners
/// remain retained; neither operation authorizes PHY release.
#[must_use = "resume the same epoch or join it to HCI retirement"]
pub struct BluetoothHardwareTimerRetired<
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> {
    runner: BluetoothHardwareRunner<MT, SC, H2C, C2H, PC>,
    interrupt: BluetoothInterruptDisabled,
    timer: RetiredTimer<MT>,
}

/// Joined idle command/HCI, drained timer and inactive IRQ ownership.
///
/// This retains the software shutdown boundary, actual task HAL, registered
/// PHY client, BLE PHY/DF and all role-memory owners removed from their static slots.
/// Their extraction does not release PHY or revoke hardware pointers. The hardware counter,
/// BTBB and PHY are still powered; final Controller storage remains statically
/// borrowed. Cold-owner release requires the subsequent checked output and platform joins.
#[must_use = "retain all owners through the physical shutdown transitions"]
pub struct BluetoothHardwareRetired<
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> {
    watchdog: &'static crate::WatchdogConfig,
    // Keep endpoint and scheduler borrows, without retaining empty command,
    // timer and IRQ state slots or an already-drained packet scratch buffer.
    _controller: LeControllerCommandEndpoint<'static, CriticalSectionRawMutex, H2C, C2H, PC>,
    _modem_driver: ModemTimerDriver<'static, CriticalSectionRawMutex>,
    _recheck: DtmAbsoluteRecheck,
    _wakers: &'static RuntimeWakers,
    _interrupt: BluetoothInterruptDisabled,
    _timer: RetiredTimer<MT>,
    _command: ControllerTaskHciRetired<'static, PublishedStorage, SC>,
}

/// Terminal Controller with its actual primary/NRT register owner extracted.
///
/// Task/HCI and timer retirement precede this transition. The post-route HAL
/// owner admits checked output release through the matching retired task.
/// Physical shutdown follows only from the released-output state. Empty ISR
/// slots remain reserved for this epoch while static software borrows exist.
#[must_use = "retain the Controller and its recovered interrupt registers"]
pub struct BluetoothHardwareInterruptsRetired<
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> {
    hardware: BluetoothHardwareRetired<MT, SC, H2C, C2H, PC>,
    _registers: oer_esp32s31_radio_esp_hal::RetiredEspHalBluetoothInterruptRegisters,
}

/// Retired Controller after physical scheduler-source and IRQ-output release.
/// The runtime counter, BTBB/PHY and memory publications remain retained.
#[must_use = "retain powered resources until PHY and clock shutdown completes"]
pub struct BluetoothHardwareOutputReleased<
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> {
    hardware: BluetoothHardwareRetired<MT, SC, H2C, C2H, PC>,
    _registers: oer_esp32s31_radio_esp_hal::ReleasedEspHalBluetoothInterruptRegisters,
}

impl<const MT: usize, const SC: usize, const H2C: usize, const C2H: usize, const PC: usize>
    BluetoothHardwareInterruptsRetired<MT, SC, H2C, C2H, PC>
{
    /// Check actual scheduler heads and pending faults before releasing output.
    /// Rejection retains the terminal Controller and its IRQ registers; it does
    /// not restore routing or claim that the physical radio has powered down.
    #[inline(never)]
    #[allow(
        clippy::result_large_err,
        reason = "rejection preserves every powered owner without allocation"
    )]
    pub fn try_release_controller_output(
        mut self,
    ) -> Result<
        BluetoothHardwareOutputReleased<MT, SC, H2C, C2H, PC>,
        (
            oer_esp32s31_bluetooth_controller::controller::BluetoothControllerOutputReleaseError,
            Self,
        ),
    > {
        match self
            ._registers
            .try_release_controller_output(&mut self.hardware._command)
        {
            Ok(registers) => Ok(BluetoothHardwareOutputReleased {
                hardware: self.hardware,
                _registers: registers,
            }),
            Err((error, registers)) => Err((
                error,
                Self {
                    hardware: self.hardware,
                    _registers: registers,
                },
            )),
        }
    }
}

impl<const MT: usize, const SC: usize, const H2C: usize, const C2H: usize, const PC: usize>
    BluetoothHardwareRetired<MT, SC, H2C, C2H, PC>
{
    /// Return the shared ISR register partition only after idle HCI retirement.
    /// Rejection changes no owner and returns this complete terminal state.
    #[inline(never)]
    #[allow(
        clippy::result_large_err,
        reason = "rejection retains all powered owners"
    )]
    pub fn try_retire_interrupts(
        self,
    ) -> Result<
        BluetoothHardwareInterruptsRetired<MT, SC, H2C, C2H, PC>,
        (
            oer_esp32s31_radio_esp_hal::EspHalBluetoothInterruptRetirementError,
            Self,
        ),
    > {
        match self._interrupt.retire_registers() {
            Ok(registers) => Ok(BluetoothHardwareInterruptsRetired {
                hardware: self,
                _registers: registers,
            }),
            Err(error) => Err((error, self)),
        }
    }
}

impl<const MT: usize, const SC: usize, const H2C: usize, const C2H: usize, const PC: usize>
    BluetoothHardwareRunner<MT, SC, H2C, C2H, PC>
{
    // Restore a rejected actor in place before assembling the larger timer
    // transition result. Keep its command-state temporary in a separate frame.
    #[inline(never)]
    fn retire_command(
        &mut self,
    ) -> Result<
        ControllerTaskHciRetired<'static, PublishedStorage, SC>,
        ControllerCommandRetirementError,
    > {
        let command = self.command.take().expect("idle actor retained");
        match command.try_retire_hci(&mut self.controller) {
            Ok(command) => Ok(command),
            Err((error, command)) => {
                self.command = Some(command);
                Err(error)
            }
        }
    }

    /// Disable IRQ and take the drained timer while the command actor is idle.
    ///
    /// This consumes an unstarted runner or one returned by `run_until_idle`.
    /// Pending timer work is
    /// returned as a lossless failure: resume it and drive the task before retry.
    /// The started hardware counter is retained, not stopped or reset.
    #[inline(never)]
    #[allow(
        clippy::result_large_err,
        reason = "all affine runner owners must survive rejection"
    )]
    pub fn retire_modem_timer(
        mut self,
    ) -> Result<
        BluetoothHardwareTimerRetired<MT, SC, H2C, C2H, PC>,
        BluetoothHardwareTimerFailure<MT, SC, H2C, C2H, PC>,
    > {
        let phase = self.command.as_ref().expect("live command actor").phase();
        if phase != ControllerCommandPhase::Idle {
            return Err(BluetoothHardwareTimerFailure {
                error: BluetoothHardwareTimerError::NotIdle(phase),
                runner: self,
                transition: TimerTransition::Unchanged,
            });
        }
        let live = self.interrupt.take().expect("live IRQ owner");
        let interrupt = match live.disable() {
            Ok(disabled) => disabled,
            Err(failure) => {
                let (error, live) = failure.into_parts();
                self.interrupt = Some(live);
                return Err(BluetoothHardwareTimerFailure {
                    error: BluetoothHardwareTimerError::Disable(error),
                    runner: self,
                    transition: TimerTransition::Unchanged,
                });
            }
        };
        if let Some(fault) = interrupt.fault() {
            return Err(BluetoothHardwareTimerFailure {
                error: BluetoothHardwareTimerError::InterruptFault(fault),
                runner: self,
                transition: TimerTransition::Disabled(interrupt),
            });
        }
        let task = self.modem_timer.take().expect("live timer owner");
        match task.try_retire() {
            Ok(timer) => Ok(BluetoothHardwareTimerRetired {
                runner: self,
                interrupt,
                timer,
            }),
            Err((error, task)) => {
                self.modem_timer = Some(task);
                Err(BluetoothHardwareTimerFailure {
                    error: BluetoothHardwareTimerError::Retire(error),
                    runner: self,
                    transition: TimerTransition::Disabled(interrupt),
                })
            }
        }
    }
}

impl<const MT: usize, const SC: usize, const H2C: usize, const C2H: usize, const PC: usize>
    BluetoothHardwareTimerRetired<MT, SC, H2C, C2H, PC>
{
    /// Restore timer storage before rebinding all three IRQ routes.
    #[allow(
        clippy::result_large_err,
        reason = "failed restoration retains the complete owner"
    )]
    pub fn resume(
        self,
    ) -> Result<
        BluetoothHardwareRunner<MT, SC, H2C, C2H, PC>,
        BluetoothHardwareTimerFailure<MT, SC, H2C, C2H, PC>,
    > {
        restore(
            self.runner,
            TimerTransition::Retired {
                interrupt: self.interrupt,
                timer: self.timer,
            },
        )
    }

    /// Join the retired timer and IRQ service with the exact idle HCI barrier.
    /// Queued packets reject closure without losing any owner. Resume this
    /// returned state to service them before retrying; success is irreversible.
    #[inline(never)]
    #[allow(
        clippy::result_large_err,
        reason = "rejected HCI drain retains every hardware owner"
    )]
    pub fn try_retire_hci(
        mut self,
    ) -> Result<
        BluetoothHardwareRetired<MT, SC, H2C, C2H, PC>,
        (ControllerCommandRetirementError, Self),
    > {
        match self.runner.retire_command() {
            Ok(command) => {
                // Each actual owner has moved to this transition before its
                // empty execution slot can be discarded. This is terminal:
                // no runner is reconstructed from the remaining borrows.
                assert!(self.runner.command.is_none());
                assert!(self.runner.modem_timer.is_none());
                assert!(self.runner.interrupt.is_none());
                Ok(BluetoothHardwareRetired {
                    watchdog: self.runner.watchdog,
                    _controller: self.runner.controller,
                    _modem_driver: self.runner.modem_driver,
                    _recheck: self.runner.recheck,
                    _wakers: self.runner.wakers,
                    _interrupt: self.interrupt,
                    _timer: self.timer,
                    _command: command,
                })
            }
            Err(error) => Err((error, self)),
        }
    }
}

impl<const MT: usize, const SC: usize, const H2C: usize, const C2H: usize, const PC: usize>
    BluetoothHardwareTimerFailure<MT, SC, H2C, C2H, PC>
{
    /// Exact rejection; inspecting it consumes no authority.
    pub const fn error(&self) -> BluetoothHardwareTimerError {
        self.error
    }

    /// Restore the retained phase, preserving sticky ISR faults across rebind.
    #[allow(clippy::result_large_err, reason = "retries retain all affine owners")]
    pub fn resume(self) -> Result<BluetoothHardwareRunner<MT, SC, H2C, C2H, PC>, Self> {
        restore(self.runner, self.transition)
    }
}

#[allow(
    clippy::result_large_err,
    reason = "every restoration rejection retains the exact phase"
)]
fn restore<
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
>(
    mut runner: BluetoothHardwareRunner<MT, SC, H2C, C2H, PC>,
    transition: TimerTransition<MT>,
) -> Result<
    BluetoothHardwareRunner<MT, SC, H2C, C2H, PC>,
    BluetoothHardwareTimerFailure<MT, SC, H2C, C2H, PC>,
> {
    let interrupt = match transition {
        TimerTransition::Unchanged => return Ok(runner),
        TimerTransition::Disabled(interrupt) => interrupt,
        TimerTransition::Retired { interrupt, timer } => match timer.resume() {
            Ok(timer) => {
                runner.modem_timer = Some(timer);
                interrupt
            }
            Err((error, timer)) => {
                return Err(BluetoothHardwareTimerFailure {
                    error: BluetoothHardwareTimerError::Restore(error),
                    runner,
                    transition: TimerTransition::Retired { interrupt, timer },
                });
            }
        },
    };
    match interrupt.bind() {
        Ok(interrupt) => {
            runner.interrupt = Some(interrupt);
            Ok(runner)
        }
        Err(failure) => {
            let (error, interrupt) = failure.into_parts();
            Err(BluetoothHardwareTimerFailure {
                error: BluetoothHardwareTimerError::Bind(error),
                runner,
                transition: TimerTransition::Disabled(interrupt),
            })
        }
    }
}

/// Result of joining a retired Controller with its platform reservation.
/// A mismatch retains both owners so the caller can supply the correct pair.
#[must_use = "retain the joined shutdown owner or correct the mismatched pair"]
pub enum BluetoothPlatformJoin<
    P: 'static,
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> {
    /// The exact platform was extracted after this Controller's HCI retirement.
    Joined(BluetoothHardwareRetiredWithPlatform<P, MT, SC, H2C, C2H, PC>),
    /// The reservation belongs to another Controller; neither owner changed.
    Mismatch {
        /// Original retired hardware owner.
        hardware: BluetoothHardwareOutputReleased<MT, SC, H2C, C2H, PC>,
        /// Unchanged platform lease for another epoch.
        platform: oer_esp32s31_bluetooth::resources::platform_retirement::ControllerRuntimePlatform<
            'static,
            P,
        >,
    },
}

/// Retired Controller and its actual platform reservation, still powered.
///
/// Role memory stays private across `release_physical`. That consuming
/// transition closes RF, powers down temperature, resets Bluetooth and restores
/// the captured clock baseline before returning cold ownership.
#[must_use = "retain all powered owners until physical teardown completes"]
pub struct BluetoothHardwareRetiredWithPlatform<
    P: 'static,
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> {
    _hardware: BluetoothHardwareOutputReleased<MT, SC, H2C, C2H, PC>,
    _platform: oer_esp32s31_bluetooth::resources::platform_retirement::ControllerRetiredPlatform<
        'static,
        P,
    >,
}

type RetiredBindings<const H2C: usize, const C2H: usize, const PC: usize> = (
    LeControllerCommandEndpoint<'static, CriticalSectionRawMutex, H2C, C2H, PC>,
    ModemTimerDriver<'static, CriticalSectionRawMutex>,
    DtmAbsoluteRecheck,
    &'static RuntimeWakers,
    BluetoothInterruptDisabled,
    &'static crate::WatchdogConfig,
);

/// Actual cold radio after RF close, Controller reset and clock restoration.
/// The closed Host epoch and old static software references remain retained.
#[must_use = "retain cold ownership and the separate old software epoch"]
pub struct BluetoothHardwareColdReleased<
    P: 'static,
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> {
    owner: oer_esp32s31_bluetooth_controller::controller::ControllerColdReleased<
        'static,
        P,
        PublishedStorage,
        SC,
        MT,
    >,
    _bindings: RetiredBindings<H2C, C2H, PC>,
}

/// Physical shutdown failed with all hardware and platform owners retained.
#[must_use = "a failed physical shutdown cannot release the platform reservation"]
pub struct BluetoothHardwareShutdownFailure<
    P: 'static,
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> {
    owner: oer_esp32s31_bluetooth_controller::controller::ControllerPhysicalShutdownFailure<
        'static,
        P,
        PublishedStorage,
        SC,
        MT,
    >,
    _bindings: RetiredBindings<H2C, C2H, PC>,
}

impl<
    P: 'static,
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> BluetoothHardwareShutdownFailure<P, MT, SC, H2C, C2H, PC>
{
    /// First hardware or PHY failure; ownership remains in this value.
    pub fn error(
        &self,
    ) -> oer_esp32s31_bluetooth_controller::controller::ControllerPhysicalShutdownError {
        self.owner.error()
    }
}

impl<
    P: 'static,
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> BluetoothHardwareColdReleased<P, MT, SC, H2C, C2H, PC>
{
    /// Consume the closed old epoch and return the actual cold radio and final
    /// calibration state. This does not reopen one-shot ISR or software storage.
    pub fn into_cold(
        self,
    ) -> (
        oer_esp32s31_bluetooth::resources::BluetoothStopped<P>,
        oer_esp32s31_phy::PhyState,
        oer_esp32s31_bluetooth_controller::controller::ControllerRetiredStorage<
            'static,
            PublishedStorage,
            SC,
            MT,
        >,
    ) {
        self.owner.into_parts()
    }
}

impl<
    P: 'static,
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> BluetoothHardwareRetiredWithPlatform<P, MT, SC, H2C, C2H, PC>
{
    /// Finish last-client RF close and reset/clock release into cold ownership.
    /// An ambiguous PHY-close failure requests system reset with all owners
    /// retained. Preparation rejection and already-closed reunion failure
    /// return their non-runnable frontiers instead.
    /// Once polled, drive to completion; cancellation retains the platform lease
    /// without implicit cleanup and does not authorize a new Controller epoch.
    pub async fn release_physical(
        self,
    ) -> Result<
        BluetoothHardwareColdReleased<P, MT, SC, H2C, C2H, PC>,
        BluetoothHardwareShutdownFailure<P, MT, SC, H2C, C2H, PC>,
    > {
        let protection = self._hardware.hardware.watchdog.shutdown();
        let BluetoothHardwareOutputReleased {
            hardware,
            _registers,
        } = self._hardware;
        let BluetoothHardwareRetired {
            watchdog,
            _controller,
            _modem_driver,
            _recheck,
            _wakers,
            _interrupt,
            _timer,
            _command,
        } = hardware;
        let bindings = (
            _controller,
            _modem_driver,
            _recheck,
            _wakers,
            _interrupt,
            watchdog,
        );
        let mut release = core::pin::pin!(
            _registers.release_physical::<P, _, crate::EmbassyPhyTime, SC, MT>(
                _command,
                _timer,
                self._platform,
            )
        );
        match core::future::poll_fn(|context| poll_physical_release(release.as_mut(), context))
            .await
        {
            Ok(owner) => {
                crate::WatchdogConfig::complete(protection);
                Ok(BluetoothHardwareColdReleased {
                    owner,
                    _bindings: bindings,
                })
            }
            Err(owner) => {
                enforce_lifecycle(&owner, &bindings, owner.phy_hardware_ambiguous());
                crate::WatchdogConfig::complete(protection);
                Err(BluetoothHardwareShutdownFailure {
                    owner,
                    _bindings: bindings,
                })
            }
        }
    }
}

impl<const MT: usize, const SC: usize, const H2C: usize, const C2H: usize, const PC: usize>
    BluetoothHardwareOutputReleased<MT, SC, H2C, C2H, PC>
{
    /// Extract and join only the platform issued by this Controller's split.
    /// No route, register, PHY or reservation release occurs on either branch.
    pub fn join_platform<P>(
        self,
        platform: oer_esp32s31_bluetooth::resources::platform_retirement::ControllerRuntimePlatform<
            'static,
            P,
        >,
    ) -> BluetoothPlatformJoin<P, MT, SC, H2C, C2H, PC> {
        match platform.try_retire(self.hardware._command.hci_proof()) {
            Ok(platform) => BluetoothPlatformJoin::Joined(BluetoothHardwareRetiredWithPlatform {
                _hardware: self,
                _platform: platform,
            }),
            Err(platform) => BluetoothPlatformJoin::Mismatch {
                hardware: self,
                platform,
            },
        }
    }
}

// RF close and the composition result each retain a complete affine graph.
// Give their polls separate frames instead of accumulating both move frontiers.
#[inline(never)]
fn poll_physical_release<F: core::future::Future>(
    future: core::pin::Pin<&mut F>,
    context: &mut core::task::Context<'_>,
) -> core::task::Poll<F::Output> {
    let poll: fn(
        core::pin::Pin<&mut F>,
        &mut core::task::Context<'_>,
    ) -> core::task::Poll<F::Output> = F::poll;
    core::hint::black_box(poll)(future, context)
}

mod restart;

// Borrow both retained halves rather than materializing another large failure
// union. HCI is already retired; no Host acknowledgement or cleanup gates reset.
fn enforce_lifecycle<E: ?Sized, B: ?Sized>(owner: &E, bindings: &B, hardware_ambiguous: bool) {
    use oer_esp32s31_phy::tracking::fail_stop::SharedPhyFailStop;
    if let Some(reason) = SharedPhyFailStop::from_ambiguous_lifecycle(hardware_ambiguous) {
        super::fail_stop::fail_stop_shared_phy(reason, (owner, bindings));
    }
}
pub use restart::{BluetoothHardwareRestartError, BluetoothHardwareRestartFailure};

mod maintenance;
pub use maintenance::{BluetoothHardwareMaintenanceError, BluetoothHardwareMaintenanceFailure};
