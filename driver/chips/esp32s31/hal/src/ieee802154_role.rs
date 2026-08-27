//! Whole-owner lifecycle for the dedicated IEEE 802.15.4 radio route.
//!
//! The entry owner consumes the protocol-neutral [`RadioHardware`] root
//! directly into the PAC's IEEE 802.15.4 route. Task-side MAC/shared-PHY
//! ownership and the inactive interrupt owner remain disjoint for the complete
//! lifecycle; no temporary Wi-Fi register owner exists. Completion of this
//! module's final state is intentionally weaker than common-PHY, BTBB, RF, or
//! operational-MAC readiness.

#![forbid(unsafe_code)]

use core::fmt;

use open_esp_radio_esp32s31_pac::{
    Ieee802154FoundationSnapshot, Ieee802154InterruptSetup, Ieee802154Pti, Ieee802154TaskRegisters,
    Ieee802154TimingPrerequisite, Ieee802154TimingReady, RadioHardware, RadioPhyReleaseError,
};

#[cfg(feature = "validation-probes")]
use crate::ieee802154_ed_event_probe::{
    Ieee802154EdEventProbeConfig, Ieee802154EdEventProbeEvidence, Ieee802154EdEventProbeIsolation,
    run_ieee802154_ed_event_probe,
};
#[cfg(feature = "validation-probes")]
use crate::ieee802154_event_status_probe::{
    Ieee802154EventStatusProbeConfig, Ieee802154EventStatusProbeEvidence,
    Ieee802154EventStatusProbeIsolation, run_ieee802154_event_status_probe,
};

use crate::{
    Ieee802154SharedPhyBorrow, SharedPhyHal,
    ieee802154::Ieee802154PacHal,
    ieee802154_lifecycle::{
        Ieee802154ClockCheckpoint, Ieee802154ClockFailure as EngineClockFailure,
        Ieee802154ClockImages, Ieee802154FoundationCheckpoint,
        Ieee802154FoundationFailure as EngineFoundationFailure, Ieee802154Lifecycle,
        Ieee802154LifecycleBackend, Ieee802154ReadbackError, Ieee802154ResetCheckpoint,
        Ieee802154ResetFailure as EngineResetFailure, Ieee802154ResetImages,
        establish_ieee802154_clocks, state as lifecycle_state,
    },
    ieee802154_operation::{
        Ieee802154OperationPollBudget, Ieee802154PolledOperation,
        Ieee802154PolledOperationEvidence, Ieee802154PolledOperationFailure,
        run_ieee802154_polled_operation,
    },
    ieee802154_policy::{
        Ieee802154AckTimeout, Ieee802154CcaMode, Ieee802154MacControl, Ieee802154MacPolicy,
        Ieee802154MacPolicyBackend, Ieee802154MacPolicyCheckpoint,
        Ieee802154MacPolicyFailure as EngineMacPolicyFailure, Ieee802154MacPolicyReadback,
        Ieee802154PanIdentity, configure_ieee802154_mac_policy,
    },
    power::{self, PowerError},
};

struct OwnedIeee802154Backend<P> {
    platform: P,
    task: Ieee802154TaskRegisters,
    interrupts: Ieee802154InterruptSetup,
}

/// Failed cold-route release retaining the complete IEEE 802.15.4 owner.
#[must_use = "failed IEEE 802.15.4 release still owns the platform and radio route"]
pub struct Ieee802154OwnedReleaseFailure<P> {
    owner: Ieee802154Owned<P>,
    error: RadioPhyReleaseError,
}

impl<P> Ieee802154OwnedReleaseFailure<P> {
    pub const fn error(&self) -> RadioPhyReleaseError {
        self.error
    }

    pub fn into_parts(self) -> (Ieee802154Owned<P>, RadioPhyReleaseError) {
        (self.owner, self.error)
    }
}

impl<P> fmt::Debug for Ieee802154OwnedReleaseFailure<P> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Ieee802154OwnedReleaseFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl<P> OwnedIeee802154Backend<P> {
    fn mac_hal(&mut self) -> Ieee802154PacHal<'_> {
        Ieee802154PacHal::from_owned(&mut self.task, &mut self.interrupts)
    }
}

