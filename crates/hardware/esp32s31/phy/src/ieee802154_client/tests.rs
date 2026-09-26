use oer_esp32s31_hal::{
    ieee802154::Ieee802154Clocked,
    owner::PhyInitializationAccess,
    root::{ConcurrentPartitions, RadioHardware},
    shared_radio::{BtbbError, RadioClient, SharedRadio},
};

use super::*;
use crate::{
    PhyConfig, PhyState, RegisteredPhyState,
    concurrent::ConcurrentPhy,
    domain::PhyDomain,
    state::client::{DEFAULT_PLL_TRACK_PERIOD_MICROS, PhyClientState},
};

struct Clock(u64);

impl PhyPllTrackClock for Clock {
    fn now_micros(&mut self) -> u64 {
        self.0
    }
}

/// An arbiter whose domain is registered in the arbiter's current epoch, or,
/// when `stale`, in an epoch a later registration retired.
fn arbiter(stale: bool) -> (SharedRadio<ConcurrentPhy>, ConcurrentPartitions) {
    let (radio, partitions) = RadioHardware::for_validation().into_concurrent(ConcurrentPhy::new());
    {
        let mut lease = radio
            .try_acquire()
            .unwrap_or_else(|_| panic!("a fresh arbiter grants its lease"));
        let epoch = lease.phy_hal().begin_registration_epoch();
        if stale {
            let _current = lease.phy_hal().begin_registration_epoch();
        }
        *lease.attachment_mut() = ConcurrentPhy::registered_for_test(PhyDomain::new(
            RegisteredPhyState::from_wrapper_test_model(PhyState::new(PhyConfig::production())),
            PhyClientState::for_registration(DEFAULT_PLL_TRACK_PERIOD_MICROS, epoch),
        ));
    }
    (radio, partitions)
}

fn ieee802154_is_client(lease: &SharedRadioLease<'_, ConcurrentPhy>) -> bool {
    lease
        .attachment()
        .client_snapshot()
        .is_some_and(|snapshot| snapshot.contains(PhyModemClient::Ieee802154))
}

#[test]
fn joining_an_unregistered_domain_changes_nothing() {
    let (radio, partitions) = RadioHardware::for_validation().into_concurrent(ConcurrentPhy::new());
    let clocked = Ieee802154Clocked::for_validation(partitions.ieee802154);
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));

    assert_eq!(
        join_ieee802154(&mut lease, &clocked, &mut Clock(0)).map(|(_, acquired)| acquired),
        Err(Ieee802154PhyClientError::Phy(
            ConcurrentPhyError::NotRegistered
        ))
    );
    assert!(!lease.holds_btbb(RadioClient::Ieee802154));
}

#[test]
fn a_stale_registration_is_rejected_before_the_client_set_changes() {
    let (radio, partitions) = arbiter(true);
    let clocked = Ieee802154Clocked::for_validation(partitions.ieee802154);
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));

    assert_eq!(
        join_ieee802154(&mut lease, &clocked, &mut Clock(0)).map(|(_, acquired)| acquired),
        Err(Ieee802154PhyClientError::StaleRegistration)
    );
    assert!(!ieee802154_is_client(&lease));
    assert!(!lease.holds_btbb(RadioClient::Ieee802154));
}

