//! Complete-route shutdown and indefinite retention of terminal affine owners.

use crate::{BluetoothInterruptDisableFailure, BluetoothInterruptFault, BluetoothInterruptRuntime};

use oer_esp32s31_bluetooth_embassy::controller::ControllerCommandTask;

use super::{CommandBoundary, ModemDriveStep, PublishedStorage};

/// Result of disabling all three routes for terminal quarantine.
pub(super) enum BluetoothRouteQuarantine {
    /// Terminal quarantine disabled source 124, 127 and 133 together.
    Disabled,
    /// Full-route disable was rejected; quarantine owns the unchanged live epoch.
    DisableRejected {
        _failure: BluetoothInterruptDisableFailure,
    },
}

/// Terminal owner retained forever after complete-route quarantine.
#[expect(
    clippy::large_enum_variant,
    reason = "no-alloc quarantine retains exact affine lower owners"
)]
pub(super) enum BluetoothHardwareQuarantine<'packet, const SCHEDULER_CAPACITY: usize> {
    Command {
        _boundary: CommandBoundary<'packet, SCHEDULER_CAPACITY>,
        _actor: ControllerCommandTask<'static, PublishedStorage, SCHEDULER_CAPACITY>,
        _routes: BluetoothRouteQuarantine,
    },
    ModemTimer {
        _step: ModemDriveStep,
        _routes: BluetoothRouteQuarantine,
    },
    InterruptFault {
        _fault: BluetoothInterruptFault,
        _routes: BluetoothRouteQuarantine,
    },
    ControllerTimeExhausted {
        _routes: BluetoothRouteQuarantine,
    },
}

pub(super) async fn retain_quarantine_forever<T>(_quarantine: T) -> ! {
    core::future::pending().await
}

pub(super) fn quarantine_routes(
    interrupt: &mut Option<BluetoothInterruptRuntime>,
) -> BluetoothRouteQuarantine {
    let runtime = interrupt
        .take()
        .expect("terminal quarantine starts from one live route epoch");
    match runtime.disable() {
        Ok(()) => BluetoothRouteQuarantine::Disabled,
        Err(failure) => BluetoothRouteQuarantine::DisableRejected { _failure: failure },
    }
}
