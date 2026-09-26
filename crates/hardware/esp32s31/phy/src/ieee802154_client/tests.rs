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
    registered_route::PhyDomain,
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
