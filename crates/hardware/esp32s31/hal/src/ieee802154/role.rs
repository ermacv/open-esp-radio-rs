//! IEEE 802.15.4 lifecycle as a client of the shared radio arbiter.
//!
//! The chain starts from the [`Ieee802154RadioPartition`] of a concurrent
//! split and holds only the MAC, its interrupt route and its ETM channels.
//! Common radio power, the module clocks and the private MAC resets live in
//! the shared radio registers, so each of those transitions borrows the
//! arbiter's [`SharedRadioLease`] for its duration and never retains it. The
//! MAC foundation and policy touch only the partition.
//!
//! The public ESP-IDF enable order is `ieee802154_enable` (module clocks),
//! `esp_phy_enable`, `esp_btbb_enable` and then `ieee802154_mac_init`, whose
//! first step is the MAC reset. The PHY client and BTBB steps belong to the
//! PHY layer, which composes them at [`Ieee802154Clocked`]. Completion of this
//! module's states is intentionally weaker than common-PHY, BTBB, RF, or
//! operational-MAC readiness.

#![deny(unsafe_code)]

use core::fmt;

use oer_esp32s31_pac::{
    Ieee802154FoundationSnapshot, Ieee802154InterruptSetup, Ieee802154Pti, Ieee802154TaskRegisters,
    RadioPhyRegisters,
};

#[cfg(feature = "validation-probes")]
use crate::ieee802154::validation::{
    ed_event::{
        Ieee802154EdEventProbeConfig, Ieee802154EdEventProbeEvidence,
        Ieee802154EdEventProbeIsolation, run_ieee802154_ed_event_probe,
    },
    event_status::{
        Ieee802154EventStatusProbeConfig, Ieee802154EventStatusProbeEvidence,
        Ieee802154EventStatusProbeIsolation, run_ieee802154_event_status_probe,
    },
};

use crate::{
    ieee802154::{
        backend::Ieee802154PacHal,
        lifecycle::{
            Ieee802154FoundationCheckpoint, Ieee802154FoundationFailure as EngineFoundationFailure,
            Ieee802154Lifecycle, Ieee802154LifecycleBackend, Ieee802154ReadbackError,
            Ieee802154ResetCheckpoint, Ieee802154ResetFailure as EngineResetFailure,
            Ieee802154ResetPort, Ieee802154ResetReadback, state as lifecycle_state,
        },
        mac::{Ieee802154InterruptSetupOwner, Ieee802154TaskOwner},
        operation::{
            Ieee802154OperationPollBudget, Ieee802154PolledOperation,
            Ieee802154PolledOperationEvidence, Ieee802154PolledOperationFailure,
            run_ieee802154_polled_operation,
        },
        policy::{
            Ieee802154AckTimeout, Ieee802154CcaMode, Ieee802154MacControl, Ieee802154MacPolicy,
            Ieee802154MacPolicyBackend, Ieee802154MacPolicyCheckpoint,
            Ieee802154MacPolicyFailure as EngineMacPolicyFailure, Ieee802154MacPolicyReadback,
            Ieee802154MacPolicyWrites, Ieee802154PanIdentity, configure_ieee802154_mac_policy,
        },
    },
    root::Ieee802154RadioPartition,
    shared_radio::{
        BtbbAcquired, BtbbError, ClientQuiescence, CommonRadioPowerError, EmptyQuiescentWindow,
        ModemClockError, PlatformClockProvider, SharedRadioLease,
    },
};

/// The IEEE 802.15.4 partition split into its task and inactive interrupt
/// halves.
struct Ieee802154Mac {
    task: Ieee802154TaskRegisters,
    interrupts: Ieee802154InterruptSetup,
}

impl Ieee802154Mac {
    fn mac_hal(&mut self) -> Ieee802154PacHal<'_> {
        Ieee802154PacHal::from_owned(&mut self.task, &mut self.interrupts)
    }
}

impl Ieee802154LifecycleBackend for Ieee802154Mac {
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

    fn apply_rx_on_delay(&mut self) {
        self.mac_hal().apply_rx_on_delay();
    }

    fn order_device_accesses(&mut self) {
        self.mac_hal().order_device_accesses();
    }

    fn foundation_snapshot(&mut self) -> Ieee802154FoundationSnapshot {
        self.mac_hal().foundation_snapshot()
    }
}

