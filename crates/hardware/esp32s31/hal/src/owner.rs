//! Hardware ownership, capabilities, and affine lifecycle transitions.

use crate::{
    bluetooth::TaskOwner,
    ieee80211::{channel, mac as wifi_mac},
    power::PowerError,
    types::{
        MacInterruptEnableState, MacInterruptMask, MacInterruptSnapshot, MacPowerInterruptSnapshot,
        MacPowerWakeCause, PhyAdcRate,
    },
};

use super::*;

use crate::phy::restore::PhyRouteState;
use crate::route_registers::WifiRegisters;

mod interrupt_checkpoint;
pub mod maintenance;
mod wifi_cold;

pub use crate::phy::registration::PhyRegistrationEpoch;
pub use interrupt_checkpoint::MacInterruptCheckpoint;
pub(crate) use wifi_cold::{WifiColdRegisters, WifiRouteState};

/// Powered-lifecycle PHY capability.
///
/// The PAC owner remains private to HAL. PHY code can pass this value only to
/// named HAL operations; it cannot dereference or recover a generic register
/// block from it.
pub struct PhyHal {
    pub(crate) registers: WifiColdRegisters,
    /// Whether this exclusive route's PHY grant-protect request is programmed.
    pub(crate) grant_protected: bool,
}

/// One PAC-observed image of the Wi-Fi baseband enable condition.
///
/// The shared PBus work-mode leaf uses this condition only to decide whether
/// its caller must execute the recovered settle pulse. Keeping it separate
/// prevents the protocol-neutral PHY owner from retaining Wi-Fi state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiBasebandEnableObservation(pub(crate) bool);

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

/// Protocol routes that can lend the shared radio PHY.
///
/// The route is part of every [`SharedPhyHal`] type, so an operation that is
/// valid only for one protocol cannot accept a borrow minted by another. For
/// example, Wi-Fi maintenance access cannot register or close the Bluetooth
/// PHY client.
pub mod route {
    mod sealed {
        pub trait Route {}
    }

    /// Closed set of routes; downstream crates cannot add a lender.
    pub trait Route: sealed::Route {}

    /// Borrowed from checked Wi-Fi maintenance access.
    pub enum Wifi {}
    /// Borrowed from the Bluetooth task owner.
    pub enum Bluetooth {}
    /// Borrowed through a lease of the shared radio arbiter.
    pub enum Shared {}

    impl sealed::Route for Wifi {}
    impl sealed::Route for Bluetooth {}
    impl sealed::Route for Shared {}
    impl Route for Wifi {}
    impl Route for Bluetooth {}
    impl Route for Shared {}
}

/// Narrow borrowed HAL capability for the protocol-neutral radio PHY.
///
/// This value cannot acquire, release, or recover the underlying PAC owner.
/// Its lifetime is bounded by the active route `R` that lent it.
///
/// A borrow keeps its route through ordinary moves:
///
/// ```
/// use oer_esp32s31_hal::owner::{SharedPhyHal, route};
/// fn keep(phy: SharedPhyHal<'_, route::Wifi>) -> SharedPhyHal<'_, route::Wifi> {
///     phy
/// }
/// ```
///
/// A borrow from one route is a different type from a borrow of another:
///
/// ```compile_fail
/// use oer_esp32s31_hal::owner::{SharedPhyHal, route};
/// fn relabel(phy: SharedPhyHal<'_, route::Wifi>) -> SharedPhyHal<'_, route::Bluetooth> {
///     phy
/// }
/// ```
pub struct SharedPhyHal<'owner, R: route::Route> {
    pub(crate) registers: &'owner mut RadioPhyRegisters,
    pub(crate) restore: &'owner mut PhyRouteState,
    route: core::marker::PhantomData<fn() -> R>,
}

impl<'owner, R: route::Route> SharedPhyHal<'owner, R> {
    pub(crate) fn new(
        registers: &'owner mut RadioPhyRegisters,
        restore: &'owner mut PhyRouteState,
    ) -> Self {
        Self {
            registers,
            restore,
            route: core::marker::PhantomData,
        }
    }
}

pub(crate) mod sealed {

