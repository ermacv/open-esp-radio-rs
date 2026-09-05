//! Executor-neutral lower HAL boundary for ESP32-S31 IEEE 802.15.4.
//!
//! This module deliberately stops below a public radio role or MAC facade.
//! The production backend is the narrow PAC lease over the generated
//! `IEEE802154_MAC` peripheral. Both the backend trait and this module remain
//! crate-private: callers never implement register access or supply addresses,
//! masks, or complete register images.

#![forbid(unsafe_code)]
// Frequency programming and numeric state diagnostics are already typed but
// remain reserved for the later PHY/dataplane transition. Keeping them inside
// this closed module avoids reopening raw PAC access in that iteration.
#![cfg_attr(not(test), allow(dead_code))]

use core::convert::Infallible;

use open_esp_radio_esp32s31_pac::{
    Ieee802154AckTimeoutUnits as PacAckTimeoutUnits, Ieee802154CcaMode as PacCcaMode,
    Ieee802154EdDurationUnits as PacEdDurationUnits,
    Ieee802154EventEnableState as PacEventEnableState, Ieee802154MacControl as PacMacControl,
    Ieee802154MacPolicySnapshot as PacMacPolicySnapshot,
    Ieee802154OperationEventEnableObservation as PacOperationEventEnableObservation,
    Ieee802154OperationRxAbortEnableObservation as PacOperationRxAbortEnableObservation,
    Ieee802154PanIdentity as PacPanIdentity, Ieee802154RxAbortEnableState as PacRxAbortEnableState,
    Ieee802154RxAbortReasonObservation,
};
pub(crate) use open_esp_radio_esp32s31_pac::{
    Ieee802154FoundationSnapshot, Ieee802154FrequencyCode, Ieee802154InterruptSetup,
    Ieee802154PolledRegisterLease, Ieee802154Pti, Ieee802154RegisterLease, Ieee802154RouteState,
    Ieee802154StateSnapshot, Ieee802154TaskRegisters,
};
#[cfg(feature = "validation-probes")]
use open_esp_radio_esp32s31_pac::{
    Ieee802154ObservedEventState, Ieee802154ValidationEdDurationState,
    Ieee802154ValidationEventEnableState,
};

use crate::ieee802154::operation::{
    Ieee802154OperationEventMaskState, Ieee802154OperationEventObservation,
    Ieee802154OperationRxAbortMaskState, Ieee802154PolledOperationBackend,
};
use crate::ieee802154::policy::{
    Ieee802154AckTimeout, Ieee802154CcaMode, Ieee802154MacControl, Ieee802154MacPolicySnapshot,
    Ieee802154PanIdentity,
};

/// Typed register operations required by the first IEEE 802.15.4 foundation.
///
/// The trait is crate-private and its production implementation is sealed to
/// [`Ieee802154PolledRegisterLease`]. A host fake can still prove HAL delegation and
/// state predicates inside this module's unit tests.
pub(crate) trait Ieee802154RegisterBackend {
    /// Mask every MAC event before an ISR owner exists.
    fn mask_all_events(&mut self);

    /// Mask every receive-abort source before an RX dataplane exists.
    fn mask_all_rx_aborts(&mut self);

    /// Mask every transmit-abort source before a TX dataplane exists.
    fn mask_all_tx_aborts(&mut self);

    /// Select average energy-detection sampling.
    fn select_average_ed_sampling(&mut self);

    /// Replace only the opaque eight-bit MAC frequency-code field.
    fn set_frequency_code(&mut self, code: Ieee802154FrequencyCode);

    /// Replace the source-confirmed CCA-mode field.
    fn set_cca_mode(&mut self, mode: PacCcaMode);

    /// Replace the signed eight-bit CCA-threshold field.
    fn set_cca_threshold_code(&mut self, threshold: i8);

    /// Apply all six MAC-control fields in their source-confirmed inner order.
    fn set_mac_control(&mut self, control: PacMacControl);

