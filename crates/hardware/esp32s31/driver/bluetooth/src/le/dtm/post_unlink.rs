//! Capacity-one handoff for the first primary event after a role unlinks its software item.
//!
//! The mailbox is protocol-neutral: it retains only a primary publication and
//! a globally unique mailbox identity and monotonically increasing arm
//! generation. The concrete role's unlinked graph remains in a generic affine
//! task owner. Controller composition serializes unlink, primary
//! capture/publication, consumption and re-arm through the same critical
//! section, so a pre-arm observation or another Controller mailbox cannot
//! cross the return gate.

#![forbid(unsafe_code)]

#[cfg(any(target_arch = "riscv32", test))]
use core::cell::Cell;
#[cfg(target_arch = "riscv32")]
use core::cell::RefCell;

use core::sync::atomic::{AtomicU8, Ordering};

#[cfg(target_arch = "riscv32")]
use critical_section::{CriticalSection, Mutex};

use crate::interrupt::PrimaryPublishedInterruptStep;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) enum DtmPostUnlinkArmError {
    Busy,
    IdentityExhausted,
    GenerationExhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct DtmPostUnlinkMailboxIdentity(u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct DtmPostUnlinkArmKey {
    identity: DtmPostUnlinkMailboxIdentity,
    generation: u32,
}

#[derive(Debug)]
#[cfg(any(target_arch = "riscv32", test))]
enum DtmPostUnlinkMailboxState<Event> {
    Unidentified,
    Idle {
        identity: DtmPostUnlinkMailboxIdentity,
        generation: u32,
    },
    Armed {
        key: DtmPostUnlinkArmKey,
    },
    Ready {
        key: DtmPostUnlinkArmKey,
        event: Event,
    },
}

#[cfg(any(target_arch = "riscv32", test))]
impl<Event> DtmPostUnlinkMailboxState<Event> {
    const fn new() -> Self {
        Self::Unidentified
    }

    fn prepare_arm(
        &mut self,
        allocate_identity: impl FnOnce() -> Option<DtmPostUnlinkMailboxIdentity>,
    ) -> Result<DtmPostUnlinkArmKey, DtmPostUnlinkArmError> {
        match self {
            Self::Unidentified => {
                let identity =
                    allocate_identity().ok_or(DtmPostUnlinkArmError::IdentityExhausted)?;
                *self = Self::Idle {
                    identity,
                    generation: 0,
                };
                Ok(DtmPostUnlinkArmKey {
                    identity,
                    generation: 1,
                })
            }
            Self::Idle {
                identity,
                generation,
            } => Ok(DtmPostUnlinkArmKey {
                identity: *identity,
                generation: generation
                    .checked_add(1)
                    .ok_or(DtmPostUnlinkArmError::GenerationExhausted)?,
            }),
            Self::Armed { .. } | Self::Ready { .. } => Err(DtmPostUnlinkArmError::Busy),
        }
    }

    fn commit_arm(&mut self, key: DtmPostUnlinkArmKey) -> bool {
        match self {
            Self::Idle {
                identity,
                generation: current,
            } if *identity == key.identity && current.checked_add(1) == Some(key.generation) => {
                *self = Self::Armed { key };
                true
            }
            Self::Unidentified | Self::Idle { .. } | Self::Armed { .. } | Self::Ready { .. } => {
                false
            }
        }
    }

    fn publish(&mut self, event: Event) -> DtmPostUnlinkMailboxPublish<Event> {
        match self {
            Self::Unidentified | Self::Idle { .. } => DtmPostUnlinkMailboxPublish::General(event),
            Self::Armed { key } => {
                let key = *key;
                *self = Self::Ready { key, event };
                DtmPostUnlinkMailboxPublish::Stored
            }
            Self::Ready { .. } => DtmPostUnlinkMailboxPublish::Full(event),
        }
    }

    fn take(&mut self, expected: DtmPostUnlinkArmKey) -> DtmPostUnlinkMailboxTake<Event> {
        match self {
            Self::Armed { key } if *key == expected => {
                *self = Self::Idle {
                    identity: expected.identity,
                    generation: expected.generation,
                };
                DtmPostUnlinkMailboxTake::Recheck { key: expected }
            }
            Self::Ready { key, .. } if *key == expected => {
                let retained = core::mem::replace(
                    self,
                    Self::Idle {
                        identity: expected.identity,
                        generation: expected.generation,
                    },
                );
                match retained {
                    Self::Ready { key, event } => DtmPostUnlinkMailboxTake::Ready { key, event },
                    state => {
                        *self = state;
                        DtmPostUnlinkMailboxTake::AffinityMismatch
                    }
                }
            }
            Self::Unidentified | Self::Idle { .. } | Self::Armed { .. } | Self::Ready { .. } => {
                DtmPostUnlinkMailboxTake::AffinityMismatch
            }
        }
    }

    fn rearm(&mut self, key: DtmPostUnlinkArmKey) -> bool {
        match self {
            Self::Idle {
                identity,
                generation: current,
            } if *identity == key.identity && *current == key.generation => {
                *self = Self::Armed { key };
                true
            }
            Self::Unidentified | Self::Idle { .. } | Self::Armed { .. } | Self::Ready { .. } => {
                false
            }
        }
    }
}

#[cfg(any(target_arch = "riscv32", test))]
fn allocate_mailbox_identity(next_identity: &Cell<u32>) -> Option<DtmPostUnlinkMailboxIdentity> {
    let identity = next_identity.get().checked_add(1)?;
    next_identity.set(identity);
    Some(DtmPostUnlinkMailboxIdentity(identity))
}

#[cfg(any(target_arch = "riscv32", test))]
enum DtmPostUnlinkMailboxPublish<Event> {
    General(Event),
    Stored,
    Full(Event),
}

#[cfg(any(target_arch = "riscv32", test))]
enum DtmPostUnlinkMailboxTake<Event> {
    Recheck {
        key: DtmPostUnlinkArmKey,
    },
    Ready {
        key: DtmPostUnlinkArmKey,
        event: Event,
    },
    AffinityMismatch,
}

/// Durable wake disposition for one newly occupied post-unlink slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmPostUnlinkMailboxPublication {
    /// The capacity-one slot changed from armed to ready.
    WakeConsumer,
    /// The slot was already ready and its durable consumer epoch remains open.
    AlreadyReady,
}

const POST_UNLINK_WAKE_PENDING: u8 = 1;

/// Durable wake epoch shared by the post-unlink interrupt and task services.
///
/// Interrupt publication makes readiness sticky until the matching mailbox
/// event is consumed. An async adapter must register its executor waker first
/// and then call [`is_pending`](Self::is_pending); a publication racing before
/// that recheck remains observable even if the immediate wake callback arrived
/// before registration.
pub struct DtmPostUnlinkWakeCell {
    state: AtomicU8,
}

impl DtmPostUnlinkWakeCell {
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn new() -> Self {
        Self {
            state: AtomicU8::new(0),
        }
    }

    /// Recheck durable readiness after the executor-specific waker is registered.
    pub fn is_pending(&self) -> bool {
        self.state.load(Ordering::Acquire) & POST_UNLINK_WAKE_PENDING != 0
    }

    /// Publish one ready edge while retaining readiness until mailbox close.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn publish_from_interrupt(&self) -> DtmPostUnlinkMailboxPublication {
        let previous = self
            .state
            .fetch_or(POST_UNLINK_WAKE_PENDING, Ordering::AcqRel);
        if previous & POST_UNLINK_WAKE_PENDING != 0 {
            DtmPostUnlinkMailboxPublication::AlreadyReady
        } else {
            DtmPostUnlinkMailboxPublication::WakeConsumer
        }
    }

    /// Close the ready epoch after the matching mailbox event was consumed.
    ///
    /// Calling this before consuming the source would discard its notification,
    /// so the Controller mailbox performs this transition in the same critical
    /// section as its successful ready take.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn close(&self) -> bool {
        self.state.swap(0, Ordering::AcqRel) & POST_UNLINK_WAKE_PENDING != 0
    }
}