impl<P> Ieee802154LifecycleBackend for OwnedIeee802154Backend<P> {
    fn configure_modem_clock_maps(&mut self) {
        self.task.configure_modem_syscon_clock_maps();
        self.task.prepare_shared_modem_clock_map();
    }

    fn configure_modem_source_clock(&mut self) {
        self.task.configure_modem_source_clocks();
    }

    fn enable_wifi_bb_80x1_clock(&mut self) {
        self.task.enable_ieee802154_wifi_bb_clock();
    }

    fn enable_etm_clock(&mut self) {
        self.task.enable_ieee802154_etm_clock();
    }

    fn enable_bt_apb_clocks(&mut self) {
        self.task.enable_ieee802154_bt_apb_clocks();
    }

    fn enable_bt_ieee802154_common_baseband_clock(&mut self) {
        self.task.enable_ieee802154_common_baseband_clock();
    }

    fn enable_ieee802154_mac_clocks(&mut self) {
        self.task.enable_ieee802154_mac_clocks();
    }

    fn set_ieee802154_mac_reset(&mut self, asserted: bool) {
        self.task.set_ieee802154_mac_reset(asserted);
    }

    fn set_ieee802154_apb_reset(&mut self, asserted: bool) {
        self.task.set_ieee802154_apb_reset(asserted);
    }

    fn ieee802154_reset_images(&self) -> Ieee802154ResetImages {
        let reset = self.task.modem_syscon_ieee802154_reset_observation();
        Ieee802154ResetImages {
            mac_reset_released: reset.mac_reset_released,
            apb_reset_released: reset.apb_reset_released,
        }
    }
    fn enable_coexistence_clock(&mut self) {
        self.task.retain_coexistence_clock();
    }

    fn ieee802154_clock_images(&self) -> Ieee802154ClockImages {
        let platform = self.task.platform_clock_power_observation();
        let shared = self.task.shared_modem_clock_observation();
        let modem = self.task.modem_syscon_ieee802154_clock_observation();
        Ieee802154ClockImages {
            modem_clock_maps_configured: modem.active_clock_map_configured
                && shared.power_state_map_configured,
            pll_160m_clock_enabled: platform.ref_160m_clock_enabled,
            modem_source_clock_configured: platform.modem_source_clocks_configured,
            coexistence_clock_enabled: shared.coexistence_clock_enabled,
            wifi_bb_80x1_clock_enabled: modem.wifi_bb_80x1_clock_enabled,
            etm_clock_enabled: modem.etm_clock_enabled,
            bt_apb_clock_enabled: modem.bt_apb_clock_enabled,
            modem_security_apb_clock_enabled: modem.modem_security_apb_clock_enabled,
            bt_ieee802154_common_baseband_clock_enabled: modem.common_baseband_clock_enabled,
            ieee802154_apb_clock_enabled: modem.ieee802154_apb_clock_enabled,
            ieee802154_mac_clock_enabled: modem.ieee802154_mac_clock_enabled,
        }
    }

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

/// Exclusive cold owner of the dedicated IEEE 802.15.4 radio route.
///
/// Construction consumes the protocol-neutral hardware root and immediately
/// separates the task and inactive interrupt partitions without touching
/// MMIO. It proves neither clocks nor common-PHY, BTBB, coexistence, RF, IRQ,
/// DMA, or MAC readiness.
#[must_use = "the IEEE 802.15.4 owner retains every radio hardware partition"]
pub struct Ieee802154Owned<P> {
    backend: OwnedIeee802154Backend<P>,
}

impl<P> Ieee802154Owned<P> {
    /// Claim the process-wide radio root directly for IEEE 802.15.4.
    ///
    /// A failed singleton claim returns the integration token unchanged. This
    /// ownership-only transition performs no clock, reset, PHY, or MAC write.
    pub fn claim(platform: P) -> Result<Self, P> {
        #[cfg(not(test))]
        let Some(hardware) = RadioHardware::take() else {
            return Err(platform);
        };
        #[cfg(test)]
        let hardware = RadioHardware::for_validation();
        Ok(Self::from_hardware(platform, hardware))
    }