    use crate::{
        bluetooth::TaskOwner, owner::WifiBasebandEnableObservation, phy::restore::PhyRouteState,
    };

    use super::RadioPhyRegisters;

    pub trait SharedPhyBorrow {
        fn phy_parts_mut(&mut self) -> (&mut RadioPhyRegisters, &mut PhyRouteState);
    }

    impl SharedPhyBorrow for TaskOwner {
        fn phy_parts_mut(&mut self) -> (&mut RadioPhyRegisters, &mut PhyRouteState) {
            self.phy_parts_mut()
        }
    }

    pub trait SharedPhyAccess {
        fn pac(&self) -> &RadioPhyRegisters;
        /// The route PHY software state lent together with the registers.
        fn route_state(&self) -> &PhyRouteState;
        /// Borrow the shared PHY registers together with the route restore
        /// slot that serializes PHY calibrations.
        fn parts_mut(&mut self) -> (&mut RadioPhyRegisters, &mut PhyRouteState);
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
pub trait SharedPhyBorrow: sealed::SharedPhyBorrow {
    /// Borrow the shared PHY for one finite Bluetooth lower-layer scope.
    ///
    /// The returned capability samples shared Wi-Fi-baseband state through
    /// the retained route PAC owner.
    fn borrow_shared_phy(&mut self) -> SharedPhyHal<'_, route::Bluetooth> {
        let (registers, restore) = sealed::SharedPhyBorrow::phy_parts_mut(self);
        SharedPhyHal::new(registers, restore)
    }
}

impl SharedPhyBorrow for TaskOwner {}

/// Sealed protocol-neutral port accepted by named PHY HAL operations.
///
/// External crates can use an acquired [`SharedPhyHal`] or Wi-Fi lifecycle
/// borrow but cannot implement this trait for an arbitrary owner or recover
/// the underlying PAC.
pub trait SharedPhyAccess: sealed::SharedPhyAccess {
    /// The registration that currently describes this PHY partition.
    ///
    /// A registration result held apart from its hardware is valid for this
    /// borrow only while its recorded epoch equals this value.
    fn registration_epoch(&self) -> Option<PhyRegistrationEpoch> {
        sealed::SharedPhyAccess::route_state(self).registration_epoch()
    }

    fn set_phy_calibration_clock(&mut self, enabled: bool) {
        phy_pac_mut(self).set_phy_calibration_clock(enabled);
    }

    fn set_rx_gain_dc_calibration(&mut self, enabled: bool) {
        phy_pac_mut(self).set_rx_gain_dc_calibration(enabled);
    }

    fn configure_power_control_tone(&mut self, selector: u16, step: u8) {
        phy_pac_mut(self).configure_power_control_tone(selector, step);
    }

    fn configure_calibration_tone(&mut self, enabled: bool, selector: u16, step: u8) {
        phy_pac_mut(self).configure_calibration_tone(enabled, selector, step);
    }

    fn configure_tx_iq_correction(&mut self, begin: bool) {
        phy_pac_mut(self).configure_tx_iq_correction(begin);
    }

    /// Capture the first-path TX-IQ tone-control state into the route slot.
    ///
    /// A second caller is rejected before reading the register and therefore
    /// cannot replace another calibration's restore authority.
    fn prepare_txiq_tone_control_restore(
        &mut self,
    ) -> Result<(), crate::phy::restore::TxIqToneControlPrepareError> {
        let (phy, restore) = sealed::SharedPhyAccess::parts_mut(self);
        restore.prepare_txiq_with(|| phy.capture_txiq_tone_control())
    }

    /// Restore and consume the saved TX-IQ tone-control state.
    ///
    /// A caller without a successful prepare operation is rejected before
    /// MMIO. The slot is cleared only after the complete accessor write.
    fn restore_txiq_tone_control(
        &mut self,
    ) -> Result<(), crate::phy::restore::TxIqToneControlRestoreError> {
        let (phy, restore) = sealed::SharedPhyAccess::parts_mut(self);
        restore.restore_txiq_with(|fields| phy.restore_txiq_tone_control(fields))
    }

