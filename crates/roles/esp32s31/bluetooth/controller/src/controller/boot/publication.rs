//! IRQ-owner publication and the split into borrowed hardware/HCI endpoints.
//!
//! Publication transfers both register owners atomically before CPU routing.
//! Rejection returns the entire pre-publication graph. Splitting claims the
//! exclusive task HAL lease, borrows software storage and claims HCI command
//! authority once. It cannot release powered hardware or create a second actor.

#[cfg(target_arch = "riscv32")]
use super::{
    ControllerIdleCommandTask, ControllerModemTimerTask, ControllerPublishedTaskService,
    ModemLpTimerInterruptDispatchStorage, ModemLpTimerSoftwareOwnerStorage,
};
#[cfg(target_arch = "riscv32")]
use embassy_sync::blocking_mutex::raw::RawMutex;
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::bluetooth::{
    BluetoothLowPowerRuntimeControlObservation, InterruptOutputPreparedOwner,
    InterruptRegistersOwner, ModemLpTimerCounterStartedOwner, ModemLpTimerInterruptReadyOwner,
};
#[cfg(target_arch = "riscv32")]
use {
    oer_esp32s31_bluetooth::scheduler::post_unlink::DtmPostUnlinkMailbox,
    oer_esp32s31_bluetooth::{
        ble_phy::ControllerBlePhyEngineInitialized,
        interrupt::{
            InterruptOwnerRestartStorage, InterruptOwnerStorage, NrtDefaultInterruptEpoch,
            SharedInterruptDispatchStorage,
        },
        low_power::ControllerRuntimeEndpoints,
        modem_lp_timer_queue::ModemLpTimerPublishedInterruptStep,
        runtime_resources::ControllerInterruptRuntime,
    },
};

/// Powered Controller after address readiness, IRQ-output preparation and
/// runtime-timer start.
///
/// This state retains the complete BLE PHY epoch, the prepared-but-unrouted
/// interrupt partition and the uniquely started low-power timer. It does not
/// claim stable ISR storage, a CPU route, scheduler activation or operational
/// Link-Layer work.
#[must_use = "the started Bluetooth Controller retains every hardware owner"]
#[cfg(target_arch = "riscv32")]
pub struct ControllerOutputTimerStarted<
    P,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
> {
    pub(crate) initialized:
        ControllerBlePhyEngineInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
    _interrupt_output: InterruptOutputPreparedOwner,
    pub(crate) timer: ModemLpTimerCounterStartedOwner,
}

/// Powered Controller with both register owners ready for ISR publication.
///
/// The controller interrupt partition and source-127 timer partition have
/// crossed their final no-MMIO ownership transitions. They remain movable and
/// no CPU route is active; the next platform composition must publish both in
/// stable ISR storage before it enables any of the three routes.
#[must_use = "the prepared Bluetooth interrupt owners must be published before routing"]
#[cfg(target_arch = "riscv32")]
pub struct ControllerInterruptOwnersReady<
    P,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
> {
    initialized: ControllerBlePhyEngineInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
    _interrupts: InterruptRegistersOwner,
    _timer: ModemLpTimerInterruptReadyOwner,
    runtime_control: BluetoothLowPowerRuntimeControlObservation,
}

/// Powered Controller after atomic stable publication of both ISR owners.
///
/// The platform lease retains stable placement, but no CPU route is active and
/// no hard-handler entry is possible from this state. The nested Controller
/// also retains the affine standalone always-awake profile selection; that
/// marker performs no RF MMIO and supplies neither RF-ready nor time-pending
/// authority.
#[must_use = "published Bluetooth interrupt owners must remain retained through route setup"]
#[cfg(target_arch = "riscv32")]
pub struct ControllerInterruptOwnersPublished<
    P,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
> {
    initialized: ControllerBlePhyEngineInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
    _storage: S,
    post_unlink_mailbox: DtmPostUnlinkMailbox,
    runtime_control: BluetoothLowPowerRuntimeControlObservation,
    scheduler_epoch: Option<oer_esp32s31_bluetooth::ControllerSchedulerEpoch>,
    roles: oer_esp32s31_bluetooth::resources::runtime_owner::RuntimeOwnerSlot<
        super::role_retirement::ControllerRoleResources,
    >,
}

