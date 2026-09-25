//! Live Reset handoff, reversible timer/IRQ probe and terminal ownership join.
//!
//! Separate synchronous frames keep the large success/failure owners out of
//! the executor poll frame and prevent adjacent transition results accumulating.

use oer_esp32s31_bluetooth_system::{
    BluetoothHardwareRunner as Runner, BluetoothHardwareTimerRetired as Retired,
};

/// Terminal diagnostic: physical cold return keeps the old software epoch closed.
#[inline(never)]
pub(super) async fn shutdown(
    runner: Runner<4, 1, 4, 4, 258>,
    platform: oer_esp32s31_bluetooth::resources::platform_retirement::ControllerRuntimePlatform<
        'static,
        oer_esp32s31_radio_esp_hal::EspHalBluetoothPlatform<'static>,
    >,
) -> Cold {
    use oer_esp32s31_bluetooth_system::BluetoothPlatformJoin;

    let retired = retire_controller(runner);
    // Keep the result in its construction slot: extracting the joined owner
    // would copy the complete powered resource graph through another temporary.
    let owner = retired.join_platform(platform);
    let owner = match owner {
        BluetoothPlatformJoin::Joined(owner) => owner,
        BluetoothPlatformJoin::Mismatch { .. } => {
            crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-platform-retirement\r\n")
        }
    };
    let owner = {
        let mut release = core::pin::pin!(owner.release_physical());
        match super::poll_with_stack_boundary(release.as_mut()).await {
            Ok(owner) => owner,
            Err(failure) => {
                core::hint::black_box(failure.error());
                crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-physical-release\r\n")
            }
        }
    };
    owner
}

pub(super) type Cold = oer_esp32s31_bluetooth_system::BluetoothHardwareColdReleased<
    oer_esp32s31_radio_esp_hal::EspHalBluetoothPlatform<'static>,
    4,
    1,
    4,
    4,
    258,
>;
pub(super) type Ready = oer_esp32s31_bluetooth_system::BluetoothSystemReady<
    oer_esp32s31_radio_esp_hal::EspHalBluetoothPlatform<'static>,
    4,
    1,
    4,
    4,
    258,
>;

pub(super) async fn probe_closed(
    hci: &super::Host,
    credits: &super::HostAclCredits,
) -> (bool, bool, bool) {
    use bt_hci::{
        cmd::{SyncCmd, controller_baseband::Reset},
        controller::Controller,
    };
    use embedded_io_async::{Error as _, ErrorKind};
    let mut buffer = hci.alloc_buf().expect("bounded HCI buffer");
    let commands = matches!(Reset::new().exec(hci).await, Err(bt_hci::cmd::Error::Io(error)) if error.kind() == ErrorKind::BrokenPipe);
    let events =
        matches!(hci.read(&mut buffer).await, Err(error) if error.kind() == ErrorKind::BrokenPipe);
    let credits = matches!(credits.return_completed_packets(&[]).await, Err(error) if error.kind() == ErrorKind::BrokenPipe);
    (commands, events, credits)
}

pub(super) async fn finish(
    owner: Cold,
    hci: &super::Host,
    credits: &super::HostAclCredits,
    console: &mut super::Console,
    request: u32,
) -> ! {
    let owner = owner.into_cold();
    let (commands_closed, events_closed, acl_credits_closed) = probe_closed(hci, credits).await;
    console
        .send_frame(
            None,
            request,
            super::peripheral_evidence(
                super::PeripheralOperation::Retire,
                super::PeripheralResult::Retired {
                    radio_cold: true,
                    commands_closed,
                    events_closed,
                    acl_credits_closed,
                },
            ),
        )
        .await;
    let mut control = core::pin::pin!(console.run_retired());
    super::poll_with_stack_boundary(control.as_mut()).await;
    core::hint::black_box(&owner);
    unreachable!()
}

pub(super) async fn restart(
    owner: Cold,
    identity: oer_esp32s31_phy::PhyCalibrationIdentity,
) -> Ready {
    let mut restarting = core::pin::pin!(owner.restart(
        oer_esp32s31_bluetooth::phy::PhyInitializationConfig::new(identity)
    ));
    let ready = match super::poll_with_stack_boundary(restarting.as_mut()).await {
        Ok(ready) => ready,
        Err(failure) => {
            core::hint::black_box(failure.error());
            crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-powered-restart\r\n")
        }
    };
    let Ready { system, platform } = ready;
    let oer_esp32s31_bluetooth_system::BluetoothSystem {
        hci,
        host_acl_credits,
        runners,
    } = system;
    let hardware = {
        let mut exercise = core::pin::pin!(exercise_timer_retirement(runners.hardware, &hci));
        super::poll_with_stack_boundary(exercise.as_mut()).await
    };
    Ready {
        platform,
        system: oer_esp32s31_bluetooth_system::BluetoothSystem {
            hci,
            host_acl_credits,
            runners: oer_esp32s31_bluetooth_system::BluetoothRunners { hardware },
        },
    }
}

