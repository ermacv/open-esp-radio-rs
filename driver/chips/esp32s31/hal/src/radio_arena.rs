//! Reclaimable stable placement for the running Wi-Fi register owner.
//!
//! The arena is the explicit serialization owner for its shared handles.
//! `RefCell` dynamic borrows serialize synchronous MMIO transactions, make the
//! arena non-`Sync`, and prevent handles from authorizing cross-thread MMIO.
//! Consumers can obtain only narrow HAL operations; no generic register
//! callback or PAC owner escapes this module.

use core::{
    cell::{RefCell, RefMut},
    sync::atomic::{AtomicU8, Ordering},
};

use open_esp_radio_esp32s31_pac::WifiRadioRegisters;

use crate::RadioRuntimeOwner;

const EMPTY: u8 = 0;
const PUBLISHED: u8 = 1;
const RESET_REQUIRED: u8 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31RadioOwnerArenaState {
    Empty,
    Published,
    ResetRequired,
}

impl Esp32s31RadioOwnerArenaState {
    const fn decode(value: u8) -> Self {
        match value {
            EMPTY => Self::Empty,
            PUBLISHED => Self::Published,
            _ => Self::ResetRequired,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31RadioOwnerArenaError {
    AlreadyPublished,
    ResetRequired,
    Borrowed,
    MissingOwner,
}

/// Failed publication retaining the exact register owner.
pub struct Esp32s31RadioOwnerPublishFailure {
    pub error: Esp32s31RadioOwnerArenaError,
    pub owner: RadioRuntimeOwner,
}

/// Exact empty-arena capability returned beside a reclaimed runtime owner.
///
/// Keeping this non-cloneable value in role-local stopped resources preserves
/// which stable arena may host the next task epoch. Higher layers therefore do
/// not need to recover an initialized arena from a global `StaticCell`.
pub struct Esp32s31RadioOwnerRepublish<'arena> {
    arena: &'arena Esp32s31RadioOwnerArena,
}

impl<'arena> Esp32s31RadioOwnerRepublish<'arena> {
    /// Publish a returned runtime owner into the exact arena reclaimed with it.
    pub fn try_publish(
        self,
        owner: RadioRuntimeOwner,
    ) -> Result<Esp32s31PublishedRadioOwner<'arena>, Esp32s31RadioOwnerRepublishFailure<'arena>>
    {
        match self.arena.publish(owner) {
            Ok(published) => Ok(published),
            Err(failure) => Err(Esp32s31RadioOwnerRepublishFailure {
                error: failure.error,
                owner: failure.owner,
                republish: self,
            }),
        }
    }
}

/// Failed exact-arena republication retaining both movable capabilities.
pub struct Esp32s31RadioOwnerRepublishFailure<'arena> {
    pub error: Esp32s31RadioOwnerArenaError,
    pub owner: RadioRuntimeOwner,
    pub republish: Esp32s31RadioOwnerRepublish<'arena>,
}

/// Runtime owner and the exact empty-arena capability reclaimed from one epoch.
pub struct Esp32s31ReclaimedRadioOwner<'arena> {
    owner: RadioRuntimeOwner,
    republish: Esp32s31RadioOwnerRepublish<'arena>,
}

impl<'arena> Esp32s31ReclaimedRadioOwner<'arena> {
    pub fn into_parts(self) -> (RadioRuntimeOwner, Esp32s31RadioOwnerRepublish<'arena>) {
        (self.owner, self.republish)
    }

    /// Discard the empty-arena binding when the caller intentionally does not
    /// need another task-stable publication.
    pub fn into_owner(self) -> RadioRuntimeOwner {
        self.owner
    }

    pub fn try_republish(
        self,
    ) -> Result<Esp32s31PublishedRadioOwner<'arena>, Esp32s31RadioOwnerRepublishFailure<'arena>>
    {
        self.republish.try_publish(self.owner)
    }
}

/// Stable storage used while executor tasks require a `'static` register
/// address.
///
/// The arena is role-neutral. Publishing transfers the unique
/// [`RadioRuntimeOwner`] value into it and returns the only movable lease.
/// Consuming that lease after every task has stopped returns the original
/// value. Dropping a live lease poisons the arena instead of making the
/// hardware owner silently reusable.
pub struct Esp32s31RadioOwnerArena {
    registers: RefCell<Option<RadioRuntimeOwner>>,
    state: AtomicU8,
}

