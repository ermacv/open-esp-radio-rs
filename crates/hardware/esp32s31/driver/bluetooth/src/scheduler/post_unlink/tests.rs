use core::cell::Cell;

use super::{
    DtmPostUnlinkArmError, DtmPostUnlinkArmKey, DtmPostUnlinkMailboxIdentity,
    DtmPostUnlinkMailboxPublication, DtmPostUnlinkMailboxPublish, DtmPostUnlinkMailboxState,
    DtmPostUnlinkMailboxTake, DtmPostUnlinkWakeCell, allocate_mailbox_identity,
};

#[derive(Debug, Eq, PartialEq)]
struct Event(u8);

fn prepare<Event>(
    mailbox: &mut DtmPostUnlinkMailboxState<Event>,
    next_identity: &Cell<u32>,
) -> DtmPostUnlinkArmKey {
    mailbox
        .prepare_arm(|| allocate_mailbox_identity(next_identity))
        .expect("the bounded test identity and generation must be available")
}

#[test]
fn pre_arm_event_remains_general_and_cannot_occupy_the_slot() {
    let mut mailbox = DtmPostUnlinkMailboxState::new();
    let DtmPostUnlinkMailboxPublish::General(Event(id)) = mailbox.publish(Event(7)) else {
        panic!("idle publication must remain general");
    };
    assert_eq!(id, 7);
    let key = prepare(&mut mailbox, &Cell::new(0));
    assert_eq!(key.generation, 1);
}

#[test]
fn full_slot_preserves_old_event_then_exposes_direct_recheck() {
    let mut mailbox = DtmPostUnlinkMailboxState::new();
    let next_identity = Cell::new(0);
    let key = prepare(&mut mailbox, &next_identity);
    assert!(mailbox.commit_arm(key));
    assert!(matches!(
        mailbox.publish(Event(3)),
        DtmPostUnlinkMailboxPublish::Stored
    ));
    let DtmPostUnlinkMailboxPublish::Full(Event(id)) = mailbox.publish(Event(4)) else {
        panic!("the occupied slot must return the newer event");
    };
    assert_eq!(id, 4);

    let DtmPostUnlinkMailboxTake::Ready {
        key: observed_key,
        event,
    } = mailbox.take(key)
    else {
        panic!("the stored event must remain ready");
    };
    assert_eq!(observed_key, key);
    assert_eq!(event, Event(3));
    assert!(mailbox.rearm(key));
    assert!(matches!(
        mailbox.take(key),
        DtmPostUnlinkMailboxTake::Recheck { key: observed } if observed == key
    ));
}

#[test]
fn direct_recheck_and_same_identity_generation_rearm_preserve_publication() {
    let mut mailbox = DtmPostUnlinkMailboxState::<Event>::new();
    let next_identity = Cell::new(0);
    let key = prepare(&mut mailbox, &next_identity);
    assert!(mailbox.commit_arm(key));
    assert!(matches!(
        mailbox.take(key),
        DtmPostUnlinkMailboxTake::Recheck { key: observed } if observed == key
    ));
    assert!(mailbox.rearm(key));
    assert!(matches!(
        mailbox.publish(Event(9)),
        DtmPostUnlinkMailboxPublish::Stored
    ));
    let DtmPostUnlinkMailboxTake::Ready { event, .. } = mailbox.take(key) else {
        panic!("publication after the recheck must remain durable");
    };
    assert_eq!(event, Event(9));
    assert!(mailbox.rearm(key));
    assert!(matches!(
        mailbox.take(key),
        DtmPostUnlinkMailboxTake::Recheck { key: observed } if observed == key
    ));
}

