//! Whole-owner IEEE 802.15.4 lifecycle bound to [`Radio`].
//!
//! The public role types wrap the existing powered radio owner. They never
//! acquire a second PAC singleton and cannot skip the reviewed clock, reset,
//! or MAC-foundation readbacks. Completion of this module's final state is
//! intentionally weaker than PHY/RF or operational-MAC readiness.

#![forbid(unsafe_code)]

use core::fmt;

use open_esp_radio_esp32s31_pac::{Ieee802154FoundationSnapshot, Ieee802154Pti};

#[cfg(feature = "validation-probes")]
use crate::ieee802154_event_status_probe::{
    Ieee802154EventStatusProbeConfig, Ieee802154EventStatusProbeEvidence,
    Ieee802154EventStatusProbeIsolation, run_ieee802154_event_status_probe,
};

use crate::{
    Radio,
    ieee802154::Ieee802154PacHal,
    ieee802154_lifecycle::{
        Ieee802154ClockCheckpoint, Ieee802154ClockFailure as EngineClockFailure,
        Ieee802154ClockImages, Ieee802154FoundationCheckpoint,
        Ieee802154FoundationFailure as EngineFoundationFailure, Ieee802154Lifecycle,
        Ieee802154LifecycleBackend, Ieee802154PlatformControl, Ieee802154ReadbackError,
        Ieee802154ResetCheckpoint, Ieee802154ResetFailure as EngineResetFailure,
        Ieee802154ResetImages, establish_ieee802154_clocks, state as lifecycle_state,
    },
    ieee802154_pac_mut,
    ieee802154_policy::{
        Ieee802154AckTimeout, Ieee802154CcaMode, Ieee802154MacControl, Ieee802154MacPolicy,
        Ieee802154MacPolicyBackend, Ieee802154MacPolicyCheckpoint,
        Ieee802154MacPolicyFailure as EngineMacPolicyFailure, Ieee802154MacPolicyReadback,
        Ieee802154PanIdentity, configure_ieee802154_mac_policy,
    },
    state as radio_state,
};

struct OwnedIeee802154Backend<P> {
    radio: Radio<P, radio_state::Powered>,
}

impl<P> OwnedIeee802154Backend<P> {
    fn platform_mut(&mut self) -> &mut P {
        self.radio.phy_hal_parts().0
    }

    fn mac_hal(&mut self) -> Ieee802154PacHal<'_> {
        let (_, phy) = self.radio.phy_hal_parts();
        Ieee802154PacHal::from_owned(ieee802154_pac_mut(phy))
    }
}

impl<P: Ieee802154PlatformControl> Ieee802154PlatformControl for OwnedIeee802154Backend<P> {
    fn configure_modem_clock_maps(&mut self) {
        self.platform_mut().configure_modem_clock_maps();
    }

    fn configure_modem_source_clock(&mut self) {
        self.platform_mut().configure_modem_source_clock();
    }

    fn enable_coexistence_clock(&mut self) {
        self.platform_mut().enable_coexistence_clock();
    }

    fn enable_wifi_bb_80x1_clock(&mut self) {
        self.platform_mut().enable_wifi_bb_80x1_clock();
    }

    fn enable_etm_clock(&mut self) {
        self.platform_mut().enable_etm_clock();
    }

    fn enable_bt_apb_clocks(&mut self) {
        self.platform_mut().enable_bt_apb_clocks();
    }

    fn enable_bt_ieee802154_common_baseband_clock(&mut self) {
        self.platform_mut()
            .enable_bt_ieee802154_common_baseband_clock();
    }

    fn enable_ieee802154_mac_clocks(&mut self) {
        self.platform_mut().enable_ieee802154_mac_clocks();
    }

    fn ieee802154_clock_images(&self) -> Ieee802154ClockImages {
        self.radio.peripheral().ieee802154_clock_images()
    }