// Synchronous owner transitions use a separate frame from the terminal
// console future and its retained joined result.
#[inline(never)]
fn retire_controller(
    runner: Runner<4, 1, 4, 4, 258>,
) -> oer_esp32s31_bluetooth_system::BluetoothHardwareOutputReleased<4, 1, 4, 4, 258> {
    release_output(retire_interrupts(retire_hci(retire(runner))))
}

#[inline(never)]
fn release_output(
    retired: oer_esp32s31_bluetooth_system::BluetoothHardwareInterruptsRetired<4, 1, 4, 4, 258>,
) -> oer_esp32s31_bluetooth_system::BluetoothHardwareOutputReleased<4, 1, 4, 4, 258> {
    match retired.try_release_controller_output() {
        Ok(owner) => owner,
        Err(_) => crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-output-release\r\n"),
    }
}

#[inline(never)]
fn retire_interrupts(
    retired: oer_esp32s31_bluetooth_system::BluetoothHardwareRetired<4, 1, 4, 4, 258>,
) -> oer_esp32s31_bluetooth_system::BluetoothHardwareInterruptsRetired<4, 1, 4, 4, 258> {
    match retired.try_retire_interrupts() {
        Ok(owner) => owner,
        Err(_) => {
            crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-irq-register-retirement\r\n")
        }
    }
}

#[inline(never)]
fn retire_hci<
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
>(
    retired: Retired<MT, SC, H2C, C2H, PC>,
) -> oer_esp32s31_bluetooth_system::BluetoothHardwareRetired<MT, SC, H2C, C2H, PC> {
    match retired.try_retire_hci() {
        Ok(owner) => owner,
        Err((error, _owner)) => {
            use oer_esp32s31_bluetooth_controller::controller::ControllerTaskRetirementError as Error;
            use oer_esp32s31_bluetooth::runtime_resources::ControllerRuntimeRetirementError as Runtime;
            let oer_esp32s31_bluetooth_runtime::controller::ControllerCommandRetirementError::Task(
                error,
            ) = error
            else {
                crate::fail(
                    c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-retire-command-state\r\n",
                )
            };
            match error {
                Error::Runtime(Runtime::LockModify) => crate::fail(
                    c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-retire-lock-worker\r\n",
                ),
                Error::Runtime(Runtime::FinishedLists) => crate::fail(
                    c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-retire-finished-worker\r\n",
                ),
                Error::Runtime(Runtime::Timeline) => {
                    crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-retire-timeline\r\n")
                }
                Error::ControllerTime(_) => crate::fail(
                    c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-retire-controller-time\r\n",
                ),
                Error::Role(_) => {
                    crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-retire-role\r\n")
                }
                Error::Hci(_) => {
                    crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-retire-hci\r\n")
                }
            }
        }
    }
}

#[inline(never)]
pub(super) async fn exercise_timer_retirement<
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
>(
    mut runner: Runner<MT, SC, H2C, C2H, PC>,
    hci: &super::Host,
) -> Runner<MT, SC, H2C, C2H, PC> {
    for cycle in 0..3 {
        let mut running = core::pin::pin!(run_reset_until_idle(runner, hci, cycle == 2));
        runner = super::poll_with_stack_boundary(running.as_mut()).await;
        runner = resume(retire(runner));
    }
    runner
}

#[inline(never)]
fn retire<const MT: usize, const SC: usize, const H2C: usize, const C2H: usize, const PC: usize>(
    runner: Runner<MT, SC, H2C, C2H, PC>,
) -> Retired<MT, SC, H2C, C2H, PC> {
    runner.retire_modem_timer().unwrap_or_else(|_failure| {
        crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-timer-retirement\r\n")
    })
}

#[inline(never)]
fn resume<const MT: usize, const SC: usize, const H2C: usize, const C2H: usize, const PC: usize>(
    retired: Retired<MT, SC, H2C, C2H, PC>,
) -> Runner<MT, SC, H2C, C2H, PC> {
    retired.resume().unwrap_or_else(|_failure| {
        crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-timer-resume\r\n")
    })
}

async fn run_reset_until_idle<
    const MT: usize,
    const SC: usize,
    const H2C: usize,
    const C2H: usize,
    const PC: usize,
