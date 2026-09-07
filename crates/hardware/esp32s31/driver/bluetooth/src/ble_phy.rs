//! Owned BLE PHY engine activation after common PHY and BTBB initialization.

#[cfg(target_arch = "riscv32")]
use crate::{baseband::ControllerBasebandInitialized, resources::InterruptBankOwner};
#[cfg(any(target_arch = "riscv32", test))]
use oer_bluetooth_hci::BluetoothPublicDeviceAddress;
#[cfg(any(target_arch = "riscv32", test))]
use oer_esp32s31_bluetooth_memory::BlePhyLe1MPacketStartCalibration;
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_bluetooth_memory::{
    BlePhyEngineCpuOwned, DirectionFindingWorkspaceCpuOwned, DirectionFindingWorkspaceLink,
    LePacketCapturedTime, PeripheralConnectionCapturedAnchorTime,
};
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::bluetooth::ModemLpTimerLowPowerHardwareInitializedOwner;
#[cfg(any(target_arch = "riscv32", test))]
use oer_esp32s31_hal::{bluetooth::ControllerPublicAddress, types::BluetoothPhyRegisterInitInputs};

/// Controller-global DF storage after its disabled-CTE descriptor is visible to hardware.
///
/// The memory and exact PAC publication proof remain joined for the complete
/// powered epoch. Only an opaque ordinary-role link can be copied out; CPU
/// access and raw descriptor images remain inaccessible.
#[must_use = "hardware-owned direction-finding storage must remain retained"]
#[cfg(target_arch = "riscv32")]
pub(crate) struct DirectionFindingWorkspaceHardwareOwned {
    storage: DirectionFindingWorkspaceCpuOwned,
    _publication: oer_esp32s31_hal::bluetooth::DirectionFindingDisabledBaselineOwner,
}

#[cfg(target_arch = "riscv32")]
impl DirectionFindingWorkspaceHardwareOwned {
    const fn link(&self) -> DirectionFindingWorkspaceLink {
        self.storage.binding().link()
    }
}

/// Observation that the source-owned normal BLE PHY transaction completed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlePhyInitializationReport;

/// One post-enable timing observation in the retained always-awake epoch.
///
/// Only a completed generation-keyed controller-time request owned by an
/// initialized BLE PHY can create this affine token. It deliberately cannot be
/// copied or manufactured from a detached scheduler image. It proves no RF
/// wake or analog readiness; the supported profile keeps those transitions
/// outside the DTM event path.
#[must_use = "the always-awake timing observation must be consumed by DTM scheduling"]
#[cfg(target_arch = "riscv32")]
pub(crate) struct AlwaysAwakeTimingReady {
    scheduler_instant: crate::SchedulerInstant,
}

#[cfg(target_arch = "riscv32")]
impl AlwaysAwakeTimingReady {
    fn from_completed_sample(
        epoch: crate::ControllerSchedulerEpoch,
        sample: crate::ControllerTimeSample,
    ) -> Self {
        Self {
            scheduler_instant: crate::SchedulerInstant::from_image(
                epoch.project_without_reanchor(&sample),
            ),
        }
    }