    fn set_ieee802154_mac_reset(&mut self, asserted: bool) {
        self.platform_mut().set_ieee802154_mac_reset(asserted);
    }

    fn set_ieee802154_apb_reset(&mut self, asserted: bool) {
        self.platform_mut().set_ieee802154_apb_reset(asserted);
    }

    fn ieee802154_reset_images(&self) -> Ieee802154ResetImages {
        self.radio.peripheral().ieee802154_reset_images()
    }
}

impl<P: Ieee802154PlatformControl> Ieee802154LifecycleBackend for OwnedIeee802154Backend<P> {
    fn mask_all_events(&mut self) {
        self.mac_hal().mask_all_events();
    }

    fn mask_all_rx_aborts(&mut self) {
        self.mac_hal().mask_all_rx_aborts();
    }

    fn mask_all_tx_aborts(&mut self) {
        self.mac_hal().mask_all_tx_aborts();
    }

    fn select_average_ed_sampling(&mut self) {
        self.mac_hal().select_average_ed_sampling();
    }

    fn set_txrx_pti(&mut self, pti: Ieee802154Pti) {
        self.mac_hal().set_txrx_pti(pti);
    }

    fn set_ack_pti(&mut self, pti: Ieee802154Pti) {
        self.mac_hal().set_ack_pti(pti);
    }

    fn order_device_accesses(&mut self) {
        self.mac_hal().order_device_accesses();
    }

    fn foundation_snapshot(&mut self) -> Ieee802154FoundationSnapshot {
        self.mac_hal().foundation_snapshot()
    }
}

/// Whole-radio owner after all IEEE 802.15.4 clock dependencies read back.
pub struct Ieee802154Clocked<P> {
    inner: Ieee802154Lifecycle<OwnedIeee802154Backend<P>, lifecycle_state::Clocked>,
}

/// Whole-radio owner after the MAC and APB resets were pulsed and released.
pub struct Ieee802154Reset<P> {
    inner: Ieee802154Lifecycle<OwnedIeee802154Backend<P>, lifecycle_state::Reset>,
}

/// Interrupt-masked static MAC foundation.
///
/// This state does not imply PHY/RF ownership, interrupt routing, DMA buffer
/// setup, or an idle/operational MAC. Those are later one-way transitions.
pub struct Ieee802154FoundationConfigured<P> {
    inner: Ieee802154Lifecycle<OwnedIeee802154Backend<P>, lifecycle_state::FoundationConfigured>,
}

/// Terminal whole-radio owner after the validation-only status experiment.
///
/// Experimental raw status writes and masked cleanup cannot preserve a normal
/// lifecycle proof. This type exposes only evidence and deliberately provides
/// no route back to foundation, policy, or operational transitions. The
/// reset-isolation capability remains consumed for the rest of the process.
#[cfg(feature = "validation-probes")]
#[must_use = "the terminal validation owner retains the radio and isolation capability"]
pub struct Ieee802154EventStatusProbeFinished<P> {
    evidence: Ieee802154EventStatusProbeEvidence,
    _foundation: Ieee802154FoundationConfigured<P>,
    _isolation: Ieee802154EventStatusProbeIsolation,
}

#[cfg(feature = "validation-probes")]
impl<P> Ieee802154EventStatusProbeFinished<P> {
    /// Borrow the complete raw evidence retained by the terminal owner.
    pub const fn evidence(&self) -> &Ieee802154EventStatusProbeEvidence {
        &self.evidence
    }
}

impl<P> Ieee802154MacPolicyBackend for Ieee802154FoundationConfigured<P> {
    fn set_channel(&mut self, channel: crate::Ieee802154Channel) {
        self.inner
            .backend_mut()
            .mac_hal()
            .set_frequency_code(channel.frequency_code());
    }

    fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) {
        self.inner.backend_mut().mac_hal().set_cca_mode(mode);
    }

    fn set_cca_threshold_code(&mut self, threshold: i8) {
        self.inner
            .backend_mut()
            .mac_hal()
            .set_cca_threshold_code(threshold);
    }

    fn set_mac_control(&mut self, control: Ieee802154MacControl) {
        self.inner.backend_mut().mac_hal().set_mac_control(control);
    }

    fn set_ack_timeout(&mut self, timeout: Ieee802154AckTimeout) {
        self.inner.backend_mut().mac_hal().set_ack_timeout(timeout);
    }

    fn set_primary_pan_identity(&mut self, identity: Ieee802154PanIdentity) {
        self.inner
            .backend_mut()
            .mac_hal()
            .set_primary_pan_identity(identity);
    }

    fn order_device_accesses(&mut self) {
        self.inner.backend_mut().mac_hal().order_device_accesses();
    }

    fn mac_policy_readback(&mut self) -> Ieee802154MacPolicyReadback {
        let mut hal = self.inner.backend_mut().mac_hal();
        let foundation = hal.foundation_snapshot();
        let policy = hal.mac_policy_snapshot();
        Ieee802154MacPolicyReadback::new(foundation, policy)
    }
}

/// Whole-radio owner after the known static MAC policy passes readback.
///
/// Event and abort delivery is still masked. This state is not PHY/RF or
/// BTBB readiness, does not route an IRQ, owns no DMA buffer, and is not an
/// operational MAC. It is not a complete vendor PIB because TX-power mapping
/// remains opaque.
pub struct Ieee802154MacPolicyConfigured<P> {
    foundation: Ieee802154FoundationConfigured<P>,
    policy: Ieee802154MacPolicy,
}

/// Failed clock readback with the original powered owner retained.
pub struct Ieee802154ClockTransitionFailure<P> {
    inner: EngineClockFailure<OwnedIeee802154Backend<P>>,
}

impl<P> fmt::Debug for Ieee802154ClockTransitionFailure<P> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Ieee802154ClockTransitionFailure")
            .field("error", &self.inner.error())
            .finish_non_exhaustive()
    }
}

impl<P> Ieee802154ClockTransitionFailure<P> {
    pub const fn error(&self) -> Ieee802154ReadbackError<Ieee802154ClockCheckpoint> {
        self.inner.error()
    }

    /// Recover the coarse powered owner for diagnosis or a controlled retry.
    pub fn into_powered(self) -> Radio<P, radio_state::Powered> {
        self.inner.into_backend().radio
    }
}

/// Failed private-reset readback retaining the last proved clocked owner.
pub struct Ieee802154ResetTransitionFailure<P> {
    inner: EngineResetFailure<OwnedIeee802154Backend<P>>,
}

impl<P> fmt::Debug for Ieee802154ResetTransitionFailure<P> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Ieee802154ResetTransitionFailure")
            .field("error", &self.inner.error())
            .finish_non_exhaustive()
    }
}

impl<P> Ieee802154ResetTransitionFailure<P> {
    pub const fn error(&self) -> Ieee802154ReadbackError<Ieee802154ResetCheckpoint> {
        self.inner.error()
    }

    /// Recover the clocked role for diagnosis or an exact reset retry.
    pub fn into_clocked(self) -> Ieee802154Clocked<P> {
        Ieee802154Clocked {
            inner: self.inner.into_lifecycle(),
        }
    }
}

/// Failed foundation readback retaining the last proved reset owner.
pub struct Ieee802154FoundationTransitionFailure<P> {
    inner: EngineFoundationFailure<OwnedIeee802154Backend<P>>,
}

impl<P> fmt::Debug for Ieee802154FoundationTransitionFailure<P> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Ieee802154FoundationTransitionFailure")
            .field("error", &self.inner.error())
            .finish_non_exhaustive()
    }
}

impl<P> Ieee802154FoundationTransitionFailure<P> {
    pub const fn error(&self) -> Ieee802154ReadbackError<Ieee802154FoundationCheckpoint> {
        self.inner.error()
    }

