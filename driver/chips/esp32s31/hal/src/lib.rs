#![no_std]
#![deny(unsafe_code)]

#[cfg(test)]
extern crate std;

use core::future::Future;

use open_esp_radio_esp32s31_pac::{
    Ieee802154TaskRegisters, MacInterruptRegisters as PacMacInterruptRegisters,
    MacInterruptSetup as PacMacInterruptSetup,
    MacPowerInterruptRegisters as PacMacPowerInterruptRegisters, RadioHardware, RadioPhyRegisters,
    RadioPhyReleaseError, WifiColdRegisters, WifiRadioRegisters,
};
pub mod analog_i2c;
pub mod bluetooth;
pub mod channel;
pub mod coex;
pub(crate) mod ieee802154;
#[cfg(any(test, feature = "validation-probes"))]
mod ieee802154_ed_event_probe;
#[cfg(any(test, feature = "validation-probes"))]
mod ieee802154_event_status_probe;
pub mod ieee802154_lifecycle;
mod ieee802154_operation;
mod ieee802154_policy;
mod ieee802154_role;
mod ieee802154_tx_power;
mod modem_clock_planner;
pub mod pbus;
pub mod phy_agc;
pub mod phy_baseband;
pub mod phy_clock;
pub mod phy_frequency;
pub mod phy_i2c;
pub mod phy_iq_estimator;
pub mod phy_memory;
pub mod phy_power_detector;
pub mod phy_prelude;
pub mod phy_rx_dco;
pub mod phy_temperature;
pub mod power;
pub mod power_detector_platform;
pub mod radio_arena;
pub mod types;
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub mod validation;
pub mod wifi_bb;
pub mod wifi_mac;
pub use bluetooth::{
    BluetoothColdOwner, BluetoothColdOwnerReleaseFailure, BluetoothControllerHal,
    BluetoothControllerHalBorrow, BluetoothControllerHalInitConfig, BluetoothControllerLatchedTime,
    BluetoothControllerSchedulerDisableBeginError, BluetoothControllerSchedulerDisableBeginFailure,
    BluetoothControllerSchedulerDisableBusyObserved,
    BluetoothControllerSchedulerDisableIdleObserved, BluetoothControllerSchedulerDisableStep,
    BluetoothControllerSchedulerDisabling, BluetoothControllerTimeLatchBeginError,
    BluetoothControllerTimeLatchStep, BluetoothControllerTimeLatchStepError,
    BluetoothInterruptOutputAfterRoutesOwner, BluetoothInterruptOutputPreparedOwner,
    BluetoothInterruptRegistersOwner, BluetoothInterruptSetupOwner,
    BluetoothLowPowerRuntimeControlObservation, BluetoothModemLpTimerCompareDisposition,
    BluetoothModemLpTimerCounterObservation, BluetoothModemLpTimerCounterStarted,
    BluetoothModemLpTimerEpoch, BluetoothModemLpTimerHandlerPendingOwner,
    BluetoothModemLpTimerHandlerRegisterObservation, BluetoothModemLpTimerHandlerRegisterStep,
    BluetoothModemLpTimerInstant, BluetoothModemLpTimerInterruptObservation,
    BluetoothModemLpTimerInterruptReadyOwner, BluetoothModemLpTimerInterruptStep,
    BluetoothModemLpTimerLowPowerHardwareInitializedOwner,
    BluetoothModemLpTimerRegistersPreparedOwner, BluetoothModemLpTimerSoftwarePendingOwner,
    BluetoothNrtInterruptAcknowledged, BluetoothSchedulerFinishedListObservation,
    BluetoothSchedulerFinishedListPop, BluetoothSchedulerHardwareListHead,
    BluetoothSchedulerHardwareListHeadError, BluetoothSchedulerHardwareListHeadPublished,
    BluetoothSchedulerHardwareListIndex, BluetoothSchedulerHardwareRunCommandPublished,
    BluetoothSchedulerInsertionCommand, BluetoothSchedulerInsertionCommandStartCleared,
    BluetoothSchedulerLockModifyInterruptObservation, BluetoothSchedulerLockModifyObservation,
    BluetoothSchedulerLockModifyPublished, BluetoothSchedulerLockModifyRequest,
    BluetoothSchedulerLockModifyTaskObservation, BluetoothSchedulerReferenceGateObservation,
    BluetoothSchedulerRunInterruptsPrepared, BluetoothSchedulerWorkObservation, BluetoothTaskOwner,
    BluetoothTaskOwnerReuniteError, BluetoothTaskOwnerReuniteFailure,
};
#[cfg(feature = "validation-probes")]
pub use ieee802154_ed_event_probe::{
    Ieee802154EdEventProbeConfig, Ieee802154EdEventProbeEvidence, Ieee802154EdEventProbeIsolation,
    Ieee802154EdEventProbeStop,
};
#[cfg(feature = "validation-probes")]
pub use ieee802154_event_status_probe::{
    Ieee802154EventStatusProbeConfig, Ieee802154EventStatusProbeEvidence,
    Ieee802154EventStatusProbeIsolation, Ieee802154EventStatusProbeStop,
};
pub use ieee802154_lifecycle::{
    IEEE802154_MAX_CHANNEL, IEEE802154_MIN_CHANNEL, Ieee802154Channel, Ieee802154ChannelError,
    Ieee802154ClockCheckpoint, Ieee802154ClockReadback, Ieee802154FoundationCheckpoint,
    Ieee802154ReadbackError, Ieee802154ResetCheckpoint, Ieee802154ResetReadback,
};
pub use ieee802154_operation::{
    Ieee802154OperationEventMaskState, Ieee802154OperationEventObservation,
    Ieee802154OperationPollBudget, Ieee802154OperationRxAbortMaskState, Ieee802154OperationStage,
    Ieee802154PolledOperation, Ieee802154PolledOperationAbortEvidence,
    Ieee802154PolledOperationEvidence, Ieee802154PolledOperationFailure,
    Ieee802154PolledOperationResult,
};
pub use ieee802154_policy::{
    IEEE802154_ACK_TIMEOUT_QUANTUM_MICROSECONDS, IEEE802154_MAX_ACK_TIMEOUT_MICROSECONDS,
    Ieee802154AckTimeout, Ieee802154AckTimeoutError, Ieee802154CcaMode, Ieee802154MacControl,
    Ieee802154MacPolicy, Ieee802154MacPolicyCheckpoint, Ieee802154PanIdentity,
};
pub use ieee802154_role::{
    Ieee802154ClockTransitionFailure, Ieee802154Clocked, Ieee802154FoundationConfigured,
    Ieee802154FoundationTransitionFailure, Ieee802154MacPolicyConfigured,
    Ieee802154MacPolicyRecovery, Ieee802154MacPolicyTransitionFailure,
    Ieee802154OperationCompleted, Ieee802154OperationFailed, Ieee802154Owned,
    Ieee802154PowerTransitionFailure, Ieee802154Powered, Ieee802154Reset,
    Ieee802154ResetTransitionFailure,
};
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub use ieee802154_role::{Ieee802154EdEventProbeFinished, Ieee802154EventStatusProbeFinished};
pub use ieee802154_tx_power::{
    Ieee802154ResolvedTxPower, Ieee802154TxPowerLevels, Ieee802154TxPowerLevelsError,
};
pub use open_esp_radio_esp32s31_pac::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
    BluetoothMemoryListSelector, Ieee802154ObservedEventState,
    Ieee802154OperationRxAbortEnableObservation, Ieee802154RxAbortReason,
    Ieee802154RxAbortReasonObservation, Ieee802154TimingPrerequisite, Ieee802154TimingReady,
    Ieee802154ValidationEdDurationState, Ieee802154ValidationEventEnableState, MacPowerWakeCause,
    MacTsfTimerIndex, PhyAdcRate, StaBeaconMissLimit, StaBeaconMissTimeoutRaw, StaModemSleepLimit,
    StaModemWakeConfig, StaModemWakePrepareError, StaModemWakeRestore, StaModemWakeRestoreError,
    StaModemWakeRestoreFailure, StaTbttAutoPeriod, StaTbttWakePrepareError, StaTbttWakeRestore,
    StaTbttWakeRestoreError, StaTbttWakeRestoreFailure, StaWakeProtectEarlyTimeRaw,
};
pub use power::{PowerCheckpoint, PowerClockReadback, PowerError};
pub use types::{
    CfrValue, ForcedRxGain, MacInterruptEnableState, MacInterruptEvents, MacInterruptMask,
    MacInterruptObservation, MacInterruptSnapshot, MacPowerInterruptObservation,
    MacPowerInterruptSnapshot,
};

