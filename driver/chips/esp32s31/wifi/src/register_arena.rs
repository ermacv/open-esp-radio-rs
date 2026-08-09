//! Reclaimable stable placement for the running Wi-Fi register owner.

use core::{
    cell::{Ref, RefCell, RefMut},
    sync::atomic::{AtomicU8, Ordering},
};

use open_esp_radio_esp32s31_hal::RadioRegisters;

const EMPTY: u8 = 0;
const PUBLISHED: u8 = 1;
const RESET_REQUIRED: u8 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31RadioRegistersArenaState {
    Empty,
    Published,
    ResetRequired,
}

impl Esp32s31RadioRegistersArenaState {
    const fn decode(value: u8) -> Self {
        match value {
            EMPTY => Self::Empty,
            PUBLISHED => Self::Published,
            _ => Self::ResetRequired,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31RadioRegistersArenaError {
    AlreadyPublished,
    ResetRequired,
    Borrowed,
    MissingOwner,
}

/// Failed publication retaining the exact register owner.
pub struct Esp32s31RadioRegistersPublishFailure {
    pub error: Esp32s31RadioRegistersArenaError,
    pub registers: RadioRegisters,
}

/// Exact empty-arena capability returned beside a reclaimed PAC owner.
///
/// Keeping this non-cloneable value in role-local stopped resources preserves
/// which stable arena may host the next task epoch. Higher layers therefore do
/// not need to recover an initialized arena from a global `StaticCell`.
pub struct Esp32s31RadioRegistersRepublish<'arena> {
    arena: &'arena Esp32s31RadioRegistersArena,
}

impl<'arena> Esp32s31RadioRegistersRepublish<'arena> {
    /// Publish a returned PAC owner into the exact arena reclaimed with it.
    pub fn try_publish(
        self,
        registers: RadioRegisters,
    ) -> Result<
        Esp32s31PublishedRadioRegisters<'arena>,
        Esp32s31RadioRegistersRepublishFailure<'arena>,
    > {
        match self.arena.publish(registers) {
            Ok(published) => Ok(published),
            Err(failure) => Err(Esp32s31RadioRegistersRepublishFailure {
                error: failure.error,
                registers: failure.registers,
                republish: self,
            }),
        }
    }
}

/// Failed exact-arena republication retaining both movable capabilities.
pub struct Esp32s31RadioRegistersRepublishFailure<'arena> {
    pub error: Esp32s31RadioRegistersArenaError,
    pub registers: RadioRegisters,
    pub republish: Esp32s31RadioRegistersRepublish<'arena>,
}

/// PAC owner and the exact empty-arena capability reclaimed from one epoch.
pub struct Esp32s31ReclaimedRadioRegisters<'arena> {
    registers: RadioRegisters,
    republish: Esp32s31RadioRegistersRepublish<'arena>,
}

impl<'arena> Esp32s31ReclaimedRadioRegisters<'arena> {
    pub fn into_parts(self) -> (RadioRegisters, Esp32s31RadioRegistersRepublish<'arena>) {
        (self.registers, self.republish)
    }

    /// Discard the empty-arena binding when the caller intentionally does not
    /// need another task-stable publication.
    pub fn into_registers(self) -> RadioRegisters {
        self.registers
    }

    pub fn try_republish(
        self,
    ) -> Result<
        Esp32s31PublishedRadioRegisters<'arena>,
        Esp32s31RadioRegistersRepublishFailure<'arena>,
    > {
        self.republish.try_publish(self.registers)
    }
}

/// Stable storage used while executor tasks require a `'static` register
/// address.
///
/// The arena is role-neutral. Publishing transfers the unique
/// [`RadioRegisters`] value into it and returns the only movable lease.
/// Consuming that lease after every task has stopped returns the original
/// value. Dropping a live lease poisons the arena instead of making the PAC
/// owner silently reusable.
pub struct Esp32s31RadioRegistersArena {
    registers: RefCell<Option<RadioRegisters>>,
    state: AtomicU8,
}

impl Esp32s31RadioRegistersArena {
    pub const fn new() -> Self {
        Self {
            registers: RefCell::new(None),
            state: AtomicU8::new(EMPTY),
        }
    }

    pub fn state(&self) -> Esp32s31RadioRegistersArenaState {
        Esp32s31RadioRegistersArenaState::decode(self.state.load(Ordering::Acquire))
    }

