//! Arbiter of the shared radio partitions for concurrently running routes.
//!
//! [`SharedRadio`] owns the shared radio registers together with the shared
//! PHY software state (registration epoch and calibration restore slots).
//! Protocol routes that run at the same time hold only their own register
//! partitions and borrow the shared owner through a [`SharedRadioLease`].
//!
//! Acquisition never blocks: [`SharedRadio::try_acquire`] either grants the
//! unique lease or reports [`SharedRadioBusy`]. A lease may be held across
//! `await` points, so a long calibration or tracking transaction keeps the
//! shared registers for its whole duration. Because acquisition cannot wait,
//! an interrupt handler that tries to acquire cannot deadlock against a task
//! holding the lease; it only observes `Busy`.
//!
//! The arbiter also owns common radio power. Each protocol enters and leaves
//! it through the lease, proving its identity with its own register set; the
//! first client runs the modem/PHY power sequence once and the last restores
//! the cold-power baseline. Refcounted modem clock dependencies are the shared
//! modem clock planner's, and are not part of this membership.
//!
//! Modem clocks are reference-counted per dependency by the shared modem clock
//! planner inside the arbiter: each client enables and disables its module's
//! clocks through the lease, and a client leaving never disables a clock
//! another client still holds. The platform clock provider supplies the
//! upstream 160 MHz source and the analog-I2C master clock.
//!
//! A lease is not a coexistence grant: it serializes register access, not
//! air time. Forgetting a lease with `mem::forget` leaks it: the arbiter
//! stays busy and no later route can reach the shared registers, which is a
//! fail-stop rather than unsound state.

use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicBool, Ordering},
};

use oer_esp32s31_pac::{
    BluetoothSchedulerStopped, BluetoothTaskRegisters, Ieee802154TaskRegisters,
    SharedRadioRegisters, WifiRadioRegisters,
};

pub use crate::clock::{CommonRadioPowerError, RadioClient};
use crate::coex::{
    CoexEventId, CoexPti, CoexPtiTable, CoexTimerBank, PHY_GRANT_PROTECT_EVENT, PhyGrantProtect,
    PhyGrantProtectError,
};
use crate::power::clock::{
    ModemClockLease, ModemClockModule, ModemClockPlanner, ModemClockPlannerIdentity,
    PoisonedModemClockAcquire, PoisonedModemClockRelease, execute_acquire, execute_release,
};
pub use crate::power::{PlatformClockError, PlatformClockProvider};
use crate::{
    clock::CommonRadioPower,
    owner::{PhyRegistrationEpoch, SharedPhyHal, route},
    phy::restore::PhyRouteState,
};

mod sealed {
    pub trait RadioClientOwner {}
}

const fn client_bit(client: RadioClient) -> u8 {
    match client {
        RadioClient::Wifi => 1,
        RadioClient::Bluetooth => 1 << 1,
        RadioClient::Ieee802154 => 1 << 2,
    }
}

/// Protocol register owner that proves which client calls the arbiter.
///
/// Only the protocol register sets implement it, so a caller cannot enter or
/// leave common power on behalf of a protocol whose registers it lacks.
pub trait RadioClientOwner: sealed::RadioClientOwner {
    const CLIENT: RadioClient;
}

impl sealed::RadioClientOwner for WifiRadioRegisters {}
impl RadioClientOwner for WifiRadioRegisters {
    const CLIENT: RadioClient = RadioClient::Wifi;
}
impl sealed::RadioClientOwner for BluetoothTaskRegisters {}
impl RadioClientOwner for BluetoothTaskRegisters {
    const CLIENT: RadioClient = RadioClient::Bluetooth;
}
impl sealed::RadioClientOwner for Ieee802154TaskRegisters {}
impl RadioClientOwner for Ieee802154TaskRegisters {
    const CLIENT: RadioClient = RadioClient::Ieee802154;
}

/// Protocol register owner that may use the shared BTBB baseband.
pub trait BtbbClientOwner: RadioClientOwner {}
impl BtbbClientOwner for BluetoothTaskRegisters {}
impl BtbbClientOwner for Ieee802154TaskRegisters {}