    fn configure_txiq_mismatch_power(
        &mut self,
        first: bool,
        polarity: bool,
        attenuation: u8,
        selector: u16,
    ) {
        phy_pac_mut(self).configure_txiq_mismatch_power(first, polarity, attenuation, selector);
    }

    fn set_tx_iq_gain_coefficient(&mut self, coefficient: i8) {
        phy_pac_mut(self).set_tx_iq_gain_coefficient(coefficient);
    }

    fn set_tx_iq_phase_coefficient(&mut self, coefficient: i8) {
        phy_pac_mut(self).set_tx_iq_phase_coefficient(coefficient);
    }

    fn set_rx_iq_gain_coefficient(&mut self, coefficient: i8) {
        phy_pac_mut(self).set_rx_iq_gain_coefficient(coefficient);
    }

    fn set_rx_iq_phase_coefficient(&mut self, coefficient: i8) {
        phy_pac_mut(self).set_rx_iq_phase_coefficient(coefficient);
    }

    fn configure_rx_iq_calibration_mode(&mut self) {
        phy_pac_mut(self).configure_rx_iq_calibration_mode();
    }

    fn configure_adc_rate(&mut self, rate: PhyAdcRate) {
        phy_pac_mut(self).configure_adc_rate(rate);
    }

    fn set_power_detector_tone_armed(&mut self, armed: bool) {
        phy_pac_mut(self).set_power_detector_tone_armed(armed);
    }

    fn stop_power_detector_tone(&mut self) {
        phy_pac_mut(self).stop_power_detector_tone();
    }

    fn trigger_tx_dc_measurement(&mut self) {
        phy_pac_mut(self).trigger_tx_dc_measurement();
    }

    fn tx_dc_measurement_is_ready(&mut self) -> bool {
        phy_pac_mut(self).tx_dc_measurement_is_ready()
    }

    fn sample_tx_dc_comparators(&mut self) -> [bool; 2] {
        phy_pac_mut(self).sample_tx_dc_comparators()
    }

    fn clear_tx_dc_measurement(&mut self) {
        phy_pac_mut(self).clear_tx_dc_measurement();
    }

    fn open_frontend_baseband_internal_clocks(&mut self) {
        phy_pac_mut(self).open_frontend_baseband_internal_clocks();
    }

    fn set_rf_circuit_power(&mut self, enabled: bool) {
        phy_pac_mut(self).set_rf_circuit_power(enabled);
    }

    fn set_bb_i2c_power_tie(&mut self, enabled: bool) {
        phy_pac_mut(self).set_bb_i2c_power_tie(enabled);
    }

    fn analog_i2c_is_powered(&self) -> bool {
        sealed::SharedPhyAccess::pac(self).analog_i2c_is_powered()
    }

    fn set_analog_i2c_power(&mut self, enabled: bool) {
        phy_pac_mut(self).set_analog_i2c_power(enabled);
    }

    fn analog_i2c_reset_is_released(&self) -> bool {
        sealed::SharedPhyAccess::pac(self).analog_i2c_reset_is_released()
    }

    fn set_analog_i2c_reset_released(&mut self, released: bool) {
        phy_pac_mut(self).set_analog_i2c_reset_released(released);
    }