/// The private MAC resets in the shared modem syscon block, borrowed from
/// one lease for one reset transition.
struct LeasedResetPort<'a> {
    radio_phy: &'a mut RadioPhyRegisters,
}

impl Ieee802154ResetPort for LeasedResetPort<'_> {
    fn set_ieee802154_mac_reset(&mut self, asserted: bool) {
        self.radio_phy.set_ieee802154_mac_reset(asserted);
    }

    fn set_ieee802154_apb_reset(&mut self, asserted: bool) {
        self.radio_phy.set_ieee802154_apb_reset(asserted);
    }

    fn ieee802154_reset_readback(&self) -> Ieee802154ResetReadback {
        let reset = self.radio_phy.ieee802154_reset_observation();
        Ieee802154ResetReadback {
            mac_reset_released: reset.mac_reset_released,
            apb_reset_released: reset.apb_reset_released,
        }
    }
}

/// Unpowered IEEE 802.15.4 client holding its partition.
///
/// Construction splits the partition without touching MMIO. It proves
/// neither power nor clocks, common-PHY, BTBB, RF, IRQ, DMA, or MAC
/// readiness.
#[must_use = "the IEEE 802.15.4 client retains its radio partition"]
pub struct Ieee802154Cold {
    mac: Ieee802154Mac,
}

impl Ieee802154Cold {
    /// Take the partition of a concurrent split. This performs no MMIO.
    pub fn from_partition(partition: Ieee802154RadioPartition) -> Self {
        let (task, interrupts) = Ieee802154TaskRegisters::new(partition.into_mac());
        Self {
            mac: Ieee802154Mac { task, interrupts },
        }
    }

    /// Return the partition. This performs no MMIO.
    pub fn into_partition(self) -> Ieee802154RadioPartition {
        let Ieee802154Mac { task, interrupts } = self.mac;
        Ieee802154RadioPartition::from_mac(task.into_partition(interrupts))
    }

    /// Enter common radio power as the IEEE 802.15.4 client.
    ///
    /// Only the first client runs the modem/PHY power sequence.
    ///
    /// # Errors
    ///
    /// The client already holds power or the first client's sequence failed a
    /// read-back checkpoint; the unchanged owner is returned.
    pub fn power_up<T>(
        self,
        lease: &mut SharedRadioLease<'_, T>,
    ) -> Result<Ieee802154Powered, Ieee802154PowerTransitionFailure<Self>> {
        match lease.enter_common_power(&self.mac.task) {
            Ok(()) => Ok(Ieee802154Powered { mac: self.mac }),
            Err(error) => Err(Ieee802154PowerTransitionFailure { owner: self, error }),
        }
    }
}

/// IEEE 802.15.4 client inside common radio power.
///
/// It proves only membership in the common modem/PHY power; the module
/// clocks and the MAC resets are later stages.
#[must_use = "the powered IEEE 802.15.4 client must leave common power"]
pub struct Ieee802154Powered {
    mac: Ieee802154Mac,
}

impl Ieee802154Powered {
    /// Leave common radio power; the last client restores the cold baseline.
    ///
    /// # Errors
    ///
    /// The baseline did not read back; the client stays entered and the
    /// unchanged owner is returned for a retry.
    pub fn power_down<T>(
        self,
        lease: &mut SharedRadioLease<'_, T>,
    ) -> Result<Ieee802154Cold, Ieee802154PowerTransitionFailure<Self>> {
        match lease.exit_common_power(&self.mac.task) {
            Ok(()) => Ok(Ieee802154Cold { mac: self.mac }),
            Err(error) => Err(Ieee802154PowerTransitionFailure { owner: self, error }),
        }
    }
}

impl Ieee802154Powered {
    /// Enable the IEEE 802.15.4 module clocks through the shared planner.
    ///
    /// This is `modem_clock_module_enable(PERIPH_IEEE802154_MODULE)`: only
    /// dependencies no other client holds are switched on; the 160 MHz source
    /// and the analog-I2C master clock go through `platform`.
    ///
    /// # Errors
    ///
    /// The planner rejected the request before any access, or a platform
    /// request failed and poisoned the modem clocks; the owner is returned.
    pub fn enable_clocks<T>(
        self,
        lease: &mut SharedRadioLease<'_, T>,
        platform: &mut impl PlatformClockProvider,
    ) -> Result<Ieee802154Clocked, Ieee802154ClockTransitionFailure<Self>> {
        match lease.enable_modem_clocks(&self.mac.task, platform) {
            Ok(()) => Ok(Ieee802154Clocked {
                inner: Ieee802154Lifecycle::clocked(self.mac),
            }),
            Err(error) => Err(Ieee802154ClockTransitionFailure { owner: self, error }),
        }
    }
}