    /// Move one register owner into stable storage for a finite task epoch.
    pub fn publish(
        &self,
        registers: RadioRegisters,
    ) -> Result<Esp32s31PublishedRadioRegisters<'_>, Esp32s31RadioRegistersPublishFailure> {
        let state = self.state();
        if state != Esp32s31RadioRegistersArenaState::Empty {
            return Err(Esp32s31RadioRegistersPublishFailure {
                error: match state {
                    Esp32s31RadioRegistersArenaState::Published => {
                        Esp32s31RadioRegistersArenaError::AlreadyPublished
                    }
                    Esp32s31RadioRegistersArenaState::ResetRequired => {
                        Esp32s31RadioRegistersArenaError::ResetRequired
                    }
                    Esp32s31RadioRegistersArenaState::Empty => unreachable!(),
                },
                registers,
            });
        }
        let mut slot = match self.registers.try_borrow_mut() {
            Ok(slot) => slot,
            Err(_) => {
                return Err(Esp32s31RadioRegistersPublishFailure {
                    error: Esp32s31RadioRegistersArenaError::Borrowed,
                    registers,
                });
            }
        };
        if slot.is_some() {
            self.state.store(RESET_REQUIRED, Ordering::Release);
            return Err(Esp32s31RadioRegistersPublishFailure {
                error: Esp32s31RadioRegistersArenaError::ResetRequired,
                registers,
            });
        }
        *slot = Some(registers);
        self.state.store(PUBLISHED, Ordering::Release);
        drop(slot);
        Ok(Esp32s31PublishedRadioRegisters {
            arena: self,
            reclaim_required: true,
        })
    }
}

impl Default for Esp32s31RadioRegistersArena {
    fn default() -> Self {
        Self::new()
    }
}

/// Unique movable lease for one published register owner.
pub struct Esp32s31PublishedRadioRegisters<'arena> {
    arena: &'arena Esp32s31RadioRegistersArena,
    reclaim_required: bool,
}

impl<'arena> Esp32s31PublishedRadioRegisters<'arena> {
    /// Copyable bounded-transaction handle for child actors in the same
    /// finite role epoch. The root lease must not be reclaimed until every
    /// actor using this handle has acknowledged shutdown.
    pub const fn access(&self) -> Esp32s31RadioRegistersAccess<'arena> {
        Esp32s31RadioRegistersAccess { arena: self.arena }
    }

    /// Run one bounded mutable register transaction.
    ///
    /// This is an integration seam for chip MAC traits. The closure cannot
    /// retain the dynamic borrow across an async suspension.
    #[doc(hidden)]
    pub fn with_mut<T>(&mut self, transaction: impl FnOnce(&mut RadioRegisters) -> T) -> T {
        let mut registers = self.borrow_mut();
        transaction(&mut registers)
    }

    /// Run one bounded shared register observation.
    #[doc(hidden)]
    pub fn with_ref<T>(&self, transaction: impl FnOnce(&RadioRegisters) -> T) -> T {
        let registers = self.borrow();
        transaction(&registers)
    }

    /// Return the exact PAC owner only while no synchronous register
    /// transaction is borrowed.
    pub fn try_reclaim(self) -> Result<RadioRegisters, (Self, Esp32s31RadioRegistersArenaError)> {
        self.try_reclaim_with_republish()
            .map(Esp32s31ReclaimedRadioRegisters::into_registers)
    }

    /// Return the PAC owner together with the exact empty-arena capability.
    ///
    /// This is the owner-preserving boundary for a later role/task epoch. The
    /// ordinary [`try_reclaim`](Self::try_reclaim) remains useful when the
    /// caller deliberately tears down stable publication permanently.
    pub fn try_reclaim_with_republish(
        mut self,
    ) -> Result<Esp32s31ReclaimedRadioRegisters<'arena>, (Self, Esp32s31RadioRegistersArenaError)>
    {
        let mut slot = match self.arena.registers.try_borrow_mut() {
            Ok(slot) => slot,
            Err(_) => return Err((self, Esp32s31RadioRegistersArenaError::Borrowed)),
        };
        let Some(registers) = slot.take() else {
            self.arena.state.store(RESET_REQUIRED, Ordering::Release);
            return Err((self, Esp32s31RadioRegistersArenaError::MissingOwner));
        };
        self.arena.state.store(EMPTY, Ordering::Release);
        self.reclaim_required = false;
        drop(slot);
        Ok(Esp32s31ReclaimedRadioRegisters {
            registers,
            republish: Esp32s31RadioRegistersRepublish { arena: self.arena },
        })
    }

    #[doc(hidden)]
    pub fn borrow(&self) -> Ref<'_, RadioRegisters> {
        Ref::map(self.arena.registers.borrow(), |slot| {
            slot.as_ref()
                .expect("published register lease retains its PAC owner")
        })
    }

    #[doc(hidden)]
    pub fn borrow_mut(&self) -> RefMut<'_, RadioRegisters> {
        RefMut::map(self.arena.registers.borrow_mut(), |slot| {
            slot.as_mut()
                .expect("published register lease retains its PAC owner")
        })
    }
}

