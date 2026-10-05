use std::vec::Vec;

use super::*;

const PEER: [u8; 6] = [0x02, 0, 0, 0, 0, 2];
const OTHER: [u8; 6] = [0x02, 0, 0, 0, 0, 3];

/// A QoS data MPDU of `sequence`, its body `body`.
fn mpdu(sequence: u16, body: u8) -> Vec<u8> {
    let mut frame = std::vec![0x88, 0x01];
    frame.extend_from_slice(&[0; 20]);
    frame.extend_from_slice(&(sequence << 4).to_le_bytes());
    frame.extend_from_slice(&[0, 0, body]);
    frame
}

fn sequence(value: u16) -> SequenceNumber {
    SequenceNumber::new(value).unwrap()
}

/// The bodies a release delivers: the current MPDU's `current`, or a
/// stored copy's.
fn bodies<const A: usize>(
    reorder: &mut RxReorder<A>,
    release: &ReorderRelease,
    current: u8,
) -> Vec<u8> {
    release
        .iter()
        .map(|mpdu| {
            if mpdu.slot == CURRENT_SLOT {
                current
            } else {
                *reorder.take(mpdu.slot).unwrap().bytes().last().unwrap()
            }
        })
        .collect()
}

#[test]
fn an_in_order_mpdu_is_released_at_once_and_kept_ones_when_the_gap_closes() {
    let mut reorder = RxReorder::<2>::new();
    assert!(matches!(
        reorder.offer(PEER, 0, &mpdu(1, 1), true),
        Offer::NoAgreement
    ));
    assert!(reorder.accept(PEER, 0, sequence(1), 8));
    assert!(!reorder.accept(PEER, 0, sequence(1), 8));

    let Offer::Released {
        release,
        current: true,
    } = reorder.offer(PEER, 0, &mpdu(1, 1), true)
    else {
        panic!("in order");
    };
    assert_eq!(bodies(&mut reorder, &release, 1), [1]);
    // 3 and 4 wait for 2, in storage.
    for (number, body) in [(3, 3), (4, 4)] {
        let Offer::Released {
            release,
            current: false,
        } = reorder.offer(PEER, 0, &mpdu(number, body), true)
        else {
            panic!("kept");
        };
        assert_eq!(release.iter().count(), 0);
    }
    assert!(matches!(
        reorder.offer(PEER, 0, &mpdu(3, 3), true),
        Offer::Duplicate
    ));
    // 2 is the next expected: it goes from the port's buffer, the kept run
    // after it.
    let Offer::Released {
        release,
        current: true,
    } = reorder.offer(PEER, 0, &mpdu(2, 2), true)
    else {
        panic!("the gap closes");
    };
    assert_eq!(bodies(&mut reorder, &release, 2), [2, 3, 4]);
    assert!(matches!(
        reorder.offer(PEER, 0, &mpdu(1, 1), true),
        Offer::Behind | Offer::Duplicate
    ));
    // Another peer's agreement of the same TID is its own.
    assert!(reorder.accept(OTHER, 0, sequence(100), 8));
    assert!(
        !reorder.accept(OTHER, 1, sequence(100), 8),
        "two agreements at most"
    );
    assert_eq!(reorder.agreements(PEER).collect::<Vec<_>>(), [0]);
}

#[test]
fn a_full_storage_makes_room_once_and_a_gap_expires_after_its_time() {
    let mut reorder = RxReorder::<1>::new();
    assert!(reorder.accept(PEER, 0, sequence(0), 64));
    // Fill every slot behind the missing 0.
    for number in 1..=PORT_REORDER_SLOTS as u16 {
        assert!(matches!(
            reorder.offer(PEER, 0, &mpdu(number, number as u8), true),
            Offer::Released { current: false, .. }
        ));
    }
    let Offer::MakeRoom(release) = reorder.offer(PEER, 0, &mpdu(20, 20), true) else {
        panic!("full");
    };
    assert_eq!(bodies(&mut reorder, &release, 0).len(), PORT_REORDER_SLOTS);
    assert!(matches!(
        reorder.offer(PEER, 0, &mpdu(30, 30), true),
        Offer::Released { current: false, .. }
    ));

    // A kept MPDU starts the gap time; past it the run goes.
    let now = Instant::from_micros(1_000);
    reorder.arm_gaps(now, Duration::from_millis(300));
    assert_eq!(
        reorder.next_gap_deadline(),
        Some(Instant::from_micros(301_000))
    );
    assert!(
        reorder
            .expire_due_gap(Instant::from_micros(300_999))
            .is_none()
    );
    let (peer, release) = reorder
        .expire_due_gap(Instant::from_micros(301_000))
        .unwrap();
    assert_eq!(peer, PEER);
    assert_eq!(bodies(&mut reorder, &release, 0), [30]);
    reorder.arm_gaps(Instant::from_micros(301_000), Duration::from_millis(300));
    assert_eq!(reorder.next_gap_deadline(), None);
}

#[test]
fn a_stopped_agreement_drops_what_it_kept() {
    let mut reorder = RxReorder::<1>::new();
    assert!(reorder.accept(PEER, 5, sequence(10), 16));
    assert!(matches!(
        reorder.offer(PEER, 5, &mpdu(12, 12), true),
        Offer::Released { current: false, .. }
    ));
    assert!(reorder.stop(PEER, 5));
    assert!(!reorder.stop(PEER, 5));
    assert!(!reorder.is_active(PEER, 5));
    assert!((0..PORT_REORDER_SLOTS as u8).all(|slot| reorder.take(slot).is_none()));
    // A BlockAckReq of an agreement that does not run moves nothing.
    assert!(reorder.move_window(PEER, 5, sequence(20)).is_none());
}