    fn enable_frontend_baseband_power(&mut self) {
        phy_pac_mut(self).enable_frontend_baseband_power();
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
pub trait PhyInitializationAccess: SharedPhyContext + sealed::PhyInitializationAccess {
    /// Begin a registration and retire every earlier epoch of this partition.
    ///
    /// Registration calls this before its first hardware edge. Any other call
    /// only invalidates existing registration results; it cannot mint one.
    fn begin_registration_epoch(&mut self) -> PhyRegistrationEpoch {
        sealed::SharedPhyAccess::parts_mut(self)
            .1
            .begin_registration_epoch()
    }
}

impl sealed::SharedPhyAccess for PhyHal {
    fn pac(&self) -> &RadioPhyRegisters {
        self.registers.radio().radio_phy()
    }

    fn route_state(&self) -> &PhyRouteState {
        self.registers.phy_state()
    }

    fn parts_mut(&mut self) -> (&mut RadioPhyRegisters, &mut PhyRouteState) {
        self.registers.phy_parts_mut()
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

impl<R: route::Route> sealed::SharedPhyAccess for SharedPhyHal<'_, R> {
    fn pac(&self) -> &RadioPhyRegisters {
        self.registers
    }

    fn route_state(&self) -> &PhyRouteState {
        self.restore
    }

    fn parts_mut(&mut self) -> (&mut RadioPhyRegisters, &mut PhyRouteState) {
        (self.registers, self.restore)
    }
}

impl<R: route::Route> sealed::SharedPhyContext for SharedPhyHal<'_, R> {
    fn wifi_baseband_enable_observation(&self) -> WifiBasebandEnableObservation {
        WifiBasebandEnableObservation::from_pac_readback(self.registers.wifi_baseband_is_enabled())
    }
}

impl<R: route::Route> SharedPhyAccess for SharedPhyHal<'_, R> {}
impl<R: route::Route> SharedPhyContext for SharedPhyHal<'_, R> {}

impl<R: route::Route> sealed::PhyInitializationAccess for SharedPhyHal<'_, R> {}

impl<R: route::Route> PhyInitializationAccess for SharedPhyHal<'_, R> {}

/// Shared PHY capability over externally supplied registers in an isolated
/// validation image.
///
/// Compiled vendor-comparison entry points receive the register partition by
/// pointer. This wrapper owns a fresh restore slot for one call, so a probe
/// cannot observe or leak another route's calibration restore obligation.
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub struct ValidationSharedPhy<'registers> {
    registers: &'registers mut RadioPhyRegisters,
    restore: PhyRouteState,
}

#[cfg(feature = "validation-probes")]
impl<'registers> ValidationSharedPhy<'registers> {
    pub fn new(registers: &'registers mut RadioPhyRegisters) -> Self {
        Self {
            registers,
            restore: PhyRouteState::for_validation(),
        }
    }
}

#[cfg(feature = "validation-probes")]
impl sealed::SharedPhyAccess for ValidationSharedPhy<'_> {
    fn pac(&self) -> &RadioPhyRegisters {
        self.registers
    }

    fn route_state(&self) -> &PhyRouteState {
        &self.restore
    }

    fn parts_mut(&mut self) -> (&mut RadioPhyRegisters, &mut PhyRouteState) {
        (self.registers, &mut self.restore)
    }
}

#[cfg(feature = "validation-probes")]
impl sealed::SharedPhyContext for ValidationSharedPhy<'_> {
    fn wifi_baseband_enable_observation(&self) -> WifiBasebandEnableObservation {
        WifiBasebandEnableObservation::from_pac_readback(self.registers.wifi_baseband_is_enabled())
    }
}

#[cfg(feature = "validation-probes")]
impl SharedPhyAccess for ValidationSharedPhy<'_> {}

#[cfg(feature = "validation-probes")]
impl SharedPhyContext for ValidationSharedPhy<'_> {}

pub(crate) fn phy_pac(access: &(impl SharedPhyAccess + ?Sized)) -> &RadioPhyRegisters {
    sealed::SharedPhyAccess::pac(access)
}

pub(crate) fn phy_pac_mut(access: &mut (impl SharedPhyAccess + ?Sized)) -> &mut RadioPhyRegisters {
    sealed::SharedPhyAccess::parts_mut(access).0
}

pub(crate) fn phy_parts_mut(
    access: &mut (impl SharedPhyAccess + ?Sized),
) -> (&mut RadioPhyRegisters, &mut PhyRouteState) {
    sealed::SharedPhyAccess::parts_mut(access)
}

/// Type states for the coarse radio power lifecycle.
pub mod state {

    use crate::owner::{MacInterruptSetup, PhyHal, RadioRuntimeOwner};

    use super::WifiColdRegisters;

    /// The application uniquely owns the peripheral, but the open driver has
    /// not yet established its clock/reset prerequisites.
    pub struct Owned {
        pub(crate) registers: WifiColdRegisters,
    }

    /// The radio clock/reset prerequisites have been established and finite
    /// PHY register operations may access MMIO.
    pub struct Powered {
        pub(crate) registers: PhyHal,
    }

