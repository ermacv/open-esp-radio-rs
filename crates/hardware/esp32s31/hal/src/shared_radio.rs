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
    attachment: T,
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
        reason = "the parts are returned exactly as arbitration held them"
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