/// Powered-lifecycle PHY capability.
///
/// The PAC owner remains private to HAL. PHY code can pass this value only to
/// named HAL operations; it cannot dereference or recover a generic register
/// block from it.
pub struct PhyHal {
    registers: WifiColdRegisters,
}

/// One PAC-observed image of the Wi-Fi baseband enable condition.
///
/// The shared PBus work-mode leaf uses this condition only to decide whether
/// its caller must execute the recovered settle pulse. Keeping it separate
/// prevents the protocol-neutral PHY owner from retaining Wi-Fi state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiBasebandEnableObservation(bool);

impl WifiBasebandEnableObservation {
    /// Record one semantic PAC readback made by the lifecycle owner.
    pub const fn from_pac_readback(enabled: bool) -> Self {
        Self(enabled)
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) const fn is_enabled(self) -> bool {
        self.0
    }
}

/// Narrow borrowed HAL capability for the protocol-neutral radio PHY.
///
/// This value cannot acquire, release, or recover the underlying PAC owner.
/// Its lifetime is bounded by the active Wi-Fi, Bluetooth, or IEEE 802.15.4
/// route.
pub struct SharedPhyHal<'owner> {
    registers: &'owner mut RadioPhyRegisters,
}

mod sealed {
    use super::{
        BluetoothTaskOwner, Ieee802154TaskRegisters, RadioPhyRegisters,
        WifiBasebandEnableObservation,
    };

    pub trait BluetoothSharedPhyBorrow {
        fn radio_phy_mut(&mut self) -> &mut RadioPhyRegisters;
    }

    impl BluetoothSharedPhyBorrow for BluetoothTaskOwner {
        fn radio_phy_mut(&mut self) -> &mut RadioPhyRegisters {
            self.radio_phy_mut()
        }
    }

    pub trait Ieee802154SharedPhyBorrow {
        fn radio_phy_mut(&mut self) -> &mut RadioPhyRegisters;
    }

    impl Ieee802154SharedPhyBorrow for Ieee802154TaskRegisters {
        fn radio_phy_mut(&mut self) -> &mut RadioPhyRegisters {
            self.radio_phy_mut()
        }
    }

    pub trait SharedPhyAccess {
        fn pac(&self) -> &RadioPhyRegisters;
        fn pac_mut(&mut self) -> &mut RadioPhyRegisters;
    }

    pub trait SharedPhyContext {
        fn wifi_baseband_enable_observation(&self) -> WifiBasebandEnableObservation;
    }

    pub trait PhyInitializationAccess {}
}