    /// Cold initialization has completed and task/ISR authority is disjoint.
    pub struct Running {
        pub(crate) registers: RadioRuntimeOwner,
        pub(crate) interrupts: MacInterruptSetup,
    }
}

/// Unique application-visible owner of an ESP32-S31 radio peripheral.
///
/// `P` is the integration layer's singleton token (for example,
/// `esp_hal::peripherals::WIFI`). Keeping it inside this value ties the open
/// driver's register capability to the safe peripheral owner.
pub struct Radio<P, State = state::Owned> {
    pub(crate) peripheral: P,
    pub(crate) state: State,
}

/// Failed cold Wi-Fi release retaining both application and radio owners.
#[must_use = "failed Wi-Fi release still owns the application token and radio route"]
pub struct RadioReleaseFailure<P> {
    pub(crate) radio: Radio<P, state::Owned>,
    pub(crate) error: RadioPhyReleaseError,
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
    pub(crate) registers: WifiRegisters,
    route: WifiRouteState,
}

impl RadioRuntimeOwner {
    /// Construct the running register owner inside an isolated validation
    /// image without exposing the underlying PAC owner.
    #[cfg(any(test, feature = "validation-probes"))]
    #[doc(hidden)]
    pub fn claim_for_validation() -> Self {
        let (registers, _interrupts, route) =
            WifiColdRegisters::from_hardware(RadioHardware::for_validation()).into_running();
        Self::from_pac(registers, route)
    }

    pub fn wifi_mac_hal(&mut self) -> wifi_mac::WifiMacHal<'_> {
        wifi_mac::WifiMacHal::from_owned(&mut self.registers)
    }

    /// Borrow the coexistence timer bank inside an isolated validation image.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn coex_timer_bank(&mut self) -> crate::coex::CoexTimerBank<'_> {
        crate::coex::CoexTimerBank::from_owned(self.registers.shared_mut())
    }

    pub fn channel_hal<'owner, P>(
        &'owner mut self,
        platform: &'owner mut P,
    ) -> channel::RadioChannelHal<'owner, P> {
        let (registers, restore) = self.channel_parts_mut();
        channel::RadioChannelHal::from_owned(platform, registers, restore)
    }

    /// Read the calibrated baseband observation without exposing the PAC
    /// owner or its register encoding.
    pub fn read_noise_floor_dbm(&self) -> i8 {
        self.registers.read_noise_floor_dbm()
    }

    pub fn access_point_receive_policy_snapshot(&self) -> wifi_mac::MacApReceivePolicySnapshot {
        self.registers.ap_receive_policy_snapshot()
    }

    pub fn receive_statistics_snapshot(&self) -> wifi_mac::MacRxStatisticsSnapshot {
        self.registers.rx_statistics_snapshot()
    }

    pub fn transmit_statistics_snapshot(&self) -> wifi_mac::MacTxStatisticsSnapshot {
        self.registers.tx_statistics_snapshot()
    }

    pub fn coex_priority_snapshot(&self) -> wifi_mac::MacCoexPrioritySnapshot {
        self.registers.mac_coex_priority_snapshot()
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

    pub(crate) fn from_pac(registers: WifiRegisters, route: WifiRouteState) -> Self {
        Self { registers, route }
    }

    pub(crate) fn pac(&self) -> &WifiRegisters {
        &self.registers
    }

    pub(crate) fn pac_mut(&mut self) -> &mut WifiRegisters {
        &mut self.registers
    }

    /// Borrow the Wi-Fi register set together with the route restore slot.
    pub(crate) fn channel_parts_mut(&mut self) -> (&mut WifiRegisters, &mut PhyRouteState) {
        (&mut self.registers, self.route.phy_state_mut())
    }

    /// Borrow the shared PHY together with the route restore slot.
    pub(crate) fn phy_parts_mut(&mut self) -> (&mut RadioPhyRegisters, &mut PhyRouteState) {
        (self.registers.radio_phy_mut(), self.route.phy_state_mut())
    }

    /// Borrow the shared PHY parts together with the coexistence timer bank.
    pub(crate) fn phy_and_coex_timer_parts_mut(
        &mut self,
    ) -> (
        &mut RadioPhyRegisters,
        &mut PhyRouteState,
        oer_esp32s31_pac::CoexTimerBankRegisters<'_>,
    ) {
        let (_, shared) = self.registers.parts_mut();
        let (phy, timers) = shared.radio_phy_and_coex_timers_mut();
        (phy, self.route.phy_state_mut(), timers)
    }

    /// Borrow the Wi-Fi register set together with the station wake state.
    pub(crate) fn station_wake_parts_mut(
        &mut self,
    ) -> (
        &mut WifiRegisters,
        &mut crate::ieee80211::station_wake::StationWakeState,
    ) {
        (&mut self.registers, self.route.station_wake_mut())
    }

    /// Borrow the station TBTT and modem-wakeup capability.
    pub fn station_wake_hal(&mut self) -> crate::ieee80211::station_wake::StationWakeHal<'_> {
        let (registers, state) = self.station_wake_parts_mut();
        crate::ieee80211::station_wake::StationWakeHal::from_owned(registers, state)
    }
}

