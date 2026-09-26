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
    BluetoothTaskRegisters, Ieee802154TaskRegisters, SharedRadioRegisters, WifiRadioRegisters,
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

/// Shared registers and the PHY and power state that must stay with them.
struct SharedRadioState {
    registers: SharedRadioRegisters,
    phy: PhyRouteState,
    power: CommonRadioPower,
}

/// Unique arbiter of the shared radio partitions.
///
/// ```compile_fail
/// use oer_esp32s31_hal::shared_radio::SharedRadio;
/// fn requires_clone<T: Clone>() {}
/// requires_clone::<SharedRadio>();
/// ```
#[must_use = "dropping the shared radio arbiter permanently loses the shared registers"]
pub struct SharedRadio {
    held: AtomicBool,
    state: UnsafeCell<SharedRadioState>,
}

// SAFETY: `state` is dereferenced only through a `SharedRadioLease`, and at
// most one lease exists at a time: `try_acquire` grants one only after
// atomically changing `held` from false to true, and the lease clears it on
// drop after its last access. The shared registers are safe to move between
// execution contexts, so granting exclusive access from any context is sound.
#[allow(unsafe_code)]
unsafe impl Sync for SharedRadio where SharedRadioRegisters: Send {}

impl SharedRadio {
    /// Place the shared owner and its PHY state under arbitration. This
    /// performs no MMIO.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "the concurrent radio root split is the production constructor"
        )
    )]
    pub(crate) fn new(registers: SharedRadioRegisters, phy: PhyRouteState) -> Self {
        Self {
            held: AtomicBool::new(false),
            state: UnsafeCell::new(SharedRadioState {
                registers,
                phy,
                power: CommonRadioPower::default(),
            }),
        }
    }

    /// Take the unique lease, or report that another holder has it.
    ///
    /// # Errors
    ///
    /// [`SharedRadioBusy`] while another lease is alive or was forgotten.
    pub fn try_acquire(&self) -> Result<SharedRadioLease<'_>, SharedRadioBusy> {
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
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "the concurrent radio root reunion is the production caller"
        )
    )]
    pub(crate) fn into_parts(self) -> (SharedRadioRegisters, PhyRouteState) {
        let SharedRadioState { registers, phy, .. } = self.state.into_inner();
        (registers, phy)
    }
}

/// Another holder owns the shared radio lease.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SharedRadioBusy;

/// Unique, scoped access to the shared radio partitions.
#[must_use = "a lease blocks every other route until it is dropped"]
pub struct SharedRadioLease<'radio> {
    radio: &'radio SharedRadio,
}

impl SharedRadioLease<'_> {
    fn state(&self) -> &SharedRadioState {
        // SAFETY: this lease is the unique holder (see `SharedRadio`'s `Sync`
        // proof), so no mutable reference to the state exists elsewhere.
        #[allow(unsafe_code)]
        unsafe {
            &*self.radio.state.get()
        }
    }

    fn state_mut(&mut self) -> &mut SharedRadioState {
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

    /// Borrow the shared PHY for one PHY-layer operation.
    pub fn phy_hal(&mut self) -> SharedPhyHal<'_, route::Shared> {
        let state = self.state_mut();
        SharedPhyHal::new(state.registers.radio_phy_mut(), &mut state.phy)
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

impl Drop for SharedRadioLease<'_> {
    fn drop(&mut self) {
        self.radio.held.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests;