/// Non-owning transaction handle derived from one published lease.
#[derive(Clone, Copy)]
pub struct Esp32s31RadioRegistersAccess<'arena> {
    arena: &'arena Esp32s31RadioRegistersArena,
}

impl Esp32s31RadioRegistersAccess<'_> {
    #[doc(hidden)]
    pub fn borrow_mut(&self) -> RefMut<'_, RadioRegisters> {
        RefMut::map(self.arena.registers.borrow_mut(), |slot| {
            slot.as_mut()
                .expect("published register access retains its PAC owner")
        })
    }

    #[doc(hidden)]
    pub fn borrow(&self) -> Ref<'_, RadioRegisters> {
        Ref::map(self.arena.registers.borrow(), |slot| {
            slot.as_ref()
                .expect("published register access retains its PAC owner")
        })
    }

    #[doc(hidden)]
    pub fn with_mut<T>(&self, transaction: impl FnOnce(&mut RadioRegisters) -> T) -> T {
        let mut registers = self.borrow_mut();
        transaction(&mut registers)
    }

    #[doc(hidden)]
    pub fn with_ref<T>(&self, transaction: impl FnOnce(&RadioRegisters) -> T) -> T {
        let registers = self.borrow();
        transaction(&registers)
    }
}

impl Drop for Esp32s31PublishedRadioRegisters<'_> {
    fn drop(&mut self) {
        if self.reclaim_required {
            self.arena.state.store(RESET_REQUIRED, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_hal::ColdRadioRegisters;

    use super::*;

    #[test]
    fn stable_publication_reclaims_exactly_once_and_drop_poison_is_sticky() {
        let cold = ColdRadioRegisters::take()
            .unwrap_or_else(|| panic!("register singleton must be free for the arena test"));
        let (registers, _interrupt_setup) = cold.into_running();
        let arena = Esp32s31RadioRegistersArena::new();
        let published = arena
            .publish(registers)
            .unwrap_or_else(|_| panic!("an empty arena must accept its first owner"));
        assert_eq!(arena.state(), Esp32s31RadioRegistersArenaState::Published);

        let access = published.access();
        let borrowed = access.borrow();
        let published = match published.try_reclaim() {
            Ok(_) => panic!("an outstanding transaction must prevent reclaim"),
            Err((published, error)) => {
                assert_eq!(error, Esp32s31RadioRegistersArenaError::Borrowed);
                published
            }
        };
        drop(borrowed);
        let reclaimed = published
            .try_reclaim_with_republish()
            .unwrap_or_else(|_| panic!("a returned transaction must permit exact reclaim"));
        assert_eq!(arena.state(), Esp32s31RadioRegistersArenaState::Empty);

        let poisoned = Esp32s31RadioRegistersArena::new();
        let published = poisoned
            .publish(reclaimed.into_registers())
            .unwrap_or_else(|_| panic!("the second empty arena must accept the reclaimed owner"));
        drop(published);
        assert_eq!(
            poisoned.state(),
            Esp32s31RadioRegistersArenaState::ResetRequired
        );
    }

    #[test]
    fn reclaimed_owner_republishes_only_through_its_exact_arena_binding() {
        let cold = ColdRadioRegisters::take()
            .unwrap_or_else(|| panic!("register singleton must be free for republish test"));
        let (registers, _interrupt_setup) = cold.into_running();
        let arena = Esp32s31RadioRegistersArena::new();
        let published = arena
            .publish(registers)
            .unwrap_or_else(|_| panic!("an empty arena must accept the PAC owner"));
        let reclaimed = published
            .try_reclaim_with_republish()
            .unwrap_or_else(|_| panic!("a quiescent lease must retain its arena binding"));
        assert_eq!(arena.state(), Esp32s31RadioRegistersArenaState::Empty);

        let published = reclaimed
            .try_republish()
            .unwrap_or_else(|_| panic!("the exact empty arena must accept republication"));
        assert_eq!(arena.state(), Esp32s31RadioRegistersArenaState::Published);
        let _registers = published
            .try_reclaim()
            .unwrap_or_else(|_| panic!("the republished owner must remain reclaimable"));
    }
}