/// Hardware/task endpoints prepared after stable interrupt-owner publication.
///
/// HCI is intentionally absent: protocol resources are bound only after this
/// hardware ownership graph has reached its final movable state.
#[must_use = "published hardware endpoints must remain in one live runtime epoch"]
#[cfg(target_arch = "riscv32")]
pub(crate) struct ControllerPublishedHardwareRuntimeEndpoints<
    'runtime,
    P,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
> {
    pub(crate) interrupt: ControllerPublishedInterruptService<'runtime, S>,
    pub(crate) task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    pub(crate) platform:
        oer_esp32s31_bluetooth::resources::platform_retirement::ControllerPlatformLease<
            'runtime,
            P,
        >,
    pub(crate) modem_timer: ControllerModemTimerTask<'runtime, S, MODEM_TIMER_CAPACITY>,
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, P, S, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    ControllerPublishedHardwareRuntimeEndpoints<
        'runtime,
        P,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
    >
{
    pub(crate) fn bind_hci<M, const H2C: usize, const C2H: usize, const PC: usize>(
        self,
        mut hci: oer_bluetooth_hci::LeControllerHciEndpoints<'runtime, M, H2C, C2H, PC>,
    ) -> ControllerPublishedRuntimeSplit<
        'runtime,
        P,
        M,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        H2C,
        C2H,
        PC,
    >
    where
        M: RawMutex,
    {
        let Self {
            interrupt,
            task,
            modem_timer,
            platform,
        } = self;
        match hci.controller.claim_initial_command_ready(task) {
            oer_bluetooth_hci::LeControllerCommandReadyClaim::Ready(ready) => {
                let platform = platform.bind(hci.controller.epoch_identity());
                ControllerPublishedRuntimeSplit::Ready(
                    ControllerPublishedRuntimeEndpoints {
                        interrupt,
                        task: ControllerIdleCommandTask::from_ready(ready),
                        modem_timer,
                        hci,
                    },
                    platform,
                )
            }
            oer_bluetooth_hci::LeControllerCommandReadyClaim::AlreadyClaimed(task) => {
                ControllerPublishedRuntimeSplit::CommandReadyUnavailable(
                    ControllerPublishedRuntimeSplitFailure {
                        _interrupt: interrupt,
                        _task: task,
                        _modem_timer: modem_timer,
                        _hci: hci,
                    },
                )
            }
        }
    }
}

/// Disjoint runtime endpoints from one statically placed final Controller.
///
/// The task endpoint leases task HAL and borrows mutable scheduler workers; the interrupt service
/// owns only stable platform dispatch plus shared publication cells, and HCI
/// exposes the Host transport and combined Controller command endpoint. Keeping
/// the backing final owner in caller-owned stable storage prevents a
/// self-referential runtime object.
#[must_use = "the final Controller endpoints must remain in one live runtime epoch"]
#[cfg(target_arch = "riscv32")]
pub struct ControllerPublishedRuntimeEndpoints<
    'runtime,
    M,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    /// Finite hard-handler service over the stable PAC/HAL owners.
    pub interrupt: ControllerPublishedInterruptService<'runtime, S>,
    /// Sole idle task carrying this epoch's affine next-command authority.
    pub task: ControllerIdleCommandTask<'runtime, S, SCHEDULER_CAPACITY>,
    /// Disjoint source-127 queue, epoch and stable-storage task endpoint.
    pub modem_timer: ControllerModemTimerTask<'runtime, S, MODEM_TIMER_CAPACITY>,
    /// Disjoint Host transport and combined Controller command endpoint.
    pub hci: oer_bluetooth_hci::LeControllerHciEndpoints<
        'runtime,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
}