#[test]
fn leaving_without_the_btbb_reference_keeps_the_membership_and_the_client() {
    let (radio, partitions) = arbiter(false);
    let clocked = Ieee802154Clocked::for_validation(partitions.ieee802154);
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
    assert_eq!(
        acquire_client(&mut lease, PhyModemClient::Ieee802154, &mut Clock(0)),
        Ok(ConcurrentAcquire::Settled)
    );

    let failure = match leave_ieee802154(&mut lease, &clocked, Ieee802154PhyMembership::for_test())
    {
        Ok(_) => panic!("a membership without its BTBB reference cannot leave"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        Ieee802154PhyClientError::Btbb(BtbbError::NotAcquired)
    );
    let _membership = failure.into_membership();
    assert!(ieee802154_is_client(&lease));
}

/// An arbiter as [`arbiter`] whose IEEE 802.15.4 client holds its BTBB
/// reference and client bit, as after a join.
fn joined() -> (SharedRadio<ConcurrentPhy>, ConcurrentPartitions) {
    let (mut radio, partitions) = arbiter(false);
    radio.hold_btbb_for_validation(RadioClient::Ieee802154);
    {
        let mut lease = radio
            .try_acquire()
            .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
        assert_eq!(
            acquire_client(&mut lease, PhyModemClient::Ieee802154, &mut Clock(0)),
            Ok(ConcurrentAcquire::Settled)
        );
    }
    (radio, partitions)
}

#[test]
fn sleep_drops_the_client_bit_and_keeps_the_btbb_reference() {
    let (radio, _partitions) = joined();
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));

    let (suspended, last) = suspend_ieee802154(&mut lease, Ieee802154PhyMembership::for_test())
        .unwrap_or_else(|_| panic!("a joined client can sleep"));
    assert!(last, "the only client was the last one");
    assert!(!ieee802154_is_client(&lease));
    assert!(lease.holds_btbb(RadioClient::Ieee802154));

    let (_membership, acquired) = resume_ieee802154(&mut lease, suspended, &mut Clock(0))
        .unwrap_or_else(|_| panic!("a settled domain wakes the client"));
    assert_eq!(acquired, ConcurrentAcquire::Settled);
    assert!(ieee802154_is_client(&lease));
    assert!(lease.holds_btbb(RadioClient::Ieee802154));
}

#[test]
fn sleep_without_the_btbb_reference_keeps_the_membership_and_the_client() {
    let (radio, _partitions) = arbiter(false);
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
    assert_eq!(
        acquire_client(&mut lease, PhyModemClient::Ieee802154, &mut Clock(0)),
        Ok(ConcurrentAcquire::Settled)
    );

    let failure = match suspend_ieee802154(&mut lease, Ieee802154PhyMembership::for_test()) {
        Ok(_) => panic!("a membership without its BTBB reference cannot sleep"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        Ieee802154PhyClientError::Btbb(BtbbError::NotAcquired)
    );
    let _membership = failure.into_membership();
    assert!(ieee802154_is_client(&lease));
}

#[test]
fn waking_a_closed_domain_is_rejected_before_the_client_set_changes() {
    let (mut radio, _partitions) =
        RadioHardware::for_validation().into_concurrent(ConcurrentPhy::new());
    radio.hold_btbb_for_validation(RadioClient::Ieee802154);
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
    let epoch = lease.phy_hal().begin_registration_epoch();
    *lease.attachment_mut() = ConcurrentPhy::rf_closed_for_test(PhyDomain::new(
        RegisteredPhyState::from_wrapper_test_model(PhyState::new(PhyConfig::production())),
        PhyClientState::for_registration(DEFAULT_PLL_TRACK_PERIOD_MICROS, epoch),
    ));

    let failure = match resume_ieee802154(
        &mut lease,
        Ieee802154PhySuspended::for_test(),
        &mut Clock(0),
    ) {
        Ok(_) => panic!("closed RF must be woken by the radio system first"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        Ieee802154PhyClientError::Phy(ConcurrentPhyError::RfClosed)
    );
    let _suspended = failure.into_suspended();
    assert!(!ieee802154_is_client(&lease));
}

#[test]
fn a_stale_registration_keeps_the_client_asleep() {
    let (mut radio, _partitions) = arbiter(true);
    radio.hold_btbb_for_validation(RadioClient::Ieee802154);
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));

    let failure = match resume_ieee802154(
        &mut lease,
        Ieee802154PhySuspended::for_test(),
        &mut Clock(0),
    ) {
        Ok(_) => panic!("a retired registration cannot admit the client"),
        Err(failure) => failure,
    };
    assert_eq!(failure.error(), Ieee802154PhyClientError::StaleRegistration);
    assert!(!ieee802154_is_client(&lease));
}
