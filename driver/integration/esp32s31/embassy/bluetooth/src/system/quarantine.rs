//! Complete-route shutdown and indefinite retention of terminal affine owners.

use open_esp_radio_esp32s31_bluetooth_embassy::EmbassyBluetoothControllerCommandTask;

use super::{CommandBoundary, ModemDriveStep, PublishedStorage};
use crate::{
    Esp32s31BluetoothInterruptDisableFailure, Esp32s31BluetoothInterruptFault,
    Esp32s31BluetoothInterruptRuntime,
};

/// Result of disabling all three routes for terminal quarantine.
pub(super) enum Esp32s31BluetoothRouteQuarantine {
    /// Terminal quarantine disabled source 124, 127 and 133 together.
    Disabled,
    /// Full-route disable was rejected; quarantine owns the unchanged live epoch.
    DisableRejected {
        _failure: Esp32s31BluetoothInterruptDisableFailure,
    },
}

/// Terminal owner retained forever after complete-route quarantine.
#[expect(
    clippy::large_enum_variant,
    reason = "no-alloc quarantine retains exact affine lower owners"
)]
pub(super) enum Esp32s31BluetoothHardwareQuarantine<'packet, const SCHEDULER_CAPACITY: usize> {
    Command {
        _boundary: CommandBoundary<'packet, SCHEDULER_CAPACITY>,
        _actor:
            EmbassyBluetoothControllerCommandTask<'static, PublishedStorage, SCHEDULER_CAPACITY>,
        _routes: Esp32s31BluetoothRouteQuarantine,
    },
    ModemTimer {
        _step: ModemDriveStep,
        _routes: Esp32s31BluetoothRouteQuarantine,
    },
    InterruptFault {
        _fault: Esp32s31BluetoothInterruptFault,
        _routes: Esp32s31BluetoothRouteQuarantine,
    },
    ControllerTimeExhausted {
        _routes: Esp32s31BluetoothRouteQuarantine,
    },
}

pub(super) async fn retain_quarantine_forever<T>(_quarantine: T) -> ! {
    core::future::pending().await
}

pub(super) fn quarantine_routes(
    interrupt: &mut Option<Esp32s31BluetoothInterruptRuntime>,
) -> Esp32s31BluetoothRouteQuarantine {
    let runtime = interrupt
        .take()
        .expect("terminal quarantine starts from one live route epoch");
    match runtime.disable() {
        Ok(()) => Esp32s31BluetoothRouteQuarantine::Disabled,
        Err(failure) => Esp32s31BluetoothRouteQuarantine::DisableRejected { _failure: failure },
    }
}