#[test]
fn generation_and_identity_exhaustion_reject_before_an_arm_exists() {
    let mut mailbox = DtmPostUnlinkMailboxState::<Event>::new();
    let exhausted_identity = Cell::new(u32::MAX);
    assert_eq!(
        mailbox.prepare_arm(|| allocate_mailbox_identity(&exhausted_identity)),
        Err(DtmPostUnlinkArmError::IdentityExhausted)
    );
    let DtmPostUnlinkMailboxPublish::General(Event(id)) = mailbox.publish(Event(17)) else {
        panic!("identity exhaustion must leave the mailbox unarmed");
    };
    assert_eq!(id, 17);

    let mut exhausted_generation = DtmPostUnlinkMailboxState::<Event>::Idle {
        identity: DtmPostUnlinkMailboxIdentity(4),
        generation: u32::MAX,
    };
    assert_eq!(
        exhausted_generation
            .prepare_arm(|| { panic!("an identified mailbox must not allocate another identity") }),
        Err(DtmPostUnlinkArmError::GenerationExhausted)
    );
}

#[test]
fn foreign_mailbox_identity_cannot_arm_take_or_rearm_exact_event() {
    let next_identity = Cell::new(0);
    let mut mailbox = DtmPostUnlinkMailboxState::new();
    let mut foreign_mailbox = DtmPostUnlinkMailboxState::<Event>::new();
    let key = prepare(&mut mailbox, &next_identity);
    let foreign_key = prepare(&mut foreign_mailbox, &next_identity);
    assert_eq!(key.generation, foreign_key.generation);
    assert_ne!(key.identity, foreign_key.identity);

    assert!(!mailbox.commit_arm(foreign_key));
    assert!(mailbox.commit_arm(key));
    assert!(matches!(
        mailbox.publish(Event(41)),
        DtmPostUnlinkMailboxPublish::Stored
    ));
    assert!(matches!(
        mailbox.take(foreign_key),
        DtmPostUnlinkMailboxTake::AffinityMismatch
    ));

    let DtmPostUnlinkMailboxTake::Ready { event, .. } = mailbox.take(key) else {
        panic!("the exact mailbox identity must retain its own event");
    };
    assert_eq!(event, Event(41));
    assert!(!mailbox.rearm(foreign_key));
    assert!(mailbox.rearm(key));
}

#[test]
fn future_generation_take_does_not_steal_the_current_arm() {
    let mut mailbox = DtmPostUnlinkMailboxState::<Event>::new();
    let next_identity = Cell::new(0);
    let key = prepare(&mut mailbox, &next_identity);
    assert!(mailbox.commit_arm(key));
    let later_key = DtmPostUnlinkArmKey {
        identity: key.identity,
        generation: key.generation + 1,
    };
    assert!(matches!(
        mailbox.take(later_key),
        DtmPostUnlinkMailboxTake::AffinityMismatch
    ));
    assert!(matches!(
        mailbox.take(key),
        DtmPostUnlinkMailboxTake::Recheck { key: observed } if observed == key
    ));
    assert_eq!(prepare(&mut mailbox, &next_identity).generation, 2);
}

#[test]
fn ready_before_consumer_recheck_remains_durable_until_close() {
    let wake = DtmPostUnlinkWakeCell::new();

    assert!(!wake.is_pending());
    assert_eq!(
        wake.publish_from_interrupt(),
        DtmPostUnlinkMailboxPublication::WakeConsumer
    );
    assert!(wake.is_pending());
    assert!(wake.close());
    assert!(!wake.is_pending());
}

#[test]
fn full_ready_publications_coalesce_and_next_epoch_wakes_again() {
    let wake = DtmPostUnlinkWakeCell::new();

    assert_eq!(
        wake.publish_from_interrupt(),
        DtmPostUnlinkMailboxPublication::WakeConsumer
    );
    assert_eq!(
        wake.publish_from_interrupt(),
        DtmPostUnlinkMailboxPublication::AlreadyReady
    );
    assert!(wake.is_pending());
    assert!(wake.close());
    assert!(!wake.close());

    assert_eq!(
        wake.publish_from_interrupt(),
        DtmPostUnlinkMailboxPublication::WakeConsumer
    );
    assert!(wake.is_pending());
}