impl Esp32s31RadioOwnerArena {
    pub const fn new() -> Self {
        Self {
            registers: RefCell::new(None),
            state: AtomicU8::new(EMPTY),
        }
    }

    pub fn state(&self) -> Esp32s31RadioOwnerArenaState {
        Esp32s31RadioOwnerArenaState::decode(self.state.load(Ordering::Acquire))
    }

    /// Borrow the published owner only through the narrow channel capability.
    /// The dynamic borrow is the serialization guard and remains held across
    /// the complete asynchronous channel transaction.
    pub fn try_channel_hal<'arena, P>(
        &'arena self,
        platform: &'arena mut P,
    ) -> Result<crate::channel::RadioChannelHal<'arena, P>, Esp32s31RadioOwnerArenaError> {
        match self.state() {
            Esp32s31RadioOwnerArenaState::Empty => {
                return Err(Esp32s31RadioOwnerArenaError::MissingOwner);
            }
            Esp32s31RadioOwnerArenaState::ResetRequired => {
                return Err(Esp32s31RadioOwnerArenaError::ResetRequired);
            }
            Esp32s31RadioOwnerArenaState::Published => {}
        }
        let slot = self
            .registers
            .try_borrow_mut()
            .map_err(|_| Esp32s31RadioOwnerArenaError::Borrowed)?;
        let owner = RefMut::filter_map(slot, Option::as_mut)
            .map_err(|_| Esp32s31RadioOwnerArenaError::MissingOwner)?;
        let registers = RefMut::map(owner, RadioRuntimeOwner::pac_mut);
        Ok(crate::channel::RadioChannelHal::from_published(
            platform, registers,
        ))
    }

    /// Borrow the published owner only through the closed Wi-Fi MAC
    /// capability. The returned guard is the complete synchronous
    /// serialization interval; it exposes no PAC owner or generic callback.
    pub fn try_wifi_mac_hal(
        &self,
    ) -> Result<crate::wifi_mac::WifiMacHal<'_>, Esp32s31RadioOwnerArenaError> {
        match self.state() {
            Esp32s31RadioOwnerArenaState::Empty => {
                return Err(Esp32s31RadioOwnerArenaError::MissingOwner);
            }
            Esp32s31RadioOwnerArenaState::ResetRequired => {
                return Err(Esp32s31RadioOwnerArenaError::ResetRequired);
            }
            Esp32s31RadioOwnerArenaState::Published => {}
        }
        let slot = self
            .registers
            .try_borrow_mut()
            .map_err(|_| Esp32s31RadioOwnerArenaError::Borrowed)?;
        let owner = RefMut::filter_map(slot, Option::as_mut)
            .map_err(|_| Esp32s31RadioOwnerArenaError::MissingOwner)?;
        let registers = RefMut::map(owner, RadioRuntimeOwner::pac_mut);
        Ok(crate::wifi_mac::WifiMacHal::from_published(registers))
    }

    /// Run one fallible, bounded observation without creating a copyable raw
    /// register capability.
    ///
    /// This is intended for value-only diagnostics at the integration
    /// boundary. It reports an inactive or synchronously borrowed arena
    /// instead of panicking, and the closure cannot retain the borrow across
    /// an async suspension.
    fn try_with_ref<T>(
        &self,
        transaction: impl FnOnce(&WifiRadioRegisters) -> T,
    ) -> Result<T, Esp32s31RadioOwnerArenaError> {
        match self.state() {
            Esp32s31RadioOwnerArenaState::Empty => {
                return Err(Esp32s31RadioOwnerArenaError::MissingOwner);
            }
            Esp32s31RadioOwnerArenaState::ResetRequired => {
                return Err(Esp32s31RadioOwnerArenaError::ResetRequired);
            }
            Esp32s31RadioOwnerArenaState::Published => {}
        }
        let slot = self
            .registers
            .try_borrow()
            .map_err(|_| Esp32s31RadioOwnerArenaError::Borrowed)?;
        let owner = slot
            .as_ref()
            .ok_or(Esp32s31RadioOwnerArenaError::MissingOwner)?;
        Ok(transaction(owner.pac()))
    }

    /// Run one synchronous mutation only while the published lifecycle is
    /// live. Keeping this helper private prevents callers from turning the
    /// arena back into an unrestricted PAC callback API.
    fn try_with_mut<T>(
        &self,
        transaction: impl FnOnce(&mut WifiRadioRegisters) -> T,
    ) -> Result<T, Esp32s31RadioOwnerArenaError> {
        match self.state() {
            Esp32s31RadioOwnerArenaState::Empty => {
                return Err(Esp32s31RadioOwnerArenaError::MissingOwner);
            }
            Esp32s31RadioOwnerArenaState::ResetRequired => {
                return Err(Esp32s31RadioOwnerArenaError::ResetRequired);
            }
            Esp32s31RadioOwnerArenaState::Published => {}
        }
        let mut slot = self
            .registers
            .try_borrow_mut()
            .map_err(|_| Esp32s31RadioOwnerArenaError::Borrowed)?;
        let owner = slot
            .as_mut()
            .ok_or(Esp32s31RadioOwnerArenaError::MissingOwner)?;
        Ok(transaction(owner.pac_mut()))
    }

    /// Read the reviewed station receive-policy projection without exposing a
    /// PAC owner or a generic closure at the integration boundary.
    pub fn try_station_receive_policy_snapshot(
        &self,
    ) -> Result<crate::wifi_mac::MacStaReceivePolicySnapshot, Esp32s31RadioOwnerArenaError> {
        self.try_with_ref(|registers| registers.sta_receive_policy_snapshot())
    }

    /// Read the reviewed MAC receive-statistics projection without exposing a
    /// PAC owner or a generic closure at the integration boundary.
    pub fn try_receive_statistics_snapshot(
        &self,
    ) -> Result<crate::wifi_mac::MacRxStatisticsSnapshot, Esp32s31RadioOwnerArenaError> {
        self.try_with_ref(|registers| registers.rx_statistics_snapshot())
    }

    /// Read the reviewed MAC RX walker projection without exposing a PAC
    /// owner or retaining the serialization guard at the caller.
    pub fn try_receive_dma_snapshot(
        &self,
    ) -> Result<crate::wifi_mac::MacRxDmaSnapshot, Esp32s31RadioOwnerArenaError> {
        self.try_with_ref(|registers| registers.mac_rx_dma_snapshot())
    }

    /// Apply the reviewed station-link receive policy as one serialized HAL
    /// transaction.
    pub fn try_configure_station_receive_policy(
        &self,
        bssid: [u8; 6],
    ) -> Result<(), Esp32s31RadioOwnerArenaError> {
        self.try_with_mut(|registers| {
            crate::wifi_mac::WifiMacHal::from_owned(registers)
                .configure_station_receive_policy(bssid);
        })
    }

    /// Apply normal STA filtering/auto-ACK restoration followed by the exact
    /// management-without-BSSID-check policy used by ESP-NOW normal RX.
    pub fn try_configure_station_esp_now_receive_policy(
        &self,
        bssid: [u8; 6],
    ) -> Result<(), Esp32s31RadioOwnerArenaError> {
        self.try_with_mut(|registers| {
            let mut hal = crate::wifi_mac::WifiMacHal::from_owned(registers);
            hal.configure_station_receive_policy(bssid);
            hal.configure_station_policy_six(bssid, crate::types::MacStaPolicyMode::Mode2);
        })
    }

    /// Read the current hardware noise floor as a bounded value observation.
    pub fn try_noise_floor_dbm(&self) -> Result<i8, Esp32s31RadioOwnerArenaError> {
        self.try_with_ref(|registers| registers.radio_phy().read_noise_floor_dbm())
    }

    /// Install one semantic station CCMP key under the runtime owner.
    pub fn try_install_station_ccmp_entry(
        &self,
        index: u8,
        identity: crate::types::MacCcmpKeyIdentity,
        temporal_key: &[u8; 16],
    ) -> Result<crate::wifi_mac::MacKeyInstallOutcome, Esp32s31RadioOwnerArenaError> {
        self.try_with_mut(|registers| {
            crate::wifi_mac::WifiMacHal::from_owned(registers).install_station_ccmp_entry(
                index,
                identity,
                temporal_key,
            )
        })
    }

    /// Install one semantic access-point CCMP key under runtime serialization
    /// owner. This is deliberately distinct from the station transaction:
    /// the vendor leaf enables the crypto engine for the selected MAC
    /// interface, so substituting interface zero corrupts simultaneous
    /// STA+AP transmit encryption even when the key-table entry itself is
    /// otherwise valid.
    pub fn try_install_access_point_ccmp_entry(
        &self,
        index: u8,
        identity: crate::types::MacCcmpKeyIdentity,
        temporal_key: &[u8; 16],
    ) -> Result<crate::wifi_mac::MacKeyInstallOutcome, Esp32s31RadioOwnerArenaError> {
        self.try_with_mut(|registers| {
            crate::wifi_mac::WifiMacHal::from_owned(registers).install_access_point_ccmp_entry(
                index,
                identity,
                temporal_key,
            )
        })
    }

    /// Clear one CCMP table entry under the runtime serialization owner.
    pub fn try_clear_ccmp_entry(&self, index: u8) -> Result<(), Esp32s31RadioOwnerArenaError> {
        self.try_with_mut(|registers| {
            crate::wifi_mac::WifiMacHal::from_owned(registers).clear_ccmp_entry(index);
        })
    }

    /// Observe one CCMP validity bit without exposing register authority.
    pub fn try_ccmp_entry_is_valid(
        &self,
        index: u8,
    ) -> Result<Option<bool>, Esp32s31RadioOwnerArenaError> {
        self.try_with_mut(|registers| {
            crate::wifi_mac::WifiMacHal::from_owned(registers).ccmp_entry_is_valid(index)
        })
    }

    /// Move one register owner into stable storage for a finite task epoch.
    pub fn publish(
        &self,
        owner: RadioRuntimeOwner,
    ) -> Result<Esp32s31PublishedRadioOwner<'_>, Esp32s31RadioOwnerPublishFailure> {
        let state = self.state();
        if state != Esp32s31RadioOwnerArenaState::Empty {
            return Err(Esp32s31RadioOwnerPublishFailure {
                error: match state {
                    Esp32s31RadioOwnerArenaState::Published => {
                        Esp32s31RadioOwnerArenaError::AlreadyPublished
                    }
                    Esp32s31RadioOwnerArenaState::ResetRequired => {
                        Esp32s31RadioOwnerArenaError::ResetRequired
                    }
                    Esp32s31RadioOwnerArenaState::Empty => unreachable!(),
                },
                owner,
            });
        }
        let mut slot = match self.registers.try_borrow_mut() {
            Ok(slot) => slot,
            Err(_) => {
                return Err(Esp32s31RadioOwnerPublishFailure {
                    error: Esp32s31RadioOwnerArenaError::Borrowed,
                    owner,
                });
            }
        };
        if slot.is_some() {
            self.state.store(RESET_REQUIRED, Ordering::Release);
            return Err(Esp32s31RadioOwnerPublishFailure {
                error: Esp32s31RadioOwnerArenaError::ResetRequired,
                owner,
            });
        }
        *slot = Some(owner);
        self.state.store(PUBLISHED, Ordering::Release);
        drop(slot);
        Ok(Esp32s31PublishedRadioOwner {
            arena: self,
            reclaim_required: true,
        })
    }
}