/// Task-side setup authority for one finite MAC interrupt epoch.
pub struct MacInterruptSetup {
    pub(crate) inner: PacMacInterruptSetup,
}

/// Proof that connected-STA interrupt policy was applied before activation.
pub struct ConnectedStaInterruptPrepared {
    pub(crate) _private: (),
}

/// Disjoint HAL capability installed in the hard MAC interrupt handler.
pub struct MacInterruptRegisters {
    pub(crate) inner: PacMacInterruptRegisters,
}

impl MacInterruptRegisters {
    /// Apply connected-STA interrupt policy while the physical route remains
    /// installed. The platform adapter must exclude the ISR while lending
    /// this capability to task context.
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
        registers: &mut WifiRegisters,
    ) -> ConnectedStaInterruptPrepared {
        let _ = self
            .inner
            .prepare_connected_sta_without_power_save(registers);
        ConnectedStaInterruptPrepared { _private: () }
    }

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
        inner: oer_esp32s31_pac::validation::mac_interrupt_setup(),
    }
}

/// Construct the hard-MAC interrupt capability inside an isolated validation
/// image without exposing the PAC partition.
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub fn validation_mac_interrupt_registers() -> MacInterruptRegisters {
    MacInterruptRegisters {
        inner: oer_esp32s31_pac::validation::mac_interrupt_registers(),
    }
}

/// Construct the hard power-interrupt capability inside an isolated probe.
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub fn validation_mac_power_interrupt_registers() -> MacPowerInterruptRegisters {
    MacPowerInterruptRegisters {
        inner: oer_esp32s31_pac::validation::mac_power_interrupt_registers(),
    }
}

/// Disjoint HAL capability installed in the hard power interrupt handler.
pub struct MacPowerInterruptRegisters {
    pub(crate) inner: PacMacPowerInterruptRegisters,
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
        registers: &mut WifiRegisters,
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
    pub(crate) radio: Radio<P, state::Owned>,
    pub(crate) error: PowerError,
}

/// Failed release of a physically closed radio back to the cold owner.
///
/// This type is constructed only by the post-PHY-close path. It retains the
/// exact powered owner because a pending PHY restore obligation prevented
/// neutral-root reconstruction.
#[must_use = "failed cold reunion retains the complete closed hardware owner"]
pub struct ColdReunionFailure<P> {
    _radio: Radio<P, state::Powered>,
    error: ColdReunionError,
}

impl<P> ColdReunionFailure<P> {
    pub const fn error(&self) -> ColdReunionError {
        self.error
    }
}

/// Restore invariant which prevented a closed radio from reaching cold state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColdReunionError {
    TxDcPwdetRestorePending,
    TxIqToneControlRestorePending,
    RxDcoControlRestorePending,
    BluetoothTxPowerControlRestorePending,
    WifiPowerRestore(crate::root::WifiPowerRestoreCheckpoint),
}