/// Result of transferring one final Controller runtime epoch.
///
/// A successful split claims the HCI epoch's initial command-ready authority
/// exactly once. A later split fails before borrowing software or claiming HCI
/// authority when the task HAL lease is already claimed. No duplicate task owner
/// or independently usable endpoint is returned.
#[must_use = "retain the ready runtime or its opaque fail-stop owner"]
#[cfg(target_arch = "riscv32")]
pub enum ControllerPublishedRuntimeSplit<
    'runtime,
    P,
    M,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    /// The only idle command task and its separately retained platform lease.
    /// The platform lease is bound to the same HCI epoch before returning.
    Ready(
        ControllerPublishedRuntimeEndpoints<
            'runtime,
            M,
            S,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        oer_esp32s31_bluetooth::resources::platform_retirement::ControllerRuntimePlatform<
            'runtime,
            P,
        >,
    ),
    /// The task HAL lease was already claimed by an earlier runtime.
    /// Software storage remains retained; no HCI authority is claimed.
    TaskOwnerUnavailable,
    /// The epoch's initial command authority had already been consumed.
    CommandReadyUnavailable(
        ControllerPublishedRuntimeSplitFailure<
            'runtime,
            M,
            S,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ),
}

/// Opaque fail-stop owner for a final runtime whose command authority was gone.
#[must_use = "the complete unavailable runtime remains intentionally fail-stopped"]
#[cfg(target_arch = "riscv32")]
pub struct ControllerPublishedRuntimeSplitFailure<
    'runtime,
    M,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    _interrupt: ControllerPublishedInterruptService<'runtime, S>,
    _task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    _modem_timer: ControllerModemTimerTask<'runtime, S, MODEM_TIMER_CAPACITY>,
    _hci: oer_bluetooth_hci::LeControllerHciEndpoints<
        'runtime,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
}

/// Stable interrupt service for a materialized final Controller epoch.
///
/// This value cannot prepare DTM descriptors or mutate task-owned scheduler
/// state. It can only execute the three bounded hardware dispositions and
/// publish their durable events into the matching runtime resources.
#[must_use = "interrupt service must remain paired with its task runtime"]
#[cfg(target_arch = "riscv32")]
pub struct ControllerPublishedInterruptService<'runtime, S> {
    storage: &'runtime S,
    runtime: ControllerInterruptRuntime<'runtime>,
    mailbox: &'runtime DtmPostUnlinkMailbox,
}

/// Failed stable publication retaining the Controller, storage and role runtimes.
#[must_use = "failed ISR publication returns every affine owner for inspection or retry"]
#[cfg(target_arch = "riscv32")]
pub struct ControllerInterruptOwnerPublicationFailure<
    P,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
> where
    S: InterruptOwnerStorage,
{
    controller: ControllerInterruptOwnersReady<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
    storage: S,
    dtm_resources: crate::le::dtm::DtmRuntimeResources,
    legacy_advertising_resources: crate::le::advertising::LegacyAdvertisingRuntimeResources,
    passive_scan_resources: crate::le::scanning::PassiveScanRuntimeResources,
    peripheral_connection_resources: crate::le::peripheral::PeripheralConnectionRuntimeResources,
    legacy_connectable_advertising_resources:
        crate::le::advertising::LegacyConnectableAdvertisingRuntimeResources,
    error: S::Error,
}

#[cfg(target_arch = "riscv32")]
impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    ControllerOutputTimerStarted<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    /// Inspect the BLE PHY input retained by this exact powered epoch.
    pub const fn ble_phy_report(
        &self,
    ) -> oer_esp32s31_bluetooth::ble_phy::BlePhyInitializationReport {
        self.initialized.report()
    }

    /// Inspect the preceding finite BTBB transition.
    pub const fn baseband_report(
        &self,
    ) -> oer_esp32s31_bluetooth::baseband::BasebandInitializationReport {
        self.initialized.baseband_report()
    }

    /// Inspect how the common PHY was obtained.
    pub const fn phy_entry(&self) -> oer_esp32s31_bluetooth::common_phy_state::ControllerPhyEntry {
        self.initialized.phy_entry()
    }

    /// Conditional runtime-control branch retained across the timer start.
    pub const fn runtime_control_observation(&self) -> BluetoothLowPowerRuntimeControlObservation {
        self.timer.runtime_control_observation()
    }
}