/// Failed module-clock transition retaining the unchanged owner.
#[must_use = "a failed IEEE 802.15.4 clock transition still owns the partition"]
pub struct Ieee802154ClockTransitionFailure<Owner> {
    owner: Owner,
    error: ModemClockError,
}

impl<Owner> Ieee802154ClockTransitionFailure<Owner> {
    pub const fn error(&self) -> ModemClockError {
        self.error
    }

    /// Recover the owner. After [`ModemClockError::Poisoned`] no further
    /// modem clock change succeeds until reset.
    pub fn into_owner(self) -> Owner {
        self.owner
    }
}

impl<Owner> fmt::Debug for Ieee802154ClockTransitionFailure<Owner> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Ieee802154ClockTransitionFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Failed common-power transition retaining the unchanged owner.
#[must_use = "a failed IEEE 802.15.4 power transition still owns the partition"]
pub struct Ieee802154PowerTransitionFailure<Owner> {
    owner: Owner,
    error: CommonRadioPowerError,
}

impl<Owner> Ieee802154PowerTransitionFailure<Owner> {
    pub const fn error(&self) -> CommonRadioPowerError {
        self.error
    }

    /// Recover the owner for a retry.
    pub fn into_owner(self) -> Owner {
        self.owner
    }
}

impl<Owner> fmt::Debug for Ieee802154PowerTransitionFailure<Owner> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Ieee802154PowerTransitionFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// IEEE 802.15.4 client whose module clocks the shared planner holds.
///
/// This is the stage at which the PHY layer enters the client into the
/// shared PHY domain and the shared BTBB baseband, before the MAC reset.
#[must_use = "the clocked IEEE 802.15.4 client must release its module clocks"]
pub struct Ieee802154Clocked {
    inner: Ieee802154Lifecycle<Ieee802154Mac, lifecycle_state::Clocked>,
}

/// IEEE 802.15.4 client after the MAC and APB resets were pulsed and
/// released.
#[must_use = "the reset IEEE 802.15.4 client retains its partition"]
pub struct Ieee802154Reset {
    inner: Ieee802154Lifecycle<Ieee802154Mac, lifecycle_state::Reset>,
}

/// Interrupt-masked static MAC foundation.
///
/// This state does not imply PHY/RF ownership, interrupt routing, DMA buffer
/// setup, or an idle/operational MAC. Those are later one-way transitions.
#[must_use = "the IEEE 802.15.4 foundation owner retains its partition"]
pub struct Ieee802154FoundationConfigured {
    inner: Ieee802154Lifecycle<Ieee802154Mac, lifecycle_state::FoundationConfigured>,
}

/// Terminal owner after the validation-only status experiment.
///
/// The reset-isolated discriminator remains validation-only and terminal.
/// Production acknowledgement uses a separate affine full-snapshot W1C owner;
/// this probe's raw paired-bit writes still cannot preserve a normal lifecycle
/// proof. This type exposes only evidence and deliberately provides no route
/// back to foundation, policy, or operational transitions. The
/// reset-isolation capability remains consumed for the rest of the process.
#[cfg(feature = "validation-probes")]
#[must_use = "the terminal validation owner retains the partition and isolation capability"]
pub struct Ieee802154EventStatusProbeFinished {
    evidence: Ieee802154EventStatusProbeEvidence,
    _foundation: Ieee802154FoundationConfigured,
    _isolation: Ieee802154EventStatusProbeIsolation,
}

#[cfg(feature = "validation-probes")]
impl Ieee802154EventStatusProbeFinished {
    /// Borrow the complete raw evidence retained by the terminal owner.
    pub const fn evidence(&self) -> &Ieee802154EventStatusProbeEvidence {
        &self.evidence
    }
}