/// Why a client cannot take, leave or use the shared BTBB baseband.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BtbbError {
    /// The client already holds the BTBB baseband.
    AlreadyAcquired,
    /// The client does not hold the BTBB baseband.
    NotAcquired,
    /// No PHY registration describes the shared PHY, so no gain parameter can
    /// come from a registered state.
    Unregistered,
}

/// Whether a BTBB acquisition ran the shared initialization.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BtbbAcquired {
    /// The first holder ran `bt_bb_v2_init_cmplx(1)`.
    Initialized,
    /// Another client already initialized the baseband; no register access.
    Joined,
}

/// Shared registers, the PHY, power and BTBB state that must stay with them,
/// and the upper layer's attachment.
struct SharedRadioState<T> {
    registers: SharedRadioRegisters,
    phy: PhyRouteState,
    power: CommonRadioPower,
    btbb_clients: u8,
    clocks: ModemClocks,
    coex_pti: CoexPtiTable,
    phy_grant_protected: bool,
    attachment: T,
}

/// Identity of the one concurrent planner epoch; the radio is a singleton.
static CONCURRENT_CLOCK_IDENTITY: ModemClockPlannerIdentity = ModemClockPlannerIdentity::new();

/// Modem clock planner and the module lease each client holds.
#[allow(
    clippy::large_enum_variant,
    reason = "the arbiter keeps the allocation-free planner or its poisoned owner inline"
)]
enum ModemClocks {
    Ready {
        planner: ModemClockPlanner<'static>,
        leases: [Option<ModemClockLease<'static>>; CLOCK_SLOTS],
    },
    /// A transaction failed at a physical edge; the retained owner prevents
    /// any further modem clock change.
    Poisoned {
        _acquire: Option<PoisonedModemClockAcquire<'static>>,
        _release: Option<PoisonedModemClockRelease<'static, 'static>>,
    },
    /// Transient state while one transaction owns the planner.
    InFlight,
}

impl ModemClocks {
    const fn new() -> Self {
        Self::Ready {
            planner: ModemClockPlanner::for_concurrent_radio(&CONCURRENT_CLOCK_IDENTITY),
            leases: [const { None }; CLOCK_SLOTS],
        }
    }

    fn any_held(&self) -> bool {
        match self {
            Self::Ready { leases, .. } => leases.iter().any(Option::is_some),
            Self::Poisoned { .. } | Self::InFlight => true,
        }
    }
}

/// Why a module's modem clocks cannot change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModemClockError {
    /// The module already holds its modem clocks.
    AlreadyEnabled,
    /// The module does not hold its modem clocks.
    NotEnabled,
    /// The planner rejected the request before any register access.
    Rejected,
    /// A platform clock request failed at a physical edge; modem clocks are
    /// poisoned until reset.
    Poisoned,
}

/// Modem clock modules the shared PHY domain requests for itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyClockModule {
    /// `PERIPH_PHY_MODULE`: ESP-IDF's `wifi_bt_common_module_enable`, held
    /// from the first `esp_phy_enable` until the last `esp_phy_disable`
    /// closes RF.
    Phy,
    /// `PERIPH_PHY_CALIBRATION_MODULE`: ESP-IDF's `phy_module_enable`, held
    /// only while the first client's calibration or RF wake runs.
    Calibration,
}

/// One lease slot per protocol client and per PHY-domain module.
const CLOCK_SLOTS: usize = 5;

const fn client_index(client: RadioClient) -> usize {
    match client {
        RadioClient::Wifi => 0,
        RadioClient::Bluetooth => 1,
        RadioClient::Ieee802154 => 2,
    }
}

const fn client_module(client: RadioClient) -> ModemClockModule {
    match client {
        RadioClient::Wifi => ModemClockModule::Wifi,
        RadioClient::Bluetooth => ModemClockModule::Bluetooth,
        RadioClient::Ieee802154 => ModemClockModule::Ieee802154,
    }
}

const fn phy_index(module: PhyClockModule) -> usize {
    match module {
        PhyClockModule::Phy => 3,
        PhyClockModule::Calibration => 4,
    }
}