#[cfg(target_arch = "riscv32")]
impl<P, S, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    ControllerInterruptOwnersPublished<P, S, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
where
    S: ModemLpTimerSoftwareOwnerStorage,
{
    /// Claim task HAL ownership and borrow disjoint interrupt/software endpoints.
    ///
    /// The caller must retain this owner in stable storage for the complete
    /// routed lifetime. HCI remains a separate protocol resource until the
    /// post-publication binding transition.
    pub(crate) fn split_hardware_runtime<'runtime>(
        &'runtime mut self,
    ) -> Option<
        ControllerPublishedHardwareRuntimeEndpoints<
            'runtime,
            P,
            S,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
        >,
    > {
        let Self {
            initialized,
            _storage,
            post_unlink_mailbox,
            scheduler_epoch,
            roles,
            ..
        } = self;
        // All three slots are claimed in one final split; reject re-entry
        // before borrowing any software endpoint or taking another lease.
        roles.as_mut()?;
        let oer_esp32s31_bluetooth::ble_phy::BlePhyRuntime {
            endpoints:
                ControllerRuntimeEndpoints {
                    interrupt,
                    task,
                    modem_timer,
                    platform,
                },
            timing: ble_phy_timing,
            physical: ble_phy_owners,
            direction_finding: direction_finding_workspace,
        } = initialized.split_runtime()?;
        let roles = roles.lease().expect("unclaimed role allocation group");
        let interrupt = ControllerPublishedInterruptService {
            storage: _storage,
            runtime: interrupt,
            mailbox: post_unlink_mailbox,
        };
        let task = ControllerPublishedTaskService {
            storage: _storage,
            runtime: task,
            mailbox: post_unlink_mailbox,
            roles,
            direction_finding_workspace,
            ble_phy_timing,
            ble_phy_owners,
            scheduler_epoch,
        };
        let modem_timer = ControllerModemTimerTask::new(_storage, modem_timer);
        Some(ControllerPublishedHardwareRuntimeEndpoints {
            interrupt,
            task,
            modem_timer,
            platform,
        })
    }

    /// Inspect the BLE PHY input retained by this exact powered epoch.
    pub const fn ble_phy_report(
        &self,
    ) -> oer_esp32s31_bluetooth::ble_phy::BlePhyInitializationReport {
        self.initialized.report()
    }

    /// Inspect the preceding finite BTBB transition.
    pub const fn baseband_report(
        &self,
    ) -> oer_esp32s31_bluetooth::baseband::BasebandInitializationReport {
        self.initialized.baseband_report()
    }

    /// Inspect how the common PHY was obtained.
    pub const fn phy_entry(&self) -> oer_esp32s31_bluetooth::common_phy_state::ControllerPhyEntry {
        self.initialized.phy_entry()
    }

    /// Conditional runtime-control branch retained across publication.
    pub const fn runtime_control_observation(&self) -> BluetoothLowPowerRuntimeControlObservation {
        self.runtime_control
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S> ControllerPublishedInterruptService<'runtime, S> {
    /// Borrow the exact stable-storage publication backing this interrupt
    /// service.
    ///
    /// Platform integration uses this only after the complete service and its
    /// executor notifications have reached stable storage, then binds the CPU
    /// routes as the final activation edge. The borrow cannot duplicate or
    /// recover the affine publication owner. Its lifetime belongs to the
    /// underlying storage, allowing the service to move into a guarded slot
    /// while the route lease continues to borrow the same stable publication.
    pub const fn storage(&self) -> &'runtime S {
        self.storage
    }

    /// Service, durably publish and route one primary source-124 epoch through
    /// the Controller-owned post-unlink mailbox.
    ///
    /// Capture/acknowledge, both ordinary cell publications and the capacity-one
    /// mailbox transition are serialized by one critical section. An armed
    /// mailbox stores exactly the first eligible event; a full mailbox returns
    /// the newer event without replacing the retained one.
    pub fn service_primary_interrupt(
        &self,
    ) -> Result<crate::le::dtm::PrimarySerializedServiceStep, S::Error>
    where
        S: SharedInterruptDispatchStorage,
    {
        critical_section::with(|critical_section| {
            let step = self.storage.service_primary_interrupt()?;
            let published = step.publish(
                self.runtime.scheduler_wake(),
                self.runtime.scheduler_lock_modify_events(),
            );
            Ok(self.mailbox.publish(critical_section, published))
        })
    }

    /// Service and durably publish one modem-timer source-127 epoch.
    ///
    /// A software-pending owner remains affine in stable platform storage.
    /// Its matching Controller wake cell is published before this method
    /// returns, so later task registration cannot lose the work request.
    pub fn service_modem_lp_timer_interrupt(
        &self,
    ) -> Result<ModemLpTimerPublishedInterruptStep, S::Error>
    where
        S: ModemLpTimerInterruptDispatchStorage,
    {
        let step = self.storage.service_modem_lp_timer_interrupt()?;
        Ok(step.publish(self.runtime.modem_lp_timer_worker_wake()))
    }

    /// Service one opaque default-profile NRT source-133 epoch.
    ///
    /// The reviewed default path intentionally publishes no scheduler or
    /// Link-Layer work and keeps the shared owner in stable platform storage.
    pub fn service_nrt_default_interrupt(&self) -> Result<NrtDefaultInterruptEpoch, S::Error>
    where
        S: SharedInterruptDispatchStorage,
    {
        self.storage.service_nrt_default_interrupt()
    }
}