    /// Construct the dedicated owner without consuming the process singleton.
    ///
    /// This validation-only entry point exists so dependent crate tests can
    /// exercise the exact production ownership route without raw PAC access.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn claim_for_validation(platform: P) -> Self {
        Self::from_hardware(platform, RadioHardware::for_validation())
    }

    /// Bind an already-owned neutral radio root to the IEEE 802.15.4 route.
    pub fn from_hardware(platform: P, hardware: RadioHardware) -> Self {
        let (task, interrupts) = hardware.into_ieee802154().separate_interrupt_owner();
        Self {
            backend: OwnedIeee802154Backend {
                platform,
                task,
                interrupts,
            },
        }
    }

    /// Return the untouched cold route to its protocol-neutral root.
    ///
    /// This method is intentionally unavailable after the first power
    /// transition, whose partial mutation requires typed retry ownership.
    ///
    /// # Errors
    ///
    /// Returns [`Ieee802154OwnedReleaseFailure`] retaining this owner and its
    /// platform while TX-DC PWDET, TX-IQ tone control, or RX-DCO control still
    /// awaits restoration in the PAC.
    pub fn release(self) -> Result<(P, RadioHardware), Ieee802154OwnedReleaseFailure<P>> {
        let OwnedIeee802154Backend {
            platform,
            task,
            interrupts,
        } = self.backend;
        let cold = task.into_cold(interrupts);
        match cold.release() {
            Ok(hardware) => Ok((platform, hardware)),
            Err(failure) => {
                let (cold, error) = failure.into_parts();
                let (task, interrupts) = cold.separate_interrupt_owner();
                Err(Ieee802154OwnedReleaseFailure {
                    owner: Self {
                        backend: OwnedIeee802154Backend {
                            platform,
                            task,
                            interrupts,
                        },
                    },
                    error,
                })
            }
        }
    }

    /// Borrow the integration token before any lifecycle mutation.
    pub const fn peripheral(&self) -> &P {
        &self.backend.platform
    }
}

/// Dedicated IEEE 802.15.4 owner after shared modem/PHY power prerequisites.
///
/// This state retains the same task and inactive interrupt partitions as the
/// cold owner. It proves only the generic modem/PHY clock and reset sequence;
/// IEEE-specific clocks and common-PHY registration remain separate stages.
#[must_use = "the powered IEEE 802.15.4 owner retains every radio partition"]
pub struct Ieee802154Powered<P> {
    backend: OwnedIeee802154Backend<P>,
}

/// Failed shared modem/PHY power transition retaining the dedicated route.
///
/// The finite transaction may already have changed shared clock/reset state.
/// Safe recovery is therefore limited to inspecting the exact checkpoint or
/// retrying with this same owner; it cannot be released as a cold radio root.
#[must_use = "a failed IEEE 802.15.4 power transition still owns the radio"]
pub struct Ieee802154PowerTransitionFailure<P> {
    backend: OwnedIeee802154Backend<P>,
    error: PowerError,
}

impl<P> Ieee802154PowerTransitionFailure<P> {
    /// Inspect the first failed shared power prerequisite.
    pub const fn error(&self) -> PowerError {
        self.error
    }
}

impl<P> Ieee802154PowerTransitionFailure<P> {
    /// Retry the exact shared power sequence with the retained route.
    pub fn retry(self) -> Result<Ieee802154Powered<P>, Self> {
        enter_ieee802154_powered(self.backend)
    }
}

impl<P> fmt::Debug for Ieee802154PowerTransitionFailure<P> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Ieee802154PowerTransitionFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
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
/// The reset-isolated discriminator remains validation-only and terminal.
/// Production acknowledgement uses a separate affine full-snapshot W1C owner;
/// this probe's raw paired-bit writes still cannot preserve a normal lifecycle
/// proof. This type
/// exposes only evidence and deliberately provides no route back to foundation,
/// policy, or operational transitions. The reset-isolation capability remains
/// consumed for the rest of the process.
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