    /// Recover the reset role for diagnosis or an exact foundation retry.
    pub fn into_reset(self) -> Ieee802154Reset<P> {
        Ieee802154Reset {
            inner: self.inner.into_lifecycle(),
        }
    }
}

/// Failed static-policy readback retaining the exact whole-radio owner until
/// recovery classifies the strongest still-proved typestate.
pub struct Ieee802154MacPolicyTransitionFailure<P> {
    inner: EngineMacPolicyFailure<Ieee802154FoundationConfigured<P>>,
}

/// Safe owner recovered after a static-policy readback failure.
///
/// Policy-only mismatches preserve the still-proved foundation for an exact
/// retry. A mismatch in masks, ED sampling, or PTI disproves that foundation
/// and therefore returns the preceding reset state instead.
pub enum Ieee802154MacPolicyRecovery<P> {
    /// The foundation still passed and only the requested policy mismatched.
    Foundation(Ieee802154FoundationConfigured<P>),
    /// A foundation invariant failed and must be configured and proved again.
    Reset(Ieee802154Reset<P>),
}

impl<P> fmt::Debug for Ieee802154MacPolicyTransitionFailure<P> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Ieee802154MacPolicyTransitionFailure")
            .field("error", &self.inner.error())
            .finish_non_exhaustive()
    }
}

impl<P> Ieee802154MacPolicyTransitionFailure<P> {
    /// Return the first mismatched foundation or policy checkpoint.
    pub const fn error(&self) -> Ieee802154ReadbackError<Ieee802154MacPolicyCheckpoint> {
        self.inner.error()
    }

    /// Recover the strongest typestate still supported by the failed
    /// readback.
    pub fn into_recovery(self) -> Ieee802154MacPolicyRecovery<P> {
        let invalidates_foundation = self.inner.error().checkpoint.invalidates_foundation();
        let foundation = self.inner.into_backend();
        if invalidates_foundation {
            Ieee802154MacPolicyRecovery::Reset(Ieee802154Reset {
                inner: foundation.inner.forget_foundation(),
            })
        } else {
            Ieee802154MacPolicyRecovery::Foundation(foundation)
        }
    }
}

impl<P: Ieee802154PlatformControl> Radio<P, radio_state::Powered> {
    /// Consume the powered owner and prove all IEEE 802.15.4 clock gates.
    ///
    /// The transition is enable-only. Shared modem clocks are not cleared on
    /// failure or drop because no shared client/refcount manager exists yet.
    pub fn into_ieee802154_clocked(
        self,
    ) -> Result<Ieee802154Clocked<P>, Ieee802154ClockTransitionFailure<P>> {
        let backend = OwnedIeee802154Backend { radio: self };
        establish_ieee802154_clocks(backend)
            .map(|inner| Ieee802154Clocked { inner })
            .map_err(|inner| Ieee802154ClockTransitionFailure { inner })
    }
}

impl<P: Ieee802154PlatformControl> Ieee802154Clocked<P> {
    /// Pulse the functional MAC reset and then the APB reset.
    pub fn reset_mac(self) -> Result<Ieee802154Reset<P>, Ieee802154ResetTransitionFailure<P>> {
        self.inner
            .reset_mac()
            .map(|inner| Ieee802154Reset { inner })
            .map_err(|inner| Ieee802154ResetTransitionFailure { inner })
    }

    /// Borrow the integration token without releasing whole-radio ownership.
    pub fn peripheral(&self) -> &P {
        self.inner.backend().radio.peripheral()
    }
}

impl<P: Ieee802154PlatformControl> Ieee802154Reset<P> {
    /// Configure the interrupt-masked, non-operational MAC foundation.
    pub fn configure_foundation(
        self,
    ) -> Result<Ieee802154FoundationConfigured<P>, Ieee802154FoundationTransitionFailure<P>> {
        self.inner
            .configure_foundation()
            .map(|inner| Ieee802154FoundationConfigured { inner })
            .map_err(|inner| Ieee802154FoundationTransitionFailure { inner })
    }