    /// Replace the complete ACK-timeout field.
    fn set_ack_timeout(&mut self, timeout: PacAckTimeoutUnits);

    /// Configure and enable the primary PAN identity.
    fn set_primary_pan_identity(&mut self, identity: PacPanIdentity);

    /// Replace only the recovered TX/RX coexistence priority field.
    fn set_txrx_pti(&mut self, pti: Ieee802154Pti);

    /// Replace only the recovered ACK coexistence priority field.
    fn set_ack_pti(&mut self, pti: Ieee802154Pti);

    /// Sample the fields written by the interrupt-masked foundation.
    fn foundation_snapshot(&mut self) -> Ieee802154FoundationSnapshot;

    /// Sample the complete known static-policy subset once per backing word.
    fn mac_policy_snapshot(&mut self) -> PacMacPolicySnapshot;

    /// Sample the receive and transmit state fields once each.
    fn sample_state(&mut self) -> Ieee802154StateSnapshot;

    /// Fence a completed ownership publication or handoff.
    fn order_device_accesses(&mut self);
}

impl Ieee802154RegisterBackend for Ieee802154PolledRegisterLease<'_> {
    fn mask_all_events(&mut self) {
        Ieee802154PolledRegisterLease::mask_all_events(self);
    }

    fn mask_all_rx_aborts(&mut self) {
        Ieee802154PolledRegisterLease::mask_all_rx_aborts(self);
    }

    fn mask_all_tx_aborts(&mut self) {
        Ieee802154PolledRegisterLease::mask_all_tx_aborts(self);
    }

    fn select_average_ed_sampling(&mut self) {
        Ieee802154PolledRegisterLease::select_average_ed_sampling(self);
    }

    fn set_frequency_code(&mut self, code: Ieee802154FrequencyCode) {
        Ieee802154RegisterLease::set_frequency_code(self, code);
    }

    fn set_cca_mode(&mut self, mode: PacCcaMode) {
        Ieee802154RegisterLease::set_cca_mode(self, mode);
    }

    fn set_cca_threshold_code(&mut self, threshold: i8) {
        Ieee802154RegisterLease::set_cca_threshold_code(self, threshold);
    }

    fn set_mac_control(&mut self, control: PacMacControl) {
        Ieee802154RegisterLease::set_mac_control(self, control);
    }

    fn set_ack_timeout(&mut self, timeout: PacAckTimeoutUnits) {
        Ieee802154RegisterLease::set_ack_timeout(self, timeout);
    }

    fn set_primary_pan_identity(&mut self, identity: PacPanIdentity) {
        Ieee802154RegisterLease::set_primary_pan_identity(self, identity);
    }

    fn set_txrx_pti(&mut self, pti: Ieee802154Pti) {
        Ieee802154RegisterLease::set_txrx_pti(self, pti);
    }

    fn set_ack_pti(&mut self, pti: Ieee802154Pti) {
        Ieee802154RegisterLease::set_ack_pti(self, pti);
    }

    fn foundation_snapshot(&mut self) -> Ieee802154FoundationSnapshot {
        Ieee802154PolledRegisterLease::foundation_snapshot(self)
    }

    fn mac_policy_snapshot(&mut self) -> PacMacPolicySnapshot {
        Ieee802154RegisterLease::mac_policy_snapshot(self)
    }

    fn sample_state(&mut self) -> Ieee802154StateSnapshot {
        self.state_snapshot()
    }

    fn order_device_accesses(&mut self) {
        Ieee802154RegisterLease::order_device_accesses(self);
    }
}

/// Temporary, exclusive borrow of one typed IEEE 802.15.4 register backend.
///
/// The wrapper has no raw-register escape hatch and owns no lifecycle state.
/// The whole-radio IEEE 802.15.4 role lends the closed PAC implementation here
/// without exposing the complete shared radio owner.
pub(crate) struct Ieee802154Hal<B: Ieee802154RegisterBackend> {
    backend: B,
}