/// Terminal whole-radio owner after the validation-only ED event experiment.
///
/// The probe rechecks historical selective clearing with fixed ED-DONE and
/// TIMER0 validation writes. Production acknowledgement uses the generated
/// affine W1C snapshot instead. The experimental cleanup still ends the normal
/// lifecycle, so this owner cannot be promoted to an operational state.
#[cfg(feature = "validation-probes")]
#[must_use = "the terminal validation owner retains the radio and isolation capability"]
pub struct Ieee802154EdEventProbeFinished<P> {
    evidence: Ieee802154EdEventProbeEvidence,
    _policy: Ieee802154MacPolicyConfigured<P>,
    _isolation: Ieee802154EdEventProbeIsolation,
}

#[cfg(feature = "validation-probes")]
impl<P> Ieee802154EdEventProbeFinished<P> {
    /// Borrow the complete raw evidence retained by the terminal owner.
    pub const fn evidence(&self) -> &Ieee802154EdEventProbeEvidence {
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
/// Event and abort delivery is masked between finite polled operations. This
/// state supports only serialized raw ED and CCA; it is not PHY/RF or BTBB
/// readiness, does not route an IRQ, owns no DMA buffer, and is not an
/// operational RX/TX MAC. It is not a complete vendor PIB because TX-power
/// mapping remains opaque.
pub struct Ieee802154MacPolicyConfigured<P> {
    foundation: Ieee802154FoundationConfigured<P>,
    policy: Ieee802154MacPolicy,
}

/// Successfully recovered finite ED/CCA operation with the reusable whole
/// radio owner retained.
///
/// The evidence is MAC-level only. An ED RSS code is uncalibrated and neither
/// result proves RFPLL tuning, RF performance, PHY conformance, or a complete
/// operational IEEE 802.15.4 dataplane.
#[must_use = "a completed IEEE 802.15.4 operation retains the reusable radio owner"]
pub struct Ieee802154OperationCompleted<P> {
    owner: Ieee802154MacPolicyConfigured<P>,
    evidence: Ieee802154PolledOperationEvidence,
}

impl<P> Ieee802154OperationCompleted<P> {
    /// Return the finite operation evidence retained across exact recovery.
    pub const fn evidence(&self) -> &Ieee802154PolledOperationEvidence {
        &self.evidence
    }

    /// Consume the evidence wrapper and recover the same static-policy owner
    /// for a subsequent serialized operation.
    pub fn into_owner(self) -> Ieee802154MacPolicyConfigured<P> {
        self.owner
    }
}

/// Terminal failed ED/CCA operation retaining the whole radio owner without a
/// recovery transition.
///
/// Abort and timeout can leave hardware activity unresolved. Invariant
/// failures mean the exact detached polling contract was not preserved. The
/// retained owner therefore cannot be reused through safe code.
#[must_use = "a failed IEEE 802.15.4 operation retains a terminal radio owner"]
pub struct Ieee802154OperationFailed<P> {
    failure: Ieee802154PolledOperationFailure,
    owner: Ieee802154MacPolicyConfigured<P>,
}

impl<P> Ieee802154OperationFailed<P> {
    /// Return complete typed failure evidence.
    pub const fn failure(&self) -> Ieee802154PolledOperationFailure {
        self.failure
    }

    /// Borrow the integration token for diagnostics without recovering the
    /// terminal radio owner.
    pub fn peripheral(&self) -> &P {
        self.owner.peripheral()
    }
}

/// Failed clock readback retaining the dedicated powered route.
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

    /// Retry the exact clock sequence without releasing the mutated route.
    pub fn retry(self) -> Result<Ieee802154Clocked<P>, Self> {
        establish_ieee802154_clocks(self.inner.into_backend())
            .map(|inner| Ieee802154Clocked { inner })
            .map_err(|inner| Self { inner })
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

impl<P> Ieee802154Owned<P> {
    /// Execute and prove the shared modem/PHY power prerequisites.
    ///
    /// The transaction is the same concrete target sequence used before the
    /// common PHY registration path. It operates only through the retained
    /// platform token and never converts the radio registers into Wi-Fi.
    pub fn power_up(self) -> Result<Ieee802154Powered<P>, Ieee802154PowerTransitionFailure<P>> {
        enter_ieee802154_powered(self.backend)
    }
}

fn enter_ieee802154_powered<P>(
    mut backend: OwnedIeee802154Backend<P>,
) -> Result<Ieee802154Powered<P>, Ieee802154PowerTransitionFailure<P>> {
    if let Err(error) = power::execute_owned(&mut backend.task) {
        return Err(Ieee802154PowerTransitionFailure { backend, error });
    }
    Ok(Ieee802154Powered { backend })
}

impl<P> Ieee802154Powered<P> {
    /// Consume the powered dedicated owner and prove IEEE 802.15.4 clock gates.
    ///
    /// Shared modem clock gates are route-owned and reference-counted by the
    /// PAC. Releasing the route restores the baseline observed by its first
    /// retained lease; the global ICG state-map initialization is monotonic.
    pub fn into_ieee802154_clocked(
        self,
    ) -> Result<Ieee802154Clocked<P>, Ieee802154ClockTransitionFailure<P>> {
        establish_ieee802154_clocks(self.backend)
            .map(|inner| Ieee802154Clocked { inner })
            .map_err(|inner| Ieee802154ClockTransitionFailure { inner })
    }
}

impl<P> Ieee802154Clocked<P> {
    /// Pulse the functional MAC reset and then the APB reset.
    pub fn reset_mac(self) -> Result<Ieee802154Reset<P>, Ieee802154ResetTransitionFailure<P>> {
        self.inner
            .reset_mac()
            .map(|inner| Ieee802154Reset { inner })
            .map_err(|inner| Ieee802154ResetTransitionFailure { inner })
    }

    /// Borrow the integration token without releasing whole-radio ownership.
    pub fn peripheral(&self) -> &P {
        &self.inner.backend().platform
    }
}

impl<P> Ieee802154Clocked<P> {
    /// Consume a terminal common-PHY prerequisite on the retained task owner.
    ///
    /// This narrow bridge neither exposes nor releases the PAC task partition.
    /// The prerequisite is affine and can only be minted at the higher-layer
    /// terminal common-PHY proof boundary; the returned marker must remain
    /// coupled to every production lifecycle state that follows.
    #[doc(hidden)]
    pub fn initialize_baseband_and_ieee802154_timing(
        &mut self,
        prerequisite: Ieee802154TimingPrerequisite,
    ) -> Ieee802154TimingReady {
        self.inner
            .backend_mut()
            .task
            .initialize_baseband_and_ieee802154_timing(prerequisite)
    }
}

impl<P> Ieee802154Clocked<P> {
    /// Borrow the exact platform and shared-PHY partitions for the recovered
    /// concrete common-PHY target transition.
    ///
    /// This method does not run that asynchronous transition or mint a
    /// registered/RF-ready state. It exists so the PHY crate can compose the
    /// same `TargetPhyRegisterPort` used by standalone Bluetooth while the
    /// inactive IEEE interrupt owner stays retained here.
    #[doc(hidden)]
    pub fn common_phy_parts(&mut self) -> (&mut P, SharedPhyHal<'_>) {
        let backend = self.inner.backend_mut();
        let shared_phy = backend.task.borrow_shared_phy();
        (&mut backend.platform, shared_phy)
    }
}

impl<P> Ieee802154Reset<P> {
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
        &self.inner.backend().platform
    }
}

impl<P> Ieee802154FoundationConfigured<P> {
    /// Run the closed `EVENT_STATUS` access-class discriminator.
    ///
    /// This consuming validation transition requires the foundation's proved
    /// zero `EVENT_ENABLE` image and the dedicated image's unique route-
    /// isolation capability. It returns a terminal owner with raw evidence;
    /// even a `Complete` stop cannot re-enter the normal lifecycle or create a
    /// general acknowledgement or active IRQ capability. Production uses the
    /// separate generated affine W1C transaction.
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
        &self.inner.backend().platform
    }
}

impl<P> Ieee802154MacPolicyConfigured<P> {
    /// Run one finite energy-detection command on the policy's proved channel.
    ///
    /// The returned signed code is raw and uncalibrated. The transaction owns
    /// no DMA buffer, installs no CPU interrupt route, and polls only while
    /// both complete source-132 route words remain at reset. Success proves
    /// a reusable owner only when the complete snapshot actually consumed by
    /// W1C acknowledgement is exactly lone `ED_DONE`; every other acknowledged
    /// image or terminal condition is retained diagnostically and fails stop.
    pub fn energy_detection_raw(
        self,
        duration: u16,
        budget: Ieee802154OperationPollBudget,
    ) -> Result<Ieee802154OperationCompleted<P>, Ieee802154OperationFailed<P>> {
        let operation =
            Ieee802154PolledOperation::energy_detection(self.policy.channel(), duration);
        self.run_polled_operation(operation, budget)
    }