    /// Borrow the integration token without releasing whole-radio ownership.
    pub fn peripheral(&self) -> &P {
        self.inner.backend().radio.peripheral()
    }
}

impl<P> Ieee802154FoundationConfigured<P> {
    /// Run the closed `EVENT_STATUS` access-class discriminator.
    ///
    /// This consuming validation transition requires the foundation's proved
    /// zero `EVENT_ENABLE` image and the dedicated image's unique route-
    /// isolation capability. It returns a terminal owner with raw evidence;
    /// even a `Complete` stop cannot re-enter the normal lifecycle or create a
    /// production acknowledge or IRQ capability.
    #[cfg(feature = "validation-probes")]
    pub fn validation_probe_event_status(
        mut self,
        config: Ieee802154EventStatusProbeConfig,
        isolation: Ieee802154EventStatusProbeIsolation,
    ) -> Ieee802154EventStatusProbeFinished<P> {
        let mut hal = self.inner.backend_mut().mac_hal();
        let evidence = run_ieee802154_event_status_probe(&mut hal, config);
        Ieee802154EventStatusProbeFinished {
            evidence,
            _foundation: self,
            _isolation: isolation,
        }
    }

    /// Configure and prove the known, interrupt-masked static MAC policy.
    ///
    /// This deterministic subset omits the vendor TX-power step because its
    /// RF-dependent mapping remains opaque. No PHY/RF/BTBB, IRQ, DMA, start,
    /// stop, or `EVENT_STATUS` operation occurs.
    pub fn configure_mac_policy(
        self,
        policy: Ieee802154MacPolicy,
    ) -> Result<Ieee802154MacPolicyConfigured<P>, Ieee802154MacPolicyTransitionFailure<P>> {
        configure_ieee802154_mac_policy(self, policy)
            .map(|foundation| Ieee802154MacPolicyConfigured { foundation, policy })
            .map_err(|inner| Ieee802154MacPolicyTransitionFailure { inner })
    }

    /// Borrow the integration token without claiming RF readiness.
    pub fn peripheral(&self) -> &P {
        self.inner.backend().radio.peripheral()
    }
}

impl<P> Ieee802154MacPolicyConfigured<P> {
    /// Return the semantically proved static policy.
    pub const fn policy(&self) -> Ieee802154MacPolicy {
        self.policy
    }

    /// Borrow the integration token without claiming operational readiness.
    pub fn peripheral(&self) -> &P {
        self.foundation.peripheral()
    }
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use crate::{Radio, state as radio_state, wifi_bb::PhyWifiBbControl};

    use super::{
        Ieee802154ClockCheckpoint, Ieee802154ClockImages, Ieee802154Clocked,
        Ieee802154PlatformControl, Ieee802154ReadbackError, Ieee802154Reset, Ieee802154ResetImages,
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        ClockMaps,
        ConfigureModemSource,
        Coex,
        WifiBb80x1,
        Etm,
        BtApb,
        CommonBb,
        MacClocks,
        MacReset(bool),
        ApbReset(bool),
    }

    #[derive(Debug)]
    struct FakePlatform {
        operations: Vec<Operation>,
        clocks: Ieee802154ClockImages,
        resets: Ieee802154ResetImages,
    }

    impl FakePlatform {
        fn ready() -> Self {
            Self {
                operations: Vec::new(),
                clocks: Ieee802154ClockImages {
                    modem_clock_maps_configured: true,
                    pll_160m_clock_enabled: true,
                    modem_source_clock_configured: true,
                    coexistence_clock_enabled: true,
                    wifi_bb_80x1_clock_enabled: true,
                    etm_clock_enabled: true,
                    bt_apb_clock_enabled: true,
                    modem_security_apb_clock_enabled: true,
                    bt_ieee802154_common_baseband_clock_enabled: true,
                    ieee802154_apb_clock_enabled: true,
                    ieee802154_mac_clock_enabled: true,
                },
                resets: Ieee802154ResetImages {
                    mac_reset_released: true,
                    apb_reset_released: true,
                },
            }
        }
    }