const fn phy_module(module: PhyClockModule) -> ModemClockModule {
    match module {
        PhyClockModule::Phy => ModemClockModule::Phy,
        PhyClockModule::Calibration => ModemClockModule::PhyCalibration,
    }
}

/// Unique arbiter of the shared radio partitions.
///
/// ```compile_fail
/// use oer_esp32s31_hal::shared_radio::SharedRadio;
/// fn requires_clone<T: Clone>() {}
/// requires_clone::<SharedRadio>();
/// ```
///
/// `T` is state an upper layer keeps under the same arbitration, such as the
/// PHY layer's registered domain. The HAL never interprets it; a lease lends
/// it together with the shared registers.
#[must_use = "dropping the shared radio arbiter permanently loses the shared registers"]
pub struct SharedRadio<T = ()> {
    held: AtomicBool,
    state: UnsafeCell<SharedRadioState<T>>,
}

// SAFETY: `state` is dereferenced only through a `SharedRadioLease`, and at
// most one lease exists at a time: `try_acquire` grants one only after
// atomically changing `held` from false to true, and the lease clears it on
// drop after its last access. The shared registers are safe to move between
// execution contexts, and the attachment is `Send`, so granting exclusive
// access from any context is sound.
#[allow(unsafe_code)]
unsafe impl<T: Send> Sync for SharedRadio<T> where SharedRadioRegisters: Send {}

impl<T> SharedRadio<T> {
    /// Place the shared owner, its PHY state and `attachment` under
    /// arbitration. This performs no MMIO.
    pub(crate) fn new(registers: SharedRadioRegisters, phy: PhyRouteState, attachment: T) -> Self {
        Self {
            held: AtomicBool::new(false),
            state: UnsafeCell::new(SharedRadioState {
                registers,
                phy,
                power: CommonRadioPower::default(),
                btbb_clients: 0,
                clocks: ModemClocks::new(),
                coex_pti: CoexPtiTable::VENDOR,
                phy_grant_protected: false,
                attachment,
            }),
        }
    }

    /// Take the unique lease, or report that another holder has it.
    ///
    /// # Errors
    ///
    /// [`SharedRadioBusy`] while another lease is alive or was forgotten.
    pub fn try_acquire(&self) -> Result<SharedRadioLease<'_, T>, SharedRadioBusy> {
        self.held
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map(|_| SharedRadioLease { radio: self })
            .map_err(|_| SharedRadioBusy)
    }

    /// Whether a lease is currently held.
    pub fn is_held(&self) -> bool {
        self.held.load(Ordering::Relaxed)
    }

    /// Leave arbitration. Consuming the arbiter proves no lease is alive.
    ///
    /// # Errors
    ///
    /// Returns the unchanged arbiter while a client still holds common power
    /// or a PHY calibration still owns a restore obligation.
    #[allow(
        clippy::type_complexity,
        clippy::result_large_err,
        reason = "the parts, or the unchanged arbiter, are returned exactly as held"
    )]
    pub(crate) fn into_parts(
        self,
    ) -> Result<(SharedRadioRegisters, PhyRouteState, T), (Self, SharedRadioReleaseError)> {
        let state = self.state.into_inner();
        if state.power.any_client() {
            return Err((
                Self::from_state(state),
                SharedRadioReleaseError::CommonPowerHeld,
            ));
        }
        if state.btbb_clients != 0 {
            return Err((Self::from_state(state), SharedRadioReleaseError::BtbbHeld));
        }
        if state.clocks.any_held() {
            return Err((
                Self::from_state(state),
                SharedRadioReleaseError::ModemClocksHeld,
            ));
        }
        if state.phy_grant_protected {
            return Err((
                Self::from_state(state),
                SharedRadioReleaseError::PhyGrantProtectHeld,
            ));
        }
        if let Err(error) = crate::root::check_phy_restore_complete(&state.phy) {
            return Err((
                Self::from_state(state),
                SharedRadioReleaseError::Restore(error),
            ));
        }
        Ok((state.registers, state.phy, state.attachment))
    }

    fn from_state(state: SharedRadioState<T>) -> Self {
        Self {
            held: AtomicBool::new(false),
            state: UnsafeCell::new(state),
        }
    }

    #[cfg(test)]
    pub(crate) fn phy_state_mut_for_test(&mut self) -> &mut PhyRouteState {
        &mut self.state.get_mut().phy
    }

    #[cfg(test)]
    pub(crate) fn hold_btbb_for_test(&mut self, client: RadioClient) {
        self.state.get_mut().btbb_clients |= client_bit(client);
    }

    #[cfg(test)]
    pub(crate) fn hold_common_power_for_test(&mut self, client: RadioClient) {
        self.state.get_mut().power.hold_for_test(client);
    }
}

