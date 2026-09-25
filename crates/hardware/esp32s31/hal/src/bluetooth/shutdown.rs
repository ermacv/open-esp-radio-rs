//! Final Bluetooth reset, timer reunion and shared power-baseline restoration.
//!
//! Caller admission retains the idle task, released output, drained source-127
//! owner and completed RF close. Reset uses the same reviewed S31
//! `modem_clock_module_mac_reset(PERIPH_BT_MODULE)` domain transaction as boot
//! and `btdm_lp_shutdown`; no synthetic runtime-counter stop register is used.

use oer_esp32s31_pac::{BluetoothInterruptSetup, BluetoothModemLpTimerRegisters};

use super::{ColdOwner, TaskOwner};
use crate::root::{RadioHardware, RadioPhyReleaseError};

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
        _task: TaskOwner,
        _output: BluetoothInterruptSetup,
        _timer: BluetoothModemLpTimerRegisters,
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
    task: &'a mut TaskOwner,
    timer: &'a mut BluetoothModemLpTimerRegisters,
}

impl Control for Hardware<'_> {
    fn time_pending(&self) -> bool {
        self.task.registers.controller_time_latch_in_flight()
    }
    fn timer_separated(&self) -> bool {
        self.task.modem_lp_timer.is_none()
    }
    fn disable_compare(&mut self) {
        // The outer retired Controller owns inactive CPU routes and a drained
        // software queue. Compare is disabled before the timer domain reset;
        // physical counter shutdown follows from reset and clock release.
        self.timer.disable_compare();
    }
    fn reset_controller(&mut self) {
        self.task.registers.reset_controller_domains();
    }
    fn reset_released(&self) -> bool {
        self.task.registers.controller_resets_released()
    }
}

/// Reunite the drained timer, reset Bluetooth and return the neutral root.
///
/// The outer lifecycle must have released Controller output and completed
/// last-client RF close and temperature-sensor power-down. All memory stays
/// retained until this operation completes. Failure has no hardware escape.
pub(super) fn release_after_phy_close(
    mut task: TaskOwner,
    output: BluetoothInterruptSetup,
    mut timer: BluetoothModemLpTimerRegisters,
) -> Result<RadioHardware, BluetoothPhysicalReleaseFailure> {
    if let Err(error) = prepare(&mut Hardware {
        task: &mut task,
        timer: &mut timer,
    }) {
        return Err(BluetoothPhysicalReleaseFailure {
            error,
            _retained: Retained::Before {
                _task: task,
                _output: output,
                _timer: timer,
            },
        });
    }
    let TaskOwner {
        registers,
        modem_lp_timer: _,
        retained,
        clocks,
        phy_restore,
        reunitable: _,
    } = task;
    let cold = ColdOwner {
        task: registers,
        modem_lp_timer: timer,
        interrupts: output,
        retained,
        clocks,
        phy_restore,
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