impl Default for Esp32s31RadioOwnerArena {
    fn default() -> Self {
        Self::new()
    }
}

/// Unique movable lease for one published register owner.
pub struct Esp32s31PublishedRadioOwner<'arena> {
    arena: &'arena Esp32s31RadioOwnerArena,
    reclaim_required: bool,
}

impl<'arena> Esp32s31PublishedRadioOwner<'arena> {
    /// Copyable bounded-transaction handle for child actors in the same
    /// finite role epoch. The root lease must not be reclaimed until every
    /// actor using this handle has acknowledged shutdown.
    pub const fn access(&self) -> Esp32s31RadioAccess<'arena> {
        Esp32s31RadioAccess { arena: self.arena }
    }

    /// Return the exact PAC owner only while no synchronous register
    /// transaction is borrowed.
    pub fn try_reclaim(self) -> Result<RadioRuntimeOwner, (Self, Esp32s31RadioOwnerArenaError)> {
        self.try_reclaim_with_republish()
            .map(Esp32s31ReclaimedRadioOwner::into_owner)
    }

    /// Return the PAC owner together with the exact empty-arena capability.
    ///
    /// This is the owner-preserving boundary for a later role/task epoch. The
    /// ordinary [`try_reclaim`](Self::try_reclaim) remains useful when the
    /// caller deliberately tears down stable publication permanently.
    pub fn try_reclaim_with_republish(
        mut self,
    ) -> Result<Esp32s31ReclaimedRadioOwner<'arena>, (Self, Esp32s31RadioOwnerArenaError)> {
        let mut slot = match self.arena.registers.try_borrow_mut() {
            Ok(slot) => slot,
            Err(_) => return Err((self, Esp32s31RadioOwnerArenaError::Borrowed)),
        };
        let Some(owner) = slot.take() else {
            self.arena.state.store(RESET_REQUIRED, Ordering::Release);
            return Err((self, Esp32s31RadioOwnerArenaError::MissingOwner));
        };
        self.arena.state.store(EMPTY, Ordering::Release);
        self.reclaim_required = false;
        drop(slot);
        Ok(Esp32s31ReclaimedRadioOwner {
            owner,
            republish: Esp32s31RadioOwnerRepublish { arena: self.arena },
        })
    }
}

