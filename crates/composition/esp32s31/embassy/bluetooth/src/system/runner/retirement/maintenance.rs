//! Quiescent PHY maintenance and return to the same Controller runner.

use super::*;
use oer_esp32s31_bluetooth_controller::controller::{
    ControllerPhyMaintenanceError, ControllerPhyMaintenanceFailure,
};
use oer_esp32s31_phy::tracking::parameters::PhyParamTrackingOutcome;
use oer_esp32s31_radio_esp_hal::{
    EspHalBluetoothInterruptRetirementError, EspHalBluetoothInterruptStorageError,
};

/// Exact failed edge while maintaining the same powered Controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothHardwareMaintenanceError {
    Registers(EspHalBluetoothInterruptRetirementError),
    Command(ControllerCommandRetirementError),
    Controller(ControllerPhyMaintenanceError<EspHalBluetoothInterruptStorageError>),
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
    _owners: FailureOwners<MT, SC, H2C, C2H, PC>,
}

// A consumed command slot and its lower failure are mutually exclusive owners.
// Preserve that distinction instead of reserving storage for both at once.
#[allow(
    dead_code,
    reason = "sealed failure variants intentionally retain all owners"
)]
#[allow(
    clippy::large_enum_variant,
    reason = "mutually exclusive affine owners need no heap or duplicate storage"
)]
enum FailureOwners<
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
> {
    Runner {
        runner: BluetoothHardwareRunner<MT, SC, H2C, C2H, PC>,
        interrupt: BluetoothInterruptDisabled,
        timer: Option<RetiredTimer<MT>>,
        registers: Option<oer_esp32s31_radio_esp_hal::RetiredEspHalBluetoothInterruptRegisters>,
    },
    Physical {
        remainder: MaintenanceRemainder<H2C, C2H, PC>,
        interrupt: BluetoothInterruptDisabled,
        lower: ControllerPhyMaintenanceFailure<'static, PublishedStorage, SC, MT>,
    },
}

