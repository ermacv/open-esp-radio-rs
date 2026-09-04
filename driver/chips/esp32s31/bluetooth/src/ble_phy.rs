//! Owned BLE PHY engine activation after common PHY and BTBB initialization.

#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_bluetooth_hci::BluetoothPublicDeviceAddress;
#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_esp32s31_bluetooth_memory::BluetoothBlePhyLe1MPacketStartCalibration;
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothBlePhyEngineCpuOwned, BluetoothDirectionFindingWorkspaceCpuOwned,
    BluetoothDirectionFindingWorkspaceLink, BluetoothLePacketCapturedTime,
    BluetoothPeripheralConnectionCapturedAnchorTime,
};
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_hal::BluetoothModemLpTimerLowPowerHardwareInitializedOwner;
#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_esp32s31_hal::{
    BluetoothControllerPublicAddress, BluetoothPhyRegisterInitInputs,
};

#[cfg(target_arch = "riscv32")]
use crate::baseband::BluetoothControllerBasebandInitialized;
#[cfg(target_arch = "riscv32")]
use crate::resources::BluetoothInterruptBankOwner;

/// Controller-global DF storage after its disabled-CTE descriptor is visible to hardware.
///
/// The memory and exact PAC publication proof remain joined for the complete
/// powered epoch. Only an opaque ordinary-role link can be copied out; CPU
/// access and raw descriptor images remain inaccessible.
#[must_use = "hardware-owned direction-finding storage must remain retained"]
#[cfg(target_arch = "riscv32")]
pub(crate) struct BluetoothDirectionFindingWorkspaceHardwareOwned {
    storage: BluetoothDirectionFindingWorkspaceCpuOwned,
    _publication: open_esp_radio_esp32s31_hal::BluetoothDirectionFindingDisabledBaselineOwner,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothDirectionFindingWorkspaceHardwareOwned {
    const fn link(&self) -> BluetoothDirectionFindingWorkspaceLink {
        self.storage.binding().link()
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

    pub(crate) fn complete_le_1m_peripheral_connection_packet_start(
        &mut self,
        epoch: crate::BluetoothControllerSchedulerEpoch,
        captured: BluetoothPeripheralConnectionCapturedAnchorTime,
    ) -> crate::peripheral_connection::BluetoothPeripheralConnectionPacketStartTiming {
        let captured_micros = epoch.project_peripheral_connection_capture(captured);
        normalize_le_1m_peripheral_connection_packet_start(
            self.le_1m_packet_start_calibration,
            captured_micros,
        )
    }
}

#[cfg(any(target_arch = "riscv32", test))]
fn normalize_le_1m_peripheral_connection_packet_start(
    calibration: BluetoothBlePhyLe1MPacketStartCalibration,
    captured_micros: u32,
) -> crate::peripheral_connection::BluetoothPeripheralConnectionPacketStartTiming {
    crate::peripheral_connection::BluetoothPeripheralConnectionPacketStartTiming::from_scheduler_micros(
        calibration.normalize_controller_micros(captured_micros),
    )
}

/// Powered Controller after BLE PHY init and public-address publication.
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
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
> {
    initialized:
        BluetoothControllerBasebandInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
    storage: BluetoothBlePhyEngineCpuOwned,
    direction_finding: BluetoothDirectionFindingWorkspaceHardwareOwned,
    report: BluetoothBlePhyInitializationReport,
}

#[cfg(target_arch = "riscv32")]
impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerBlePhyEngineInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
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

    /// Opaque ordinary-role link into this powered epoch's global DF workspace.
    pub(crate) const fn direction_finding_workspace_link(
        &self,
    ) -> BluetoothDirectionFindingWorkspaceLink {
        self.direction_finding.link()
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

    /// Split the already initialized hardware runtime after this
    /// complete BLE-PHY owner has reached stable final placement.
    pub(crate) fn split_runtime(
        &mut self,
    ) -> (
        crate::BluetoothControllerRuntimeEndpoints<'_, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
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
impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerBasebandInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    /// Publish the recovered BLE PHY register transaction while consuming and
    /// retaining the complete address-bound allocation graph.
    ///
    /// The transition is fail-stop after its first MMIO write. It has no
    /// caller-supplied raw address and no escape that releases the storage
    /// while hardware may still retain either pointer. `public_address`
    /// remains in canonical display order at this boundary; only the HAL owns
    /// conversion to Controller wire order. The returned state therefore
    /// proves both BLE PHY completion and public-address readiness.
    #[allow(
        unsafe_code,
        reason = "this affine state and consumed storage prove the narrow HAL bridge prerequisites"
    )]
    pub fn initialize_ble_phy_engine(
        mut self,
        storage: BluetoothBlePhyEngineCpuOwned,
        direction_finding: BluetoothDirectionFindingWorkspaceCpuOwned,
        public_address: BluetoothPublicDeviceAddress,
    ) -> BluetoothControllerBlePhyEngineInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
    {
        let controller = &mut self.initialized.controller;
        let report = apply_register_init_then_public_address(
            &storage,
            public_address,
            controller,
            |controller, inputs| {
                // SAFETY: `self` retains the powered scheduler, low-power,
                // common-PHY and BTBB owners. `storage` is a consumed static
                // owner for the complete published allocation graph and is
                // moved into the return state.
                unsafe {
                    controller.task_mut().enable_ble_base_stack_hardware(inputs);
                }
            },
            |controller, address| {
                controller.task_mut().program_public_device_address(address);
            },
        );
        let descriptor = direction_finding
            .binding()
            .disabled_cte_descriptor_address();
        // SAFETY: the complete powered Controller remains in `self`; the
        // initialized static workspace is consumed into the returned state,
        // which retains it together with the exact PAC publication proof.
        let publication = unsafe {
            self.initialized
                .controller
                .task_mut()
                .prepare_direction_finding_disabled_baseline(descriptor)
        };

        BluetoothControllerBlePhyEngineInitialized {
            initialized: self,
            storage,
            direction_finding: BluetoothDirectionFindingWorkspaceHardwareOwned {
                storage: direction_finding,
                _publication: publication,
            },
            report,
        }
    }
}

#[cfg(any(target_arch = "riscv32", test))]
fn apply_register_init_then_public_address<T>(
    storage: &open_esp_radio_esp32s31_bluetooth_memory::BluetoothBlePhyEngineCpuOwned,
    public_address: BluetoothPublicDeviceAddress,
    target: &mut T,
    initialize: impl FnOnce(&mut T, BluetoothPhyRegisterInitInputs),
    publish_address: impl FnOnce(&mut T, BluetoothControllerPublicAddress),
) -> BluetoothBlePhyInitializationReport {
    let report = apply_register_init(storage, |inputs| initialize(target, inputs));
    publish_address(
        target,
        BluetoothControllerPublicAddress::from_canonical_bytes(public_address.canonical_bytes()),
    );
    report
}

#[cfg(any(target_arch = "riscv32", test))]
fn apply_register_init(
    storage: &open_esp_radio_esp32s31_bluetooth_memory::BluetoothBlePhyEngineCpuOwned,
    initialize: impl FnOnce(BluetoothPhyRegisterInitInputs),
) -> BluetoothBlePhyInitializationReport {
    let binding = storage.binding();
    initialize(BluetoothPhyRegisterInitInputs::normal_controller_profile(
        binding.environment_address(),
        binding.resolving_list_address(),
    ));
    BluetoothBlePhyInitializationReport
}

#[cfg(test)]
mod tests {
    use open_esp_radio_bluetooth_hci::BluetoothPublicDeviceAddress;
    use open_esp_radio_esp32s31_bluetooth_memory::{
        BluetoothBlePhyEngineCpuOwned, BluetoothBlePhyEngineModelAddress,
        BluetoothBlePhyEngineStorage,
    };

