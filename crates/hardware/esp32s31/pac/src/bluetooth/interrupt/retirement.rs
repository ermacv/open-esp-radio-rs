//! Terminal output release after independent task and CPU-route retirement.
//!
//! The idle preflight retains every hardware list pointer on rejection. The
//! accepted suffix composes the reviewed scheduler source mask/run-disable,
//! link-basic acknowledgement and controller-output release transactions.
//! It does not stop the modem counter or revoke PHY/RX/DF publications.

use super::*;
use crate::{BluetoothSchedulerHardwareListIndex, BluetoothTaskRegisters};

/// Hardware obligation preventing terminal Controller interrupt-output release.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothControllerOutputReleaseError {
    /// The task-side controller-time latch is still owned.
    ControllerTimePending,
    /// The scheduler still reports hardware execution.
    SchedulerBusy,
    /// A list still names hardware-visible memory; it is left unchanged.
    PublishedHead(BluetoothSchedulerHardwareListIndex),
    /// A fatal or unclassified primary source remains unacknowledged.
    InterruptFault,
}

trait Control {
    fn busy(&mut self) -> bool;
    fn head_present(&mut self, index: BluetoothSchedulerHardwareListIndex) -> bool;
    fn fault_pending(&mut self) -> bool;
    fn mask_dynamic(&mut self);
    fn disable_run(&mut self);
    fn fence(&mut self);
    fn clear_dynamic(&mut self);
    fn release_output(&mut self);
}

fn validate_idle(control: &mut impl Control) -> Result<(), BluetoothControllerOutputReleaseError> {
    use BluetoothControllerOutputReleaseError as Error;
    if control.busy() {
        return Err(Error::SchedulerBusy);
    }
    for index in 0..16 {
        let index = BluetoothSchedulerHardwareListIndex::new(index).expect("hardware list");
        if control.head_present(index) {
            return Err(Error::PublishedHead(index));
        }
    }
    if control.fault_pending() {
        return Err(Error::InterruptFault);
    }
    Ok(())
}

fn release(control: &mut impl Control) -> Result<(), BluetoothControllerOutputReleaseError> {
    use BluetoothControllerOutputReleaseError as Error;
    validate_idle(control)?;
    control.mask_dynamic();
    control.disable_run();
    control.fence();
    // Do not acknowledge a new fault or release output if execution changed
    // during the preflight. CPU routing stays disabled on every result.
    if control.busy() {
        return Err(Error::SchedulerBusy);
    }
    if control.fault_pending() {
        return Err(Error::InterruptFault);
    }
    control.clear_dynamic();
    control.release_output();
    control.fence();
    Ok(())
}

struct Hardware<'a> {
    task: &'a mut BluetoothTaskRegisters,
    output: &'a BluetoothInterruptOutputPrepared,
}

impl Control for Hardware<'_> {
    fn busy(&mut self) -> bool {
        field_read::observe_bluetooth_scheduler_software_list_busy(
            &self
                .output
                .peripherals
                .bluetooth_scheduler_interrupt_runtime,
        )
    }

    fn head_present(&mut self, index: BluetoothSchedulerHardwareListIndex) -> bool {
        self.task.scheduler_head_present(index)
    }

    fn fault_pending(&mut self) -> bool {
        let mut control = HardwareInterruptControl {
            bank: &self.output.peripherals.bluetooth_interrupt_bank,
        };
        let bank_0 = control.sample_bank_0();
        let bank_1 = control.sample_bank_1();
        let faults = BluetoothPrimaryFaultSources::from_status(
            control.bank_0_status(&bank_0),
            control.bank_1_status(&bank_1),
        );
        faults.is_fault()
    }

    fn mask_dynamic(&mut self) {
        let bank = &self.output.peripherals.bluetooth_interrupt_bank;
        crate::generated::mask_bluetooth_scheduler_run_interrupts_bank_0(bank);
        crate::generated::mask_bluetooth_scheduler_run_interrupts_bank_1(bank);
    }

    fn disable_run(&mut self) {
        crate::generated::disable_ble_scheduler_run_event_source(
            &self.task.bluetooth.btmac_ble_phy_init,
        );
    }

    fn fence(&mut self) {
        device_fence();
    }

    fn clear_dynamic(&mut self) {
        let mut control = HardwareInterruptControl {
            bank: &self.output.peripherals.bluetooth_interrupt_bank,
        };
        control.clear_scheduler_run_bank_0();
        control.clear_scheduler_run_bank_1();
    }

    fn release_output(&mut self) {
        execute_primary_release(&mut HardwareInterruptControl {
            bank: &self.output.peripherals.bluetooth_interrupt_bank,
        });
    }
}

impl BluetoothInterruptOutputPrepared {
    /// Observe idle admission without changing masks, heads or pending status.
    /// The caller retains the task and unrouted interrupt partitions throughout.
    pub fn validate_idle_controller(
        &self,
        task: &mut BluetoothTaskRegisters,
    ) -> Result<(), BluetoothControllerOutputReleaseError> {
        if task.controller_time_latch.in_flight() {
            return Err(BluetoothControllerOutputReleaseError::ControllerTimePending);
        }
        validate_idle(&mut Hardware { task, output: self })
    }

    /// Release output only for an idle, headless scheduler without primary faults.
    ///
    /// The lifecycle caller must own the retired command task and must have
    /// removed CPU routes before calling. Rejection retains both partitions;
    /// after the preflight it may leave dynamic sources masked and RUN disabled.
    /// No list head is erased to manufacture idle, and faults are never cleared.
    pub fn try_release_idle_controller_output(
        self,
        task: &mut BluetoothTaskRegisters,
    ) -> Result<BluetoothInterruptSetup, (BluetoothControllerOutputReleaseError, Self)> {
        if task.controller_time_latch.in_flight() {
            return Err((
                BluetoothControllerOutputReleaseError::ControllerTimePending,
                self,
            ));
        }
        if let Err(error) = release(&mut Hardware {
            task,
            output: &self,
        }) {
            return Err((error, self));
        }
        Ok(BluetoothInterruptSetup {
            peripherals: self.peripherals,
        })
    }
}

#[cfg(test)]
mod tests;