/// Sealed conversion from the exclusive Bluetooth task owner to one narrow
/// shared-PHY borrow.
///
/// The implementing PAC owner remains private to the Bluetooth hardware
/// boundary. Callers can neither implement this trait for another owner nor
/// recover the underlying register partition from the returned capability.
#[doc(hidden)]
pub trait BluetoothSharedPhyBorrow: sealed::BluetoothSharedPhyBorrow {
    /// Borrow the shared PHY for one finite Bluetooth lower-layer scope.
    ///
    /// The returned capability samples shared Wi-Fi-baseband state through
    /// the retained route PAC owner.
    fn borrow_shared_phy(&mut self) -> SharedPhyHal<'_> {
        SharedPhyHal {
            registers: sealed::BluetoothSharedPhyBorrow::radio_phy_mut(self),
        }
    }
}

impl BluetoothSharedPhyBorrow for BluetoothTaskOwner {}

/// Sealed conversion from the exclusive IEEE 802.15.4 task owner to one
/// narrow shared-PHY borrow.
///
/// This is the same concrete common-PHY port used by the standalone Bluetooth
/// lifecycle. It conveys neither Wi-Fi ownership nor a PHY/RF-ready claim.
#[doc(hidden)]
pub trait Ieee802154SharedPhyBorrow: sealed::Ieee802154SharedPhyBorrow {
    /// Borrow the shared PHY for one finite IEEE 802.15.4 lower-layer scope.
    ///
    /// The returned capability samples shared Wi-Fi-baseband state through
    /// the retained route PAC owner.
    fn borrow_shared_phy(&mut self) -> SharedPhyHal<'_> {
        SharedPhyHal {
            registers: sealed::Ieee802154SharedPhyBorrow::radio_phy_mut(self),
        }
    }
}

impl Ieee802154SharedPhyBorrow for Ieee802154TaskRegisters {}

/// Sealed protocol-neutral port accepted by named PHY HAL operations.
///
/// External crates can use an acquired [`SharedPhyHal`] or Wi-Fi lifecycle
/// borrow but cannot implement this trait for an arbitrary owner or recover
/// the underlying PAC.
pub trait SharedPhyAccess: sealed::SharedPhyAccess {
    fn set_phy_calibration_clock(&mut self, enabled: bool) {
        sealed::SharedPhyAccess::pac_mut(self).set_phy_calibration_clock(enabled);
    }

    fn set_rx_gain_dc_calibration(&mut self, enabled: bool) {
        sealed::SharedPhyAccess::pac_mut(self).set_rx_gain_dc_calibration(enabled);
    }

    fn configure_power_control_tone(&mut self, selector: u16, step: u8) {
        sealed::SharedPhyAccess::pac_mut(self).configure_power_control_tone(selector, step);
    }

    fn configure_calibration_tone(&mut self, enabled: bool, selector: u16, step: u8) {
        sealed::SharedPhyAccess::pac_mut(self).configure_calibration_tone(enabled, selector, step);
    }

    fn configure_tx_iq_correction(&mut self, begin: bool) {
        sealed::SharedPhyAccess::pac_mut(self).configure_tx_iq_correction(begin);
    }

    fn prepare_txiq_tone_control_restore(
        &mut self,
    ) -> Result<(), open_esp_radio_esp32s31_pac::TxIqToneControlPrepareError> {
        sealed::SharedPhyAccess::pac_mut(self).prepare_txiq_tone_control_restore()
    }

    fn restore_txiq_tone_control(
        &mut self,
    ) -> Result<(), open_esp_radio_esp32s31_pac::TxIqToneControlRestoreError> {
        sealed::SharedPhyAccess::pac_mut(self).restore_txiq_tone_control()
    }

    fn configure_txiq_mismatch_power(
        &mut self,
        first: bool,
        polarity: bool,
        attenuation: u8,
        selector: u16,
    ) {
        sealed::SharedPhyAccess::pac_mut(self).configure_txiq_mismatch_power(
            first,
            polarity,
            attenuation,
            selector,
        );
    }

    fn set_tx_iq_gain_coefficient(&mut self, coefficient: i8) {
        sealed::SharedPhyAccess::pac_mut(self).set_tx_iq_gain_coefficient(coefficient);
    }

    fn set_tx_iq_phase_coefficient(&mut self, coefficient: i8) {
        sealed::SharedPhyAccess::pac_mut(self).set_tx_iq_phase_coefficient(coefficient);
    }

    fn set_rx_iq_gain_coefficient(&mut self, coefficient: i8) {
        sealed::SharedPhyAccess::pac_mut(self).set_rx_iq_gain_coefficient(coefficient);
    }

    fn set_rx_iq_phase_coefficient(&mut self, coefficient: i8) {
        sealed::SharedPhyAccess::pac_mut(self).set_rx_iq_phase_coefficient(coefficient);
    }

    fn configure_rx_iq_calibration_mode(&mut self) {
        sealed::SharedPhyAccess::pac_mut(self).configure_rx_iq_calibration_mode();
    }

    fn configure_adc_rate(&mut self, rate: PhyAdcRate) {
        sealed::SharedPhyAccess::pac_mut(self).configure_adc_rate(rate);
    }

    fn set_power_detector_tone_armed(&mut self, armed: bool) {
        sealed::SharedPhyAccess::pac_mut(self).set_power_detector_tone_armed(armed);
    }

    fn stop_power_detector_tone(&mut self) {
        sealed::SharedPhyAccess::pac_mut(self).stop_power_detector_tone();
    }

    fn trigger_tx_dc_measurement(&mut self) {
        sealed::SharedPhyAccess::pac_mut(self).trigger_tx_dc_measurement();
    }