/// Terminal owner after the validation-only ED event experiment.
///
/// The probe rechecks historical selective clearing with fixed ED-DONE and
/// TIMER0 validation writes. Production acknowledgement uses the generated
/// affine W1C snapshot instead. The experimental cleanup still ends the normal
/// lifecycle, so this owner cannot be promoted to an operational state.
#[cfg(feature = "validation-probes")]
#[must_use = "the terminal validation owner retains the partition and isolation capability"]
pub struct Ieee802154EdEventProbeFinished {
    evidence: Ieee802154EdEventProbeEvidence,
    _policy: Ieee802154MacPolicyConfigured,
    _isolation: Ieee802154EdEventProbeIsolation,
}

#[cfg(feature = "validation-probes")]
impl Ieee802154EdEventProbeFinished {
    /// Borrow the complete raw evidence retained by the terminal owner.
    pub const fn evidence(&self) -> &Ieee802154EdEventProbeEvidence {
        &self.evidence
    }
}

impl Ieee802154MacPolicyWrites for Ieee802154FoundationConfigured {
    fn set_channel(&mut self, channel: crate::ieee802154::Ieee802154Channel) {
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
}

impl Ieee802154MacPolicyBackend for Ieee802154FoundationConfigured {
    fn mac_policy_readback(&mut self) -> Ieee802154MacPolicyReadback {
        let mut hal = self.inner.backend_mut().mac_hal();
        let foundation = hal.foundation_snapshot();
        let policy = hal.mac_policy_snapshot();
        Ieee802154MacPolicyReadback::new(foundation, policy)
    }
}

/// Foundation owner after the known static MAC policy passed readback.
///
/// Event and abort delivery is masked between finite polled operations. This
/// state supports only serialized raw ED and CCA; it is not PHY/RF or BTBB
/// readiness, does not route an IRQ, owns no DMA buffer, and is not an
/// operational RX/TX MAC. It is not a complete vendor PIB because TX-power
/// mapping remains opaque.
#[must_use = "the IEEE 802.15.4 policy owner retains its partition"]
pub struct Ieee802154MacPolicyConfigured {
    foundation: Ieee802154FoundationConfigured,
    policy: Ieee802154MacPolicy,
}

/// Successfully recovered finite ED/CCA operation with the reusable owner
/// retained.
///
/// The evidence is MAC-level only. An ED RSS code is uncalibrated and neither
/// result proves RFPLL tuning, RF performance, PHY conformance, or a complete
/// operational IEEE 802.15.4 dataplane.
#[must_use = "a completed IEEE 802.15.4 operation retains the reusable owner"]
pub struct Ieee802154OperationCompleted {
    owner: Ieee802154MacPolicyConfigured,
    evidence: Ieee802154PolledOperationEvidence,
}

impl Ieee802154OperationCompleted {
    /// Return the finite operation evidence retained across exact recovery.
    pub const fn evidence(&self) -> &Ieee802154PolledOperationEvidence {
        &self.evidence
    }

    /// Consume the evidence wrapper and recover the same static-policy owner
    /// for a subsequent serialized operation.
    pub fn into_owner(self) -> Ieee802154MacPolicyConfigured {
        self.owner
    }
}

/// Terminal failed ED/CCA operation retaining the partition without a
/// recovery transition.
///
/// Abort and timeout can leave hardware activity unresolved. Invariant
/// failures mean the exact detached polling contract was not preserved. The
/// retained owner therefore cannot be reused through safe code.
#[must_use = "a failed IEEE 802.15.4 operation retains a terminal owner"]
pub struct Ieee802154OperationFailed {
    failure: Ieee802154PolledOperationFailure,
    _owner: Ieee802154MacPolicyConfigured,
}

impl Ieee802154OperationFailed {
    /// Return complete typed failure evidence.
    pub const fn failure(&self) -> Ieee802154PolledOperationFailure {
        self.failure
    }
}

/// Failed private-reset readback retaining the clocked owner.
pub struct Ieee802154ResetTransitionFailure {
    inner: EngineResetFailure<Ieee802154Mac>,
}

impl fmt::Debug for Ieee802154ResetTransitionFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Ieee802154ResetTransitionFailure")
            .field("error", &self.inner.error())
            .finish_non_exhaustive()
    }
}

impl Ieee802154ResetTransitionFailure {
    pub const fn error(&self) -> Ieee802154ReadbackError<Ieee802154ResetCheckpoint> {
        self.inner.error()
    }

    /// Recover the clocked owner for diagnosis or an exact reset retry.
    pub fn into_clocked(self) -> Ieee802154Clocked {
        Ieee802154Clocked {
            inner: self.inner.into_lifecycle(),
        }
    }
}

