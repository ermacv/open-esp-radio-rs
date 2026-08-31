//! Capacity-one handoff for the first primary event after DTM software unlink.
//!
//! The mailbox is protocol-neutral: it retains only a primary publication and
//! a globally unique mailbox identity and monotonically increasing arm
//! generation. The role-specific unlinked graph remains in an affine task
//! owner. Controller composition serializes unlink, primary
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

use crate::BluetoothPrimaryPublishedInterruptStep;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) enum BluetoothDtmPostUnlinkArmError {
    Busy,
    IdentityExhausted,
    GenerationExhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct BluetoothDtmPostUnlinkMailboxIdentity(u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct BluetoothDtmPostUnlinkArmKey {
    identity: BluetoothDtmPostUnlinkMailboxIdentity,
    generation: u32,
}

#[derive(Debug)]
#[cfg(any(target_arch = "riscv32", test))]
enum BluetoothDtmPostUnlinkMailboxState<Event> {
    Unidentified,
    Idle {
        identity: BluetoothDtmPostUnlinkMailboxIdentity,
        generation: u32,
    },
    Armed {
        key: BluetoothDtmPostUnlinkArmKey,
    },
    Ready {
        key: BluetoothDtmPostUnlinkArmKey,
        event: Event,
    },
}

#[cfg(any(target_arch = "riscv32", test))]
impl<Event> BluetoothDtmPostUnlinkMailboxState<Event> {
    const fn new() -> Self {
        Self::Unidentified
    }

    fn prepare_arm(
        &mut self,
        allocate_identity: impl FnOnce() -> Option<BluetoothDtmPostUnlinkMailboxIdentity>,
    ) -> Result<BluetoothDtmPostUnlinkArmKey, BluetoothDtmPostUnlinkArmError> {
        match self {
            Self::Unidentified => {
                let identity =
                    allocate_identity().ok_or(BluetoothDtmPostUnlinkArmError::IdentityExhausted)?;
                *self = Self::Idle {
                    identity,
                    generation: 0,
                };
                Ok(BluetoothDtmPostUnlinkArmKey {
                    identity,
                    generation: 1,
                })
            }
            Self::Idle {
                identity,
                generation,
            } => Ok(BluetoothDtmPostUnlinkArmKey {
                identity: *identity,
                generation: generation
                    .checked_add(1)
                    .ok_or(BluetoothDtmPostUnlinkArmError::GenerationExhausted)?,
            }),
            Self::Armed { .. } | Self::Ready { .. } => Err(BluetoothDtmPostUnlinkArmError::Busy),
        }
    }

    fn commit_arm(&mut self, key: BluetoothDtmPostUnlinkArmKey) -> bool {
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

    fn publish(&mut self, event: Event) -> BluetoothDtmPostUnlinkMailboxPublish<Event> {
        match self {
            Self::Unidentified | Self::Idle { .. } => {
                BluetoothDtmPostUnlinkMailboxPublish::General(event)
            }
            Self::Armed { key } => {
                let key = *key;
                *self = Self::Ready { key, event };
                BluetoothDtmPostUnlinkMailboxPublish::Stored
            }
            Self::Ready { .. } => BluetoothDtmPostUnlinkMailboxPublish::Full(event),
        }
    }

    fn take(
        &mut self,
        expected: BluetoothDtmPostUnlinkArmKey,
    ) -> BluetoothDtmPostUnlinkMailboxTake<Event> {
        match self {
            Self::Armed { key } if *key == expected => {
                *self = Self::Idle {
                    identity: expected.identity,
                    generation: expected.generation,
                };
                BluetoothDtmPostUnlinkMailboxTake::Recheck { key: expected }
            }
            Self::Ready { key, .. } if *key == expected => {
                let Self::Ready { key, event } = core::mem::replace(
                    self,
                    Self::Idle {
                        identity: expected.identity,
                        generation: expected.generation,
                    },
                ) else {
                    unreachable!("the matching ready state was checked above");
                };
                BluetoothDtmPostUnlinkMailboxTake::Ready { key, event }
            }
            Self::Unidentified | Self::Idle { .. } | Self::Armed { .. } | Self::Ready { .. } => {
                BluetoothDtmPostUnlinkMailboxTake::AffinityMismatch
            }
        }
    }

    fn rearm(&mut self, key: BluetoothDtmPostUnlinkArmKey) -> bool {
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

    fn cancel(
        &mut self,
        expected: BluetoothDtmPostUnlinkArmKey,
    ) -> BluetoothDtmPostUnlinkMailboxCancel {
        match self {
            Self::Armed { key } if *key == expected => {
                *self = Self::Idle {
                    identity: expected.identity,
                    generation: expected.generation,
                };
                BluetoothDtmPostUnlinkMailboxCancel::Cancelled
            }
            Self::Ready { key, .. } if *key == expected => {
                BluetoothDtmPostUnlinkMailboxCancel::EventReady
            }
            Self::Unidentified | Self::Idle { .. } | Self::Armed { .. } | Self::Ready { .. } => {
                BluetoothDtmPostUnlinkMailboxCancel::AffinityMismatch
            }
        }
    }
}

#[cfg(any(target_arch = "riscv32", test))]
fn allocate_mailbox_identity(
    next_identity: &Cell<u32>,
) -> Option<BluetoothDtmPostUnlinkMailboxIdentity> {
    let identity = next_identity.get().checked_add(1)?;
    next_identity.set(identity);
    Some(BluetoothDtmPostUnlinkMailboxIdentity(identity))
}

#[cfg(any(target_arch = "riscv32", test))]
enum BluetoothDtmPostUnlinkMailboxPublish<Event> {
    General(Event),
    Stored,
    Full(Event),
}

#[cfg(any(target_arch = "riscv32", test))]
enum BluetoothDtmPostUnlinkMailboxTake<Event> {
    Recheck {
        key: BluetoothDtmPostUnlinkArmKey,
    },
    Ready {
        key: BluetoothDtmPostUnlinkArmKey,
        event: Event,
    },
    AffinityMismatch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
enum BluetoothDtmPostUnlinkMailboxCancel {
    Cancelled,
    EventReady,
    AffinityMismatch,
}

/// Durable wake disposition for one newly occupied post-unlink slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmPostUnlinkMailboxPublication {
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
pub struct BluetoothDtmPostUnlinkWakeCell {
    state: AtomicU8,
}

impl BluetoothDtmPostUnlinkWakeCell {
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
    pub(crate) fn publish_from_interrupt(&self) -> BluetoothDtmPostUnlinkMailboxPublication {
        let previous = self
            .state
            .fetch_or(POST_UNLINK_WAKE_PENDING, Ordering::AcqRel);
        if previous & POST_UNLINK_WAKE_PENDING != 0 {
            BluetoothDtmPostUnlinkMailboxPublication::AlreadyReady
        } else {
            BluetoothDtmPostUnlinkMailboxPublication::WakeConsumer
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
pub enum BluetoothPrimarySerializedServiceStep {
    /// No DTM return gate was armed; retain the ordinary primary result.
    General {
        published: BluetoothPrimaryPublishedInterruptStep,
        ordinary: BluetoothPrimaryOrdinaryPublication,
    },
    /// The first eligible post-unlink event is durable in the capacity-one slot.
    DtmStored {
        mailbox: BluetoothDtmPostUnlinkMailboxPublication,
        ordinary: BluetoothPrimaryOrdinaryPublication,
    },
    /// A preceding eligible event remains stored; the newer result is returned.
    MailboxFull {
        published: BluetoothPrimaryPublishedInterruptStep,
        mailbox: BluetoothDtmPostUnlinkMailboxPublication,
        ordinary: BluetoothPrimaryOrdinaryPublication,
    },
}

/// Ordinary worker publications completed before the serialized service returns.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothPrimaryOrdinaryPublication {
    /// Fault, recovery and empty primary epochs publish no ordinary worker wake.
    None,
    /// Both ordinary Controller cells accepted the same scheduler observation.
    Scheduler {
        scheduler: crate::BluetoothSchedulerWakePublication,
        lock_modify: crate::BluetoothSchedulerLockModifyEventPublication,
    },
}

impl BluetoothPrimaryOrdinaryPublication {
    #[cfg(target_arch = "riscv32")]
    fn from_published(event: &BluetoothPrimaryPublishedInterruptStep) -> Self {
        match event {
            BluetoothPrimaryPublishedInterruptStep::Scheduler {
                scheduler,
                lock_modify,
                ..
            } => Self::Scheduler {
                scheduler: *scheduler,
                lock_modify: *lock_modify,
            },
            BluetoothPrimaryPublishedInterruptStep::Fault(_)
            | BluetoothPrimaryPublishedInterruptStep::NoSchedulerWork(_) => Self::None,
        }
    }
}

/// Protocol-neutral capacity-one primary-event mailbox.
#[cfg(target_arch = "riscv32")]
pub(crate) struct BluetoothDtmPostUnlinkMailbox {
    state:
        Mutex<RefCell<BluetoothDtmPostUnlinkMailboxState<BluetoothPrimaryPublishedInterruptStep>>>,
    wake: BluetoothDtmPostUnlinkWakeCell,
}

#[cfg(target_arch = "riscv32")]
static BLUETOOTH_DTM_POST_UNLINK_NEXT_IDENTITY: Mutex<Cell<u32>> = Mutex::new(Cell::new(0));

#[cfg(target_arch = "riscv32")]
impl BluetoothDtmPostUnlinkMailbox {
    pub(crate) const fn new() -> Self {
        Self {
            state: Mutex::new(RefCell::new(BluetoothDtmPostUnlinkMailboxState::new())),
            wake: BluetoothDtmPostUnlinkWakeCell::new(),
        }
    }

    pub(crate) const fn wake(&self) -> &BluetoothDtmPostUnlinkWakeCell {
        &self.wake
    }

    pub(crate) fn prepare_arm(
        &self,
        critical_section: CriticalSection<'_>,
    ) -> Result<BluetoothDtmPostUnlinkArmKey, BluetoothDtmPostUnlinkArmError> {
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
        key: BluetoothDtmPostUnlinkArmKey,
    ) -> bool {
        self.state
            .borrow(critical_section)
            .borrow_mut()
            .commit_arm(key)
    }

    pub(crate) fn publish(
        &self,
        critical_section: CriticalSection<'_>,
        event: BluetoothPrimaryPublishedInterruptStep,
    ) -> BluetoothPrimarySerializedServiceStep {
        let ordinary = BluetoothPrimaryOrdinaryPublication::from_published(&event);
        match self
            .state
            .borrow(critical_section)
            .borrow_mut()
            .publish(event)
        {
            BluetoothDtmPostUnlinkMailboxPublish::General(event) => {
                BluetoothPrimarySerializedServiceStep::General {
                    published: event,
                    ordinary,
                }
            }
            BluetoothDtmPostUnlinkMailboxPublish::Stored => {
                BluetoothPrimarySerializedServiceStep::DtmStored {
                    mailbox: self.wake.publish_from_interrupt(),
                    ordinary,
                }
            }
            BluetoothDtmPostUnlinkMailboxPublish::Full(event) => {
                BluetoothPrimarySerializedServiceStep::MailboxFull {
                    published: event,
                    mailbox: self.wake.publish_from_interrupt(),
                    ordinary,
                }
            }
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn take<Role>(
        &self,
        critical_section: CriticalSection<'_>,
        awaiting: BluetoothDtmPostUnlinkAwaiting<Role>,
    ) -> BluetoothDtmPostUnlinkTake<Role> {
        match self
            .state
            .borrow(critical_section)
            .borrow_mut()
            .take(awaiting.key)
        {
            BluetoothDtmPostUnlinkMailboxTake::Recheck { key } => {
                BluetoothDtmPostUnlinkTake::Recheck {
                    key,
                    unlinked: awaiting.unlinked,
                }
            }
            BluetoothDtmPostUnlinkMailboxTake::Ready { key, event } => {
                let _ = self.wake.close();
                BluetoothDtmPostUnlinkTake::Ready {
                    key,
                    event: BluetoothDtmSoftwareListRemovalPublishedEvent {
                        unlinked: awaiting.unlinked,
                        published: event,
                    },
                }
            }
            BluetoothDtmPostUnlinkMailboxTake::AffinityMismatch => {
                BluetoothDtmPostUnlinkTake::AffinityMismatch(awaiting)
            }
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn rearm<Role>(
        &self,
        critical_section: CriticalSection<'_>,
        key: BluetoothDtmPostUnlinkArmKey,
        unlinked: crate::BluetoothDtmSchedulerSoftwareListUnlinked<Role>,
    ) -> BluetoothDtmPostUnlinkRearm<Role> {
        if self.state.borrow(critical_section).borrow_mut().rearm(key) {
            BluetoothDtmPostUnlinkRearm::Armed(BluetoothDtmPostUnlinkAwaiting { unlinked, key })
        } else {
            BluetoothDtmPostUnlinkRearm::AffinityMismatch(unlinked)
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn cancel<Role>(
        &self,
        critical_section: CriticalSection<'_>,
        awaiting: BluetoothDtmPostUnlinkAwaiting<Role>,
    ) -> BluetoothDtmPostUnlinkCancelStep<Role> {
        match self
            .state
            .borrow(critical_section)
            .borrow_mut()
            .cancel(awaiting.key)
        {
            BluetoothDtmPostUnlinkMailboxCancel::Cancelled => {
                BluetoothDtmPostUnlinkCancelStep::Cancelled(awaiting.unlinked)
            }
            BluetoothDtmPostUnlinkMailboxCancel::EventReady => {
                BluetoothDtmPostUnlinkCancelStep::EventReady(awaiting)
            }
            BluetoothDtmPostUnlinkMailboxCancel::AffinityMismatch => {
                BluetoothDtmPostUnlinkCancelStep::AffinityMismatch(awaiting)
            }
        }
    }
}

/// Already-unlinked DTM owner tied to one exact mailbox identity and arm generation.
#[cfg(target_arch = "riscv32")]
#[must_use = "the armed post-unlink owner must consume an event or cancel"]
pub struct BluetoothDtmPostUnlinkAwaiting<Role> {
    unlinked: crate::BluetoothDtmSchedulerSoftwareListUnlinked<Role>,
    key: BluetoothDtmPostUnlinkArmKey,
}

#[cfg(target_arch = "riscv32")]
impl<Role> BluetoothDtmPostUnlinkAwaiting<Role> {
    pub(crate) const fn new(
        unlinked: crate::BluetoothDtmSchedulerSoftwareListUnlinked<Role>,
        key: BluetoothDtmPostUnlinkArmKey,
    ) -> Self {
        Self { unlinked, key }
    }

    /// Exact arm generation retained for diagnostics.
    pub const fn generation(&self) -> u32 {
        self.key.generation
    }

    /// Role retained by the already-unlinked event.
    pub const fn role(&self) -> crate::BluetoothDtmRole {
        self.unlinked.role()
    }
}

/// Opaque pairing minted only by the matching armed mailbox.
#[cfg(target_arch = "riscv32")]
#[must_use = "the paired post-unlink event and graph must be consumed together"]
pub(crate) struct BluetoothDtmSoftwareListRemovalPublishedEvent<Role> {
    unlinked: crate::BluetoothDtmSchedulerSoftwareListUnlinked<Role>,
    published: BluetoothPrimaryPublishedInterruptStep,
}

#[cfg(target_arch = "riscv32")]
impl<Role> BluetoothDtmSoftwareListRemovalPublishedEvent<Role> {
    pub(crate) fn into_parts(
        self,
    ) -> (
        crate::BluetoothDtmSchedulerSoftwareListUnlinked<Role>,
        BluetoothPrimaryPublishedInterruptStep,
    ) {
        (self.unlinked, self.published)
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) enum BluetoothDtmPostUnlinkTake<Role> {
    Recheck {
        key: BluetoothDtmPostUnlinkArmKey,
        unlinked: crate::BluetoothDtmSchedulerSoftwareListUnlinked<Role>,
    },
    Ready {
        key: BluetoothDtmPostUnlinkArmKey,
        event: BluetoothDtmSoftwareListRemovalPublishedEvent<Role>,
    },
    AffinityMismatch(BluetoothDtmPostUnlinkAwaiting<Role>),
}

#[cfg(target_arch = "riscv32")]
pub(crate) enum BluetoothDtmPostUnlinkRearm<Role> {
    Armed(BluetoothDtmPostUnlinkAwaiting<Role>),
    AffinityMismatch(crate::BluetoothDtmSchedulerSoftwareListUnlinked<Role>),
}

/// Lossless result of cancelling one armed post-unlink wait.
#[cfg(target_arch = "riscv32")]
#[must_use = "retain the unlinked owner or consume the already-ready event"]
pub enum BluetoothDtmPostUnlinkCancelStep<Role> {
    Cancelled(crate::BluetoothDtmSchedulerSoftwareListUnlinked<Role>),
    EventReady(BluetoothDtmPostUnlinkAwaiting<Role>),
    AffinityMismatch(BluetoothDtmPostUnlinkAwaiting<Role>),
}

#[cfg(test)]
mod tests {
    use core::cell::Cell;

    use super::{
        BluetoothDtmPostUnlinkArmError, BluetoothDtmPostUnlinkArmKey,
        BluetoothDtmPostUnlinkMailboxCancel, BluetoothDtmPostUnlinkMailboxIdentity,
        BluetoothDtmPostUnlinkMailboxPublication, BluetoothDtmPostUnlinkMailboxPublish,
        BluetoothDtmPostUnlinkMailboxState, BluetoothDtmPostUnlinkMailboxTake,
        BluetoothDtmPostUnlinkWakeCell, allocate_mailbox_identity,
    };

    #[derive(Debug, Eq, PartialEq)]
    struct Event(u8);

    fn prepare<Event>(
        mailbox: &mut BluetoothDtmPostUnlinkMailboxState<Event>,
        next_identity: &Cell<u32>,
    ) -> BluetoothDtmPostUnlinkArmKey {
        mailbox
            .prepare_arm(|| allocate_mailbox_identity(next_identity))
            .expect("the bounded test identity and generation must be available")
    }

    #[test]
    fn pre_arm_event_remains_general_and_cannot_occupy_the_slot() {
        let mut mailbox = BluetoothDtmPostUnlinkMailboxState::new();
        let BluetoothDtmPostUnlinkMailboxPublish::General(Event(id)) = mailbox.publish(Event(7))
        else {
            panic!("idle publication must remain general");
        };
        assert_eq!(id, 7);
        let key = prepare(&mut mailbox, &Cell::new(0));
        assert_eq!(key.generation, 1);
    }

    #[test]
    fn one_armed_event_is_durable_and_the_next_is_returned_losslessly() {
        let mut mailbox = BluetoothDtmPostUnlinkMailboxState::new();
        let next_identity = Cell::new(0);
        let key = prepare(&mut mailbox, &next_identity);
        assert!(mailbox.commit_arm(key));
        assert!(matches!(
            mailbox.publish(Event(3)),
            BluetoothDtmPostUnlinkMailboxPublish::Stored
        ));
        let BluetoothDtmPostUnlinkMailboxPublish::Full(Event(id)) = mailbox.publish(Event(4))
        else {
            panic!("the occupied slot must return the newer event");
        };
        assert_eq!(id, 4);

        let BluetoothDtmPostUnlinkMailboxTake::Ready {
            key: observed_key,
            event,
        } = mailbox.take(key)
        else {
            panic!("the stored event must remain ready");
        };
        assert_eq!(observed_key, key);
        assert_eq!(event, Event(3));
    }

    #[test]
    fn direct_recheck_and_same_identity_generation_rearm_preserve_publication() {
        let mut mailbox = BluetoothDtmPostUnlinkMailboxState::<Event>::new();
        let next_identity = Cell::new(0);
        let key = prepare(&mut mailbox, &next_identity);
        assert!(mailbox.commit_arm(key));
        assert!(matches!(
            mailbox.take(key),
            BluetoothDtmPostUnlinkMailboxTake::Recheck { key: observed } if observed == key
        ));
        assert!(mailbox.rearm(key));
        assert!(matches!(
            mailbox.publish(Event(9)),
            BluetoothDtmPostUnlinkMailboxPublish::Stored
        ));
        let BluetoothDtmPostUnlinkMailboxTake::Ready { event, .. } = mailbox.take(key) else {
            panic!("publication after the recheck must remain durable");
        };
        assert_eq!(event, Event(9));
        assert!(mailbox.rearm(key));
        assert!(matches!(
            mailbox.take(key),
            BluetoothDtmPostUnlinkMailboxTake::Recheck { key: observed } if observed == key
        ));
    }

    #[test]
    fn generation_and_identity_exhaustion_reject_before_an_arm_exists() {
        let mut mailbox = BluetoothDtmPostUnlinkMailboxState::<Event>::new();
        let exhausted_identity = Cell::new(u32::MAX);
        assert_eq!(
            mailbox.prepare_arm(|| allocate_mailbox_identity(&exhausted_identity)),
            Err(BluetoothDtmPostUnlinkArmError::IdentityExhausted)
        );
        let BluetoothDtmPostUnlinkMailboxPublish::General(Event(id)) = mailbox.publish(Event(17))
        else {
            panic!("identity exhaustion must leave the mailbox unarmed");
        };
        assert_eq!(id, 17);

        let mut exhausted_generation = BluetoothDtmPostUnlinkMailboxState::<Event>::Idle {
            identity: BluetoothDtmPostUnlinkMailboxIdentity(4),
            generation: u32::MAX,
        };
        assert_eq!(
            exhausted_generation.prepare_arm(|| {
                panic!("an identified mailbox must not allocate another identity")
            }),
            Err(BluetoothDtmPostUnlinkArmError::GenerationExhausted)
        );
    }

    #[test]
    fn foreign_mailbox_identity_cannot_arm_take_cancel_or_rearm_exact_event() {
        let next_identity = Cell::new(0);
        let mut mailbox = BluetoothDtmPostUnlinkMailboxState::new();
        let mut foreign_mailbox = BluetoothDtmPostUnlinkMailboxState::<Event>::new();
        let key = prepare(&mut mailbox, &next_identity);
        let foreign_key = prepare(&mut foreign_mailbox, &next_identity);
        assert_eq!(key.generation, foreign_key.generation);
        assert_ne!(key.identity, foreign_key.identity);

        assert!(!mailbox.commit_arm(foreign_key));
        assert!(mailbox.commit_arm(key));
        assert!(matches!(
            mailbox.publish(Event(41)),
            BluetoothDtmPostUnlinkMailboxPublish::Stored
        ));
        assert_eq!(
            mailbox.cancel(foreign_key),
            BluetoothDtmPostUnlinkMailboxCancel::AffinityMismatch
        );
        assert!(matches!(
            mailbox.take(foreign_key),
            BluetoothDtmPostUnlinkMailboxTake::AffinityMismatch
        ));

        let BluetoothDtmPostUnlinkMailboxTake::Ready { event, .. } = mailbox.take(key) else {
            panic!("the exact mailbox identity must retain its own event");
        };
        assert_eq!(event, Event(41));
        assert!(!mailbox.rearm(foreign_key));
        assert!(mailbox.rearm(key));
    }

    #[test]
    fn cancellation_and_generation_checks_do_not_steal_state() {
        let mut mailbox = BluetoothDtmPostUnlinkMailboxState::<Event>::new();
        let next_identity = Cell::new(0);
        let key = prepare(&mut mailbox, &next_identity);
        assert!(mailbox.commit_arm(key));
        let later_key = BluetoothDtmPostUnlinkArmKey {
            identity: key.identity,
            generation: key.generation + 1,
        };
        assert_eq!(
            mailbox.cancel(later_key),
            BluetoothDtmPostUnlinkMailboxCancel::AffinityMismatch
        );
        assert_eq!(
            mailbox.cancel(key),
            BluetoothDtmPostUnlinkMailboxCancel::Cancelled
        );
        assert_eq!(prepare(&mut mailbox, &next_identity).generation, 2);
    }

    #[test]
    fn cancellation_cannot_discard_an_already_stored_event() {
        let mut mailbox = BluetoothDtmPostUnlinkMailboxState::new();
        let next_identity = Cell::new(0);
        let key = prepare(&mut mailbox, &next_identity);
        assert!(mailbox.commit_arm(key));
        assert!(matches!(
            mailbox.publish(Event(23)),
            BluetoothDtmPostUnlinkMailboxPublish::Stored
        ));
        assert_eq!(
            mailbox.cancel(key),
            BluetoothDtmPostUnlinkMailboxCancel::EventReady
        );

        let BluetoothDtmPostUnlinkMailboxTake::Ready { event, .. } = mailbox.take(key) else {
            panic!("cancellation must retain the exact stored event");
        };
        assert_eq!(event, Event(23));
    }

    #[test]
    fn ready_before_consumer_recheck_remains_durable_until_close() {
        let wake = BluetoothDtmPostUnlinkWakeCell::new();

        assert!(!wake.is_pending());
        assert_eq!(
            wake.publish_from_interrupt(),
            BluetoothDtmPostUnlinkMailboxPublication::WakeConsumer
        );
        assert!(wake.is_pending());
        assert!(wake.close());
        assert!(!wake.is_pending());
    }

    #[test]
    fn full_ready_publications_coalesce_and_next_epoch_wakes_again() {
        let wake = BluetoothDtmPostUnlinkWakeCell::new();

        assert_eq!(
            wake.publish_from_interrupt(),
            BluetoothDtmPostUnlinkMailboxPublication::WakeConsumer
        );
        assert_eq!(
            wake.publish_from_interrupt(),
            BluetoothDtmPostUnlinkMailboxPublication::AlreadyReady
        );
        assert!(wake.is_pending());
        assert!(wake.close());
        assert!(!wake.close());

        assert_eq!(
            wake.publish_from_interrupt(),
            BluetoothDtmPostUnlinkMailboxPublication::WakeConsumer
        );
        assert!(wake.is_pending());
    }
}