    fn tx_dc_measurement_is_ready(&mut self) -> bool {
        sealed::SharedPhyAccess::pac_mut(self).tx_dc_measurement_is_ready()
    }

    fn sample_tx_dc_comparators(&mut self) -> [bool; 2] {
        sealed::SharedPhyAccess::pac_mut(self).sample_tx_dc_comparators()
    }

    fn clear_tx_dc_measurement(&mut self) {
        sealed::SharedPhyAccess::pac_mut(self).clear_tx_dc_measurement();
    }

    fn open_frontend_baseband_internal_clocks(&mut self) {
        sealed::SharedPhyAccess::pac_mut(self).open_frontend_baseband_internal_clocks();
    }

    fn set_rf_circuit_power(&mut self, enabled: bool) {
        sealed::SharedPhyAccess::pac_mut(self).set_rf_circuit_power(enabled);
    }

    fn set_bb_i2c_power_tie(&mut self, enabled: bool) {
        sealed::SharedPhyAccess::pac_mut(self).set_bb_i2c_power_tie(enabled);
    }

    fn analog_i2c_is_powered(&self) -> bool {
        sealed::SharedPhyAccess::pac(self).analog_i2c_is_powered()
    }

    fn set_analog_i2c_power(&mut self, enabled: bool) {
        sealed::SharedPhyAccess::pac_mut(self).set_analog_i2c_power(enabled);
    }

    fn analog_i2c_reset_is_released(&self) -> bool {
        sealed::SharedPhyAccess::pac(self).analog_i2c_reset_is_released()
    }

    fn set_analog_i2c_reset_released(&mut self, released: bool) {
        sealed::SharedPhyAccess::pac_mut(self).set_analog_i2c_reset_released(released);
    }

    fn enable_frontend_baseband_power(&mut self) {
        sealed::SharedPhyAccess::pac_mut(self).enable_frontend_baseband_power();
    }
}

/// Shared PHY access paired with one explicit Wi-Fi-baseband observation.
///
/// Only the PBus work-mode settle decision needs this additional context.
/// Ordinary shared-PHY leaves require [`SharedPhyAccess`] alone.
pub trait SharedPhyContext: SharedPhyAccess + sealed::SharedPhyContext {
    fn wifi_baseband_enable_observation(&self) -> WifiBasebandEnableObservation {
        sealed::SharedPhyContext::wifi_baseband_enable_observation(self)
    }
}

/// Common PHY-initialization port that tracks temporary Wi-Fi-BB edges.
///
/// `register_chipv7_phy` temporarily drives the physical Wi-Fi-BB enable state
/// even when entered by the standalone Bluetooth lifecycle. Implementations
/// sample it through the retained PAC owner; this capability conveys no Wi-Fi
/// MAC or protocol-role ownership.
pub trait PhyInitializationAccess: SharedPhyContext + sealed::PhyInitializationAccess {}

impl sealed::SharedPhyAccess for PhyHal {
    fn pac(&self) -> &RadioPhyRegisters {
        self.registers.radio().radio_phy()
    }

    fn pac_mut(&mut self) -> &mut RadioPhyRegisters {
        self.registers.radio_mut().radio_phy_mut()
    }
}

impl sealed::SharedPhyContext for PhyHal {
    fn wifi_baseband_enable_observation(&self) -> WifiBasebandEnableObservation {
        WifiBasebandEnableObservation::from_pac_readback(
            self.registers
                .radio()
                .radio_phy()
                .wifi_baseband_is_enabled(),
        )
    }
}

impl SharedPhyAccess for PhyHal {}
impl SharedPhyContext for PhyHal {}

impl sealed::PhyInitializationAccess for PhyHal {}

impl PhyInitializationAccess for PhyHal {}

impl sealed::SharedPhyAccess for SharedPhyHal<'_> {
    fn pac(&self) -> &RadioPhyRegisters {
        self.registers
    }

    fn pac_mut(&mut self) -> &mut RadioPhyRegisters {
        self.registers
    }
}

impl sealed::SharedPhyContext for SharedPhyHal<'_> {
    fn wifi_baseband_enable_observation(&self) -> WifiBasebandEnableObservation {
        WifiBasebandEnableObservation::from_pac_readback(self.registers.wifi_baseband_is_enabled())
    }
}

impl SharedPhyAccess for SharedPhyHal<'_> {}
impl SharedPhyContext for SharedPhyHal<'_> {}

impl sealed::PhyInitializationAccess for SharedPhyHal<'_> {}

impl PhyInitializationAccess for SharedPhyHal<'_> {}

impl sealed::SharedPhyAccess for RadioPhyRegisters {
    fn pac(&self) -> &RadioPhyRegisters {
        self
    }

    fn pac_mut(&mut self) -> &mut RadioPhyRegisters {
        self
    }
}

impl SharedPhyAccess for RadioPhyRegisters {}

pub(crate) fn phy_pac(access: &(impl SharedPhyAccess + ?Sized)) -> &RadioPhyRegisters {
    sealed::SharedPhyAccess::pac(access)
}

pub(crate) fn phy_pac_mut(access: &mut (impl SharedPhyAccess + ?Sized)) -> &mut RadioPhyRegisters {
    sealed::SharedPhyAccess::pac_mut(access)
}

/// Type states for the coarse radio power lifecycle.
pub mod state {
    use super::{MacInterruptSetup, PhyHal, RadioRuntimeOwner, WifiColdRegisters};

    /// The application uniquely owns the peripheral, but the open driver has
    /// not yet established its clock/reset prerequisites.
    pub struct Owned {
        pub(super) registers: WifiColdRegisters,
    }

    /// The radio clock/reset prerequisites have been established and finite
    /// PHY register operations may access MMIO.
    pub struct Powered {
        pub(super) registers: PhyHal,
    }