/// Concrete closed HAL capability backed by the generated ESP32-S31 PAC.
pub(crate) type Ieee802154PacHal<'registers> =
    Ieee802154Hal<Ieee802154PolledRegisterLease<'registers>>;

impl<'registers> Ieee802154PacHal<'registers> {
    /// Borrow the IEEE 802.15.4 leaf from the unique task-side radio owner.
    pub(crate) fn from_owned(
        registers: &'registers mut Ieee802154TaskRegisters,
        interrupts: &'registers mut Ieee802154InterruptSetup,
    ) -> Self {
        Self::from_register_backend(interrupts.polled_register_lease(registers))
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_event_enable_state(&mut self) -> Ieee802154ValidationEventEnableState {
        self.backend.validation_event_enable_state()
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_enable_timer_events(&mut self) {
        self.backend.validation_enable_timer_events();
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_disable_all_events(&mut self) {
        self.backend.validation_disable_all_events();
    }

    pub(crate) fn interrupt_route_state(&mut self) -> Ieee802154RouteState {
        self.backend.interrupt_route_state()
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_event_status_state(&mut self) -> Ieee802154ObservedEventState {
        self.backend.validation_event_status_state()
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_event_timer0_value(&mut self) -> u32 {
        self.backend.validation_event_timer0_value()
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_event_timer1_value(&mut self) -> u32 {
        self.backend.validation_event_timer1_value()
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_set_event_timer_thresholds(&mut self, threshold: u32) {
        self.backend
            .validation_set_event_timer_thresholds(threshold);
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_start_event_timer0(&mut self) {
        self.backend.validation_start_event_timer0();
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_stop_event_timer0(&mut self) {
        self.backend.validation_stop_event_timer0();
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_start_event_timer1(&mut self) {
        self.backend.validation_start_event_timer1();
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_stop_event_timer1(&mut self) {
        self.backend.validation_stop_event_timer1();
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_write_event_timer0(&mut self) {
        self.backend.validation_write_event_timer0();
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_write_event_timer1(&mut self) {
        self.backend.validation_write_event_timer1();
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_ed_event_enable_state(
        &mut self,
    ) -> Ieee802154ValidationEventEnableState {
        self.backend.validation_ed_event_enable_state()
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_enable_ed_timer_abort_events(&mut self) {
        self.backend.validation_enable_ed_timer_abort_events();
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_disable_ed_events(&mut self) {
        self.backend.validation_disable_ed_events();
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_ed_rx_abort_enable_state(
        &mut self,
    ) -> PacOperationRxAbortEnableObservation {
        self.backend.validation_ed_rx_abort_enable_state()
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_enable_ed_abort_reasons(&mut self) {
        self.backend.validation_enable_ed_abort_reasons();
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_disable_ed_abort_reasons(&mut self) {
        self.backend.validation_disable_ed_abort_reasons();
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_ed_event_status_state(&mut self) -> Ieee802154ObservedEventState {
        self.backend.validation_ed_event_status_state()
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_ed_rx_abort_reason(&mut self) -> Ieee802154RxAbortReasonObservation {
        self.backend.validation_ed_rx_abort_reason()
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_ed_duration_state(&mut self) -> Ieee802154ValidationEdDurationState {
        self.backend.validation_ed_duration_state()
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_set_ed_duration_eight(&mut self) {
        self.backend.validation_set_ed_duration_eight();
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_ed_timer0_value(&mut self) -> u32 {
        self.backend.validation_ed_timer0_value()
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_set_ed_timer0_threshold(&mut self, threshold: u32) {
        self.backend.validation_set_ed_timer0_threshold(threshold);
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_start_ed_timer0(&mut self) {
        self.backend.validation_start_ed_timer0();
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_stop_ed_timer0(&mut self) {
        self.backend.validation_stop_ed_timer0();
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_start_ed(&mut self) {
        self.backend.validation_start_ed();
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_stop_ed_operation(&mut self) {
        self.backend.validation_stop_ed_operation();
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_write_ed_done_event(&mut self) {
        self.backend.validation_write_ed_done_event();
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn validation_write_ed_timer0_event(&mut self) {
        self.backend.validation_write_ed_timer0_event();
    }
}

impl Ieee802154PolledOperationBackend for Ieee802154PacHal<'_> {
    type Error = Infallible;

    fn set_channel(&mut self, channel: crate::Ieee802154Channel) -> Result<(), Self::Error> {
        self.backend.set_frequency_code(channel.frequency_code());
        Ok(())
    }

    fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) -> Result<(), Self::Error> {
        self.backend.set_cca_mode(mode.into_pac());
        Ok(())
    }

    fn set_cca_threshold_code(&mut self, threshold: i8) -> Result<(), Self::Error> {
        self.backend.set_cca_threshold_code(threshold);
        Ok(())
    }

    fn set_ed_duration(&mut self, duration: u16) -> Result<(), Self::Error> {
        let Some(duration) = PacEdDurationUnits::new(u32::from(duration)) else {
            unreachable!("every u16 is accepted by the reviewed ED-duration subset")
        };
        self.backend.set_ed_duration(duration);
        Ok(())
    }

    fn cpu_interrupt_route_is_detached(&mut self) -> Result<bool, Self::Error> {
        Ok(self.interrupt_route_state().is_reset_detached())
    }

    fn operation_event_mask_state(
        &mut self,
    ) -> Result<Ieee802154OperationEventMaskState, Self::Error> {
        Ok(match self.backend.operation_event_enable_observation() {
            PacOperationEventEnableObservation::AllMasked => {
                Ieee802154OperationEventMaskState::AllMasked
            }
            PacOperationEventEnableObservation::EdDoneAndRxAbortOnly => {
                Ieee802154OperationEventMaskState::EdDoneAndRxAbortOnly
            }
            PacOperationEventEnableObservation::Unexpected => {
                Ieee802154OperationEventMaskState::Unexpected
            }
        })
    }

    fn operation_rx_abort_mask_state(
        &mut self,
    ) -> Result<Ieee802154OperationRxAbortMaskState, Self::Error> {
        Ok(match self.backend.operation_rx_abort_enable_observation() {
            PacOperationRxAbortEnableObservation::AllMasked => {
                Ieee802154OperationRxAbortMaskState::AllMasked
            }
            PacOperationRxAbortEnableObservation::EdOperationReasonsOnly => {
                Ieee802154OperationRxAbortMaskState::EdOperationReasonsOnly
            }
            PacOperationRxAbortEnableObservation::Unexpected => {
                Ieee802154OperationRxAbortMaskState::Unexpected
            }
        })
    }

    fn enable_ed_done_and_rx_abort(&mut self) -> Result<(), Self::Error> {
        self.backend
            .set_event_enable(PacEventEnableState::EdOperation);
        Ok(())
    }

    fn enable_ed_operation_rx_abort_reasons(&mut self) -> Result<(), Self::Error> {
        self.backend
            .set_rx_abort_enable(PacRxAbortEnableState::EdOperationReasons);
        Ok(())
    }

    fn mask_ed_done_and_rx_abort(&mut self) -> Result<(), Self::Error> {
        self.backend
            .set_event_enable(PacEventEnableState::AllMasked);
        Ok(())
    }

    fn mask_ed_operation_rx_abort_reasons(&mut self) -> Result<(), Self::Error> {
        self.backend
            .set_rx_abort_enable(PacRxAbortEnableState::AllMasked);
        Ok(())
    }

    fn order_device_accesses(&mut self) -> Result<(), Self::Error> {
        self.backend.order_device_accesses();
        Ok(())
    }

    fn request_ed_start(&mut self) -> Result<(), Self::Error> {
        self.backend.request_ed_start();
        Ok(())
    }

    fn sample_event_status(&mut self) -> Result<Ieee802154OperationEventObservation, Self::Error> {
        Ok(Ieee802154OperationEventObservation::from_classification(
            self.backend.event_status_observation().classification(),
        ))
    }

    fn acknowledge_pending_events(
        &mut self,
    ) -> Result<Ieee802154OperationEventObservation, Self::Error> {
        Ok(Ieee802154OperationEventObservation::from_classification(
            self.backend.acknowledge_pending_events().classification(),
        ))
    }

    fn sample_rx_abort_reason(
        &mut self,
    ) -> Result<Ieee802154RxAbortReasonObservation, Self::Error> {
        Ok(self.backend.rx_abort_reason_observation())
    }

    fn sample_ed_rss_code(&mut self) -> Result<i8, Self::Error> {
        Ok(self.backend.ed_rss_code())
    }

    fn sample_cca_busy(&mut self) -> Result<bool, Self::Error> {
        Ok(self.backend.cca_busy())
    }
}

impl<B: Ieee802154RegisterBackend> Ieee802154Hal<B> {
    /// Bind one exclusive semantic register backend.
    ///
    /// This constructor is a lower-layer integration hook, not a radio-role
    /// constructor; it neither powers nor enables the MAC.
    #[doc(hidden)]
    pub(crate) fn from_register_backend(backend: B) -> Self {
        Self { backend }
    }

    #[cfg(test)]
    fn into_register_backend(self) -> B {
        self.backend
    }

    pub(crate) fn set_frequency_code(&mut self, code: Ieee802154FrequencyCode) {
        self.backend.set_frequency_code(code);
    }

    pub(crate) fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) {
        self.backend.set_cca_mode(mode.into_pac());
    }

    pub(crate) fn set_cca_threshold_code(&mut self, threshold: i8) {
        self.backend.set_cca_threshold_code(threshold);
    }

    pub(crate) fn set_mac_control(&mut self, control: Ieee802154MacControl) {
        self.backend.set_mac_control(control.into_pac());
    }

    pub(crate) fn set_ack_timeout(&mut self, timeout: Ieee802154AckTimeout) {
        self.backend.set_ack_timeout(timeout.into_pac());
    }

    pub(crate) fn set_primary_pan_identity(&mut self, identity: Ieee802154PanIdentity) {
        self.backend.set_primary_pan_identity(identity.into_pac());
    }

    pub(crate) fn mask_all_events(&mut self) {
        self.backend.mask_all_events();
    }

    pub(crate) fn mask_all_rx_aborts(&mut self) {
        self.backend.mask_all_rx_aborts();
    }

    pub(crate) fn mask_all_tx_aborts(&mut self) {
        self.backend.mask_all_tx_aborts();
    }

    pub(crate) fn select_average_ed_sampling(&mut self) {
        self.backend.select_average_ed_sampling();
    }

    pub(crate) fn set_txrx_pti(&mut self, pti: Ieee802154Pti) {
        self.backend.set_txrx_pti(pti);
    }

    pub(crate) fn set_ack_pti(&mut self, pti: Ieee802154Pti) {
        self.backend.set_ack_pti(pti);
    }

    pub(crate) fn foundation_snapshot(&mut self) -> Ieee802154FoundationSnapshot {
        self.backend.foundation_snapshot()
    }

    pub(crate) fn mac_policy_snapshot(&mut self) -> Ieee802154MacPolicySnapshot {
        Ieee802154MacPolicySnapshot::from_pac(self.backend.mac_policy_snapshot())
    }

    /// Sample the typed RX/TX state fields once each.
    pub(crate) fn state_snapshot(&mut self) -> Ieee802154StateSnapshot {
        self.backend.sample_state()
    }

    /// Test only whether both sampled numeric state codes are zero.
    ///
    /// This deliberately does not claim reset readiness or lifecycle
    /// quiescence before those semantics are recovered.
    pub(crate) fn state_codes_are_zero(&mut self) -> bool {
        self.state_snapshot().all_codes_zero()
    }

    /// Publish a device-ordering boundary after a completed typed operation.
    pub(crate) fn order_device_accesses(&mut self) {
        self.backend.order_device_accesses();
    }
}

#[cfg(test)]
mod tests;
