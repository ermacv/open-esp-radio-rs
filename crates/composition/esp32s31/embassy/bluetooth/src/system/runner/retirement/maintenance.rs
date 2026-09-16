//! Quiescent PHY maintenance and return to the same Controller runner.

use super::*;
use oer_esp32s31_bluetooth::controller::{
    ControllerPhyMaintenanceError, ControllerPhyMaintenanceFailure,
};
use oer_esp32s31_bluetooth_embassy::controller::DtmRecheckStartError;
use oer_esp32s31_phy::tracking::parameters::PhyParamTrackingOutcome;
use oer_esp32s31_radio_platform_esp_hal::{
    EspHalBluetoothInterruptRetirementError, EspHalBluetoothInterruptStorageError,
};

/// Exact failed edge while maintaining the same powered Controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothHardwareMaintenanceError {
    Registers(EspHalBluetoothInterruptRetirementError),
    Command(ControllerCommandRetirementError),
    Controller(ControllerPhyMaintenanceError<EspHalBluetoothInterruptStorageError>),
    Recheck(DtmRecheckStartError),
    Bind(BluetoothInterruptBindError),
}

/// Failed maintenance retains the complete unrouted runner and lower owners.
#[must_use = "failed maintenance never authorizes radio resumption"]
pub struct BluetoothHardwareMaintenanceFailure<
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> {
    error: BluetoothHardwareMaintenanceError,
    _runner: BluetoothHardwareRunner<MT, SC, H2C, C2H, PC>,
    _interrupt: BluetoothInterruptDisabled,
    _timer: Option<RetiredTimer<MT>>,
    _registers:
        Option<oer_esp32s31_radio_platform_esp_hal::RetiredEspHalBluetoothInterruptRegisters>,
    _lower: Option<ControllerPhyMaintenanceFailure<'static, PublishedStorage, SC, MT>>,
}
impl<const MT: usize, const SC: usize, const H2C: usize, const C2H: usize, const PC: usize>
    BluetoothHardwareMaintenanceFailure<MT, SC, H2C, C2H, PC>
{
    pub const fn error(&self) -> BluetoothHardwareMaintenanceError {
        self.error
    }
}

impl<const MT: usize, const SC: usize, const H2C: usize, const C2H: usize, const PC: usize>
    BluetoothHardwareTimerRetired<MT, SC, H2C, C2H, PC>
{
    /// Execute a due PHY request with CPU routes disabled, then restore both
    /// original ISR owners and the same HCI/task/timer epoch before routing.
    /// Not-due requests return `None`; they do not advance the tracking clock.
    /// Once polled, drive to completion; failure or cancellation requires reset.
    pub async fn maintain_phy<P: 'static>(
        self,
        platform: &mut oer_esp32s31_bluetooth::resources::platform_retirement::ControllerRuntimePlatform<'static, P>,
    ) -> Result<
        (
            BluetoothHardwareRunner<MT, SC, H2C, C2H, PC>,
            Option<PhyParamTrackingOutcome>,
        ),
        BluetoothHardwareMaintenanceFailure<MT, SC, H2C, C2H, PC>,
    > {
        let Self {
            mut runner,
            interrupt,
            timer,
        } = self;
        let registers = match interrupt.retire_registers() {
            Ok(registers) => registers,
            Err(error) => {
                return Err(BluetoothHardwareMaintenanceFailure {
                    error: BluetoothHardwareMaintenanceError::Registers(error),
                    _runner: runner,
                    _interrupt: interrupt,
                    _timer: Some(timer),
                    _registers: None,
                    _lower: None,
                });
            }
        };
        let idle = match take_idle(&mut runner) {
            Ok(idle) => idle,
            Err(error) => {
                return Err(BluetoothHardwareMaintenanceFailure {
                    error: BluetoothHardwareMaintenanceError::Command(error),
                    _runner: runner,
                    _interrupt: interrupt,
                    _timer: Some(timer),
                    _registers: Some(registers),
                    _lower: None,
                });
            }
        };
        let result = {
            let mut clock = crate::EmbassyPhyTime;
            let mut work = core::pin::pin!(
                registers.maintain_phy::<P, _, crate::EmbassyPhyTime, _, SC, MT, H2C, C2H, PC>(
                    idle,
                    timer,
                    platform,
                    &mut runner.controller,
                    &mut clock
                )
            );
            core::future::poll_fn(|cx| poll_physical_release(work.as_mut(), cx)).await
        };
        finish(runner, interrupt, result)
    }
}

#[inline(never)]
#[allow(
    clippy::result_large_err,
    reason = "the unchanged actor stays inside the runner on rejection"
)]
fn take_idle<
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
>(
    runner: &mut BluetoothHardwareRunner<MT, SC, H2C, C2H, PC>,
) -> Result<
    oer_esp32s31_bluetooth::controller::ControllerIdleCommandTask<'static, PublishedStorage, SC>,
    ControllerCommandRetirementError,
> {
    let command = runner
        .command
        .take()
        .expect("retired timer retains idle actor");
    match command.try_into_idle() {
        Ok(idle) => Ok(idle),
        Err((error, command)) => {
            runner.command = Some(command);
            Err(error)
        }
    }
}

#[inline(never)]
#[allow(
    clippy::result_large_err,
    reason = "failure retains the complete allocation-free runner"
)]
fn finish<const MT: usize, const SC: usize, const H2C: usize, const C2H: usize, const PC: usize>(
    mut runner: BluetoothHardwareRunner<MT, SC, H2C, C2H, PC>,
    interrupt: BluetoothInterruptDisabled,
    result: Result<
        oer_esp32s31_bluetooth::controller::ControllerPhyMaintained<
            'static,
            PublishedStorage,
            SC,
            MT,
        >,
        ControllerPhyMaintenanceFailure<'static, PublishedStorage, SC, MT>,
    >,
) -> Result<
    (
        BluetoothHardwareRunner<MT, SC, H2C, C2H, PC>,
        Option<PhyParamTrackingOutcome>,
    ),
    BluetoothHardwareMaintenanceFailure<MT, SC, H2C, C2H, PC>,
> {
    let maintained = match result {
        Ok(maintained) => maintained,
        Err(lower) => {
            return Err(BluetoothHardwareMaintenanceFailure {
                error: BluetoothHardwareMaintenanceError::Controller(*lower.error()),
                _runner: runner,
                _interrupt: interrupt,
                _timer: None,
                _registers: None,
                _lower: Some(lower),
            });
        }
    };
    runner.command = Some(ControllerCommandTask::new(maintained.task));
    runner.modem_timer = Some(maintained.timer);
    if let Err(error) = runner.recheck.reanchor_after_idle() {
        return Err(BluetoothHardwareMaintenanceFailure {
            error: BluetoothHardwareMaintenanceError::Recheck(error),
            _runner: runner,
            _interrupt: interrupt,
            _timer: None,
            _registers: None,
            _lower: None,
        });
    }
    match interrupt.bind() {
        Ok(interrupt) => runner.interrupt = Some(interrupt),
        Err(failure) => {
            let (error, interrupt) = failure.into_parts();
            return Err(BluetoothHardwareMaintenanceFailure {
                error: BluetoothHardwareMaintenanceError::Bind(error),
                _runner: runner,
                _interrupt: interrupt,
                _timer: None,
                _registers: None,
                _lower: None,
            });
        }
    }
    Ok((runner, maintained.outcome))
}