/// Failed foundation readback retaining the last proved reset owner.
pub struct Ieee802154FoundationTransitionFailure {
    inner: EngineFoundationFailure<Ieee802154Mac>,
}

impl fmt::Debug for Ieee802154FoundationTransitionFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Ieee802154FoundationTransitionFailure")
            .field("error", &self.inner.error())
            .finish_non_exhaustive()
    }
}

impl Ieee802154FoundationTransitionFailure {
    pub const fn error(&self) -> Ieee802154ReadbackError<Ieee802154FoundationCheckpoint> {
        self.inner.error()
    }

    /// Recover the reset owner for diagnosis or an exact foundation retry.
    pub fn into_reset(self) -> Ieee802154Reset {
        Ieee802154Reset {
            inner: self.inner.into_lifecycle(),
        }
    }
}

/// Failed static-policy readback retaining the exact owner until recovery
/// classifies the strongest still-proved typestate.
pub struct Ieee802154MacPolicyTransitionFailure {
    inner: EngineMacPolicyFailure<Ieee802154FoundationConfigured>,
}

/// Safe owner recovered after a static-policy readback failure.
///
/// Policy-only mismatches preserve the still-proved foundation for an exact
/// retry. A mismatch in masks, ED sampling, PTI or the receive-on delay
/// disproves that foundation and therefore returns the preceding reset state
/// instead.
pub enum Ieee802154MacPolicyRecovery {
    /// The foundation still passed and only the requested policy mismatched.
    Foundation(Ieee802154FoundationConfigured),
    /// A foundation invariant failed and must be configured and proved again.
    Reset(Ieee802154Reset),
}

impl fmt::Debug for Ieee802154MacPolicyTransitionFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Ieee802154MacPolicyTransitionFailure")
            .field("error", &self.inner.error())
            .finish_non_exhaustive()
    }
}

impl Ieee802154MacPolicyTransitionFailure {
    /// Return the first mismatched foundation or policy checkpoint.
    pub const fn error(&self) -> Ieee802154ReadbackError<Ieee802154MacPolicyCheckpoint> {
        self.inner.error()
    }

