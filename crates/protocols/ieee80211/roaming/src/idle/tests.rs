use super::*;
fn link() -> LinkIdentity {
    LinkIdentity {
        peer: [2, 0, 0, 0, 0, 1],
        generation: 1,
    }
}
fn at(us: u64) -> Instant {
    Instant::from_micros(us)
}
fn options() -> BssMaxIdle {
    BssMaxIdle {
        period: 1,
        options: 1,
    }
}
#[test]
fn only_protected_station_activity_extends_a_protected_ap_lease() {
    let mut owner = BssIdleAccessPoint::new(link(), options(), at(0)).unwrap();
    assert_eq!(
        owner.activity(link(), false, at(500_000)).unwrap(),
        IdleEvent::Ignored
    );
    assert_eq!(owner.next_deadline(), Some(at(1_024_000)));
    assert_eq!(
        owner.activity(link(), true, at(500_000)).unwrap(),
        IdleEvent::ActivityAccepted
    );
    assert_eq!(owner.next_deadline(), Some(at(1_524_000)));
    assert_eq!(
        owner.poll(at(1_524_000)).unwrap(),
        IdleEvent::Expired { link: link() }
    );
    assert_eq!(owner.poll(at(1_524_001)).unwrap(), IdleEvent::Ignored);
    assert!(owner.next_deadline().is_none());
}
#[test]
fn queued_failed_and_late_keep_alive_do_not_extend_the_idle_deadline() {
    let mut owner = BssIdleStation::new(
        link(),
        options(),
        true,
        Duration::from_micros(100_000),
        at(0),
    )
    .unwrap();
    assert_eq!(owner.poll(at(923_999)).unwrap(), IdleEvent::Ignored);
    let IdleEvent::KeepAlive {
        id,
        protected: true,
        until,
    } = owner.poll(at(924_000)).unwrap()
    else {
        panic!()
    };
    assert_eq!(until, at(1_024_000));
    assert_eq!(
        owner.tx_completed(id, TxOutcome::Acknowledged, at(924_000)),
        Err(IdleError::Protocol(Error::NotAdmitted))
    );
    owner.admitted(id, at(924_000)).unwrap();
    assert_eq!(
        owner
            .tx_completed(id, TxOutcome::Failed, at(924_001))
            .unwrap(),
        IdleEvent::KeepAliveFailed { id }
    );
    let IdleEvent::KeepAlive { id: next, .. } = owner.poll(at(924_002)).unwrap() else {
        panic!()
    };
    assert_ne!(id, next);
    owner.admitted(next, at(924_002)).unwrap();
    assert_eq!(
        owner
            .tx_completed(id, TxOutcome::Acknowledged, at(924_003))
            .unwrap(),
        IdleEvent::Ignored
    );
    assert_eq!(
        owner
            .tx_completed(next, TxOutcome::Acknowledged, at(1_024_000))
            .unwrap(),
        IdleEvent::Expired { link: link() }
    );
}
#[test]
fn acknowledged_keep_alive_and_other_traffic_reset_the_deadline_once() {
    let mut owner = BssIdleStation::new(
        link(),
        options(),
        true,
        Duration::from_micros(100_000),
        at(0),
    )
    .unwrap();
    let IdleEvent::KeepAlive { id, .. } = owner.poll(at(924_000)).unwrap() else {
        panic!()
    };
    owner.admitted(id, at(924_000)).unwrap();
    assert_eq!(
        owner
            .tx_completed(id, TxOutcome::Acknowledged, at(950_000))
            .unwrap(),
        IdleEvent::ActivityAccepted
    );
    assert_eq!(owner.next_deadline(), Some(at(1_874_000)));
    assert_eq!(
        owner
            .activity(
                LinkIdentity {
                    generation: 0,
                    ..link()
                },
                true,
                at(1_000_000)
            )
            .unwrap(),
        IdleEvent::Ignored
    );
    assert_eq!(
        owner.activity(link(), true, at(1_000_000)).unwrap(),
        IdleEvent::ActivityAccepted
    );
    assert_eq!(owner.next_deadline(), Some(at(1_924_000)));
}
#[test]
fn invalid_protection_margin_time_and_overflow_are_explicit() {
    assert!(matches!(
        BssIdleStation::new(link(), options(), false, Duration::from_micros(1), at(0)),
        Err(IdleError::ProtectionUnavailable)
    ));
    assert!(matches!(
        BssIdleStation::new(link(), options(), true, Duration::from_secs(2), at(0)),
        Err(IdleError::InvalidMargin)
    ));
    assert!(matches!(
        BssIdleAccessPoint::new(link(), options(), at(u64::MAX)),
        Err(IdleError::Protocol(Error::TimeOverflow))
    ));
    let mut owner = BssIdleAccessPoint::new(link(), options(), at(10)).unwrap();
    assert_eq!(
        owner.activity(link(), true, at(9)),
        Err(IdleError::Protocol(Error::TimeBeforeOperation))
    );
    assert_eq!(owner.next_deadline(), Some(at(1_024_010)));
}
