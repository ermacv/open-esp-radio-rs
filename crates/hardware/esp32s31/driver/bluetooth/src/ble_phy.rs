//! Owned BLE PHY engine activation after common PHY and BTBB initialization.

#[cfg(target_arch = "riscv32")]
use crate::{
    baseband::ControllerBasebandInitialized,
    resources::{
        InterruptBankOwner,
        runtime_owner::{RuntimeOwnerLease, RuntimeOwnerSlot},
    },
};
#[cfg(any(target_arch = "riscv32", test, feature = "test-support"))]
use oer_bluetooth_hci::BluetoothPublicDeviceAddress;
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_bluetooth_memory::BlePhyLe1MPacketStartCalibration;
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_bluetooth_memory::{
    BlePhyEngineCpuOwned, DirectionFindingWorkspaceCpuOwned, DirectionFindingWorkspaceLink,
    LePacketCapturedTime, PeripheralConnectionCapturedAnchorTime,
};
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::bluetooth::ModemLpTimerLowPowerHardwareInitializedOwner;
#[cfg(any(target_arch = "riscv32", test, feature = "test-support"))]
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
pub struct AlwaysAwakeTimingReady {
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
    pub const fn into_scheduler_instant(self) -> crate::SchedulerInstant {
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
pub struct BlePhyTimingAuthority {
    le_1m_packet_start_calibration: BlePhyLe1MPacketStartCalibration,
}

#[cfg(target_arch = "riscv32")]
impl BlePhyTimingAuthority {
    fn new(le_1m_packet_start_calibration: BlePhyLe1MPacketStartCalibration) -> Self {
        Self {
            le_1m_packet_start_calibration,
        }
    }

    pub fn complete_always_awake(
        &mut self,
        epoch: crate::ControllerSchedulerEpoch,
        sample: crate::ControllerTimeSample,
    ) -> AlwaysAwakeTimingReady {
        AlwaysAwakeTimingReady::from_completed_sample(epoch, sample)
    }

    /// Scheduler-time packet start of one captured LE 1M packet, in
    /// microseconds normalized by this PHY's packet-start calibration.
    pub fn complete_le_1m_packet_start(
        &mut self,
        epoch: crate::ControllerSchedulerEpoch,
        captured: LePacketCapturedTime,
    ) -> u32 {
        let captured_micros = epoch.project_le_packet_capture(captured);
        self.le_1m_packet_start_calibration
            .normalize_controller_micros(captured_micros)
    }

    /// Scheduler-time packet start of one captured connection anchor, in
    /// microseconds normalized by this PHY's packet-start calibration.
    pub fn complete_le_1m_peripheral_connection_packet_start(
        &mut self,
        epoch: crate::ControllerSchedulerEpoch,
        captured: PeripheralConnectionCapturedAnchorTime,
    ) -> u32 {
        let captured_micros = epoch.project_peripheral_connection_capture(captured);
        self.le_1m_packet_start_calibration
            .normalize_controller_micros(captured_micros)
    }
}

/// Powered Controller after BLE PHY init and public-address publication.
///
/// The address-bound environment, resolving-list storage and registered PHY
/// client share an exclusive slot, transferred to the task after final placement.
/// They remain retained throughout the hardware epoch together with the affine standalone
/// always-awake profile selection. That selection performs no RF MMIO and is
/// not an RF-ready or completed-time proof. This state does not claim that an
/// interrupt route, packet engine, Link Layer role, advertising set, scanner,
/// connection, or HCI dataplane is operational.
#[must_use = "initialized BLE PHY retains every hardware and storage owner"]
#[cfg(target_arch = "riscv32")]
pub struct ControllerBlePhyEngineInitialized<P, const MODEM_TIMER_CAPACITY: usize> {
    controller: crate::low_power::ControllerLowPowerHardwareInitialized<P, MODEM_TIMER_CAPACITY>,
    physical: RuntimeOwnerSlot<BlePhyRetainedOwners>,
    phy_entry: crate::common_phy_state::ControllerPhyEntry,
    baseband_report: crate::baseband::BasebandInitializationReport,
    report: BlePhyInitializationReport,
}

/// Registered PHY and the BLE PHY/DF allocation graph still referenced by BTBB.
///
/// Extraction from boot storage does not revoke hardware pointers or release the
/// PHY client. The retired Controller keeps these private until physical teardown.
#[cfg(target_arch = "riscv32")]
pub struct BlePhyRetainedOwners {
    _phy: oer_esp32s31_phy::RegisteredBluetoothPhyClient,
    _calibration_cache: Option<oer_esp32s31_phy::PhyCalibrationCache>,
    storage: BlePhyEngineCpuOwned,
    direction_finding: DirectionFindingWorkspaceHardwareOwned,
}

/// Actual BLE allocations retained across physical shutdown.
#[cfg(target_arch = "riscv32")]
pub struct BlePhyRetiredMemory {
    _storage: BlePhyEngineCpuOwned,
    _direction_finding: DirectionFindingWorkspaceHardwareOwned,
    _calibration_cache: Option<oer_esp32s31_phy::PhyCalibrationCache>,
}

#[cfg(target_arch = "riscv32")]
impl BlePhyRetainedOwners {
    pub fn set_tracking_debug(
        &mut self,
        debug: oer_esp32s31_phy::state::PhyTemperatureTrackingDebug,
    ) -> oer_esp32s31_phy::state::PhyTemperatureTrackingDebug {
        self._phy.set_tracking_debug(debug)
    }
    pub fn tracking_schedule_at(
        &self,
        now_micros: u64,
    ) -> Result<
        oer_esp32s31_phy::tracking::schedule::Schedule,
        oer_esp32s31_phy::state::client::PhyTrackTimeError,
    > {
        self._phy.client_snapshot().tracking_schedule_at(now_micros)
    }

    pub fn into_shutdown_parts(
        self,
    ) -> (
        oer_esp32s31_phy::RegisteredBluetoothPhyClient,
        BlePhyRetiredMemory,
    ) {
        (
            self._phy,
            BlePhyRetiredMemory {
                _storage: self.storage,
                _direction_finding: self.direction_finding,
                _calibration_cache: self._calibration_cache,
            },
        )
    }
}

/// Disjoint software endpoints and an exclusive lease on this exact PHY graph.
#[cfg(target_arch = "riscv32")]
pub struct BlePhyRuntime<'runtime, P, const MT: usize> {
    pub endpoints: crate::low_power::ControllerRuntimeEndpoints<'runtime, P, MT>,
    pub timing: BlePhyTimingAuthority,
    pub physical: RuntimeOwnerLease<'runtime, BlePhyRetainedOwners>,
    pub direction_finding: DirectionFindingWorkspaceLink,
}

#[cfg(target_arch = "riscv32")]
impl<P, const MODEM_TIMER_CAPACITY: usize>
    ControllerBlePhyEngineInitialized<P, MODEM_TIMER_CAPACITY>
{
    /// Observe completion of the source-owned normal BLE PHY transaction.
    pub const fn report(&self) -> BlePhyInitializationReport {
        self.report
    }

    /// Inspect the preceding finite BTBB transition.
    pub const fn baseband_report(&self) -> crate::baseband::BasebandInitializationReport {
        self.baseband_report
    }

    /// Inspect the complete common-PHY transition.
    pub const fn phy_entry(&self) -> crate::common_phy_state::ControllerPhyEntry {
        self.phy_entry
    }

    /// Take the interrupt bank with its controller output prepared, and the
    /// low-power timer hardware, exactly once for this completed epoch.
    ///
    /// The bank leaves this engine's custody here for the first time, so no
    /// CPU-route installer can have received it; this complete initialization
    /// is the matching Controller epoch. Both discharge the output
    /// preparation contract. The timer is returned unstarted and only with
    /// the prepared output, so it cannot be started before the output.
    #[allow(
        unsafe_code,
        reason = "the completed epoch and first custody transfer discharge the output contract"
    )]
    pub fn take_activation_owners_with_output_prepared(
        &mut self,
    ) -> (
        oer_esp32s31_hal::bluetooth::InterruptOutputPreparedOwner,
        ModemLpTimerLowPowerHardwareInitializedOwner,
    ) {
        let controller = &mut self.controller;
        let interrupts: InterruptBankOwner = controller.take_interrupt_owner();
        let timer = controller.take_timer_hardware();
        // SAFETY: `self` is the completed matching Controller initialization,
        // and the bank has just left its one-shot custody, so all three CPU
        // routes remain inactive.
        let output = unsafe { interrupts.prepare_controller_output() };
        (output, timer)
    }

    /// Split the already initialized hardware runtime after this
    /// complete BLE-PHY owner has reached stable final placement.
    pub fn split_runtime(&mut self) -> Option<BlePhyRuntime<'_, P, MODEM_TIMER_CAPACITY>> {
        // Read before claiming either lease: repeated splitting must reject
        // without accessing an owner already moved into retirement.
        let physical = self.physical.as_mut()?;
        let calibration = physical.storage.le_1m_packet_start_calibration();
        let direction_finding = physical.direction_finding.link();
        let endpoints = self.controller.split_runtime()?;
        let physical = self.physical.lease().expect("unclaimed BLE PHY graph");
        Some(BlePhyRuntime {
            endpoints,
            timing: BlePhyTimingAuthority::new(calibration),
            physical,
            direction_finding,
        })
    }
}