    /// Recover the strongest typestate still supported by the failed
    /// readback.
    pub fn into_recovery(self) -> Ieee802154MacPolicyRecovery {
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

impl Ieee802154Clocked {
    /// Release the IEEE 802.15.4 module clocks; clocks another client holds
    /// stay enabled.
    ///
    /// # Errors
    ///
    /// The planner rejected the release before any access, or a platform
    /// request failed and poisoned the modem clocks; the owner is returned.
    pub fn disable_clocks<T>(
        self,
        lease: &mut SharedRadioLease<'_, T>,
        platform: &mut impl PlatformClockProvider,
    ) -> Result<Ieee802154Powered, Ieee802154ClockTransitionFailure<Self>> {
        match lease.disable_modem_clocks(&self.inner.backend().task, platform) {
            Ok(()) => Ok(Ieee802154Powered {
                mac: self.inner.into_backend(),
            }),
            Err(error) => Err(Ieee802154ClockTransitionFailure { owner: self, error }),
        }
    }

    /// Prove that IEEE 802.15.4 performs no RF and leaves the shared PHY
    /// alone until `release_by_micros`, for shared PHY maintenance.
    ///
    /// A clocked owner has not reset or started the MAC, and the proof borrows
    /// it mutably, so no IEEE 802.15.4 operation can start while it lives.
    ///
    /// # Errors
    ///
    /// The window is empty.
    pub fn quiescence(
        &mut self,
        issued_at_micros: u64,
        release_by_micros: u64,
    ) -> Result<ClientQuiescence<'_>, EmptyQuiescentWindow> {
        ClientQuiescence::until(
            &mut self.inner.backend_mut().task,
            issued_at_micros,
            release_by_micros,
        )
    }

    /// Pulse the functional MAC reset and then the APB reset.
    ///
    /// # Errors
    ///
    /// A reset line did not read back released; the clocked owner is
    /// retained for a retry.
    pub fn reset_mac<T>(
        self,
        lease: &mut SharedRadioLease<'_, T>,
    ) -> Result<Ieee802154Reset, Ieee802154ResetTransitionFailure> {
        let mut port = LeasedResetPort {
            radio_phy: lease.registers_mut().radio_phy_mut(),
        };
        self.inner
            .reset_mac(&mut port)
            .map(|inner| Ieee802154Reset { inner })
            .map_err(|inner| Ieee802154ResetTransitionFailure { inner })
    }

    /// Join the shared BTBB baseband as the IEEE 802.15.4 client.
    ///
    /// This is `esp_btbb_enable`: only the first holder initializes the
    /// baseband.
    ///
    /// # Errors
    ///
    /// IEEE 802.15.4 already holds BTBB, or no PHY registration describes the
    /// shared PHY.
    ///
    /// # Safety
    ///
    /// The shared PHY registration must be complete, and `gain_parameter`
    /// must be the byte at offset `0x120` of that registration's `phy_param`
    /// state. The module clocks and resets BTBB requires are held by this
    /// clocked owner.
    #[allow(
        unsafe_code,
        reason = "the unsafe signature carries the arbiter's common-PHY prerequisite"
    )]
    pub unsafe fn acquire_btbb<T>(
        &self,
        lease: &mut SharedRadioLease<'_, T>,
        gain_parameter: u8,
    ) -> Result<BtbbAcquired, BtbbError> {
        // SAFETY: this owner holds the IEEE 802.15.4 module clocks, which
        // include the BTBB clocks; the caller upholds the registration and
        // gain provenance.
        unsafe { lease.btbb_acquire(&self.inner.backend().task, gain_parameter) }
    }

    /// Apply the IEEE 802.15.4 shared transmit-on delay override.
    ///
    /// ESP-IDF's `ieee802154_mac_init` writes it after `esp_btbb_enable`; the
    /// last write wins and no release restores it.
    ///
    /// # Errors
    ///
    /// IEEE 802.15.4 does not hold BTBB.
    pub fn override_tx_on_delay<T>(
        &self,
        lease: &mut SharedRadioLease<'_, T>,
    ) -> Result<(), BtbbError> {
        lease.override_ieee802154_tx_on_delay(&self.inner.backend().task)
    }

    /// Leave the shared BTBB baseband. This performs no register access.
    ///
    /// # Errors
    ///
    /// IEEE 802.15.4 does not hold BTBB.
    pub fn release_btbb<T>(&self, lease: &mut SharedRadioLease<'_, T>) -> Result<(), BtbbError> {
        lease.btbb_release(&self.inner.backend().task)
    }
}

impl Ieee802154Reset {
    /// Configure the interrupt-masked, non-operational MAC foundation.
    ///
    /// # Errors
    ///
    /// A foundation field did not read back; the reset owner is retained.
    pub fn configure_foundation(
        self,
    ) -> Result<Ieee802154FoundationConfigured, Ieee802154FoundationTransitionFailure> {
        self.inner
            .configure_foundation()
            .map(|inner| Ieee802154FoundationConfigured { inner })
            .map_err(|inner| Ieee802154FoundationTransitionFailure { inner })
    }

    /// Retreat to the clocked owner before releasing the module clocks.
    /// This performs no MMIO.
    pub fn into_clocked(self) -> Ieee802154Clocked {
        Ieee802154Clocked {
            inner: self.inner.into_clocked(),
        }
    }
}