/// How long a client performs no RF and does not touch the shared PHY.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuiescentSpan {
    /// Quiet for as long as the proof lives.
    Stopped,
    /// The shared PHY must be released by `release_by_micros`.
    ///
    /// Both instants are in the PHY monotonic clock (`PhyAsyncDelay::now_micros`,
    /// microseconds). `issued_at_micros` is the PHY-clock half of the paired
    /// sample the issuer used to translate its own schedule; the translation
    /// and its error bound are the issuer's obligation. `release_by_micros` is
    /// the last instant the shared PHY may still be held, after the issuer
    /// subtracted its own sequencing, hardware preparation and restore margins.
    Until {
        issued_at_micros: u64,
        release_by_micros: u64,
    },
}

/// A `Until` proof whose window is empty.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EmptyQuiescentWindow;

/// Proof that one client performs no RF and does not touch the shared PHY for
/// its span.
///
/// The proof borrows the owner that guarantees it for its whole lifetime, so
/// the client cannot restart or publish new radio work while it lives.
///
/// ```compile_fail
/// use oer_esp32s31_hal::shared_radio::ClientQuiescence;
/// fn publish_while_quiet<O: oer_esp32s31_hal::shared_radio::RadioClientOwner>(owner: &mut O) {
///     let proof = ClientQuiescence::until(owner, 10, 20).unwrap();
///     let _reuse = &mut *owner;
///     drop(proof);
/// }
/// ```
#[must_use = "a quiescence proof admits PHY maintenance only while it lives"]
pub struct ClientQuiescence<'client> {
    client: RadioClient,
    span: QuiescentSpan,
    _hold: core::marker::PhantomData<&'client ()>,
}

impl<'client> ClientQuiescence<'client> {
    /// Bluetooth is quiet while its scheduler stays stopped.
    ///
    /// The receipt comes from the executor's stopped state; restarting needs
    /// the executor mutably, which this borrow prevents while the proof lives.
    pub fn bluetooth_stopped(_stopped: &'client BluetoothSchedulerStopped) -> Self {
        Self {
            client: RadioClient::Bluetooth,
            span: QuiescentSpan::Stopped,
            _hold: core::marker::PhantomData,
        }
    }

    /// The client that owns `owner` stays quiet until `release_by_micros`.
    ///
    /// The issuer guarantees, from a fresh sample, that no event of its own
    /// is running now and none starts before the window closes. Every
    /// hardware action of the client goes through `owner`, which this proof
    /// borrows mutably.
    ///
    /// # Errors
    ///
    /// The window is empty (`issued_at_micros >= release_by_micros`).
    pub fn until<O: RadioClientOwner>(
        _owner: &'client mut O,
        issued_at_micros: u64,
        release_by_micros: u64,
    ) -> Result<Self, EmptyQuiescentWindow> {
        if issued_at_micros >= release_by_micros {
            return Err(EmptyQuiescentWindow);
        }
        Ok(Self {
            client: O::CLIENT,
            span: QuiescentSpan::Until {
                issued_at_micros,
                release_by_micros,
            },
            _hold: core::marker::PhantomData,
        })
    }

    /// Mint a proof for a validation or host-test image, without any owner.
    #[cfg(any(test, feature = "validation-probes"))]
    #[doc(hidden)]
    pub const fn for_validation(client: RadioClient, span: QuiescentSpan) -> Self {
        Self {
            client,
            span,
            _hold: core::marker::PhantomData,
        }
    }

