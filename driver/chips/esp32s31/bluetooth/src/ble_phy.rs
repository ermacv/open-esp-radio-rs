//! Owned BLE PHY engine activation after common PHY and BTBB initialization.

#[cfg(target_arch = "riscv32")]
use embassy_sync::blocking_mutex::raw::RawMutex;
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothBlePhyEngineCpuOwned, BluetoothBlePhyLe1MPacketStartCalibration,
    BluetoothLePacketCapturedTime,
};
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_hal::BluetoothModemLpTimerLowPowerHardwareInitializedOwner;
#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_esp32s31_hal::BluetoothPhyRegisterInitInputs;

#[cfg(target_arch = "riscv32")]
use crate::baseband::BluetoothControllerBasebandInitialized;
#[cfg(target_arch = "riscv32")]
use crate::resources::BluetoothInterruptBankOwner;

/// Source-owned normal BLE PHY policy for the reviewed ESP32-S31 baseline.
///
/// The caller cannot override values recovered from the target's private and
/// public Controller configuration. Parameterized variants remain confined to
/// validation probes so production never acquires a vendor-object ABI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
struct BluetoothBlePhyInitializationProfile {
    private_timing_source_byte: u8,
    set_branch_control_0470_bit_18: bool,
    runtime_configuration_low_byte: u8,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothBlePhyInitializationProfile {
    const NORMAL: Self = Self {
        private_timing_source_byte: 61,
        set_branch_control_0470_bit_18: false,
        runtime_configuration_low_byte: 0x9c,
    };

    const fn register_inputs(
        self,
        storage: &open_esp_radio_esp32s31_bluetooth_memory::BluetoothBlePhyEngineCpuOwned,
    ) -> BluetoothPhyRegisterInitInputs {
        let binding = storage.binding();
        BluetoothPhyRegisterInitInputs::new(
            self.private_timing_source_byte,
            binding.environment_address(),
            binding.resolving_list_address(),
            self.set_branch_control_0470_bit_18,
            self.runtime_configuration_low_byte,
        )
    }
}

/// Observation that the source-owned normal BLE PHY transaction completed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothBlePhyInitializationReport;

/// One post-enable timing observation in the retained always-awake epoch.
///
/// Only a completed generation-keyed controller-time request owned by an
/// initialized BLE PHY can create this affine token. It deliberately cannot be
/// copied or manufactured from a detached scheduler image. It proves no RF
/// wake or analog readiness; the supported profile keeps those transitions
/// outside the DTM event path.
#[must_use = "the always-awake timing observation must be consumed by DTM scheduling"]
#[cfg(target_arch = "riscv32")]
pub(crate) struct BluetoothAlwaysAwakeTimingReady {
    scheduler_instant: crate::BluetoothSchedulerInstant,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothAlwaysAwakeTimingReady {
    fn from_completed_sample(
        epoch: crate::BluetoothControllerSchedulerEpoch,
        sample: crate::BluetoothControllerTimeSample,
    ) -> Self {
        Self {
            scheduler_instant: crate::BluetoothSchedulerInstant::from_image(
                epoch.project_without_reanchor(&sample),
            ),
        }
    }

    /// Consume the post-enable timing proof into its microsecond-domain instant.
    pub(crate) const fn into_scheduler_instant(self) -> crate::BluetoothSchedulerInstant {
        self.scheduler_instant
    }
}

/// Exclusive BLE-PHY authority for completing always-awake timing samples.
///
/// This capability is created only while splitting a fully initialized BLE-PHY
/// owner. It remains private inside the published task service, so a scheduler
/// or detached controller-time sample cannot manufacture this ordered timing
/// observation. The authority makes no RF-readiness claim.
#[must_use = "the timing authority must remain owned by the BLE-PHY task service"]
#[cfg(target_arch = "riscv32")]
pub(crate) struct BluetoothBlePhyTimingAuthority {
    le_1m_packet_start_calibration: BluetoothBlePhyLe1MPacketStartCalibration,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothBlePhyTimingAuthority {
    fn new(le_1m_packet_start_calibration: BluetoothBlePhyLe1MPacketStartCalibration) -> Self {
        Self {
            le_1m_packet_start_calibration,
        }
    }

    pub(crate) fn complete_always_awake(
        &mut self,
        epoch: crate::BluetoothControllerSchedulerEpoch,
        sample: crate::BluetoothControllerTimeSample,
    ) -> BluetoothAlwaysAwakeTimingReady {
        BluetoothAlwaysAwakeTimingReady::from_completed_sample(epoch, sample)
    }

    pub(crate) fn complete_le_1m_packet_start(
        &mut self,
        epoch: crate::BluetoothControllerSchedulerEpoch,
        captured: BluetoothLePacketCapturedTime,
    ) -> crate::BluetoothLe1MPacketStartTiming {
        let captured_micros = epoch.project_le_packet_capture(captured);
        crate::BluetoothLe1MPacketStartTiming::from_scheduler_micros(
            self.le_1m_packet_start_calibration
                .normalize_controller_micros(captured_micros),
        )
    }
}

/// Powered Controller after the complete BLE PHY register transaction.
///
/// The address-bound environment and resolving-list storage remain nested for
/// the full hardware epoch together with the private affine standalone
/// always-awake profile selection. That selection performs no RF MMIO and is
/// not an RF-ready or completed-time proof. This state does not claim that an
/// interrupt route, packet engine, Link Layer role, advertising set, scanner,
/// connection, or HCI dataplane is operational.
#[must_use = "initialized BLE PHY retains every hardware and storage owner"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerBlePhyEngineInitialized<
    P,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
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
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    storage: BluetoothBlePhyEngineCpuOwned,
    report: BluetoothBlePhyInitializationReport,
}

#[cfg(target_arch = "riscv32")]
impl<
    P,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerBlePhyEngineInitialized<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    /// Observe completion of the source-owned normal BLE PHY transaction.
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