/// Non-owning transaction handle derived from one published lease.
#[derive(Clone, Copy)]
pub struct Esp32s31RadioAccess<'arena> {
    arena: &'arena Esp32s31RadioOwnerArena,
}

impl<'arena> Esp32s31RadioAccess<'arena> {
    pub fn try_channel_hal<'access, P>(
        &'access self,
        platform: &'access mut P,
    ) -> Result<crate::channel::RadioChannelHal<'access, P>, Esp32s31RadioOwnerArenaError> {
        self.arena.try_channel_hal(platform)
    }

    /// Start one serialized Wi-Fi MAC transaction without exposing the
    /// published PAC owner.
    pub fn try_wifi_mac_hal(
        &self,
    ) -> Result<crate::wifi_mac::WifiMacHal<'arena>, Esp32s31RadioOwnerArenaError> {
        self.arena.try_wifi_mac_hal()
    }

    /// Prepare the finite connected-STA interrupt state while the runtime
    /// register owner remains serialized inside the arena.
    pub fn try_prepare_connected_sta_without_power_save(
        &self,
        setup: &mut crate::MacInterruptSetup,
    ) -> Result<crate::ConnectedStaInterruptPrepared, Esp32s31RadioOwnerArenaError> {
        self.arena
            .try_with_mut(|registers| setup.prepare_connected_sta_with_pac(registers))
    }

    /// Read the associated-STA policy as a value-only HAL observation.
    pub fn try_station_receive_policy_snapshot(
        &self,
    ) -> Result<crate::wifi_mac::MacStaReceivePolicySnapshot, Esp32s31RadioOwnerArenaError> {
        self.arena.try_station_receive_policy_snapshot()
    }

    /// Read receive counters as a value-only HAL observation.
    pub fn try_receive_statistics_snapshot(
        &self,
    ) -> Result<crate::wifi_mac::MacRxStatisticsSnapshot, Esp32s31RadioOwnerArenaError> {
        self.arena.try_receive_statistics_snapshot()
    }

    /// Read the RX DMA walker as a value-only HAL observation.
    pub fn try_receive_dma_snapshot(
        &self,
    ) -> Result<crate::wifi_mac::MacRxDmaSnapshot, Esp32s31RadioOwnerArenaError> {
        self.arena.try_receive_dma_snapshot()
    }

    /// Apply the reviewed associated-STA receive policy.
    pub fn try_configure_station_receive_policy(
        &self,
        bssid: [u8; 6],
    ) -> Result<(), Esp32s31RadioOwnerArenaError> {
        self.arena.try_configure_station_receive_policy(bssid)
    }

    /// Apply serialized normal STA plus ESP-NOW management admission policy.
    pub fn try_configure_station_esp_now_receive_policy(
        &self,
        bssid: [u8; 6],
    ) -> Result<(), Esp32s31RadioOwnerArenaError> {
        self.arena
            .try_configure_station_esp_now_receive_policy(bssid)
    }

    /// Read the hardware noise floor without exposing the PAC owner.
    pub fn try_noise_floor_dbm(&self) -> Result<i8, Esp32s31RadioOwnerArenaError> {
        self.arena.try_noise_floor_dbm()
    }

    /// Install one semantic station CCMP key without exposing the table owner.
    pub fn try_install_station_ccmp_entry(
        &self,
        index: u8,
        identity: crate::types::MacCcmpKeyIdentity,
        temporal_key: &[u8; 16],
    ) -> Result<crate::wifi_mac::MacKeyInstallOutcome, Esp32s31RadioOwnerArenaError> {
        self.arena
            .try_install_station_ccmp_entry(index, identity, temporal_key)
    }

    /// Install one semantic access-point CCMP key without exposing the table
    /// owner or erasing the role-specific crypto-interface transaction.
    pub fn try_install_access_point_ccmp_entry(
        &self,
        index: u8,
        identity: crate::types::MacCcmpKeyIdentity,
        temporal_key: &[u8; 16],
    ) -> Result<crate::wifi_mac::MacKeyInstallOutcome, Esp32s31RadioOwnerArenaError> {
        self.arena
            .try_install_access_point_ccmp_entry(index, identity, temporal_key)
    }

    /// Clear one CCMP table entry without exposing the key-table owner.
    pub fn try_clear_ccmp_entry(&self, index: u8) -> Result<(), Esp32s31RadioOwnerArenaError> {
        self.arena.try_clear_ccmp_entry(index)
    }

    /// Observe one CCMP validity bit without exposing the key-table owner.
    pub fn try_ccmp_entry_is_valid(
        &self,
        index: u8,
    ) -> Result<Option<bool>, Esp32s31RadioOwnerArenaError> {
        self.arena.try_ccmp_entry_is_valid(index)
    }
}

