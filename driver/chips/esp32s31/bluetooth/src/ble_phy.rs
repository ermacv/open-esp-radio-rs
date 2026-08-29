//! Owned BLE PHY engine activation after common PHY and BTBB initialization.

#[cfg(target_arch = "riscv32")]
use embassy_sync::blocking_mutex::raw::RawMutex;
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_bluetooth_memory::BluetoothBlePhyEngineCpuOwned;
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_hal::BluetoothModemLpTimerLowPowerHardwareInitializedOwner;
#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_esp32s31_hal::BluetoothPhyRegisterInitInputs;

#[cfg(target_arch = "riscv32")]
use crate::baseband::BluetoothControllerBasebandInitialized;
#[cfg(target_arch = "riscv32")]
use crate::resources::BluetoothInterruptBankOwner;

/// Source-owned values consumed by the finite BLE PHY register transaction.
///
/// Names remain positional where recovered code has not yet established a
/// stable hardware meaning. Keeping them explicit prevents one vendor build's
/// private configuration object from becoming an implicit ABI dependency.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothBlePhyInitializationConfig {
    private_configuration_byte_0x10: u8,
    option_byte_0x55_nonzero: bool,
    option_byte_0x59: u8,
}

impl BluetoothBlePhyInitializationConfig {
    /// Capture every non-address input consumed by the reviewed transaction.
    pub const fn new(
        private_configuration_byte_0x10: u8,
        option_byte_0x55_nonzero: bool,
        option_byte_0x59: u8,
    ) -> Self {
        Self {
            private_configuration_byte_0x10,
            option_byte_0x55_nonzero,
            option_byte_0x59,
        }
    }

    /// Return the still-positional byte read from private configuration `+0x10`.
    pub const fn private_configuration_byte_0x10(self) -> u8 {
        self.private_configuration_byte_0x10
    }

    /// Return whether the still-positional `+0x55` option selects its branch.
    pub const fn option_byte_0x55_nonzero(self) -> bool {
        self.option_byte_0x55_nonzero
    }

    /// Return the still-positional runtime byte read at `+0x59`.
    pub const fn option_byte_0x59(self) -> u8 {
        self.option_byte_0x59
    }
}

/// Value-only observation of the completed BLE PHY register transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothBlePhyInitializationReport {
    /// Exact source-owned configuration consumed by this Controller epoch.
    pub config: BluetoothBlePhyInitializationConfig,
}

/// Powered Controller after the complete BLE PHY register transaction.
///
/// The address-bound environment and resolving-list storage remain nested for
/// the full hardware epoch. This state does not claim that an interrupt route,
/// packet engine, Link Layer role, advertising set, scanner, connection, or
/// HCI dataplane is operational.
#[must_use = "initialized BLE PHY retains every hardware and storage owner"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerBlePhyEngineInitialized<
    P,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    initialized: BluetoothControllerBasebandInitialized<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    _storage: BluetoothBlePhyEngineCpuOwned,
    report: BluetoothBlePhyInitializationReport,
}

#[cfg(target_arch = "riscv32")]
impl<
    P,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerBlePhyEngineInitialized<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    /// Inspect the exact source-owned configuration used by this epoch.
    pub const fn report(&self) -> BluetoothBlePhyInitializationReport {
        self.report
    }

    /// Inspect the preceding finite BTBB transition.
    pub const fn baseband_report(&self) -> crate::BluetoothBasebandInitializationReport {
        self.initialized.baseband_report()
    }

    /// Inspect the complete common-PHY transition.
    pub const fn phy_report(&self) -> crate::BluetoothPhyInitializationReport {
        self.initialized.phy_report()
    }

    pub(crate) fn take_activation_owners(
        &mut self,
    ) -> (
        BluetoothInterruptBankOwner,
        BluetoothModemLpTimerLowPowerHardwareInitializedOwner,
    ) {
        let controller = &mut self.initialized.initialized.controller;
        let interrupts = controller.take_interrupt_owner();
        let timer = controller.take_timer_hardware();
        (interrupts, timer)
    }

    pub(crate) fn modem_lp_timer_software_parts_mut(
        &mut self,
    ) -> (
        &mut crate::BluetoothModemLpTimerQueue<MODEM_TIMER_CAPACITY>,
        &mut open_esp_radio_esp32s31_hal::BluetoothModemLpTimerEpoch,
        &crate::BluetoothModemLpTimerEventCell,
    ) {
        self.initialized
            .initialized
            .controller
            .modem_lp_timer_software_parts_mut()
    }
}

#[cfg(target_arch = "riscv32")]
impl<
    P,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerBasebandInitialized<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    /// Publish the recovered BLE PHY register transaction while consuming and
    /// retaining the complete address-bound allocation graph.
    ///
    /// The transition is fail-stop after its first MMIO write. It has no
    /// caller-supplied raw address and no escape that releases the storage
    /// while hardware may still retain either pointer.
    #[allow(
        unsafe_code,
        reason = "this affine state and consumed storage prove the narrow HAL bridge prerequisites"
    )]
    pub fn initialize_ble_phy_engine(
        mut self,
        storage: BluetoothBlePhyEngineCpuOwned,
        config: BluetoothBlePhyInitializationConfig,
    ) -> BluetoothControllerBlePhyEngineInitialized<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    > {
        let report = apply_register_init(&storage, config, |inputs| {
            // SAFETY: `self` retains the powered scheduler/HCI, low-power,
            // common-PHY and BTBB owners. `storage` is a consumed static owner
            // for the complete published allocation graph and is moved into
            // the return state.
            unsafe {
                self.initialized
                    .controller
                    .task_mut()
                    .initialize_ble_phy_registers(inputs);
            }
        });

        BluetoothControllerBlePhyEngineInitialized {
            initialized: self,
            _storage: storage,
            report,
        }
    }
}

#[cfg(any(target_arch = "riscv32", test))]
fn apply_register_init(
    storage: &open_esp_radio_esp32s31_bluetooth_memory::BluetoothBlePhyEngineCpuOwned,
    config: BluetoothBlePhyInitializationConfig,
    initialize: impl FnOnce(BluetoothPhyRegisterInitInputs),
) -> BluetoothBlePhyInitializationReport {
    let binding = storage.binding();
    initialize(BluetoothPhyRegisterInitInputs::new(
        config.private_configuration_byte_0x10,
        binding.environment_address(),
        binding.resolving_list_address(),
        config.option_byte_0x55_nonzero,
        config.option_byte_0x59,
    ));
    BluetoothBlePhyInitializationReport { config }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_bluetooth_memory::{
        BluetoothBlePhyEngineModelAddress, BluetoothBlePhyEngineStorage,
    };

    use super::{BluetoothBlePhyInitializationConfig, apply_register_init};

    #[test]
    fn register_transition_consumes_one_complete_input_set() {
        let storage =
            std::boxed::Box::leak(std::boxed::Box::new(BluetoothBlePhyEngineStorage::new()));
        let base = BluetoothBlePhyEngineModelAddress::new(0x2f00_0100)
            .expect("model base uses the controller-SRAM encoding");
        let owner = BluetoothBlePhyEngineStorage::pin_static_model(storage, base)
            .expect("complete model storage fits physical SRAM");
        let config = BluetoothBlePhyInitializationConfig::new(61, false, 0);
        let mut calls = 0;

        let report = apply_register_init(&owner, config, |_| calls += 1);

        assert_eq!(calls, 1);
        assert_eq!(report.config, config);
        assert!(owner.binding().range().0 < owner.binding().range().1);
    }
}