/// Result of one serialized primary service and both ordinary publications.
#[must_use = "the primary event or DTM mailbox wake must reach its consumer"]
pub enum PrimarySerializedServiceStep {
    /// No DTM return gate was armed; retain the ordinary primary result.
    General {
        published: PrimaryPublishedInterruptStep,
        ordinary: PrimaryOrdinaryPublication,
    },
    /// The first eligible post-unlink event is durable in the capacity-one slot.
    DtmStored {
        mailbox: DtmPostUnlinkMailboxPublication,
        ordinary: PrimaryOrdinaryPublication,
    },
    /// A preceding eligible event remains stored; the newer result is returned.
    MailboxFull {
        published: PrimaryPublishedInterruptStep,
        mailbox: DtmPostUnlinkMailboxPublication,
        ordinary: PrimaryOrdinaryPublication,
    },
}

/// Ordinary worker publications completed before the serialized service returns.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrimaryOrdinaryPublication {
    /// Fault, recovery and empty primary epochs publish no ordinary worker wake.
    None,
    /// Both ordinary Controller cells accepted the same scheduler observation.
    Scheduler {
        scheduler: crate::interrupt::SchedulerWakePublication,
        lock_modify: crate::scheduler::SchedulerLockModifyEventPublication,
    },
}

impl PrimaryOrdinaryPublication {
    #[cfg(target_arch = "riscv32")]
    fn from_published(event: &PrimaryPublishedInterruptStep) -> Self {
        match event {
            PrimaryPublishedInterruptStep::Scheduler {
                scheduler,
                lock_modify,
                ..
            } => Self::Scheduler {
                scheduler: *scheduler,
                lock_modify: *lock_modify,
            },
            PrimaryPublishedInterruptStep::Fault(_)
            | PrimaryPublishedInterruptStep::NoSchedulerWork(_) => Self::None,
        }
    }
}