    impl Ieee802154PlatformControl for FakePlatform {
        fn configure_modem_clock_maps(&mut self) {
            self.operations.push(Operation::ClockMaps);
        }

        fn configure_modem_source_clock(&mut self) {
            self.operations.push(Operation::ConfigureModemSource);
        }

        fn enable_coexistence_clock(&mut self) {
            self.operations.push(Operation::Coex);
        }

        fn enable_wifi_bb_80x1_clock(&mut self) {
            self.operations.push(Operation::WifiBb80x1);
        }

        fn enable_etm_clock(&mut self) {
            self.operations.push(Operation::Etm);
        }

        fn enable_bt_apb_clocks(&mut self) {
            self.operations.push(Operation::BtApb);
        }

        fn enable_bt_ieee802154_common_baseband_clock(&mut self) {
            self.operations.push(Operation::CommonBb);
        }

        fn enable_ieee802154_mac_clocks(&mut self) {
            self.operations.push(Operation::MacClocks);
        }

        fn ieee802154_clock_images(&self) -> Ieee802154ClockImages {
            self.clocks
        }

        fn set_ieee802154_mac_reset(&mut self, asserted: bool) {
            self.operations.push(Operation::MacReset(asserted));
        }

        fn set_ieee802154_apb_reset(&mut self, asserted: bool) {
            self.operations.push(Operation::ApbReset(asserted));
        }

        fn ieee802154_reset_images(&self) -> Ieee802154ResetImages {
            self.resets
        }
    }

    impl PhyWifiBbControl for FakePlatform {
        fn clear_cold_start_wifi_control(&mut self) {}

        fn wifi_baseband_is_enabled(&self) -> bool {
            false
        }

        fn set_wifi_baseband_enabled(&mut self, _enabled: bool) {}

        fn set_bss_cbw_40_digital(&mut self, _enabled: bool) {}

        fn set_bb_agc_update_encoding(&mut self, _encoding: u8) {}

        fn set_mac_baseband_enabled(&mut self, _enabled: bool) {}
    }

    fn require_powered(_: &Radio<FakePlatform, radio_state::Powered>) {}
    fn require_clocked(_: &Ieee802154Clocked<FakePlatform>) {}
    fn require_reset(_: &Ieee802154Reset<FakePlatform>) {}

    #[test]
    fn concrete_role_consumes_the_existing_powered_owner() {
        let powered = Radio::claim(FakePlatform::ready())
            .expect("validation claim")
            .assume_powered_after_external_initialization();
        require_powered(&powered);

        let clocked = powered.into_ieee802154_clocked().expect("clock readback");
        require_clocked(&clocked);
        assert_eq!(clocked.peripheral().operations.len(), 8);

        let reset = clocked.reset_mac().expect("reset readback");
        require_reset(&reset);
        assert_eq!(reset.peripheral().operations.len(), 12);
    }

    #[test]
    fn clock_failure_recovers_the_same_powered_owner() {
        let mut platform = FakePlatform::ready();
        platform.clocks.etm_clock_enabled = false;
        let powered = Radio::claim(platform)
            .expect("validation claim")
            .assume_powered_after_external_initialization();

        let failure = match powered.into_ieee802154_clocked() {
            Ok(_) => panic!("failed readback must not publish Clocked"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            Ieee802154ReadbackError {
                checkpoint: Ieee802154ClockCheckpoint::EtmClock,
                expected: true,
                observed: false,
            }
        );
        let powered = failure.into_powered();
        require_powered(&powered);
        assert_eq!(powered.peripheral().operations.len(), 8);
    }
}