impl From<RadioPhyReleaseError> for ColdReunionError {
    fn from(error: RadioPhyReleaseError) -> Self {
        match error {
            RadioPhyReleaseError::TxDcPwdetRestorePending => Self::TxDcPwdetRestorePending,
            RadioPhyReleaseError::TxIqToneControlRestorePending => {
                Self::TxIqToneControlRestorePending
            }
            RadioPhyReleaseError::RxDcoControlRestorePending => Self::RxDcoControlRestorePending,
            RadioPhyReleaseError::BluetoothTxPowerControlRestorePending => {
                Self::BluetoothTxPowerControlRestorePending
            }
            RadioPhyReleaseError::WifiPowerRestore(checkpoint) => {
                Self::WifiPowerRestore(checkpoint)
            }
        }
    }
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
                registers: WifiColdRegisters::from_hardware(hardware),
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
                registers: WifiColdRegisters::from_hardware(RadioHardware::for_validation()),
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
            Err((registers, error)) => Err(RadioReleaseFailure {
                radio: Radio {
                    peripheral,
                    state: state::Owned { registers },
                },
                error,
            }),
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
                    grant_protected: false,
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
        self.state.registers.prepare_wifi_power_epoch();
        if let Err(error) = power::execute_owned(&mut self.state.registers.power_route()) {
            return Err(PowerUpFailure { radio: self, error });
        }
        Ok(Radio {
            peripheral: self.peripheral,
            state: state::Powered {
                registers: PhyHal {
                    registers: self.state.registers,
                    grant_protected: false,
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
        let (registers, restore) = self.state.registers.registers.radio_parts_mut();
        channel::RadioChannelHal::from_owned(&mut self.peripheral, registers, restore)
    }

    /// Borrow the platform and PHY capability independently.
    ///
    /// The two mutable borrows are tied to this unique powered owner and refer
    /// to disjoint fields, allowing a lifecycle function to coordinate an
    /// official system operation with internal Wi-Fi MMIO.
    pub fn phy_hal_parts(&mut self) -> (&mut P, &mut PhyHal) {
        (&mut self.peripheral, &mut self.state.registers)
    }

    /// Borrow the platform, the shared PHY and this route's PHY grant-protect
    /// request together, for PHY maintenance that brackets its hardware
    /// regions with the request. The exclusive route has no radio arbiter;
    /// the request carries the arbiter's cold event-48 priority.
    pub fn phy_hal_parts_with_grant(
        &mut self,
    ) -> (
        &mut P,
        SharedPhyHal<'_, route::Wifi>,
        crate::coex::PhyGrantProtect<'_>,
    ) {
        let hal = &mut self.state.registers;
        let (registers, restore, timers) = hal.registers.phy_and_coex_timer_parts_mut();
        (
            &mut self.peripheral,
            SharedPhyHal::new(registers, restore),
            crate::coex::PhyGrantProtect::new(
                timers,
                crate::coex::CoexPtiTable::VENDOR.pti(crate::coex::PHY_GRANT_PROTECT_EVENT),
                &mut hal.grant_protected,
            ),
        )
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

    /// Return a physically closed radio to the cold ownership frontier.
    ///
    /// The PHY layer must complete RF close and temperature-sensor power-down
    /// before calling it; ordinary application flow reaches it only through
    /// the registered PHY cold-release transaction. This releases retained
    /// shared clocks and restores the route-owned cold-power baseline before
    /// reconstructing the cold owner.
    #[doc(hidden)]
    pub fn reunite_cold_after_phy_close(
        self,
    ) -> Result<Radio<P, state::Owned>, ColdReunionFailure<P>> {
        let Radio {
            peripheral,
            state: state::Powered { registers },
        } = self;
        match registers.registers.release() {
            Ok(hardware) => Ok(Radio::from_hardware(peripheral, hardware)),
            Err((registers, error)) => Err(ColdReunionFailure {
                _radio: Radio {
                    peripheral,
                    state: state::Powered {
                        registers: PhyHal {
                            registers,
                            grant_protected: false,
                        },
                    },
                },
                error: error.into(),
            }),
        }
    }
}

/// Rejected hand-over of a closed radio to the retained root.
#[must_use = "failed retained release still owns the powered radio"]
pub struct RetainedReleaseFailure<P> {
    radio: Radio<P, state::Powered>,
    error: crate::root::RetainedRadioReleaseError,
}

impl<P> RetainedReleaseFailure<P> {
    pub const fn error(&self) -> crate::root::RetainedRadioReleaseError {
        self.error
    }

    /// Recover the unchanged powered radio.
    pub fn into_radio(self) -> Radio<P, state::Powered> {
        self.radio
    }
}

impl<P> Radio<P, state::Powered> {
    /// Enter the Wi-Fi route from a retained root.
    ///
    /// The common PHY power sequence of the previous route stays in effect,
    /// so this powered owner skips [`Radio::power_up`]; the registration epoch
    /// of the retained PHY stays current.
    pub fn from_retained(peripheral: P, hardware: crate::root::RetainedRadioHardware) -> Self {
        Radio {
            peripheral,
            state: state::Powered {
                registers: PhyHal {
                    registers: WifiColdRegisters::from_retained(hardware),
                    grant_protected: false,
                },
            },
        }
    }

    /// Report, without MMIO, why
    /// [`Self::release_retained_after_phy_close`] would reject this radio.
    ///
    /// # Errors
    ///
    /// A calibration restore obligation remains, or the route never
    /// established common PHY power.
    pub fn check_retained_release(&self) -> Result<(), crate::root::RetainedRadioReleaseError> {
        self.state.registers.registers.check_retained_release()
    }

    /// Hand a physically closed radio to the retained root.
    ///
    /// The PHY layer must complete RF close and temperature-sensor power-down
    /// first; ordinary application flow reaches it only through the registered
    /// PHY retained release. Wi-Fi's own leases are released, and the common
    /// PHY power stays in effect for the next route.
    #[doc(hidden)]
    #[allow(
        clippy::result_large_err,
        reason = "rejection returns the complete powered radio without allocation"
    )]
    pub fn release_retained_after_phy_close(
        self,
    ) -> Result<(P, crate::root::RetainedRadioHardware), RetainedReleaseFailure<P>> {
        let Radio {
            peripheral,
            state: state::Powered { registers },
        } = self;
        match registers.registers.release_retained() {
            Ok(hardware) => Ok((peripheral, hardware)),
            Err((registers, error)) => Err(RetainedReleaseFailure {
                radio: Radio {
                    peripheral,
                    state: state::Powered {
                        registers: PhyHal {
                            registers,
                            grant_protected: false,
                        },
                    },
                },
                error,
            }),
        }
    }
}

impl<P> Radio<P, state::Powered> {
    /// Complete the one-way ownership transition after cold MAC setup.
    pub fn into_running(self) -> Radio<P, state::Running> {
        let (registers, interrupts, route) = self.state.registers.registers.into_running();
        Radio {
            peripheral: self.peripheral,
            state: state::Running {
                registers: RadioRuntimeOwner::from_pac(registers, route),
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

    /// Reunite an inactive runtime partition into the powered cold frontier.
    ///
    /// This is an ownership-only transition and performs no MMIO. The caller
    /// must have completed its protocol stop before reconstructing `Running`:
    /// no DMA owner or installed interrupt route may remain outside this
    /// value. RF and shared clocks stay physically powered.
    #[doc(hidden)]
    pub fn reunite_powered(self) -> Radio<P, state::Powered> {
        let RadioRuntimeOwner { registers, route } = self.state.registers;
        let registers =
            WifiColdRegisters::from_running(registers, self.state.interrupts.inner, route);
        Radio {
            peripheral: self.peripheral,
            state: state::Powered {
                registers: PhyHal {
                    registers,
                    grant_protected: false,
                },
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
        crate::phy::frequency::set_wifi_enabled(registers, true);
    }

    /// Disable the Wi-Fi RX/baseband path at a stopped protocol frontier.
    ///
    /// This is the per-client release edge used before shared RF sleep or a
    /// handoff to another protocol. It does not close RF or release clocks.
    #[cfg(target_arch = "riscv32")]
    pub fn disable_wifi_rx(&mut self) {
        let (_, registers) = self.phy_hal_parts();
        crate::phy::frequency::set_wifi_enabled(registers, false);
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
mod tests;
