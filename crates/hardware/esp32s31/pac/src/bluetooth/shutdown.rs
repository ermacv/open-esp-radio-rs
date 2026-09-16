//! Final Bluetooth reset, timer reunion and shared power-baseline restoration.
//!
//! Caller admission retains the idle task, released output, drained source-127
//! owner and completed RF close. Reset uses the same reviewed S31
//! `modem_clock_module_mac_reset(PERIPH_BT_MODULE)` domain transaction as boot
//! and `btdm_lp_shutdown`; no synthetic runtime-counter stop register is used.

use crate::{
    BluetoothColdRegisters, BluetoothInterruptSetup, BluetoothModemLpTimerInterruptReady,
    BluetoothTaskRegisters, RadioHardware, RadioPhyReleaseError,
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
        _output: BluetoothInterruptSetup,
        _timer: BluetoothModemLpTimerInterruptReady,
    },
    Cold {
        _owner: BluetoothColdRegisters,
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
        self.task.controller_time_latch.in_flight()
    }
    fn timer_separated(&self) -> bool {
        self.task.modem_lp_timer.is_none()
    }
    fn disable_compare(&mut self) {
        self.timer.disable_for_shutdown();
    }
    fn reset_controller(&mut self) {
        self.task.radio_phy.reset_bluetooth_controller_domains();
        crate::device_fence();
    }
    fn reset_released(&self) -> bool {
        self.task
            .radio_phy
            .bluetooth_clock_observation()
            .controller_resets_released
    }
}

impl BluetoothTaskRegisters {
    /// Reunite the drained timer, reset Bluetooth and return the neutral root.
    ///
    /// The outer lifecycle must have released Controller output and completed
    /// last-client RF close and temperature-sensor power-down. All memory stays
    /// retained until this operation completes. Failure has no hardware escape.
    pub fn release_after_phy_close(
        mut self,
        output: BluetoothInterruptSetup,
        mut timer: BluetoothModemLpTimerInterruptReady,
    ) -> Result<RadioHardware, BluetoothPhysicalReleaseFailure> {
        if let Err(error) = prepare(&mut Hardware {
            task: &mut self,
            timer: &mut timer,
        }) {
            return Err(BluetoothPhysicalReleaseFailure {
                error,
                _retained: Retained::Before {
                    _task: self,
                    _output: output,
                    _timer: timer,
                },
            });
        }
        self.modem_lp_timer = Some(timer.into_shutdown_registers());
        let cold = BluetoothColdRegisters {
            task: self,
            interrupts: output,
        };
        cold.release().map_err(|failure| {
            let (owner, error) = failure.into_parts();
            BluetoothPhysicalReleaseFailure {
                error: BluetoothPhysicalReleaseError::Radio(error),
                _retained: Retained::Cold { _owner: owner },
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Model {
        pending: bool,
        separated: bool,
        reset_ok: bool,
        compare_enabled: bool,
        counter_running: bool,
        reset_count: usize,
    }
    impl Control for Model {
        fn time_pending(&self) -> bool {
            self.pending
        }
        fn timer_separated(&self) -> bool {
            self.separated
        }
        fn disable_compare(&mut self) {
            self.compare_enabled = false;
        }
        fn reset_controller(&mut self) {
            assert!(
                !self.compare_enabled,
                "compare must be disabled before reset"
            );
            self.reset_count += 1;
            if self.reset_ok {
                self.counter_running = false;
            }
        }
        fn reset_released(&self) -> bool {
            self.reset_ok
        }
    }
    #[test]
    fn outstanding_time_and_missing_timer_partition_prevent_shutdown_mutation() {
        for (pending, separated, error) in [
            (
                true,
                true,
                BluetoothPhysicalReleaseError::ControllerTimePending,
            ),
            (
                false,
                false,
                BluetoothPhysicalReleaseError::TimerOwnerPresent,
            ),
        ] {
            let mut model = Model {
                pending,
                separated,
                reset_ok: true,
                compare_enabled: true,
                counter_running: true,
                reset_count: 0,
            };
            assert_eq!(prepare(&mut model), Err(error));
            assert!(model.compare_enabled && model.counter_running);
            assert_eq!(model.reset_count, 0);
        }
    }
    #[test]
    fn reset_readback_is_required_before_returning_hardware_ownership() {
        for reset_ok in [false, true] {
            let mut model = Model {
                pending: false,
                separated: true,
                reset_ok,
                compare_enabled: true,
                counter_running: true,
                reset_count: 0,
            };
            assert_eq!(
                prepare(&mut model),
                if reset_ok {
                    Ok(())
                } else {
                    Err(BluetoothPhysicalReleaseError::ControllerReset)
                }
            );
            assert!(!model.compare_enabled);
            assert_eq!(model.reset_count, 1);
            assert_eq!(model.counter_running, !reset_ok);
        }
    }
}