#[cfg(target_arch = "riscv32")]
impl<P, const MODEM_TIMER_CAPACITY: usize> ControllerBasebandInitialized<P, MODEM_TIMER_CAPACITY> {
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
    ) -> ControllerBlePhyEngineInitialized<P, MODEM_TIMER_CAPACITY> {
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

        let ControllerBasebandInitialized {
            initialized,
            baseband_report,
        } = self;
        let crate::common_phy_state::ControllerPhyInitialized {
            controller,
            phy,
            calibration_cache,
            report: phy_entry,
        } = initialized;
        ControllerBlePhyEngineInitialized {
            controller,
            physical: RuntimeOwnerSlot::new(BlePhyRetainedOwners {
                _phy: phy,
                _calibration_cache: calibration_cache,
                storage,
                direction_finding: DirectionFindingWorkspaceHardwareOwned {
                    storage: direction_finding,
                    _publication: publication,
                },
            }),
            phy_entry,
            baseband_report,
            report,
        }
    }
}

#[cfg(any(target_arch = "riscv32", test, feature = "test-support"))]
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

#[cfg(any(target_arch = "riscv32", test, feature = "test-support"))]
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

#[cfg(target_arch = "riscv32")]
pub struct BlePhyRestartParts<P, const MT: usize> {
    pub scheduler: crate::scheduler::core::SchedulerRestartParts<P, MT>,
    pub physical: BlePhyRetainedOwners,
    pub timing: BlePhyTimingAuthority,
    pub direction_finding: DirectionFindingWorkspaceLink,
}

#[cfg(target_arch = "riscv32")]
impl<P, const MT: usize> ControllerBlePhyEngineInitialized<P, MT> {
    pub fn into_restart_parts(self) -> BlePhyRestartParts<P, MT> {
        let physical = self.physical.into_unclaimed();
        let timing = BlePhyTimingAuthority::new(physical.storage.le_1m_packet_start_calibration());
        let direction_finding = physical.direction_finding.link();
        BlePhyRestartParts {
            scheduler: self.controller.into_restart_parts(),
            physical,
            timing,
            direction_finding,
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl BlePhyRetiredMemory {
    pub fn with_client(
        self,
        phy: oer_esp32s31_phy::RegisteredBluetoothPhyClient,
    ) -> BlePhyRetainedOwners {
        BlePhyRetainedOwners {
            _phy: phy,
            _calibration_cache: self._calibration_cache,
            storage: self._storage,
            direction_finding: self._direction_finding,
        }
    }

    /// Caller owns the completed physical cold-release proof. No raw reference
    /// is recovered: these are the original pinned CPU allocation owners.
    pub fn into_restart_parts(self) -> (BlePhyEngineCpuOwned, DirectionFindingWorkspaceCpuOwned) {
        (self._storage, self._direction_finding.storage)
    }
}