    /// Split the already initialized scheduler and HCI resources after this
    /// complete BLE-PHY owner has reached stable final placement.
    pub(crate) fn split_runtime(
        &mut self,
    ) -> (
        crate::BluetoothControllerRuntimeEndpoints<
            '_,
            M,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        BluetoothBlePhyTimingAuthority,
    ) {
        let calibration = self.storage.le_1m_packet_start_calibration();
        (
            self.initialized.initialized.controller.split_runtime(),
            BluetoothBlePhyTimingAuthority::new(calibration),
        )
    }
}

#[cfg(target_arch = "riscv32")]
impl<
    P,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerBasebandInitialized<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
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
    ) -> BluetoothControllerBlePhyEngineInitialized<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    > {
        let report = apply_register_init(&storage, |inputs| {
            // SAFETY: `self` retains the powered scheduler/HCI, low-power,
            // common-PHY and BTBB owners. `storage` is a consumed static owner
            // for the complete published allocation graph and is moved into
            // the return state.
            unsafe {
                self.initialized
                    .controller
                    .task_mut()
                    .enable_ble_base_stack_hardware(inputs);
            }
        });

        BluetoothControllerBlePhyEngineInitialized {
            initialized: self,
            storage,
            report,
        }
    }
}

#[cfg(any(target_arch = "riscv32", test))]
fn apply_register_init(
    storage: &open_esp_radio_esp32s31_bluetooth_memory::BluetoothBlePhyEngineCpuOwned,
    initialize: impl FnOnce(BluetoothPhyRegisterInitInputs),
) -> BluetoothBlePhyInitializationReport {
    initialize(BluetoothBlePhyInitializationProfile::NORMAL.register_inputs(storage));
    BluetoothBlePhyInitializationReport
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_bluetooth_memory::{
        BluetoothBlePhyEngineModelAddress, BluetoothBlePhyEngineStorage,
    };

    use super::{BluetoothBlePhyInitializationReport, apply_register_init};

    #[test]
    fn register_transition_consumes_one_complete_input_set() {
        let storage =
            std::boxed::Box::leak(std::boxed::Box::new(BluetoothBlePhyEngineStorage::new()));
        let base = BluetoothBlePhyEngineModelAddress::new(0x2f00_0100)
            .expect("model base uses the controller-SRAM encoding");
        let owner = BluetoothBlePhyEngineStorage::pin_static_model(storage, base)
            .expect("complete model storage fits physical SRAM");
        let mut calls = 0;

        let report = apply_register_init(&owner, |_| calls += 1);

        assert_eq!(calls, 1);
        assert_eq!(report, BluetoothBlePhyInitializationReport);
        assert!(owner.binding().range().0 < owner.binding().range().1);
    }
}
