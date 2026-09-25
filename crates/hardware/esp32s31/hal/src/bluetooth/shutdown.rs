//! Final Bluetooth reset, timer reunion and shared power-baseline restoration.
//!
//! Caller admission retains the idle task, released output, drained source-127
//! owner and completed RF close. Reset uses the same reviewed S31
//! `modem_clock_module_mac_reset(PERIPH_BT_MODULE)` domain transaction as boot
//! and `btdm_lp_shutdown`; no synthetic runtime-counter stop register is used.

use oer_esp32s31_pac::{
    BluetoothInterruptSetup, BluetoothModemLpTimerInterruptReady, BluetoothTaskRegisters,
};

use super::ColdOwner;
use crate::{
    clock::BluetoothClocks,
    root::{RadioHardware, RadioPhyReleaseError, RetainedWifi},
};

/// Final physical Bluetooth release rejection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothPhysicalReleaseError {
    ControllerTimePending,
    TimerOwnerPresent,
    ControllerReset,
    Radio(RadioPhyReleaseError),
}

enum Retained {
    Before {
        _task: BluetoothTaskRegisters,
        _retained: RetainedWifi,
        _clocks: BluetoothClocks,
        _output: BluetoothInterruptSetup,
        _timer: BluetoothModemLpTimerInterruptReady,
    },
    Cold {
        _owner: ColdOwner,
    },
}

/// Failed physical release keeps all original hardware partitions private.
#[must_use = "failed physical release retains the complete radio"]
pub struct BluetoothPhysicalReleaseFailure {
    error: BluetoothPhysicalReleaseError,
    _retained: Retained,
}

impl BluetoothPhysicalReleaseFailure {
    pub const fn error(&self) -> BluetoothPhysicalReleaseError {
        self.error
    }
}

trait Control {
    fn time_pending(&self) -> bool;
    fn timer_separated(&self) -> bool;
    fn disable_compare(&mut self);
    fn reset_controller(&mut self);
    fn reset_released(&self) -> bool;
}

fn prepare(control: &mut impl Control) -> Result<(), BluetoothPhysicalReleaseError> {
    if control.time_pending() {
        return Err(BluetoothPhysicalReleaseError::ControllerTimePending);
    }
    if !control.timer_separated() {
        return Err(BluetoothPhysicalReleaseError::TimerOwnerPresent);
    }
    control.disable_compare();
    control.reset_controller();
    if !control.reset_released() {
        return Err(BluetoothPhysicalReleaseError::ControllerReset);
    }
    Ok(())
}

struct Hardware<'a> {
    task: &'a mut BluetoothTaskRegisters,
    timer: &'a mut BluetoothModemLpTimerInterruptReady,
}

impl Control for Hardware<'_> {
    fn time_pending(&self) -> bool {
        self.task.controller_time_latch_in_flight()
    }
    fn timer_separated(&self) -> bool {
        self.task.modem_lp_timer_separated()
    }
    fn disable_compare(&mut self) {
        self.timer.disable_for_shutdown();
    }
    fn reset_controller(&mut self) {
        self.task.reset_controller_domains();
    }
    fn reset_released(&self) -> bool {
        self.task.controller_resets_released()
    }
}

/// Reunite the drained timer, reset Bluetooth and return the neutral root.
///
/// The outer lifecycle must have released Controller output and completed
/// last-client RF close and temperature-sensor power-down. All memory stays
/// retained until this operation completes. Failure has no hardware escape.
pub(super) fn release_after_phy_close(
    mut task: BluetoothTaskRegisters,
    retained: RetainedWifi,
    clocks: BluetoothClocks,
    output: BluetoothInterruptSetup,
    mut timer: BluetoothModemLpTimerInterruptReady,
) -> Result<RadioHardware, BluetoothPhysicalReleaseFailure> {
    if let Err(error) = prepare(&mut Hardware {
        task: &mut task,
        timer: &mut timer,
    }) {
        return Err(BluetoothPhysicalReleaseFailure {
            error,
            _retained: Retained::Before {
                _task: task,
                _retained: retained,
                _clocks: clocks,
                _output: output,
                _timer: timer,
            },
        });
    }
    task.restore_modem_lp_timer(timer.into_shutdown_registers());
    let cold = ColdOwner {
        task,
        interrupts: output,
        retained,
        clocks,
    };
    cold.release().map_err(|failure| {
        let (owner, error) = failure.into_parts();
        BluetoothPhysicalReleaseFailure {
            error: BluetoothPhysicalReleaseError::Radio(error),
            _retained: Retained::Cold { _owner: owner },
        }
    })
}

#[cfg(test)]
mod tests;