    /// Cold initialization has completed and task/ISR authority is disjoint.
    pub struct Running {
        pub(super) registers: RadioRuntimeOwner,
        pub(super) interrupts: MacInterruptSetup,
    }
}

/// Unique application-visible owner of an ESP32-S31 radio peripheral.
///
/// `P` is the integration layer's singleton token (for example,
/// `esp_hal::peripherals::WIFI`). Keeping it inside this value ties the open
/// driver's register capability to the safe peripheral owner.
pub struct Radio<P, State = state::Owned> {
    peripheral: P,
    state: State,
}

/// Failed cold Wi-Fi release retaining both application and radio owners.
#[must_use = "failed Wi-Fi release still owns the application token and radio route"]
pub struct RadioReleaseFailure<P> {
    radio: Radio<P, state::Owned>,
    error: RadioPhyReleaseError,
}

impl<P> RadioReleaseFailure<P> {
    pub const fn error(&self) -> RadioPhyReleaseError {
        self.error
    }

    pub fn into_parts(self) -> (Radio<P, state::Owned>, RadioPhyReleaseError) {
        (self.radio, self.error)
    }
}

impl<P> core::fmt::Debug for RadioReleaseFailure<P> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("RadioReleaseFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Opaque task-side owner of the running radio register partition.
///
/// This value can be moved between lifecycle owners and the runtime arena, but
/// it exposes no PAC owner, dereference operation, or generic register
/// callback. Finite hardware transactions are borrowed through HAL
/// capabilities.
pub struct RadioRuntimeOwner {
    registers: WifiRadioRegisters,
}

impl RadioRuntimeOwner {
    /// Construct the running register owner inside an isolated validation
    /// image without exposing the underlying PAC owner.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn claim_for_validation() -> Self {
        Self::from_pac(open_esp_radio_esp32s31_pac::validation::wifi_radio_registers())
    }

    pub fn wifi_mac_hal(&mut self) -> wifi_mac::WifiMacHal<'_> {
        wifi_mac::WifiMacHal::from_owned(&mut self.registers)
    }

    pub fn channel_hal<'owner, P>(
        &'owner mut self,
        platform: &'owner mut P,
    ) -> channel::RadioChannelHal<'owner, P> {
        channel::RadioChannelHal::from_owned(platform, &mut self.registers)
    }

    /// Read the calibrated baseband observation without exposing the PAC
    /// owner or its register encoding.
    pub fn read_noise_floor_dbm(&self) -> i8 {
        self.registers.radio_phy().read_noise_floor_dbm()
    }

    pub fn access_point_receive_policy_snapshot(&self) -> wifi_mac::MacApReceivePolicySnapshot {
        self.registers.ap_receive_policy_snapshot()
    }

    pub fn receive_statistics_snapshot(&self) -> wifi_mac::MacRxStatisticsSnapshot {
        self.registers.rx_statistics_snapshot()
    }

    pub fn receive_dma_snapshot(&self) -> wifi_mac::MacRxDmaSnapshot {
        self.registers.mac_rx_dma_snapshot()
    }

    /// Latch one indirect SoftAP receive-BA bank through the reviewed HAL
    /// projection without exposing its PAC owner.
    pub fn extra_softap_rx_block_ack_entry_snapshot(
        &mut self,
        index: u8,
    ) -> Option<wifi_mac::ExtraSoftApRxBlockAckEntrySnapshot> {
        let index = types::MacExtraSoftApRxBlockAckEntryIndex::new(u32::from(index))?;
        Some(
            self.wifi_mac_hal()
                .extra_softap_rx_block_ack_entry_snapshot(index),
        )
    }

    /// Sample one ordinary direct receive-BA bank without exposing its PAC
    /// owner.
    pub fn rx_block_ack_entry_snapshot(
        &mut self,
        index: u8,
    ) -> Option<wifi_mac::RxBlockAckEntrySnapshot> {
        let index = types::MacRxBlockAckEntryIndex::new(u32::from(index))?;
        self.wifi_mac_hal().rx_block_ack_entry_snapshot(index)
    }

    /// Copy the reviewed Trigger-queue readback for one reservation.
    pub fn he_trigger_based_queue_snapshot(
        &self,
        reservation: types::MacHeTbLinkReservation,
    ) -> types::MacHeTriggerTxQueueSnapshot {
        self.registers.he_trigger_based_queue_snapshot(reservation)
    }

    pub fn he_trigger_receive_diagnostics(&self) -> types::MacHeTriggerRxDiagnostics {
        self.registers.he_trigger_receive_diagnostics()
    }

    pub(crate) fn from_pac(registers: WifiRadioRegisters) -> Self {
        Self { registers }
    }

    pub(crate) fn pac(&self) -> &WifiRadioRegisters {
        &self.registers
    }

    pub(crate) fn pac_mut(&mut self) -> &mut WifiRadioRegisters {
        &mut self.registers
    }
}

/// Task-side setup authority for one finite MAC interrupt epoch.
pub struct MacInterruptSetup {
    inner: PacMacInterruptSetup,
}

/// Proof that connected-STA interrupt policy was applied before activation.
pub struct ConnectedStaInterruptPrepared {
    _private: (),
}

/// Disjoint HAL capability installed in the hard MAC interrupt handler.
pub struct MacInterruptRegisters {
    inner: PacMacInterruptRegisters,
}

impl MacInterruptRegisters {
    pub fn mac_interrupt_status(&self) -> MacInterruptSnapshot {
        self.inner.mac_interrupt_status()
    }