struct MaintenanceRemainder<const H2C: usize, const C2H: usize, const PC: usize> {
    _watchdog: &'static crate::WatchdogConfig,
    _restoration_protection: Option<oer_esp32s31_soc_esp_hal::watchdog::DeadlineLease<'static>>,
    _controller: LeControllerCommandEndpoint<'static, CriticalSectionRawMutex, H2C, C2H, PC>,
    _modem_driver: ModemTimerDriver<'static, CriticalSectionRawMutex>,
    _packet: [u8; PC],
    _recheck: DtmAbsoluteRecheck,
    _advertising_delay: BluetoothAdvertisingDelaySource,
    _wakers: &'static RuntimeWakers,
    _schedule: HardwareRunnerSchedule,
}
impl<const H2C: usize, const C2H: usize, const PC: usize> MaintenanceRemainder<H2C, C2H, PC> {
    fn retain<const MT: usize, const SC: usize>(
        runner: BluetoothHardwareRunner<MT, SC, H2C, C2H, PC>,
    ) -> Self {
        let BluetoothHardwareRunner {
            watchdog,
            restoration_protection,
            command,
            controller,
            modem_timer,
            modem_driver,
            interrupt,
            packet,
            recheck,
            advertising_delay,
            wakers,
            schedule,
        } = runner;
        assert!(
            command.is_none() && modem_timer.is_none() && interrupt.is_none(),
            "all extracted authorities are retained by the physical failure"
        );
        Self {
            _watchdog: watchdog,
            _restoration_protection: restoration_protection,
            _controller: controller,
            _modem_driver: modem_driver,
            _packet: packet,
            _recheck: recheck,
            _advertising_delay: advertising_delay,
            _wakers: wakers,
            _schedule: schedule,
        }
    }
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
    /// Once polled, drive to completion. Admission failures retain the original
    /// quiesced frontier. Execution or restoration failure closes HCI and
    /// requests system reset with all owners retained. Cancellation never
    /// restores an owner; the independently armed SoC deadline remains active.
    /// IRQ/timer retirement and idle admission precede this physical lease.
    /// An optional absolute tracking deadline guards PHY work only; callers
    /// must reserve time for IRQ and protocol restoration separately.
    pub async fn maintain_phy<P: 'static>(
        self,
        platform: &mut oer_esp32s31_bluetooth::resources::platform_retirement::ControllerRuntimePlatform<'static, P>,
        tracking_deadline: Option<oer_esp32s31_phy::tracking::deadline::TrackingDeadline>,
        calibration_debug: Option<oer_esp32s31_phy::state::PhyTemperatureTrackingDebug>,
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
                    _owners: FailureOwners::Runner {
                        runner,
                        interrupt,
                        timer: Some(timer),
                        registers: None,
                    },
                });
            }
        };
        let mut idle = match take_idle(&mut runner) {
            Ok(idle) => idle,
            Err(error) => {
                return Err(BluetoothHardwareMaintenanceFailure {
                    error: BluetoothHardwareMaintenanceError::Command(error),
                    _owners: FailureOwners::Runner {
                        runner,
                        interrupt,
                        timer: Some(timer),
                        registers: Some(registers),
                    },
                });
            }
        };
        let old_debug = calibration_debug.map(|debug| idle.set_phy_tracking_debug(debug));
        let protection = runner.watchdog.maintenance(None);
        crate::maintenance_observation::begin(
            embassy_time::Instant::now().as_micros(),
            tracking_deadline.map(|d| d.expires_at_micros()),
            None,
        );
        let mut result = {
            let mut clock = crate::EmbassyPhyTime;
            let mut work = core::pin::pin!(
                registers.maintain_phy::<P, _, crate::EmbassyPhyTime, _, SC, MT, H2C, C2H, PC>(
                    idle,
                    timer,
                    platform,
                    &mut runner.controller,
                    &mut clock,
                    crate::maintenance_observation::Observer,
                    tracking_deadline,
                )
            );
            core::future::poll_fn(|cx| poll_physical_release(work.as_mut(), cx)).await
        };
        if let Ok(maintained) = &mut result {
            crate::maintenance_observation::physical(maintained.outcome);
            if let Some(debug) = old_debug {
                maintained.task.set_phy_tracking_debug(debug);
            }
        }
        finish(runner, interrupt, result, protection)
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
    oer_esp32s31_bluetooth_controller::controller::ControllerIdleCommandTask<
        'static,
        PublishedStorage,
        SC,
    >,
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
        oer_esp32s31_bluetooth_controller::controller::ControllerPhyMaintained<
            'static,
            PublishedStorage,
            SC,
            MT,
        >,
        ControllerPhyMaintenanceFailure<'static, PublishedStorage, SC, MT>,
    >,
    protection: oer_esp32s31_soc_esp_hal::watchdog::DeadlineLease<'static>,
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
            if let Some(reason) = lower.failure_stage().shared_phy_failure() {
                runner.controller.close_transport();
                super::super::fail_stop_shared_phy(reason, (&runner, &interrupt, &lower));
            }
            crate::WatchdogConfig::complete(protection);
            return Err(BluetoothHardwareMaintenanceFailure {
                error: BluetoothHardwareMaintenanceError::Controller(*lower.error()),
                _owners: FailureOwners::Physical {
                    remainder: MaintenanceRemainder::retain(runner),
                    interrupt,
                    lower,
                },
            });
        }
    };
    runner.command = Some(ControllerCommandTask::new(maintained.task));
    runner.modem_timer = Some(maintained.timer);
    if let Err(error) = runner.recheck.reanchor_after_idle() {
        runner.controller.close_transport();
        super::super::fail_stop_shared_phy(
            oer_esp32s31_phy::tracking::fail_stop::SharedPhyFailStop::MaintenanceFailed,
            (&runner, &interrupt, error),
        );
    }
    match interrupt.bind() {
        Ok(interrupt) => runner.interrupt = Some(interrupt),
        Err(failure) => {
            runner.controller.close_transport();
            super::super::fail_stop_shared_phy(
                oer_esp32s31_phy::tracking::fail_stop::SharedPhyFailStop::MaintenanceFailed,
                (&runner, &failure),
            );
        }
    }
    crate::WatchdogConfig::complete(protection);
    Ok((runner, maintained.outcome))
}
