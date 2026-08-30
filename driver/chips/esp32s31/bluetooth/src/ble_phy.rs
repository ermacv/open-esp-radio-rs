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

/// One standalone RF-ready observation in the retained scheduler epoch.
///
/// Only a completed generation-keyed controller-time request owned by an
/// initialized BLE PHY can create this affine token. It deliberately cannot be
/// copied or manufactured from a detached scheduler image.
#[must_use = "the RF-ready observation must be consumed by DTM scheduling"]
#[cfg(target_arch = "riscv32")]
pub(crate) struct BluetoothDtmRfReady {
    scheduler_instant: crate::BluetoothDtmSchedulerInstant,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothDtmRfReady {
    fn from_completed_sample(
        epoch: crate::BluetoothControllerSchedulerEpoch,
        sample: crate::BluetoothControllerTimeSample,
    ) -> Self {
        Self {
            scheduler_instant: crate::BluetoothDtmSchedulerInstant::from_image(
                epoch.project_without_reanchor(&sample),
            ),
        }
    }

    /// Consume the RF-ready proof into its scheduler-domain instant.
    pub(crate) const fn into_scheduler_instant(self) -> crate::BluetoothDtmSchedulerInstant {
        self.scheduler_instant
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
    _storage: BluetoothBlePhyEngineCpuOwned,
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

    /// Scheduler scale retained by this exact initialized Controller epoch.
    pub(crate) const fn controller_time_scale(
        &self,
    ) -> open_esp_radio_esp32s31_pac::BluetoothControllerTimeScale {
        self.initialized
            .initialized
            .controller
            .controller_time_scale()
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

    pub(crate) const fn modem_lp_timer_worker_wake(
        &self,
    ) -> &crate::BluetoothModemLpTimerWorkerWakeCell {
        self.initialized
            .initialized
            .controller
            .modem_lp_timer_worker_wake()
    }

    pub(crate) fn task_mut(&mut self) -> &mut crate::resources::BluetoothTaskResources {
        self.initialized.initialized.controller.task_mut()
    }

    /// Split the already initialized scheduler and HCI resources after this
    /// complete BLE-PHY owner has reached stable final placement.
    pub(crate) fn split_runtime(
        &mut self,
    ) -> crate::BluetoothControllerRuntimeEndpoints<
        '_,
        M,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    > {
        self.initialized.initialized.controller.split_runtime()
    }

    /// Borrow the sole scheduler owner for upper phase-ordered DTM preparation.
    pub(crate) fn dtm_scheduler_mut(
        &mut self,
    ) -> &mut crate::BluetoothSchedulerInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
    {
        self.initialized.initialized.controller.dtm_scheduler_mut()
    }

    #[cfg(target_arch = "riscv32")]
    #[expect(
        clippy::result_large_err,
        reason = "the internal no-alloc delegation preserves the complete rejected graph"
    )]
    pub(crate) fn cancel_dtm_transmitter_first_item(
        &mut self,
        merged: crate::BluetoothDtmEmptySchedulerMergePrepared<
            crate::BluetoothDtmTransmitterEvent,
            crate::BluetoothDtmInitialSchedulerItemPhase,
        >,
    ) -> Result<
        (
            open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmMemoryGraphCpuOwned,
            crate::BluetoothDtmPayloadPattern,
            crate::BluetoothDtmPayloadLength,
        ),
        crate::BluetoothDtmEmptySchedulerMergePrepared<
            crate::BluetoothDtmTransmitterEvent,
            crate::BluetoothDtmInitialSchedulerItemPhase,
        >,
    > {
        self.initialized
            .initialized
            .controller
            .cancel_dtm_transmitter_first_item(merged)
    }

    #[cfg(target_arch = "riscv32")]
    #[expect(
        clippy::result_large_err,
        reason = "the internal no-alloc delegation preserves the complete rejected graph"
    )]
    pub(crate) fn cancel_dtm_receiver_first_item(
        &mut self,
        merged: crate::BluetoothDtmEmptySchedulerMergePrepared<
            crate::BluetoothDtmReceiverEvent,
            crate::BluetoothDtmInitialSchedulerItemPhase,
        >,
    ) -> Result<
        crate::BluetoothDtmReceiverCpuOwned,
        crate::BluetoothDtmEmptySchedulerMergePrepared<
            crate::BluetoothDtmReceiverEvent,
            crate::BluetoothDtmInitialSchedulerItemPhase,
        >,
    > {
        self.initialized
            .initialized
            .controller
            .cancel_dtm_receiver_first_item(merged)
    }

    #[cfg(target_arch = "riscv32")]
    #[expect(
        clippy::result_large_err,
        reason = "the internal no-alloc delegation preserves the complete rejected graph"
    )]
    pub(crate) fn cancel_dtm_transmitter_recurring_item(
        &mut self,
        merged: crate::BluetoothDtmEmptySchedulerMergePrepared<
            crate::BluetoothDtmTransmitterEvent,
            crate::BluetoothDtmRecurringSchedulerItemPhase,
        >,
    ) -> Result<
        crate::BluetoothDtmActiveTransmitterCpuOwned,
        crate::BluetoothDtmEmptySchedulerMergePrepared<
            crate::BluetoothDtmTransmitterEvent,
            crate::BluetoothDtmRecurringSchedulerItemPhase,
        >,
    > {
        self.initialized
            .initialized
            .controller
            .cancel_dtm_transmitter_recurring_item(merged)
    }

    #[cfg(target_arch = "riscv32")]
    #[expect(
        clippy::result_large_err,
        reason = "the internal no-alloc delegation preserves the complete rejected graph"
    )]
    pub(crate) fn cancel_dtm_receiver_recurring_item(
        &mut self,
        merged: crate::BluetoothDtmEmptySchedulerMergePrepared<
            crate::BluetoothDtmReceiverEvent,
            crate::BluetoothDtmRecurringSchedulerItemPhase,
        >,
    ) -> Result<
        crate::BluetoothDtmActiveReceiverCpuOwned,
        crate::BluetoothDtmEmptySchedulerMergePrepared<
            crate::BluetoothDtmReceiverEvent,
            crate::BluetoothDtmRecurringSchedulerItemPhase,
        >,
    > {
        self.initialized
            .initialized
            .controller
            .cancel_dtm_receiver_recurring_item(merged)
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn retain_running_dtm_first_item(
        &mut self,
        address: open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress,
    ) {
        self.initialized
            .initialized
            .controller
            .retain_running_dtm_first_item(address);
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn observe_dtm_completion<Role>(
        &mut self,
        running: crate::BluetoothDtmSchedulerRunning<Role>,
    ) -> crate::BluetoothDtmSchedulerCompletionStep<Role> {
        self.initialized
            .initialized
            .controller
            .observe_dtm_completion(running)
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn observe_dtm_hardware_head_retirement<Role>(
        &mut self,
        completed: crate::BluetoothDtmSchedulerCompletionObserved<Role>,
    ) -> crate::BluetoothDtmSchedulerHardwareHeadRetirementStep<Role> {
        self.initialized
            .initialized
            .controller
            .observe_dtm_hardware_head_retirement(completed)
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn unlink_dtm_software_list<Role>(
        &mut self,
        observed: crate::BluetoothDtmSchedulerHardwareHeadEmptyObserved<Role>,
    ) -> crate::scheduler::BluetoothDtmSchedulerSoftwareListUnlinkStep<Role> {
        self.initialized
            .initialized
            .controller
            .unlink_dtm_software_list(observed)
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn join_dtm_software_list_removal<Role>(
        &mut self,
        unlinked: crate::BluetoothDtmSchedulerSoftwareListUnlinked<Role>,
        event: crate::BluetoothPrimarySchedulerEvent,
    ) -> crate::scheduler::BluetoothDtmSchedulerSoftwareListRemovalJoin<Role> {
        self.initialized
            .initialized
            .controller
            .join_dtm_software_list_removal(unlinked, event)
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn recycle_dtm_completed<Role>(
        &mut self,
        ready: crate::BluetoothDtmSchedulerSoftwareListRemovalReady<Role>,
    ) -> crate::BluetoothDtmSchedulerRecycleStep<Role> {
        self.initialized
            .initialized
            .controller
            .recycle_dtm_completed(ready)
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn recycle_dtm_receiver_success(
        &mut self,
        ready: crate::BluetoothDtmSchedulerSoftwareListRemovalReady<
            crate::BluetoothDtmReceiverEvent,
        >,
    ) -> crate::BluetoothDtmSchedulerRxSuccessRecycleStep {
        self.initialized
            .initialized
            .controller
            .recycle_dtm_receiver_success(ready)
    }

    pub(crate) fn request_controller_time(
        &mut self,
    ) -> Result<
        crate::controller_time::BluetoothControllerTimeRequest,
        crate::controller_time::BluetoothControllerTimeRequestError,
    > {
        self.initialized
            .initialized
            .controller
            .request_controller_time()
    }

    /// Request the fresh controller-time observation used by standalone DTM
    /// RF readiness.
    ///
    /// The returned identity remains generation-keyed by the sole durable time
    /// worker. Completion must pass its affine sample back to
    /// [`Self::complete_standalone_dtm_rf_ready`].
    pub(crate) fn request_standalone_dtm_rf_ready_time(
        &mut self,
    ) -> Result<
        crate::controller_time::BluetoothControllerTimeRequest,
        crate::controller_time::BluetoothControllerTimeRequestError,
    > {
        self.initialized
            .initialized
            .controller
            .request_controller_time()
    }

    /// Mint the affine standalone RF-ready token from one completed request.
    ///
    /// Projection uses the retained scheduler epoch exactly as observed and
    /// intentionally does not perform the task-run reanchor transition.
    pub(crate) fn complete_standalone_dtm_rf_ready(
        &mut self,
        epoch: crate::BluetoothControllerSchedulerEpoch,
        sample: crate::BluetoothControllerTimeSample,
    ) -> BluetoothDtmRfReady {
        BluetoothDtmRfReady::from_completed_sample(epoch, sample)
    }

    #[expect(
        clippy::result_large_err,
        reason = "the internal no-alloc delegation preserves the complete rejected graph"
    )]
    pub(crate) fn publish_dtm_scheduler_head<Role, Phase>(
        &mut self,
        merged: crate::BluetoothDtmEmptySchedulerMergePrepared<Role, Phase>,
    ) -> Result<
        crate::BluetoothDtmSchedulerHeadPublished<Role>,
        crate::BluetoothDtmSchedulerHeadPublicationFailure<Role, Phase>,
    >
    where
        Phase: crate::BluetoothDtmSchedulerItemPhase<Role>,
    {
        self.initialized
            .initialized
            .controller
            .publish_dtm_scheduler_head(merged)
    }

    pub(crate) fn cancel_owned_controller_time(
        &mut self,
        request: crate::controller_time::BluetoothControllerTimeRequest,
    ) -> Result<(), crate::controller_time::BluetoothControllerTimeEventError> {
        self.initialized
            .initialized
            .controller
            .cancel_owned_controller_time(request)
    }

    pub(crate) fn recheck_owned_controller_time(
        &mut self,
        request: crate::controller_time::BluetoothControllerTimeRequest,
    ) -> Result<
        crate::controller_time::BluetoothControllerTimeEventStep,
        crate::controller_time::BluetoothControllerTimeEventError,
    > {
        self.initialized
            .initialized
            .controller
            .recheck_owned_controller_time(request)
    }

    pub(crate) fn drain_orphan_controller_time(
        &mut self,
    ) -> Result<
        crate::controller_time::BluetoothControllerTimeEventStep,
        crate::controller_time::BluetoothControllerTimeEventError,
    > {
        self.initialized
            .initialized
            .controller
            .drain_orphan_controller_time()
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
        config: BluetoothBlePhyInitializationConfig,
    ) -> BluetoothControllerBlePhyEngineInitialized<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
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
