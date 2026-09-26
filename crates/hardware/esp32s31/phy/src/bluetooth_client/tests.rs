use oer_esp32s31_hal::{
    bluetooth::{ClockedOwner, TaskOwner},
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

fn task(partitions: ConcurrentPartitions) -> TaskOwner {
    let (task, _interrupts) =
        ClockedOwner::for_validation(partitions.bluetooth).separate_interrupt_owner();
    task
}

fn bluetooth_is_client(lease: &SharedRadioLease<'_, ConcurrentPhy>) -> bool {
    lease
        .attachment()
        .client_snapshot()
        .is_some_and(|snapshot| snapshot.contains(PhyModemClient::Bluetooth))
}

#[test]
fn joining_an_unregistered_domain_changes_nothing() {
    let (radio, partitions) = RadioHardware::for_validation().into_concurrent(ConcurrentPhy::new());
    let task = task(partitions);
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));

    assert_eq!(
        join_bluetooth(&mut lease, &task, &mut Clock(0)).map(|(_, acquired)| acquired),
        Err(BluetoothPhyClientError::Phy(
            ConcurrentPhyError::NotRegistered
        ))
    );
    assert!(!lease.holds_btbb(RadioClient::Bluetooth));
}

#[test]
fn a_stale_registration_is_rejected_before_the_client_set_changes() {
    let (radio, partitions) = arbiter(true);
    let task = task(partitions);
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));

    assert_eq!(
        join_bluetooth(&mut lease, &task, &mut Clock(0)).map(|(_, acquired)| acquired),
        Err(BluetoothPhyClientError::StaleRegistration)
    );
    assert!(!bluetooth_is_client(&lease));
    assert!(!lease.holds_btbb(RadioClient::Bluetooth));
}

#[test]
fn leaving_without_the_btbb_reference_keeps_the_membership_and_the_client() {
    let (radio, partitions) = arbiter(false);
    let task = task(partitions);
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
    assert_eq!(
        acquire_client(&mut lease, PhyModemClient::Bluetooth, &mut Clock(0)),
        Ok(ConcurrentAcquire::Settled)
    );

    let failure = match leave_bluetooth(&mut lease, &task, BluetoothPhyMembership::for_test()) {
        Ok(_) => panic!("a membership without its BTBB reference cannot leave"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        BluetoothPhyClientError::Btbb(BtbbError::NotAcquired)
    );
    let _membership = failure.into_membership();
    assert!(bluetooth_is_client(&lease));
}