impl Ieee802154FoundationConfigured {
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
    ) -> Ieee802154EventStatusProbeFinished {
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
    ///
    /// # Errors
    ///
    /// A foundation or policy field did not read back; recovery returns the
    /// strongest still-proved owner.
    pub fn configure_mac_policy(
        self,
        policy: Ieee802154MacPolicy,
    ) -> Result<Ieee802154MacPolicyConfigured, Ieee802154MacPolicyTransitionFailure> {
        configure_ieee802154_mac_policy(self, policy)
            .map(|foundation| Ieee802154MacPolicyConfigured { foundation, policy })
            .map_err(|inner| Ieee802154MacPolicyTransitionFailure { inner })
    }

    /// Hand the task and inactive interrupt owners to the operational MAC.
    ///
    /// This transition performs no MMIO. Event and abort delivery stay masked
    /// until the interrupt owner is activated. The operational MAC publishes
    /// its own PIB; both owners return through [`Self::from_operational`].
    pub fn into_operational(self) -> Ieee802154Operational {
        let Ieee802154Mac { task, interrupts } = self.inner.into_backend();
        Ieee802154Operational {
            task: Ieee802154TaskOwner::new(task),
            interrupts: Ieee802154InterruptSetupOwner::new(interrupts),
        }
    }

    /// Reunite the quiescent operational owners and prove the foundation
    /// again.
    ///
    /// The interrupt owner is inactive only after its teardown zeroed every
    /// event and abort enable and consumed the final pending image. No
    /// register is written here. The operational epoch rewrote the PIB, so
    /// only the foundation is re-proved; a policy is not.
    ///
    /// # Errors
    ///
    /// A foundation field did not read back; the reset owner is retained so
    /// the foundation can be configured again.
    pub fn from_operational(
        operational: Ieee802154Operational,
    ) -> Result<Self, Ieee802154FoundationTransitionFailure> {
        let Ieee802154Operational { task, interrupts } = operational;
        Ieee802154Lifecycle::resume(Ieee802154Mac {
            task: task.into_registers(),
            interrupts: interrupts.into_pac(),
        })
        .map(|inner| Self { inner })
        .map_err(|inner| Ieee802154FoundationTransitionFailure { inner })
    }

    /// Retreat to the clocked owner before releasing the module clocks.
    /// This performs no MMIO.
    pub fn into_clocked(self) -> Ieee802154Clocked {
        Ieee802154Clocked {
            inner: self.inner.into_clocked(),
        }
    }
}

impl Ieee802154MacPolicyConfigured {
    /// Run one finite energy-detection command on the policy's proved channel.
    ///
    /// The returned signed code is raw and uncalibrated. The transaction owns
    /// no DMA buffer, installs no CPU interrupt route, and polls only while
    /// both complete source-132 route words remain at reset. Success proves
    /// a reusable owner only when the complete snapshot actually consumed by
    /// W1C acknowledgement is exactly lone `ED_DONE`; every other acknowledged
    /// image or terminal condition is retained diagnostically and fails stop.
    ///
    /// # Errors
    ///
    /// Abort, timeout or an invariant mismatch; the owner is terminal.
    pub fn energy_detection_raw(
        self,
        duration: u16,
        budget: Ieee802154OperationPollBudget,
    ) -> Result<Ieee802154OperationCompleted, Ieee802154OperationFailed> {
        let operation =
            Ieee802154PolledOperation::energy_detection(self.policy.channel(), duration);
        self.run_polled_operation(operation, budget)
    }

    /// Run one finite CCA command using the complete proved static CCA policy.
    ///
    /// The result is the source-confirmed `CCA_BUSY` bit. It is not an RF
    /// sensitivity, timing-conformance, coexistence, IRQ, or dataplane claim.
    ///
    /// # Errors
    ///
    /// Abort, timeout or an invariant mismatch; the owner is terminal.
    pub fn clear_channel_assessment(
        self,
        budget: Ieee802154OperationPollBudget,
    ) -> Result<Ieee802154OperationCompleted, Ieee802154OperationFailed> {
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
    ) -> Result<Ieee802154OperationCompleted, Ieee802154OperationFailed> {
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
                _owner: self,
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
    ) -> Ieee802154EdEventProbeFinished {
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

    /// Keep the foundation and forget the policy. This performs no MMIO: the
    /// policy readback included the complete foundation.
    pub fn into_foundation(self) -> Ieee802154FoundationConfigured {
        self.foundation
    }
}

/// The owners of one operational IEEE 802.15.4 MAC epoch.
///
/// The task owner executes commands; the interrupt owner is inactive until
/// its platform route activates it. Neither is RF readiness.
#[must_use = "the operational owners must return through the foundation owner"]
pub struct Ieee802154Operational {
    /// Task-side MAC registers.
    pub task: Ieee802154TaskOwner,
    /// Inactive interrupt ownership for the platform CPU route.
    pub interrupts: Ieee802154InterruptSetupOwner,
}

#[cfg(any(test, feature = "validation-probes"))]
impl Ieee802154Clocked {
    /// Enter the clocked phase without the modem clock planner, for host
    /// tests of the later stages. It proves no clock.
    #[doc(hidden)]
    pub fn for_validation(partition: Ieee802154RadioPartition) -> Self {
        let Ieee802154Cold { mac } = Ieee802154Cold::from_partition(partition);
        Self {
            inner: Ieee802154Lifecycle::clocked(mac),
        }
    }
}

#[cfg(test)]
mod tests;
