//! Thin HIL composition of checked production retirement transitions.
use oer_esp32s31_bluetooth_integration as radio;
type Runner = radio::BluetoothHardwareRunner<4, 1, 4, 4, 258>;
type Platform = oer_esp32s31_radio_platform_esp_hal::EspHalBluetoothPlatform<'static>;
type PlatformOwner =
    oer_esp32s31_bluetooth::resources::platform_retirement::ControllerRuntimePlatform<
        'static,
        Platform,
    >;
type Cold = radio::BluetoothHardwareColdReleased<Platform, 4, 1, 4, 4, 258>;

pub(super) async fn finish(
    runner: Runner,
    platform: PlatformOwner,
    exit: super::HostExit<'_>,
    identity: oer_esp32s31_phy::PhyCalibrationIdentity,
    state: &super::State,
) -> radio::BluetoothSystemReady<Platform, 4, 1, 4, 4, 258> {
    use bluetooth_example::security::epoch::ShutdownAction;
    if exit.action() == ShutdownAction::Retain {
        state.stopped();
        retain((&runner, &platform, &exit)).await;
    }
    let cold = {
        let mut close = core::pin::pin!(close(runner, platform));
        let mut result = None;
        core::future::poll_fn(|cx| super::poll_into(close.as_mut(), &mut result, cx)).await;
        result.expect("completed cold release")
    };
    let closed = old_hci_closed(&exit.controller.inner).await;
    state.cold(closed);
    if !closed || exit.action() == ShutdownAction::Close {
        state.stopped();
        // A successfully reset failed Host may close RF, but never erase its
        // failure and restart as if an application had requested a fresh epoch.
        retain((&cold, &exit)).await;
    }
    let mut restart = core::pin::pin!(restart(cold, identity));
    let mut result = None;
    core::future::poll_fn(|cx| super::poll_into(restart.as_mut(), &mut result, cx)).await;
    // Retain the old facade until the new physical epoch is fully initialized.
    drop(exit);
    result.expect("completed cold restart")
}

async fn retain<T>(owners: T) -> ! {
    core::future::pending::<()>().await;
    core::hint::black_box(owners);
    unreachable!("terminal owners cannot be resumed")
}

async fn close(runner: Runner, platform: PlatformOwner) -> Cold {
    let owner = join(retire(runner), platform);
    let mut release = core::pin::pin!(owner.release_physical());
    match core::future::poll_fn(|cx| super::super::poll_live(release.as_mut(), cx)).await {
        Ok(cold) => cold,
        Err(failure) => {
            core::hint::black_box(failure.error());
            crate::fail(c"GATT physical release failed\r\n")
        }
    }
}

// Keep synchronous result unions out of the physical-release poll frame.
#[inline(never)]
fn join(
    output: radio::BluetoothHardwareOutputReleased<4, 1, 4, 4, 258>,
    platform: PlatformOwner,
) -> radio::BluetoothHardwareRetiredWithPlatform<Platform, 4, 1, 4, 4, 258> {
    match output.join_platform(platform) {
        radio::BluetoothPlatformJoin::Joined(owner) => owner,
        failure => {
            core::hint::black_box(&failure);
            crate::fail(c"GATT platform retirement failed\r\n")
        }
    }
}

#[inline(never)]
fn retire(runner: Runner) -> radio::BluetoothHardwareOutputReleased<4, 1, 4, 4, 258> {
    release(interrupts(hci(timer(runner))))
}
#[inline(never)]
fn timer(runner: Runner) -> radio::BluetoothHardwareTimerRetired<4, 1, 4, 4, 258> {
    match runner.retire_modem_timer() {
        Ok(owner) => owner,
        Err(failure) => {
            core::hint::black_box(&failure);
            crate::fail(c"GATT timer retirement failed\r\n")
        }
    }
}
#[inline(never)]
fn hci(
    owner: radio::BluetoothHardwareTimerRetired<4, 1, 4, 4, 258>,
) -> radio::BluetoothHardwareRetired<4, 1, 4, 4, 258> {
    match owner.try_retire_hci() {
        Ok(owner) => owner,
        Err(failure) => {
            core::hint::black_box(&failure);
            crate::fail(c"GATT HCI retirement failed\r\n")
        }
    }
}
#[inline(never)]
fn interrupts(
    owner: radio::BluetoothHardwareRetired<4, 1, 4, 4, 258>,
) -> radio::BluetoothHardwareInterruptsRetired<4, 1, 4, 4, 258> {
    match owner.try_retire_interrupts() {
        Ok(owner) => owner,
        Err(failure) => {
            core::hint::black_box(&failure);
            crate::fail(c"GATT IRQ retirement failed\r\n")
        }
    }
}
#[inline(never)]
fn release(
    owner: radio::BluetoothHardwareInterruptsRetired<4, 1, 4, 4, 258>,
) -> radio::BluetoothHardwareOutputReleased<4, 1, 4, 4, 258> {
    match owner.try_release_controller_output() {
        Ok(owner) => owner,
        Err(failure) => {
            core::hint::black_box(&failure);
            crate::fail(c"GATT output release failed\r\n")
        }
    }
}

async fn old_hci_closed(hci: &radio::BluetoothHostController<4, 4, 258>) -> bool {
    use bt_hci::{
        cmd::{SyncCmd, controller_baseband::Reset},
        controller::Controller,
    };
    use embedded_io_async::{Error, ErrorKind};
    let commands = matches!(Reset::new().exec(hci).await, Err(bt_hci::cmd::Error::Io(error)) if error.kind() == ErrorKind::BrokenPipe);
    let mut buffer = hci.alloc_buf().expect("bounded HCI buffer");
    let events =
        matches!(hci.read(&mut buffer).await, Err(error) if error.kind() == ErrorKind::BrokenPipe);
    commands && events
}

async fn restart(
    cold: Cold,
    identity: oer_esp32s31_phy::PhyCalibrationIdentity,
) -> radio::BluetoothSystemReady<Platform, 4, 1, 4, 4, 258> {
    let mut restart = core::pin::pin!(cold.restart(
        oer_esp32s31_bluetooth::phy::PhyInitializationConfig::new(identity)
    ));
    match core::future::poll_fn(|cx| super::super::poll_live(restart.as_mut(), cx)).await {
        Ok(ready) => ready,
        Err(failure) => {
            core::hint::black_box(failure.error());
            crate::fail(c"GATT powered restart failed\r\n")
        }
    }
}