    /// Run one finite CCA command using the complete proved static CCA policy.
    ///
    /// The result is the source-confirmed `CCA_BUSY` bit. It is not an RF
    /// sensitivity, timing-conformance, coexistence, IRQ, or dataplane claim.
    /// Success returns the reusable owner; abort, timeout, or any invariant
    /// mismatch is terminal.
    pub fn clear_channel_assessment(
        self,
        budget: Ieee802154OperationPollBudget,
    ) -> Result<Ieee802154OperationCompleted<P>, Ieee802154OperationFailed<P>> {
        let operation = Ieee802154PolledOperation::clear_channel_assessment(
            self.policy.channel(),
            self.policy.cca_mode(),
            self.policy.cca_threshold_code(),
        );
        self.run_polled_operation(operation, budget)
    }

    fn run_polled_operation(
        mut self,
        operation: Ieee802154PolledOperation,
        budget: Ieee802154OperationPollBudget,
    ) -> Result<Ieee802154OperationCompleted<P>, Ieee802154OperationFailed<P>> {
        let result = {
            let hal = self.foundation.inner.backend_mut().mac_hal();
            run_ieee802154_polled_operation(hal, operation, budget)
        };
        match result {
            Ok(evidence) => Ok(Ieee802154OperationCompleted {
                owner: self,
                evidence,
            }),
            Err(failure) => Err(Ieee802154OperationFailed {
                failure,
                owner: self,
            }),
        }
    }