    pub const fn client(&self) -> RadioClient {
        self.client
    }

    pub const fn span(&self) -> QuiescentSpan {
        self.span
    }
}

/// Why the shared radio cannot leave arbitration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SharedRadioReleaseError {
    /// A client still holds common radio power.
    CommonPowerHeld,
    /// A client still holds the shared BTBB baseband.
    BtbbHeld,
    /// A client still holds modem clocks, or they are poisoned.
    ModemClocksHeld,
    /// The PHY grant-protect request is still programmed.
    PhyGrantProtectHeld,
    /// A PHY calibration still owns a restore obligation.
    Restore(crate::root::RadioPhyReleaseError),
}

/// Another holder owns the shared radio lease.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SharedRadioBusy;

/// Unique, scoped access to the shared radio partitions.
#[must_use = "a lease blocks every other route until it is dropped"]
pub struct SharedRadioLease<'radio, T = ()> {
    radio: &'radio SharedRadio<T>,
}

impl<T> SharedRadioLease<'_, T> {
    fn state(&self) -> &SharedRadioState<T> {
        // SAFETY: this lease is the unique holder (see `SharedRadio`'s `Sync`
        // proof), so no mutable reference to the state exists elsewhere.
        #[allow(unsafe_code)]
        unsafe {
            &*self.radio.state.get()
        }
    }

    fn state_mut(&mut self) -> &mut SharedRadioState<T> {
        // SAFETY: this lease is the unique holder and `&mut self` prevents a
        // second borrow through it, so the reference is exclusive.
        #[allow(unsafe_code)]
        unsafe {
            &mut *self.radio.state.get()
        }
    }

    /// The registration that currently describes the shared PHY.
    pub fn registration_epoch(&self) -> Option<PhyRegistrationEpoch> {
        self.state().phy.registration_epoch()
    }

    /// Whether `client` currently holds common radio power.
    pub fn holds_common_power(&self, client: RadioClient) -> bool {
        self.state().power.holds(client)
    }

    /// Enter common radio power as the protocol that owns `owner`.
    ///
    /// Only the first client runs the modem/PHY power sequence; it pulses the
    /// Wi-Fi baseband and MAC resets, so it never runs while another client
    /// holds power.
    ///
    /// # Errors
    ///
    /// The client already holds power, or the first client's sequence failed
    /// a read-back checkpoint and admitted no client.
    pub fn enter_common_power<O: RadioClientOwner>(
        &mut self,
        _owner: &O,
    ) -> Result<(), CommonRadioPowerError> {
        let state = self.state_mut();
        state
            .power
            .enter(state.registers.radio_phy_mut(), O::CLIENT)
    }

    /// Leave common radio power as the protocol that owns `owner`.
    ///
    /// The last client releases the PHY-I2C gate and restores the cold-power
    /// baseline captured before the first power edge.
    ///
    /// # Errors
    ///
    /// The client does not hold power, or the baseline did not read back; the
    /// client then stays entered so the exit can be retried.
    pub fn exit_common_power<O: RadioClientOwner>(
        &mut self,
        _owner: &O,
    ) -> Result<(), CommonRadioPowerError> {
        let state = self.state_mut();
        state.power.exit(state.registers.radio_phy_mut(), O::CLIENT)
    }

    /// Whether `client` currently holds the shared BTBB baseband.
    pub fn holds_btbb(&self, client: RadioClient) -> bool {
        self.state().btbb_clients & client_bit(client) != 0
    }

    /// Take the shared BTBB baseband as the protocol that owns `owner`.
    ///
    /// This is ESP-IDF's reference-counted `esp_btbb_enable`: only the first
    /// holder runs the `bt_bb_v2_init_cmplx(1)` body, which also writes the
    /// Bluetooth value of the shared TX-on delay; later holders join without
    /// register access.
    ///
    /// # Errors
    ///
    /// The client already holds BTBB, or no registration describes the
    /// shared PHY. Both are rejected before any register access.
    ///
    /// # Safety
    ///
    /// The Bluetooth/BTBB clocks and resets must be active, the shared PHY
    /// registration must be complete, and `gain_parameter` must be the byte at
    /// offset `0x120` of that registration's `phy_param` state.
    #[allow(
        unsafe_code,
        reason = "the unsafe signature carries the common-PHY and clock prerequisites"
    )]
    pub unsafe fn btbb_acquire<O: BtbbClientOwner>(
        &mut self,
        _owner: &O,
        gain_parameter: u8,
    ) -> Result<BtbbAcquired, BtbbError> {
        let bit = client_bit(O::CLIENT);
        let state = self.state_mut();
        if state.btbb_clients & bit != 0 {
            return Err(BtbbError::AlreadyAcquired);
        }
        if state.phy.registration_epoch().is_none() {
            return Err(BtbbError::Unregistered);
        }
        let acquired = if state.btbb_clients == 0 {
            state.registers.initialize_btbb_v2_arg_one(gain_parameter);
            BtbbAcquired::Initialized
        } else {
            BtbbAcquired::Joined
        };
        state.btbb_clients |= bit;
        Ok(acquired)
    }

    /// Leave the shared BTBB baseband. Like ESP-IDF's `esp_btbb_disable`,
    /// this only drops the reference and performs no register access.
    ///
    /// # Errors
    ///
    /// The client does not hold BTBB.
    pub fn btbb_release<O: BtbbClientOwner>(&mut self, _owner: &O) -> Result<(), BtbbError> {
        let bit = client_bit(O::CLIENT);
        let state = self.state_mut();
        if state.btbb_clients & bit == 0 {
            return Err(BtbbError::NotAcquired);
        }
        state.btbb_clients &= !bit;
        Ok(())
    }

    /// Apply the IEEE 802.15.4 value of the shared TX-on delay.
    ///
    /// ESP-IDF writes it at IEEE 802.15.4 MAC initialization, after
    /// `esp_btbb_enable`; the last write wins and no release restores the
    /// Bluetooth value. The device fence follows with the caller's MAC timing.
    ///
    /// # Errors
    ///
    /// IEEE 802.15.4 does not hold BTBB, so the BTBB initialization that this
    /// override must follow may not have run.
    pub fn override_ieee802154_tx_on_delay(
        &mut self,
        _owner: &Ieee802154TaskRegisters,
    ) -> Result<(), BtbbError> {
        let state = self.state_mut();
        if state.btbb_clients & client_bit(RadioClient::Ieee802154) == 0 {
            return Err(BtbbError::NotAcquired);
        }
        state.registers.override_ieee802154_shared_tx_on_delay();
        Ok(())
    }

    /// Borrow the shared PHY for one PHY-layer operation.
    pub fn phy_hal(&mut self) -> SharedPhyHal<'_, route::Shared> {
        let state = self.state_mut();
        SharedPhyHal::new(state.registers.radio_phy_mut(), &mut state.phy)
    }

    /// Enable the modem clocks of the protocol that owns `owner`.
    ///
    /// This is ESP-IDF's reference-counted `modem_clock_module_enable` for the
    /// client's module: the monotonic modem ICG maps are ORed in, then only
    /// dependencies at zero references are enabled. The upstream 160 MHz
    /// source and the analog-I2C master clock go through `platform`.
    ///
    /// # Errors
    ///
    /// The client already holds its clocks, the planner rejected the request
    /// before any access, or a platform request failed and poisoned the modem
    /// clocks.
    pub fn enable_modem_clocks<O: RadioClientOwner>(
        &mut self,
        _owner: &O,
        platform: &mut impl PlatformClockProvider,
    ) -> Result<(), ModemClockError> {
        self.enable_slot(client_index(O::CLIENT), client_module(O::CLIENT), platform)
    }

    /// Disable the modem clocks of the protocol that owns `owner`; only
    /// dependencies no other module holds are disabled.
    ///
    /// # Errors
    ///
    /// The client does not hold its clocks, the planner rejected the release
    /// before any access, or a platform request failed and poisoned the modem
    /// clocks.
    pub fn disable_modem_clocks<O: RadioClientOwner>(
        &mut self,
        _owner: &O,
        platform: &mut impl PlatformClockProvider,
    ) -> Result<(), ModemClockError> {
        self.disable_slot(client_index(O::CLIENT), platform)
    }

    /// Enable a modem clock module of the shared PHY domain, with the same
    /// reference-counted transaction as a client's module.
    ///
    /// # Errors
    ///
    /// As [`Self::enable_modem_clocks`], for the PHY module's own slot.
    pub fn enable_phy_modem_clocks(
        &mut self,
        module: PhyClockModule,
        platform: &mut impl PlatformClockProvider,
    ) -> Result<(), ModemClockError> {
        self.enable_slot(phy_index(module), phy_module(module), platform)
    }

    /// Disable a modem clock module of the shared PHY domain.
    ///
    /// # Errors
    ///
    /// As [`Self::disable_modem_clocks`], for the PHY module's own slot.
    pub fn disable_phy_modem_clocks(
        &mut self,
        module: PhyClockModule,
        platform: &mut impl PlatformClockProvider,
    ) -> Result<(), ModemClockError> {
        self.disable_slot(phy_index(module), platform)
    }

    fn enable_slot(
        &mut self,
        index: usize,
        module: ModemClockModule,
        platform: &mut impl PlatformClockProvider,
    ) -> Result<(), ModemClockError> {
        let state = self.state_mut();
        let (planner, mut leases) =
            match core::mem::replace(&mut state.clocks, ModemClocks::InFlight) {
                ModemClocks::Ready { planner, leases } => (planner, leases),
                other => {
                    state.clocks = other;
                    return Err(ModemClockError::Poisoned);
                }
            };
        if leases[index].is_some() {
            state.clocks = ModemClocks::Ready { planner, leases };
            return Err(ModemClockError::AlreadyEnabled);
        }
        let prepared = match planner.prepare_module_acquire(module) {
            Ok(prepared) => prepared,
            Err(failure) => {
                state.clocks = ModemClocks::Ready {
                    planner: failure.into_planner(),
                    leases,
                };
                return Err(ModemClockError::Rejected);
            }
        };
        let phy = state.registers.radio_phy_mut();
        phy.prepare_modem_syscon_clock_map();
        phy.prepare_shared_modem_clock_map();
        match execute_acquire(prepared, phy, platform) {
            Ok((planner, lease)) => {
                leases[index] = Some(lease);
                state.clocks = ModemClocks::Ready { planner, leases };
                Ok(())
            }
            Err(poisoned) => {
                // Remaining leases retire with the poisoned planner epoch.
                state.clocks = ModemClocks::Poisoned {
                    _acquire: Some(poisoned),
                    _release: None,
                };
                Err(ModemClockError::Poisoned)
            }
        }
    }

    fn disable_slot(
        &mut self,
        index: usize,
        platform: &mut impl PlatformClockProvider,
    ) -> Result<(), ModemClockError> {
        let state = self.state_mut();
        let (planner, mut leases) =
            match core::mem::replace(&mut state.clocks, ModemClocks::InFlight) {
                ModemClocks::Ready { planner, leases } => (planner, leases),
                other => {
                    state.clocks = other;
                    return Err(ModemClockError::Poisoned);
                }
            };
        let Some(lease) = leases[index].take() else {
            state.clocks = ModemClocks::Ready { planner, leases };
            return Err(ModemClockError::NotEnabled);
        };
        let prepared = match planner.prepare_release(lease) {
            Ok(prepared) => prepared,
            Err(failure) => {
                let (planner, lease) = failure.into_owners();
                leases[index] = Some(lease);
                state.clocks = ModemClocks::Ready { planner, leases };
                return Err(ModemClockError::Rejected);
            }
        };
        match execute_release(prepared, state.registers.radio_phy_mut(), platform) {
            Ok(planner) => {
                state.clocks = ModemClocks::Ready { planner, leases };
                Ok(())
            }
            Err(poisoned) => {
                state.clocks = ModemClocks::Poisoned {
                    _acquire: None,
                    _release: Some(poisoned),
                };
                Err(ModemClockError::Poisoned)
            }
        }
    }

    /// The current coexistence priority of `event`.
    pub fn coex_pti(&self, event: CoexEventId) -> CoexPti {
        self.state().coex_pti.pti(event)
    }

    /// The complete coexistence priority table.
    pub fn coex_pti_table(&self) -> CoexPtiTable {
        self.state().coex_pti
    }

    /// Replace the coexistence priority of `event`, as `coex_pti_set` does.
    ///
    /// Only the table changes; a protocol republishes its MAC PTI registers
    /// from the new value.
    pub fn set_coex_pti(&mut self, event: CoexEventId, pti: CoexPti) {
        self.state_mut().coex_pti.set(event, pti);
    }

    /// Borrow the PHY grant-protect request for one maintenance operation.
    pub fn phy_grant_protect(&mut self) -> PhyGrantProtect<'_> {
        let state = self.state_mut();
        let pti = state.coex_pti.pti(PHY_GRANT_PROTECT_EVENT);
        PhyGrantProtect::new(
            state.registers.coex_timers_mut(),
            pti,
            &mut state.phy_grant_protected,
        )
    }

    /// Program the PHY grant-protect request ([`PhyGrantProtect::acquire`]).
    ///
    /// # Errors
    ///
    /// The request is already programmed; nothing is written.
    pub fn acquire_phy_grant_protect(&mut self) -> Result<(), PhyGrantProtectError> {
        self.phy_grant_protect().acquire()
    }

    /// Withdraw the PHY grant-protect request ([`PhyGrantProtect::release`]).
    ///
    /// # Errors
    ///
    /// No request is programmed; nothing is written.
    pub fn release_phy_grant_protect(&mut self) -> Result<(), PhyGrantProtectError> {
        self.phy_grant_protect().release()
    }

    /// Whether the PHY grant-protect request is programmed.
    pub fn phy_grant_protected(&self) -> bool {
        self.state().phy_grant_protected
    }

    /// Borrow the coexistence timer bank for one policy transaction.
    pub fn coex_timer_bank(&mut self) -> CoexTimerBank<'_> {
        CoexTimerBank::from_owned(&mut self.state_mut().registers)
    }

    /// Borrow the upper layer's attachment.
    pub fn attachment(&self) -> &T {
        &self.state().attachment
    }

    /// Mutably borrow the upper layer's attachment.
    pub fn attachment_mut(&mut self) -> &mut T {
        &mut self.state_mut().attachment
    }

    /// Borrow the shared PHY together with the attachment, for an upper-layer
    /// operation that updates its state while driving the PHY.
    pub fn phy_hal_with_attachment(&mut self) -> (SharedPhyHal<'_, route::Shared>, &mut T) {
        let state = self.state_mut();
        (
            SharedPhyHal::new(state.registers.radio_phy_mut(), &mut state.phy),
            &mut state.attachment,
        )
    }

    /// Borrow the shared PHY, the attachment and the PHY grant-protect
    /// request together, for PHY maintenance that brackets its hardware
    /// regions with the request.
    pub fn phy_hal_with_attachment_and_grant(
        &mut self,
    ) -> (SharedPhyHal<'_, route::Shared>, &mut T, PhyGrantProtect<'_>) {
        let state = self.state_mut();
        let pti = state.coex_pti.pti(PHY_GRANT_PROTECT_EVENT);
        let (phy, timers) = state.registers.radio_phy_and_coex_timers_mut();
        (
            SharedPhyHal::new(phy, &mut state.phy),
            &mut state.attachment,
            PhyGrantProtect::new(timers, pti, &mut state.phy_grant_protected),
        )
    }

    /// Borrow the shared registers for a protocol transaction that also
    /// touches protocol registers.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "concurrent protocol routes are the production callers"
        )
    )]
    pub(crate) fn registers_mut(&mut self) -> &mut SharedRadioRegisters {
        &mut self.state_mut().registers
    }
}

impl<T> Drop for SharedRadioLease<'_, T> {
    fn drop(&mut self) {
        self.radio.held.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests;
