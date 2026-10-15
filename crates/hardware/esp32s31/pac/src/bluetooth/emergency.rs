//! Restricted emergency views, serialized by the HAL installation admission.

use crate::{BluetoothTaskRegisters, device_fence};

/// Only the BLE task and interrupt registers touched by the scheduler stop.
/// This owner exposes no shared PHY, coexistence, clock or reset operation.
#[must_use = "retain the emergency register authority with its installed task"]
pub struct BluetoothEmergencyRegisters {
    controller: crate::svd::BluetoothControllerCore,
    run_event: crate::svd::BtmacBlePhyInit,
    interrupts: crate::svd::BluetoothInterruptBank,
    scheduler: crate::svd::BluetoothSchedulerInterruptRuntime,
}

impl BluetoothTaskRegisters {
    /// Create the emergency views while retaining the ordinary task owner.
    ///
    /// # Safety
    ///
    /// The caller must issue exactly one view per live task epoch, hide the
    /// ordinary task behind installation admission, and close and drain that
    /// admission before using the emergency views. The ordinary owner cannot
    /// be extracted until this view is consumed; after emergency closure no
    /// caller may reactivate it. All hardware-visible memory remains durable.
    #[allow(
        unsafe_code,
        reason = "two views have mutually exclusive installation admission"
    )]
    pub unsafe fn emergency_registers(&mut self) -> BluetoothEmergencyRegisters {
        // SAFETY: the caller retains the original unique owners and guarantees
        // that the emergency views are used only after ordinary admission has
        // permanently closed and all previously admitted accesses have left.
        let (controller, run_event, interrupts, scheduler) = unsafe {
            (
                crate::svd::BluetoothControllerCore::steal(),
                crate::svd::BtmacBlePhyInit::steal(),
                crate::svd::BluetoothInterruptBank::steal(),
                crate::svd::BluetoothSchedulerInterruptRuntime::steal(),
            )
        };
        BluetoothEmergencyRegisters {
            controller,
            run_event,
            interrupts,
            scheduler,
        }
    }
}

impl BluetoothEmergencyRegisters {
    /// Mask the reviewed dynamic groups, disable RUN events, then fence.
    pub fn preamble(&mut self) {
        let bank = &self.interrupts;
        crate::generated::mask_bluetooth_scheduler_run_interrupts_bank_0(bank);
        crate::generated::mask_bluetooth_scheduler_run_interrupts_bank_1(bank);
        crate::generated::disable_ble_scheduler_run_event_source(&self.run_event);
        device_fence();
    }

    /// One diagnostic pair, selected through the restricted interrupt view.
    /// The HAL bounds retries when the two samples disagree.
    pub fn scheduler_busy(&mut self) -> Option<bool> {
        crate::bluetooth::scheduler::runtime::sample_scheduler_busy(&self.scheduler)
    }

    /// Read command-zero status first; read command-one only if zero is ready.
    pub fn commands_ready(&mut self) -> bool {
        crate::svd::field_read::observe_bluetooth_scheduler_software_list_command_0_status_26(
            &self.controller,
        ) && crate::svd::field_read::observe_bluetooth_scheduler_software_list_command_1_status_18(
            &self.controller,
        )
    }

    /// Publish the source-owned lifecycle image and order device accesses.
    pub fn request(&mut self) {
        crate::svd::fixed_register_image::publish_bluetooth_scheduler_lifecycle_request(
            &self.controller,
        );
        device_fence();
    }

    /// Order the final stable BUSY-clear observation.
    pub fn fence(&mut self) {
        device_fence();
    }
}
