//! Owned controller HAL initialization immediately after clock setup.
//!
//! The complete reviewed `r_btdm_task_init` hardware subsequence executes the
//! 50-operation controller HAL component before scheduler initialization.
//! Vendor task-environment and broker setup are software architecture and are
//! replaced by Rust-owned runtime resources rather than copied into this
//! hardware typestate.

use open_esp_radio_esp32s31_pac::{BluetoothControllerHalInitConfig, BluetoothControllerTimeScale};

use crate::{
    BluetoothClockedResources,
    resources::{
        BluetoothInterruptBankOwner, BluetoothTaskResources, BluetoothTeardownPendingPlatform,
        separate_interrupt_owner,
    },
};

/// Affine selection of the source-owned standalone always-awake profile.
///
/// The marker records only that this Controller epoch was constructed without
/// a modem-sleep/wake policy. It performs no MMIO and proves neither that RF
/// is ready nor that a controller-time request has completed. Its private
/// field keeps construction in the clocked-to-controller-HAL transition, and
/// the absence of `Copy` or `Clone` keeps the selection bound to one epoch.
#[must_use = "the standalone always-awake selection belongs to one Controller epoch"]
pub(crate) struct BluetoothStandaloneAlwaysAwake {
    _private: (),
}

impl BluetoothStandaloneAlwaysAwake {
    const fn mint() -> Self {
        Self { _private: () }
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) const fn gate_controller_time_request(&self) {
        // Presence of this private affine value in the structural owner is the
        // gate. The marker never accepts an independently supplied task owner.
    }
}

/// Bluetooth hardware after the complete controller HAL-init component.
///
/// This state proves only the reviewed MMIO component and retains its exact
/// scheduler time scale. It does not claim initialized scheduler lists,
/// interrupts, PHY, BTBB, Link Layer, HCI or a running controller. Dropping it
/// is fail-stop because no verified rollback exists after the first write.
#[must_use = "the controller HAL state retains every powered Bluetooth owner"]
pub struct BluetoothControllerHalInitialized<P> {
    pub(crate) task: BluetoothTaskResources,
    pub(crate) interrupts: BluetoothInterruptBankOwner,
    pub(crate) platform: BluetoothTeardownPendingPlatform<P>,
    pub(crate) time_scale: BluetoothControllerTimeScale,
    pub(crate) standalone_always_awake: BluetoothStandaloneAlwaysAwake,
}

impl<P> BluetoothControllerHalInitialized<P> {
    /// Return the scheduler scale established for this hardware epoch.
    pub const fn controller_time_scale(&self) -> BluetoothControllerTimeScale {
        self.time_scale
    }
}

impl<P> BluetoothClockedResources<P> {
    /// Execute the complete reviewed standalone controller HAL component.
    ///
    /// This consumes the reversible clock state and arms fail-stop ownership
    /// before the first controller write. The result must continue through
    /// scheduler and interrupt initialization before it can run radio work.
    #[cfg(target_arch = "riscv32")]
    #[allow(
        unsafe_code,
        reason = "the affine clocked state discharges the HAL transaction's external prerequisites"
    )]
    pub fn initialize_controller_hal(self) -> BluetoothControllerHalInitialized<P> {
        self.into_controller_hal_initialized(|task, config| {
            // SAFETY: `BluetoothClockedResources` retains the exact enabled
            // clock/reset owner, this transition uniquely owns the task and
            // inactive interrupt partitions, and the fixed S31 SRAM-prefix
            // profile is retained by the returned affine state.
            unsafe {
                task.initialize_controller_hal(config);
            }
        })
    }

    #[cfg(test)]
    pub(crate) fn initialize_controller_hal_with(
        self,
        initialize: impl FnOnce(&mut BluetoothTaskResources, BluetoothControllerHalInitConfig),
    ) -> BluetoothControllerHalInitialized<P> {
        self.into_controller_hal_initialized(initialize)
    }

    fn into_controller_hal_initialized(
        self,
        initialize: impl FnOnce(&mut BluetoothTaskResources, BluetoothControllerHalInitConfig),
    ) -> BluetoothControllerHalInitialized<P> {
        let config = BluetoothControllerHalInitConfig::reviewed_standalone();
        let standalone_always_awake = BluetoothStandaloneAlwaysAwake::mint();
        let time_scale = config.controller_time_scale();
        let (registers, platform) = self.into_parts();
        // Arm fail-stop ownership before the first controller MMIO mutation.
        let platform = BluetoothTeardownPendingPlatform::new(platform);
        let (mut task, interrupts) = separate_interrupt_owner(registers);
        initialize(&mut task, config);
        BluetoothControllerHalInitialized {
            task,
            interrupts,
            platform,
            time_scale,
            standalone_always_awake,
        }
    }
}