    /// Run the closed ED-DONE/TIMER0 event discriminator.
    ///
    /// Requiring this state proves that an explicit IEEE channel and the
    /// reviewed static MAC policy read back before ED starts. The transition
    /// still starts no DMA and installs no CPU interrupt route. It returns a
    /// terminal evidence owner even on success. Its fixed validation writes do
    /// not create another production acknowledgement API, active IRQ,
    /// concurrency, or RF-readiness claim.
    #[cfg(feature = "validation-probes")]
    pub fn validation_probe_ed_event_status(
        mut self,
        config: Ieee802154EdEventProbeConfig,
        isolation: Ieee802154EdEventProbeIsolation,
    ) -> Ieee802154EdEventProbeFinished<P> {
        let mut hal = self.foundation.inner.backend_mut().mac_hal();
        let evidence = run_ieee802154_ed_event_probe(&mut hal, config);
        Ieee802154EdEventProbeFinished {
            evidence,
            _policy: self,
            _isolation: isolation,
        }
    }

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
    use open_esp_radio_esp32s31_pac::RadioHardware;

    use super::Ieee802154Owned;

    #[derive(Debug)]
    struct FakePlatform;

    #[test]
    fn untouched_owner_releases_the_complete_neutral_root() {
        let owned = Ieee802154Owned::from_hardware(FakePlatform, RadioHardware::for_validation());
        let (_platform, hardware) = owned
            .release()
            .expect("an untouched IEEE 802.15.4 route can be released");

        let ieee = hardware.into_ieee802154();
        let (task, interrupts) = ieee.separate_interrupt_owner();
        let _hardware = task
            .into_cold(interrupts)
            .release()
            .expect("an untouched IEEE 802.15.4 route can be released");
    }
}