    pub fn acknowledge_mac_interrupts(&mut self, snapshot: MacInterruptSnapshot) {
        self.inner.acknowledge_mac_interrupts(snapshot);
    }

    pub fn mask_rx_delivery_interrupts(&mut self) {
        self.inner.mask_rx_delivery_interrupts();
    }

    pub fn unmask_rx_delivery_interrupts(&mut self) {
        self.inner.unmask_rx_delivery_interrupts();
    }

    pub fn deactivate(self, power: MacPowerInterruptRegisters) -> MacInterruptSetup {
        MacInterruptSetup {
            inner: self.inner.deactivate(power.inner),
        }
    }
}

/// Construct the task-side interrupt setup owner inside an isolated probe.
///
/// The returned value is the same finite capability consumed by production
/// connected-STA setup; only its singleton acquisition is validation-only.
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub fn validation_mac_interrupt_setup() -> MacInterruptSetup {
    MacInterruptSetup {
        inner: open_esp_radio_esp32s31_pac::validation::mac_interrupt_setup(),
    }
}

/// Construct the hard-MAC interrupt capability inside an isolated validation
/// image without exposing the PAC partition.
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub fn validation_mac_interrupt_registers() -> MacInterruptRegisters {
    MacInterruptRegisters {
        inner: open_esp_radio_esp32s31_pac::validation::mac_interrupt_registers(),
    }
}

/// Construct the hard power-interrupt capability inside an isolated probe.
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub fn validation_mac_power_interrupt_registers() -> MacPowerInterruptRegisters {
    MacPowerInterruptRegisters {
        inner: open_esp_radio_esp32s31_pac::validation::mac_power_interrupt_registers(),
    }
}

/// Disjoint HAL capability installed in the hard power interrupt handler.
pub struct MacPowerInterruptRegisters {
    inner: PacMacPowerInterruptRegisters,
}

impl MacPowerInterruptRegisters {
    pub fn mask_and_acknowledge_wake_cause(&mut self, cause: MacPowerWakeCause) {
        self.inner.mask_and_acknowledge_wake_cause(cause);
    }

    pub fn acknowledge_wake_cause(&mut self, cause: MacPowerWakeCause) {
        self.inner.acknowledge_wake_cause(cause);
    }

    pub fn power_interrupt_status(&self) -> MacPowerInterruptSnapshot {
        self.inner.power_interrupt_status()
    }

    pub fn acknowledge_power_interrupts(&mut self, snapshot: MacPowerInterruptSnapshot) {
        self.inner.acknowledge_power_interrupts(snapshot);
    }
}

impl MacInterruptSetup {
    pub fn prepare_connected_sta_without_power_save(
        &mut self,
        radio: &mut RadioRuntimeOwner,
    ) -> ConnectedStaInterruptPrepared {
        let _ = self
            .inner
            .prepare_connected_sta_without_power_save(&mut radio.registers);
        ConnectedStaInterruptPrepared { _private: () }
    }

    pub(crate) fn prepare_connected_sta_with_pac(
        &mut self,
        registers: &mut WifiRadioRegisters,
    ) -> ConnectedStaInterruptPrepared {
        let _ = self
            .inner
            .prepare_connected_sta_without_power_save(registers);
        ConnectedStaInterruptPrepared { _private: () }
    }

    pub fn activate(
        self,
        event_mask: MacInterruptMask,
    ) -> (MacInterruptRegisters, MacPowerInterruptRegisters) {
        let (mac, power) = self.inner.activate(event_mask);
        (
            MacInterruptRegisters { inner: mac },
            MacPowerInterruptRegisters { inner: power },
        )
    }
}

/// Failed power transition retaining the unique partially-mutated radio owner.
///
/// The contained owner deliberately has no `release` or protocol-switch
/// escape. Platform clocks and resets may already differ from the cold
/// baseline, so only a controlled retry may recover a powered owner.
pub struct PowerUpFailure<P> {
    radio: Radio<P, state::Owned>,
    error: PowerError,
}

impl<P> PowerUpFailure<P> {
    /// Inspect the exact failed read-back checkpoint.
    pub const fn error(&self) -> PowerError {
        self.error
    }
}

impl<P> PowerUpFailure<P> {
    /// Retry the idempotent prerequisite sequence without exposing a cold
    /// owner or allowing a protocol switch through partially-mutated state.
    pub fn retry(self) -> Result<Radio<P, state::Powered>, Self> {
        self.radio.power_up()
    }
}

impl<P> Radio<P, state::Owned> {
    /// Bind the integration layer's unique peripheral token to the open
    /// driver's register capability.
    ///
    /// The platform token and custom radio PAC singleton must both be free.
    /// A second claim returns the platform token unchanged.
    pub fn claim(peripheral: P) -> Result<Self, P> {
        #[cfg(not(test))]
        let Some(hardware) = RadioHardware::take() else {
            return Err(peripheral);
        };
        #[cfg(test)]
        let hardware = RadioHardware::for_validation();
        Ok(Self::from_hardware(peripheral, hardware))
    }

    /// Bind an already-owned neutral radio root to the standalone Wi-Fi HAL.
    ///
    /// This is the consuming re-entry after [`Self::release`] or after a
    /// mutually exclusive Bluetooth lifecycle has returned the same root. It
    /// does not acquire another singleton and performs no MMIO.
    pub fn from_hardware(peripheral: P, hardware: RadioHardware) -> Self {
        Self {
            peripheral,
            state: state::Owned {
                registers: hardware.into_wifi(),
            },
        }
    }