/// Protocol-neutral capacity-one primary-event mailbox.
#[cfg(target_arch = "riscv32")]
pub(crate) struct DtmPostUnlinkMailbox {
    state: Mutex<RefCell<DtmPostUnlinkMailboxState<PrimaryPublishedInterruptStep>>>,
    wake: DtmPostUnlinkWakeCell,
}

#[cfg(target_arch = "riscv32")]
static BLUETOOTH_DTM_POST_UNLINK_NEXT_IDENTITY: Mutex<Cell<u32>> = Mutex::new(Cell::new(0));

#[cfg(target_arch = "riscv32")]
impl DtmPostUnlinkMailbox {
    pub(crate) const fn new() -> Self {
        Self {
            state: Mutex::new(RefCell::new(DtmPostUnlinkMailboxState::new())),
            wake: DtmPostUnlinkWakeCell::new(),
        }
    }

    pub(crate) const fn wake(&self) -> &DtmPostUnlinkWakeCell {
        &self.wake
    }

    pub(crate) fn prepare_arm(
        &self,
        critical_section: CriticalSection<'_>,
    ) -> Result<DtmPostUnlinkArmKey, DtmPostUnlinkArmError> {
        self.state
            .borrow(critical_section)
            .borrow_mut()
            .prepare_arm(|| {
                allocate_mailbox_identity(
                    BLUETOOTH_DTM_POST_UNLINK_NEXT_IDENTITY.borrow(critical_section),
                )
            })
    }

    pub(crate) fn commit_arm(
        &self,
        critical_section: CriticalSection<'_>,
        key: DtmPostUnlinkArmKey,
    ) -> bool {
        self.state
            .borrow(critical_section)
            .borrow_mut()
            .commit_arm(key)
    }