    use super::{
        BluetoothBlePhyInitializationReport, apply_register_init,
        apply_register_init_then_public_address,
        normalize_le_1m_peripheral_connection_packet_start,
    };

    fn model_storage() -> BluetoothBlePhyEngineCpuOwned {
        let storage =
            std::boxed::Box::leak(std::boxed::Box::new(BluetoothBlePhyEngineStorage::new()));
        let base = BluetoothBlePhyEngineModelAddress::new(0x2f00_0100)
            .expect("model base uses the controller-SRAM encoding");
        BluetoothBlePhyEngineStorage::pin_static_model(storage, base)
            .expect("complete model storage fits physical SRAM")
    }

    #[test]
    fn normal_register_profile_is_applied_once_without_releasing_storage() {
        let owner = model_storage();
        let calibration = owner.le_1m_packet_start_calibration();
        let mut calls = 0;

        let report = apply_register_init(&owner, |_| calls += 1);

        assert_eq!(calls, 1);
        assert_eq!(report, BluetoothBlePhyInitializationReport);
        assert_eq!(owner.le_1m_packet_start_calibration(), calibration);
    }

    #[test]
    fn public_identity_is_published_after_phy_register_initialization() {
        let owner = model_storage();
        let public_address =
            BluetoothPublicDeviceAddress::from_canonical_bytes([2, 3, 5, 7, 11, 13]);
        let mut operations = std::vec::Vec::new();

        let report = apply_register_init_then_public_address(
            &owner,
            public_address,
            &mut operations,
            |operations, _| operations.push("phy"),
            |operations, address| {
                assert_eq!(address.canonical_bytes(), public_address.canonical_bytes());
                operations.push("public-address");
            },
        );

        assert_eq!(report, BluetoothBlePhyInitializationReport);
        assert_eq!(operations, ["phy", "public-address"]);
    }

    #[test]
    fn connection_packet_start_normalization_preserves_elapsed_time() {
        let storage =
            std::boxed::Box::leak(std::boxed::Box::new(BluetoothBlePhyEngineStorage::new()));
        let base = BluetoothBlePhyEngineModelAddress::new(0x2f00_4000)
            .expect("model base uses the controller-SRAM encoding");
        let owner = BluetoothBlePhyEngineStorage::pin_static_model(storage, base)
            .expect("complete model storage fits physical SRAM");
        let calibration = owner.le_1m_packet_start_calibration();

        let first = normalize_le_1m_peripheral_connection_packet_start(calibration, 20_000);
        let second = normalize_le_1m_peripheral_connection_packet_start(calibration, 20_017);

        assert_eq!(second.elapsed_since(&first), 17);
    }
}