    /// Construct the complete owner inside an isolated validation image.
    ///
    /// This bypasses only the process-wide singleton acquisition required by
    /// production. The returned ownership and lifecycle API is identical.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn claim_for_validation(peripheral: P) -> Self {
        Self {
            peripheral,
            state: state::Owned {
                registers: RadioHardware::for_validation().into_wifi(),
            },
        }
    }

    /// Release a radio that has not crossed into the powered state.
    ///
    /// Both singleton authorities are returned. Dropping the neutral radio
    /// root would permanently make the Wi-Fi and Bluetooth routes
    /// unavailable for this boot.
    ///
    /// # Errors
    ///
    /// Returns [`RadioReleaseFailure`] with this complete radio owner when a
    /// pending TX-DC PWDET, TX-IQ, RX-DCO, or Bluetooth TX-power calibration must restore its
    /// PAC-owned state first.
    pub fn release(self) -> Result<(P, RadioHardware), RadioReleaseFailure<P>> {
        let Self { peripheral, state } = self;
        match state.registers.release() {
            Ok(hardware) => Ok((peripheral, hardware)),
            Err(failure) => {
                let (registers, error) = failure.into_parts();
                Err(RadioReleaseFailure {
                    radio: Radio {
                        peripheral,
                        state: state::Owned { registers },
                    },
                    error,
                })
            }
        }
    }

    /// Construct a powered owner inside an isolated validation process.
    ///
    /// The validation harness is responsible for establishing the hardware
    /// prerequisites before using this value. Production firmware cannot
    /// enable this API.
    #[cfg(any(test, feature = "validation-probes"))]
    #[doc(hidden)]
    pub fn assume_powered_for_validation(self) -> Radio<P, state::Powered> {
        Radio {
            peripheral: self.peripheral,
            state: state::Powered {
                registers: PhyHal {
                    registers: self.state.registers,
                },
            },
        }
    }
}

impl<P> Radio<P, state::Owned> {
    /// Execute the finite modem/PHY clock and reset prerequisites.
    ///
    /// Every register transaction goes through the reviewed custom PAC. The
    /// exact operation order reproduces the qualified S31 `esp-hal` clock
    /// path; the ROM-only frontend gates are a
    /// later owned PHY transition and are not folded into this type-state
    /// change.
    ///
    /// `P` remains an affine integration witness. A successful PAC read-back
    /// is the only safe path into `Radio<P, Powered>`.
    pub fn power_up(mut self) -> Result<Radio<P, state::Powered>, PowerUpFailure<P>> {
        if let Err(error) = power::execute_owned(&mut self.state.registers) {
            return Err(PowerUpFailure { radio: self, error });
        }
        Ok(Radio {
            peripheral: self.peripheral,
            state: state::Powered {
                registers: PhyHal {
                    registers: self.state.registers,
                },
            },
        })
    }
}

impl<P> Radio<P, state::Powered> {
    /// Borrow the platform and the narrow cold-MAC capability together for
    /// one lifecycle transition.
    pub fn cold_mac_parts(&mut self) -> (&mut P, wifi_mac::WifiMacColdHal<'_>) {
        (
            &mut self.peripheral,
            wifi_mac::WifiMacColdHal::from_owned(&mut self.state.registers.registers),
        )
    }

    /// Close the cold polling interrupt phase before constructing disjoint
    /// task/ISR runtime capabilities.
    pub fn close_cold_interrupt_phase(&mut self) -> MacInterruptEnableState {
        let mask = self.state.registers.registers.mac_interrupt_enable();
        self.state
            .registers
            .registers
            .mask_and_clear_all_mac_interrupts();
        mask
    }

    /// Borrow the integration token without releasing register ownership.
    pub const fn peripheral(&self) -> &P {
        &self.peripheral
    }

    /// Borrow a narrow channel capability for one transaction.
    pub fn channel_hal(&mut self) -> channel::RadioChannelHal<'_, P> {
        channel::RadioChannelHal::from_owned(
            &mut self.peripheral,
            self.state.registers.registers.radio_mut(),
        )
    }

    /// Borrow the platform and PHY capability independently.
    ///
    /// The two mutable borrows are tied to this unique powered owner and refer
    /// to disjoint fields, allowing a lifecycle function to coordinate an
    /// official system operation with internal Wi-Fi MMIO.
    pub fn phy_hal_parts(&mut self) -> (&mut P, &mut PhyHal) {
        (&mut self.peripheral, &mut self.state.registers)
    }

    /// Enable the Wi-Fi RX/baseband path after the PHY transition completes.
    ///
    /// Espressif's `enable_phy_with_wifi_rx` lifecycle wrapper performs this
    /// operation after `register_chipv7_phy` or `phy_wakeup_init`.  Keeping it
    /// on the powered owner makes that final lifecycle edge explicit and
    /// prevents application code from writing `WIFI_BB_CFG` without owning the
    /// radio peripheral.
    /// Internal PHY capability used by source-owned target bindings.
    ///
    /// The returned borrow cannot outlive the unique powered radio owner.
    pub fn phy_hal_mut(&mut self) -> &mut PhyHal {
        &mut self.state.registers
    }
}

impl<P> Radio<P, state::Powered> {
    /// Complete the one-way ownership transition after cold MAC setup.
    pub fn into_running(self) -> Radio<P, state::Running> {
        let (registers, interrupts) = self.state.registers.registers.into_running();
        Radio {
            peripheral: self.peripheral,
            state: state::Running {
                registers: RadioRuntimeOwner::from_pac(registers),
                interrupts: MacInterruptSetup { inner: interrupts },
            },
        }
    }
}

impl<P> Radio<P, state::Running> {
    pub const fn peripheral(&self) -> &P {
        &self.peripheral
    }