#[cfg(target_arch = "riscv32")]
impl<P, S, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    ControllerInterruptOwnerPublicationFailure<P, S, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
where
    S: InterruptOwnerStorage,
{
    /// Inspect the exact platform rejection.
    pub const fn error(&self) -> &S::Error {
        &self.error
    }

    /// Recover the complete pre-publication Controller, storage and role runtimes.
    pub fn into_parts(
        self,
    ) -> (
        ControllerInterruptOwnersReady<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
        S,
        crate::le::dtm::DtmRuntimeResources,
        crate::le::advertising::LegacyAdvertisingRuntimeResources,
        crate::le::scanning::PassiveScanRuntimeResources,
        crate::le::peripheral::PeripheralConnectionRuntimeResources,
        crate::le::advertising::LegacyConnectableAdvertisingRuntimeResources,
        S::Error,
    ) {
        (
            self.controller,
            self.storage,
            self.dtm_resources,
            self.legacy_advertising_resources,
            self.passive_scan_resources,
            self.peripheral_connection_resources,
            self.legacy_connectable_advertising_resources,
            self.error,
        )
    }
}

#[cfg(target_arch = "riscv32")]
impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    ControllerInterruptOwnersReady<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    /// Inspect the BLE PHY input retained by this exact powered epoch.
    pub const fn ble_phy_report(
        &self,
    ) -> oer_esp32s31_bluetooth::ble_phy::BlePhyInitializationReport {
        self.initialized.report()
    }

    /// Inspect the preceding finite BTBB transition.
    pub const fn baseband_report(
        &self,
    ) -> oer_esp32s31_bluetooth::baseband::BasebandInitializationReport {
        self.initialized.baseband_report()
    }

    /// Inspect how the common PHY was obtained.
    pub const fn phy_entry(&self) -> oer_esp32s31_bluetooth::common_phy_state::ControllerPhyEntry {
        self.initialized.phy_entry()
    }

    /// Conditional runtime-control branch retained by the ISR-ready timer.
    pub const fn runtime_control_observation(&self) -> BluetoothLowPowerRuntimeControlObservation {
        self.runtime_control
    }

    /// Atomically publish both owners in caller-selected stable ISR storage.
    ///
    /// Rejection occurs before publication and returns this exact state plus
    /// the storage capability and unmodified role runtimes. Success still leaves
    /// every CPU route inactive.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc failure must return every affine powered owner"
    )]
    pub fn publish_interrupt_owners<S>(
        self,
        storage: S,
        dtm_resources: crate::le::dtm::DtmRuntimeResources,
        legacy_advertising_resources: crate::le::advertising::LegacyAdvertisingRuntimeResources,
        passive_scan_resources: crate::le::scanning::PassiveScanRuntimeResources,
        peripheral_connection_resources: crate::le::peripheral::PeripheralConnectionRuntimeResources,
        legacy_connectable_advertising_resources:
            crate::le::advertising::LegacyConnectableAdvertisingRuntimeResources,
    ) -> Result<
        ControllerInterruptOwnersPublished<
            P,
            S::Published,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
        >,
        ControllerInterruptOwnerPublicationFailure<P, S, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
    >
    where
        S: InterruptOwnerStorage,
    {
        let Self {
            initialized,
            _interrupts: interrupts,
            _timer: timer,
            runtime_control,
        } = self;
        match storage.publish(interrupts, timer) {
            Ok(published) => Ok(ControllerInterruptOwnersPublished {
                initialized,
                _storage: published,
                post_unlink_mailbox: DtmPostUnlinkMailbox::new(),
                runtime_control,
                scheduler_epoch: None,
                roles: oer_esp32s31_bluetooth::resources::runtime_owner::RuntimeOwnerSlot::new(
                    super::role_retirement::ControllerRoleResources {
                        dtm_resources,
                        legacy_advertising_resources,
                        passive_scan_resources,
                        peripheral_connection_resources,
                        legacy_connectable_advertising_resources,
                    },
                ),
            }),
            Err((error, storage, interrupts, timer)) => {
                Err(ControllerInterruptOwnerPublicationFailure {
                    controller: ControllerInterruptOwnersReady {
                        initialized,
                        _interrupts: interrupts,
                        _timer: timer,
                        runtime_control,
                    },
                    storage,
                    dtm_resources,
                    legacy_advertising_resources,
                    passive_scan_resources,
                    peripheral_connection_resources,
                    legacy_connectable_advertising_resources,
                    error,
                })
            }
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    ControllerOutputTimerStarted<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    /// Transfer both disjoint register owners into their pre-route states.
    ///
    /// This is an ownership-only transition. It performs no MMIO and does not
    /// claim stable placement or a live interrupt epoch.
    pub fn stage_interrupt_owners(
        self,
    ) -> ControllerInterruptOwnersReady<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY> {
        let Self {
            initialized,
            _interrupt_output: interrupt_output,
            timer,
        } = self;
        let runtime_control = timer.runtime_control_observation();
        ControllerInterruptOwnersReady {
            initialized,
            _interrupts: interrupt_output.stage_for_cpu_routes(),
            _timer: timer.stage_for_interrupt(),
            runtime_control,
        }
    }
}

/// Controller activation of a completed BLE-PHY engine.
#[cfg(target_arch = "riscv32")]
pub trait ControllerOutputActivation<
    P,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
>
{
    /// Prepare Controller IRQ output and then start the runtime timer once.
    fn prepare_controller_output_and_start_runtime_timer(
        self,
    ) -> ControllerOutputTimerStarted<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>;
}

#[cfg(target_arch = "riscv32")]
impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    ControllerOutputActivation<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
    for ControllerBlePhyEngineInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    /// Prepare Controller IRQ output and then start the runtime timer once.
    ///
    /// The consuming BLE-PHY state proves that controller HAL, scheduler,
    /// low-power hardware, common PHY, BTBB, BLE PHY initialization and public
    /// Controller-address publication all belong to this epoch. CPU routes
    /// remain inaccessible; the BLE-PHY engine discharges the interrupt
    /// prerequisite when it releases the bank, and hands the timer out only
    /// with the output already prepared.
    fn prepare_controller_output_and_start_runtime_timer(
        mut self,
    ) -> ControllerOutputTimerStarted<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY> {
        let (interrupt_output, timer) = self.take_activation_owners_with_output_prepared();
        let timer = timer.start_runtime_timer();

        ControllerOutputTimerStarted {
            initialized: self,
            _interrupt_output: interrupt_output,
            timer,
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<P, const MT: usize, const SC: usize> ControllerInterruptOwnersReady<P, MT, SC> {
    #[allow(
        clippy::result_large_err,
        reason = "rejection retains the complete initialized hardware graph"
    )]
    pub(super) fn restore_interrupt_owners<S: InterruptOwnerRestartStorage>(
        self,
        storage: &S,
    ) -> Result<
        oer_esp32s31_bluetooth::ble_phy::BlePhyRestartParts<P, MT, SC>,
        (S::RestartError, Self),
    > {
        match storage.restore_initialized_interrupt_owners(self._interrupts, self._timer) {
            Ok(()) => Ok(self.initialized.into_restart_parts()),
            Err((error, interrupts, timer)) => Err((
                error,
                Self {
                    initialized: self.initialized,
                    _interrupts: interrupts,
                    _timer: timer,
                    runtime_control: self.runtime_control,
                },
            )),
        }
    }
}