    pub(crate) fn publish(
        &self,
        critical_section: CriticalSection<'_>,
        event: PrimaryPublishedInterruptStep,
    ) -> PrimarySerializedServiceStep {
        let ordinary = PrimaryOrdinaryPublication::from_published(&event);
        match self
            .state
            .borrow(critical_section)
            .borrow_mut()
            .publish(event)
        {
            DtmPostUnlinkMailboxPublish::General(event) => PrimarySerializedServiceStep::General {
                published: event,
                ordinary,
            },
            DtmPostUnlinkMailboxPublish::Stored => PrimarySerializedServiceStep::DtmStored {
                mailbox: self.wake.publish_from_interrupt(),
                ordinary,
            },
            DtmPostUnlinkMailboxPublish::Full(event) => PrimarySerializedServiceStep::MailboxFull {
                published: event,
                mailbox: self.wake.publish_from_interrupt(),
                ordinary,
            },
        }
    }

    pub(crate) fn take<RoleOwner>(
        &self,
        critical_section: CriticalSection<'_>,
        awaiting: BluetoothPostUnlinkAwaiting<RoleOwner>,
    ) -> PostUnlinkTake<RoleOwner> {
        match self
            .state
            .borrow(critical_section)
            .borrow_mut()
            .take(awaiting.key)
        {
            DtmPostUnlinkMailboxTake::Recheck { key } => PostUnlinkTake::Recheck {
                key,
                unlinked: awaiting.unlinked,
            },
            DtmPostUnlinkMailboxTake::Ready { key, event } => {
                let _ = self.wake.close();
                PostUnlinkTake::Ready {
                    key,
                    event: SoftwareListRemovalPublishedEvent {
                        unlinked: awaiting.unlinked,
                        published: event,
                    },
                }
            }
            DtmPostUnlinkMailboxTake::AffinityMismatch => {
                PostUnlinkTake::AffinityMismatch(awaiting)
            }
        }
    }

    pub(crate) fn rearm<RoleOwner>(
        &self,
        critical_section: CriticalSection<'_>,
        key: DtmPostUnlinkArmKey,
        unlinked: RoleOwner,
    ) -> PostUnlinkRearm<RoleOwner> {
        if self.state.borrow(critical_section).borrow_mut().rearm(key) {
            PostUnlinkRearm::Armed(BluetoothPostUnlinkAwaiting { unlinked, key })
        } else {
            PostUnlinkRearm::AffinityMismatch(unlinked)
        }
    }
}

/// Already-unlinked role owner tied to one exact mailbox identity and arm generation.
#[cfg(target_arch = "riscv32")]
#[must_use = "the armed post-unlink owner must consume its exact event"]
pub struct BluetoothPostUnlinkAwaiting<RoleOwner> {
    unlinked: RoleOwner,
    key: DtmPostUnlinkArmKey,
}

#[cfg(target_arch = "riscv32")]
impl<RoleOwner> BluetoothPostUnlinkAwaiting<RoleOwner> {
    pub(crate) const fn new(unlinked: RoleOwner, key: DtmPostUnlinkArmKey) -> Self {
        Self { unlinked, key }
    }
}

/// Opaque pairing minted only by the matching armed mailbox.
#[cfg(target_arch = "riscv32")]
#[must_use = "the paired post-unlink event and graph must be consumed together"]
pub(crate) struct SoftwareListRemovalPublishedEvent<RoleOwner> {
    unlinked: RoleOwner,
    published: PrimaryPublishedInterruptStep,
}

#[cfg(target_arch = "riscv32")]
impl<RoleOwner> SoftwareListRemovalPublishedEvent<RoleOwner> {
    pub(crate) fn into_parts(self) -> (RoleOwner, PrimaryPublishedInterruptStep) {
        (self.unlinked, self.published)
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) enum PostUnlinkTake<RoleOwner> {
    Recheck {
        key: DtmPostUnlinkArmKey,
        unlinked: RoleOwner,
    },
    Ready {
        key: DtmPostUnlinkArmKey,
        event: SoftwareListRemovalPublishedEvent<RoleOwner>,
    },
    AffinityMismatch(BluetoothPostUnlinkAwaiting<RoleOwner>),
}

#[cfg(target_arch = "riscv32")]
pub(crate) enum PostUnlinkRearm<RoleOwner> {
    Armed(BluetoothPostUnlinkAwaiting<RoleOwner>),
    AffinityMismatch(RoleOwner),
}

#[cfg(test)]
mod tests;