impl Drop for Esp32s31PublishedRadioOwner<'_> {
    fn drop(&mut self) {
        if self.reclaim_required {
            self.arena.state.store(RESET_REQUIRED, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{MacInterruptSetup, RadioHardware, RadioRuntimeOwner};

    use super::*;

    #[test]
    fn stable_publication_reclaims_exactly_once_and_drop_poison_is_sticky() {
        let cold = RadioHardware::for_validation().into_wifi();
        let (registers, _interrupt_setup) = cold.into_running();
        let owner = RadioRuntimeOwner::from_pac(registers);
        let arena = Esp32s31RadioOwnerArena::new();
        let published = arena
            .publish(owner)
            .unwrap_or_else(|_| panic!("an empty arena must accept its first owner"));
        assert_eq!(arena.state(), Esp32s31RadioOwnerArenaState::Published);

        let borrowed = arena.registers.borrow();
        let published = match published.try_reclaim() {
            Ok(_) => panic!("an outstanding transaction must prevent reclaim"),
            Err((published, error)) => {
                assert_eq!(error, Esp32s31RadioOwnerArenaError::Borrowed);
                published
            }
        };
        drop(borrowed);
        let reclaimed = published
            .try_reclaim_with_republish()
            .unwrap_or_else(|_| panic!("a returned transaction must permit exact reclaim"));
        assert_eq!(arena.state(), Esp32s31RadioOwnerArenaState::Empty);

        let poisoned = Esp32s31RadioOwnerArena::new();
        let published = poisoned
            .publish(reclaimed.into_owner())
            .unwrap_or_else(|_| panic!("the second empty arena must accept the reclaimed owner"));
        drop(published);
        assert_eq!(
            poisoned.state(),
            Esp32s31RadioOwnerArenaState::ResetRequired
        );
    }

    #[test]
    fn reclaimed_owner_republishes_only_through_its_exact_arena_binding() {
        let cold = RadioHardware::for_validation().into_wifi();
        let (registers, _interrupt_setup) = cold.into_running();
        let owner = RadioRuntimeOwner::from_pac(registers);
        let arena = Esp32s31RadioOwnerArena::new();
        let published = arena
            .publish(owner)
            .unwrap_or_else(|_| panic!("an empty arena must accept the runtime owner"));
        let reclaimed = published
            .try_reclaim_with_republish()
            .unwrap_or_else(|_| panic!("a quiescent lease must retain its arena binding"));
        assert_eq!(arena.state(), Esp32s31RadioOwnerArenaState::Empty);

        let published = reclaimed
            .try_republish()
            .unwrap_or_else(|_| panic!("the exact empty arena must accept republication"));
        assert_eq!(arena.state(), Esp32s31RadioOwnerArenaState::Published);
        let _registers = published
            .try_reclaim()
            .unwrap_or_else(|_| panic!("the republished owner must remain reclaimable"));
    }

    #[test]
    fn published_channel_capability_holds_the_arena_serialization_guard() {
        let cold = RadioHardware::for_validation().into_wifi();
        let (registers, _interrupt_setup) = cold.into_running();
        let owner = RadioRuntimeOwner::from_pac(registers);
        let arena = Esp32s31RadioOwnerArena::new();
        let published = arena
            .publish(owner)
            .unwrap_or_else(|_| panic!("an empty arena must accept the runtime owner"));
        let access = published.access();
        let mut platform = ();
        let channel = access
            .try_channel_hal(&mut platform)
            .unwrap_or_else(|_| panic!("published registers must yield a channel capability"));

        let published = match published.try_reclaim() {
            Ok(_) => panic!("a live channel capability must prevent reclaim"),
            Err((published, error)) => {
                assert_eq!(error, Esp32s31RadioOwnerArenaError::Borrowed);
                published
            }
        };
        drop(channel);
        let _registers = published
            .try_reclaim()
            .unwrap_or_else(|_| panic!("dropping the channel capability must release the guard"));
    }

    #[test]
    fn published_wifi_mac_capability_holds_the_arena_serialization_guard() {
        let cold = RadioHardware::for_validation().into_wifi();
        let (registers, _interrupt_setup) = cold.into_running();
        let owner = RadioRuntimeOwner::from_pac(registers);
        let arena = Esp32s31RadioOwnerArena::new();
        let published = arena
            .publish(owner)
            .unwrap_or_else(|_| panic!("an empty arena must accept the runtime owner"));
        let access = published.access();
        let wifi_mac = access
            .try_wifi_mac_hal()
            .unwrap_or_else(|_| panic!("published registers must yield a Wi-Fi MAC capability"));

        let published = match published.try_reclaim() {
            Ok(_) => panic!("a live Wi-Fi MAC capability must prevent reclaim"),
            Err((published, error)) => {
                assert_eq!(error, Esp32s31RadioOwnerArenaError::Borrowed);
                published
            }
        };
        drop(wifi_mac);
        let _registers = published
            .try_reclaim()
            .unwrap_or_else(|_| panic!("dropping the Wi-Fi MAC capability must release the guard"));
    }

    #[test]
    fn stale_access_cannot_mutate_a_reset_required_arena() {
        let cold = RadioHardware::for_validation().into_wifi();
        let (registers, interrupt_setup) = cold.into_running();
        let owner = RadioRuntimeOwner::from_pac(registers);
        let mut interrupt_setup = MacInterruptSetup {
            inner: interrupt_setup,
        };
        let arena = Esp32s31RadioOwnerArena::new();
        let published = arena
            .publish(owner)
            .unwrap_or_else(|_| panic!("an empty arena must accept the runtime owner"));
        let access = published.access();

        drop(published);
        assert_eq!(arena.state(), Esp32s31RadioOwnerArenaState::ResetRequired);
        assert!(matches!(
            access.try_prepare_connected_sta_without_power_save(&mut interrupt_setup),
            Err(Esp32s31RadioOwnerArenaError::ResetRequired)
        ));
    }
}