>(
    runner: Runner<MT, SC, H2C, C2H, PC>,
    hci: &super::Host,
    delayed_request: bool,
) -> Runner<MT, SC, H2C, C2H, PC> {
    use bt_hci::{
        cmd::{SyncCmd, controller_baseband::Reset},
        controller::Controller,
    };
    use core::{
        future::{Future, poll_fn},
        pin::pin,
        task::Poll,
    };
    let reset = Reset::new();
    let mut command = pin!(reset.exec(hci));
    // Publish before the hardware future is polled: an immediate stop request
    // must not bypass this accepted command or its still-unread completion.
    poll_fn(|context| {
        if command.as_mut().poll(context).is_ready() {
            crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-reset-not-queued\r\n");
        }
        Poll::Ready(())
    })
    .await;
    let read_started = core::cell::Cell::new(false);
    let host = async {
        embassy_time::Timer::after_millis(2).await;
        let mut buffer = hci.alloc_buf().unwrap_or_else(|_| {
            crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-retirement-buffer\r\n")
        });
        read_started.set(true);
        super::command_pump::with_event_pump(hci, &mut buffer, command.as_mut(), |_| {
            crate::fail(
                c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-retirement-unexpected-event\r\n",
            )
        })
        .await
        .unwrap_or_else(|_| {
            crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-retirement-read\r\n")
        })
        .unwrap_or_else(|_| {
            crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-retirement-reset\r\n")
        });
    };
    let mut hardware = pin!(async {
        let request = async {
            if delayed_request {
                embassy_time::Timer::after_millis(1).await;
            }
        };
        let mut running = pin!(runner.run_until_idle(request));
        let owner = super::poll_with_stack_boundary(running.as_mut()).await;
        if !read_started.get() {
            crate::fail(
                c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-retirement-before-reset-reply\r\n",
            );
        }
        owner
    });
    let ((), owner) = embassy_futures::join::join(host, hardware.as_mut()).await;
    owner
}

pub(super) async fn run_active(
    hardware: Runner<4, 1, 4, 4, 258>,
    hci: &super::Host,
    credits: &super::HostAclCredits,
    console: &mut super::Console,
    announce: bool,
    platform: &mut oer_esp32s31_bluetooth::resources::platform_retirement::ControllerRuntimePlatform<'static, oer_esp32s31_radio_esp_hal::EspHalBluetoothPlatform<'static>>,
) -> (Runner<4, 1, 4, 4, 258>, u32, super::PeripheralOperation) {
    let requested = core::cell::Cell::new(None);
    let hardware = {
        let request = async {
            requested.set(Some(console.run(hci, credits, announce).await));
        };
        #[cfg(not(feature = "bluetooth-phy-maintenance"))]
        let _ = platform;
        #[cfg(not(feature = "bluetooth-phy-maintenance"))]
        let mut running = core::pin::pin!(hardware.run_until_idle(request));
        #[cfg(feature = "bluetooth-phy-maintenance")]
        let mut running = core::pin::pin!(hardware.run_with_phy_maintenance_until_idle(
            platform,
            diagnostic_maintenance_policy(),
            request
        ));
        match embassy_futures::select::select(
            super::poll_with_stack_boundary(running.as_mut()),
            super::pump(hci, credits),
        )
        .await
        {
            embassy_futures::select::Either::First(owner) => owner,
            embassy_futures::select::Either::Second(()) => {
                crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-pump-returned\r\n")
            }
        }
    };
    let (request, operation) = requested.get().expect("lifecycle request completed");
    (hardware, request, operation)
}

/// Track on the original powered epoch, then execute Reset through the same Host.
pub(super) async fn maintain(
    runner: Runner<4, 1, 4, 4, 258>,
    platform: &mut oer_esp32s31_bluetooth::resources::platform_retirement::ControllerRuntimePlatform<'static, oer_esp32s31_radio_esp_hal::EspHalBluetoothPlatform<'static>>,
    hci: &super::Host,
    calibration_threshold: Option<u8>,
) -> (
    Runner<4, 1, 4, 4, 258>,
    Option<oer_esp32s31_phy::tracking::parameters::PhyParamTrackingOutcome>,
) {
    let retired = retire(runner);
    let (runner, outcome) = {
        let mut maintenance = core::pin::pin!(retired.maintain_phy(
            platform,
            None,
            calibration_threshold.map(|threshold| {
                oer_esp32s31_phy::state::PhyTemperatureTrackingDebug {
                    first: 3,
                    second: threshold,
                }
            })
        ));
        match super::poll_with_stack_boundary(maintenance.as_mut()).await {
            Ok(result) => result,
            Err(failure) => {
                core::hint::black_box(failure.error());
                crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-phy-maintenance\r\n")
            }
        }
    };
    let mut reset = core::pin::pin!(run_reset_until_idle(runner, hci, false));
    (
        super::poll_with_stack_boundary(reset.as_mut()).await,
        outcome,
    )
}

/// Engineering admission values, not qualified thermal or blocking-poll bounds.
/// The production actor owns all scheduling, LL decisions and RF transitions.
#[cfg(feature = "bluetooth-phy-maintenance")]
fn diagnostic_maintenance_policy()
-> oer_esp32s31_bluetooth_runtime::controller::maintenance::PhyMaintenancePolicy {
    use core::num::NonZeroU32;
    use oer_esp32s31_bluetooth_controller::le::peripheral::maintenance::PeripheralMaintenanceBudget;
    use oer_esp32s31_bluetooth_runtime::controller::maintenance::PhyMaintenancePolicy;
    let n = |value| NonZeroU32::new(value).unwrap();
    let budget = PeripheralMaintenanceBudget::new(n(20_000), n(5_000), n(2_000)).unwrap();
    PhyMaintenancePolicy::new(budget, 200_000, 10_000_000).unwrap()
}