    pub fn channel_hal(&mut self) -> channel::RadioChannelHal<'_, P> {
        self.state.registers.channel_hal(&mut self.peripheral)
    }

    pub fn wifi_mac_hal(&mut self) -> wifi_mac::WifiMacHal<'_> {
        self.state.registers.wifi_mac_hal()
    }

    /// Split only at the role-epoch ownership boundary. Both returned
    /// capabilities remain opaque and can be recombined only through
    /// [`Self::from_runtime_parts`].
    pub fn into_runtime_parts(self) -> (P, RadioRuntimeOwner, MacInterruptSetup) {
        (self.peripheral, self.state.registers, self.state.interrupts)
    }

    pub fn from_runtime_parts(
        peripheral: P,
        registers: RadioRuntimeOwner,
        interrupts: MacInterruptSetup,
    ) -> Self {
        Self {
            peripheral,
            state: state::Running {
                registers,
                interrupts,
            },
        }
    }
}

impl<P> Radio<P, state::Powered> {
    /// Enable the Wi-Fi RX/baseband path after the PHY transition completes.
    ///
    /// The typed route-PAC operation and the PBus-visible owned state are
    /// updated together under the unique radio owner.
    #[cfg(target_arch = "riscv32")]
    pub fn enable_wifi_rx(&mut self) {
        let (_, registers) = self.phy_hal_parts();
        phy_frequency::set_wifi_enabled(registers, true);
    }
}

/// Executor-neutral source of asynchronous deadlines.
pub trait AsyncDelay {
    type Error;

    fn delay_micros(&mut self, micros: u32) -> impl Future<Output = Result<(), Self::Error>> + '_;
}

/// Executor-neutral interrupt/event edge.
pub trait AsyncEvent {
    type Event;
    type Error;

    fn wait(&mut self) -> impl Future<Output = Result<Self::Event, Self::Error>> + '_;
}

#[cfg(test)]
mod tests {
    use super::{Radio, state};

    #[derive(Debug, Eq, PartialEq)]
    struct TestPeripheral {
        id: u8,
        ready: bool,
    }

    fn require_owned(_: &Radio<TestPeripheral, state::Owned>) {}
    fn require_powered(_: &Radio<TestPeripheral, state::Powered>) {}

    #[test]
    #[cfg(target_arch = "riscv32")]
    fn peripheral_token_follows_the_type_state_owner() {
        let owned = Radio::claim(TestPeripheral { id: 7, ready: true })
            .unwrap_or_else(|_| panic!("test radio claim failed"));
        require_owned(&owned);

        let powered = owned
            .power_up()
            .unwrap_or_else(|_| panic!("fake prerequisite sequence failed"));
        require_powered(&powered);
        assert_eq!(powered.peripheral(), &TestPeripheral { id: 7, ready: true });
    }

    #[test]
    fn validation_powered_owner_preserves_the_unique_owner() {
        let owned = Radio::claim(TestPeripheral { id: 8, ready: true })
            .unwrap_or_else(|_| panic!("test radio claim failed"));
        require_owned(&owned);

        let powered = owned.assume_powered_for_validation();
        require_powered(&powered);
        assert_eq!(powered.peripheral(), &TestPeripheral { id: 8, ready: true });
    }

    #[test]
    fn channel_capability_is_a_temporary_borrow_not_a_consuming_split() {
        let owned = Radio::claim(TestPeripheral {
            id: 10,
            ready: true,
        })
        .unwrap_or_else(|_| panic!("test radio claim failed"));
        let mut powered = owned.assume_powered_for_validation();
        let channel = powered.channel_hal();
        drop(channel);
        assert_eq!(powered.peripheral().id, 10);
    }

    #[test]
    fn unpowered_owner_releases_platform_and_neutral_radio_roots() {
        let owned = Radio::claim(TestPeripheral { id: 9, ready: true })
            .unwrap_or_else(|_| panic!("test radio claim failed"));
        let (peripheral, hardware) = owned
            .release()
            .expect("an untouched Wi-Fi route can be released");
        assert_eq!(peripheral, TestPeripheral { id: 9, ready: true });

        let hardware = hardware
            .into_bluetooth()
            .release()
            .expect("an untouched Bluetooth route can be released");
        let _wifi = hardware.into_wifi();
    }

    #[test]
    fn released_hardware_reenters_wifi_after_exclusive_bluetooth_route() {
        let owned = Radio::claim(TestPeripheral {
            id: 12,
            ready: true,
        })
        .unwrap_or_else(|_| panic!("test radio claim failed"));
        let (peripheral, hardware) = owned
            .release()
            .expect("an untouched Wi-Fi route can be released");
        let hardware = hardware
            .into_bluetooth()
            .release()
            .expect("an untouched Bluetooth route can be released");

        let returned = Radio::from_hardware(peripheral, hardware);
        require_owned(&returned);
        let (peripheral, _hardware) = returned
            .release()
            .expect("the returned untouched route can be released");
        assert_eq!(
            peripheral,
            TestPeripheral {
                id: 12,
                ready: true
            }
        );
    }

    #[test]
    #[cfg(target_arch = "riscv32")]
    fn failed_power_transition_can_only_retry_the_unique_owner() {
        let owned = Radio::claim(TestPeripheral {
            id: 11,
            ready: false,
        })
        .unwrap_or_else(|_| panic!("test radio claim failed"));
        let failure = match owned.power_up() {
            Ok(_) => panic!("stuck reset unexpectedly powered the radio"),
            Err(failure) => failure,
        };
        let first_error = failure.error();
        let retried = match failure.retry() {
            Ok(_) => panic!("stuck reset unexpectedly powered the radio on retry"),
            Err(failure) => failure,
        };
        assert_eq!(retried.error(), first_error);
    }
}