    /// Consume the post-enable timing proof into its microsecond-domain instant.
    pub(crate) const fn into_scheduler_instant(self) -> crate::SchedulerInstant {
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
pub(crate) struct BlePhyTimingAuthority {
    le_1m_packet_start_calibration: BlePhyLe1MPacketStartCalibration,
}

#[cfg(target_arch = "riscv32")]
impl BlePhyTimingAuthority {
    fn new(le_1m_packet_start_calibration: BlePhyLe1MPacketStartCalibration) -> Self {
        Self {
            le_1m_packet_start_calibration,
        }
    }

    pub(crate) fn complete_always_awake(
        &mut self,
        epoch: crate::ControllerSchedulerEpoch,
        sample: crate::ControllerTimeSample,
    ) -> AlwaysAwakeTimingReady {
        AlwaysAwakeTimingReady::from_completed_sample(epoch, sample)
    }

    pub(crate) fn complete_le_1m_packet_start(
        &mut self,
        epoch: crate::ControllerSchedulerEpoch,
        captured: LePacketCapturedTime,
    ) -> crate::le::peripheral::Le1MPacketStartTiming {
        let captured_micros = epoch.project_le_packet_capture(captured);
        crate::le::peripheral::Le1MPacketStartTiming::from_scheduler_micros(
            self.le_1m_packet_start_calibration
                .normalize_controller_micros(captured_micros),
        )
    }

    pub(crate) fn complete_le_1m_peripheral_connection_packet_start(
        &mut self,
        epoch: crate::ControllerSchedulerEpoch,
        captured: PeripheralConnectionCapturedAnchorTime,
    ) -> crate::le::peripheral::connection::PeripheralConnectionPacketStartTiming {
        let captured_micros = epoch.project_peripheral_connection_capture(captured);
        normalize_le_1m_peripheral_connection_packet_start(
            self.le_1m_packet_start_calibration,
            captured_micros,
        )
    }
}

#[cfg(any(target_arch = "riscv32", test))]
fn normalize_le_1m_peripheral_connection_packet_start(
    calibration: BlePhyLe1MPacketStartCalibration,
    captured_micros: u32,
) -> crate::le::peripheral::connection::PeripheralConnectionPacketStartTiming {
    crate::le::peripheral::connection::PeripheralConnectionPacketStartTiming::from_scheduler_micros(
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
pub struct ControllerBlePhyEngineInitialized<
    P,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
> {
    initialized: ControllerBasebandInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
    storage: BlePhyEngineCpuOwned,
    direction_finding: DirectionFindingWorkspaceHardwareOwned,
    report: BlePhyInitializationReport,
}

#[cfg(target_arch = "riscv32")]
impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    ControllerBlePhyEngineInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    /// Observe completion of the source-owned normal BLE PHY transaction.
    pub const fn report(&self) -> BlePhyInitializationReport {
        self.report
    }

    /// Inspect the preceding finite BTBB transition.
    pub const fn baseband_report(&self) -> crate::baseband::BasebandInitializationReport {
        self.initialized.baseband_report()
    }

    /// Inspect the complete common-PHY transition.
    pub const fn phy_report(&self) -> crate::common_phy_state::PhyInitializationReport {
        self.initialized.phy_report()
    }

    /// Opaque ordinary-role link into this powered epoch's global DF workspace.
    pub(crate) const fn direction_finding_workspace_link(&self) -> DirectionFindingWorkspaceLink {
        self.direction_finding.link()
    }

    pub(crate) fn take_activation_owners(
        &mut self,
    ) -> (
        InterruptBankOwner,
        ModemLpTimerLowPowerHardwareInitializedOwner,
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
        crate::low_power::ControllerRuntimeEndpoints<'_, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
        BlePhyTimingAuthority,
    ) {
        let calibration = self.storage.le_1m_packet_start_calibration();
        (
            self.initialized.initialized.controller.split_runtime(),
            BlePhyTimingAuthority::new(calibration),
        )
    }
}

#[cfg(target_arch = "riscv32")]
impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    ControllerBasebandInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
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
        storage: BlePhyEngineCpuOwned,
        direction_finding: DirectionFindingWorkspaceCpuOwned,
        public_address: BluetoothPublicDeviceAddress,
    ) -> ControllerBlePhyEngineInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY> {
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

        ControllerBlePhyEngineInitialized {
            initialized: self,
            storage,
            direction_finding: DirectionFindingWorkspaceHardwareOwned {
                storage: direction_finding,
                _publication: publication,
            },
            report,
        }
    }
}

#[cfg(any(target_arch = "riscv32", test))]
fn apply_register_init_then_public_address<T>(
    storage: &oer_esp32s31_bluetooth_memory::BlePhyEngineCpuOwned,
    public_address: BluetoothPublicDeviceAddress,
    target: &mut T,
    initialize: impl FnOnce(&mut T, BluetoothPhyRegisterInitInputs),
    publish_address: impl FnOnce(&mut T, ControllerPublicAddress),
) -> BlePhyInitializationReport {
    let report = apply_register_init(storage, |inputs| initialize(target, inputs));
    publish_address(
        target,
        ControllerPublicAddress::from_canonical_bytes(public_address.canonical_bytes()),
    );
    report
}

#[cfg(any(target_arch = "riscv32", test))]
fn apply_register_init(
    storage: &oer_esp32s31_bluetooth_memory::BlePhyEngineCpuOwned,
    initialize: impl FnOnce(BluetoothPhyRegisterInitInputs),
) -> BlePhyInitializationReport {
    let binding = storage.binding();
    initialize(BluetoothPhyRegisterInitInputs::normal_controller_profile(
        binding.environment_address(),
        binding.resolving_list_address(),
    ));
    BlePhyInitializationReport
}

#[cfg(test)]
mod tests;
