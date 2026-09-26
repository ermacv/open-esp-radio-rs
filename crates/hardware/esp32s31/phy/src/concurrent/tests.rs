use oer_esp32s31_hal::{
    owner::PhyInitializationAccess,
    root::RadioHardware,
    shared_radio::{ClientQuiescence, QuiescentSpan, RadioClient, SharedRadio},
};

use super::*;
use crate::{
    PhyConfig, PhyState,
    state::client::{DEFAULT_PLL_TRACK_PERIOD_MICROS, PhyClientState},
};

struct Clock(u64);

impl PhyPllTrackClock for Clock {
    fn now_micros(&mut self) -> u64 {
        self.0
    }
}

/// An arbiter whose domain is registered in the arbiter's own epoch.
fn registered_arbiter() -> SharedRadio<ConcurrentPhy> {
    let (radio, _partitions) =
        RadioHardware::for_validation().into_concurrent(ConcurrentPhy::new());
    {
        let mut lease = radio
            .try_acquire()
            .unwrap_or_else(|_| panic!("a fresh arbiter grants its lease"));
        let epoch = lease.phy_hal().begin_registration_epoch();
        *lease.attachment_mut() = ConcurrentPhy::registered_for_test(PhyDomain::new(
            RegisteredPhyState::from_wrapper_test_model(PhyState::new(PhyConfig::production())),
            PhyClientState::for_registration(DEFAULT_PLL_TRACK_PERIOD_MICROS, epoch),
        ));
    }
    radio
}

/// An arbiter whose registered domain has RF closed.
fn rf_closed_arbiter() -> SharedRadio<ConcurrentPhy> {
    let (radio, _partitions) =
        RadioHardware::for_validation().into_concurrent(ConcurrentPhy::new());
    {
        let mut lease = radio
            .try_acquire()
            .unwrap_or_else(|_| panic!("a fresh arbiter grants its lease"));
        let epoch = lease.phy_hal().begin_registration_epoch();
        *lease.attachment_mut() = ConcurrentPhy::rf_closed_for_test(PhyDomain::new(
            RegisteredPhyState::from_wrapper_test_model(PhyState::new(PhyConfig::production())),
            PhyClientState::for_registration(DEFAULT_PLL_TRACK_PERIOD_MICROS, epoch),
        ));
    }
    radio
}

fn until(client: RadioClient, issued_at: u64, release_by: u64) -> ClientQuiescence<'static> {
    ClientQuiescence::for_validation(
        client,
        QuiescentSpan::Until {
            issued_at_micros: issued_at,
            release_by_micros: release_by,
        },
    )
}

#[test]
fn clients_share_one_domain_and_leave_it_one_by_one() {
    let radio = registered_arbiter();
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
    assert_eq!(
        acquire_client(&mut lease, PhyModemClient::Wifi, &mut Clock(0)),
        Ok(ConcurrentAcquire::Settled)
    );
    assert_eq!(
        acquire_client(&mut lease, PhyModemClient::Bluetooth, &mut Clock(0)),
        Ok(ConcurrentAcquire::Settled)
    );
    let snapshot = lease
        .attachment()
        .client_snapshot()
        .unwrap_or_else(|| panic!("a registered domain has a client set"));
    assert!(snapshot.contains(PhyModemClient::Wifi));
    assert!(snapshot.contains(PhyModemClient::Bluetooth));
    assert!(matches!(
        acquire_client(&mut lease, PhyModemClient::Wifi, &mut Clock(0)),
        Err(ConcurrentPhyError::Acquire(_))
    ));

    assert_eq!(release_client(&mut lease, PhyModemClient::Wifi), Ok(false));
    assert_eq!(
        release_client(&mut lease, PhyModemClient::Bluetooth),
        Ok(true)
    );
    assert!(matches!(
        release_client(&mut lease, PhyModemClient::Bluetooth),
        Err(ConcurrentPhyError::Release(_))
    ));
}

#[test]
fn an_unregistered_domain_rejects_every_client_operation() {
    let (radio, _partitions) =
        RadioHardware::for_validation().into_concurrent(ConcurrentPhy::new());
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a fresh arbiter grants its lease"));
    assert_eq!(
        acquire_client(&mut lease, PhyModemClient::Wifi, &mut Clock(0)),
        Err(ConcurrentPhyError::NotRegistered)
    );
    assert_eq!(
        evaluate_periodic_tracking(&mut lease, &mut Clock(0)),
        Err(ConcurrentPhyError::NotRegistered)
    );
    assert_eq!(
        admit_maintenance(&lease, &[], 0),
        Err(ConcurrentPhyError::NotRegistered)
    );
}

#[test]
fn due_tracking_blocks_clients_until_maintenance_is_admitted() {
    let radio = registered_arbiter();
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
    assert_eq!(
        acquire_client(&mut lease, PhyModemClient::Wifi, &mut Clock(0)),
        Ok(ConcurrentAcquire::Settled)
    );
    assert_eq!(
        admit_maintenance(&lease, &[], 0),
        Err(ConcurrentPhyError::NoTrackingPending)
    );
    // One period later the periodic callback requests tracking.
    assert_eq!(
        evaluate_periodic_tracking(&mut lease, &mut Clock(DEFAULT_PLL_TRACK_PERIOD_MICROS)),
        Ok(true)
    );
    assert!(lease.attachment().tracking_pending());
    assert_eq!(
        acquire_client(&mut lease, PhyModemClient::Bluetooth, &mut Clock(0)),
        Err(ConcurrentPhyError::TrackingPending)
    );
    assert_eq!(
        admit_maintenance(&lease, &[], 5),
        Err(ConcurrentPhyError::MissingQuiescence(PhyModemClient::Wifi))
    );
    assert_eq!(
        admit_maintenance(&lease, &[until(RadioClient::Wifi, 1, 100)], 5)
            .map(MaintenanceAdmission::release_by_micros),
        Ok(Some(100))
    );
}

#[test]
fn admission_takes_the_earliest_window_and_ignores_inactive_clients() {
    let mut clients = PhyClientState::without_registration(DEFAULT_PLL_TRACK_PERIOD_MICROS);
    for client in [PhyModemClient::Wifi, PhyModemClient::Bluetooth] {
        clients = clients
            .acquire(client, &mut Clock(0))
            .unwrap_or_else(|_| panic!("acquisition"))
            .into_owner()
            .unwrap_or_else(|_| panic!("no tracking at time zero"));
    }
    let active = clients.snapshot();
    let stopped =
        || ClientQuiescence::for_validation(RadioClient::Bluetooth, QuiescentSpan::Stopped);

    assert_eq!(
        admit(active, &[until(RadioClient::Wifi, 1, 100), stopped()], 10)
            .map(MaintenanceAdmission::release_by_micros),
        Ok(Some(100))
    );
    // An inactive IEEE 802.15.4 proof neither satisfies nor narrows admission.
    assert_eq!(
        admit(
            active,
            &[
                until(RadioClient::Wifi, 1, 100),
                until(RadioClient::Bluetooth, 1, 60),
                until(RadioClient::Ieee802154, 1, 20),
            ],
            10
        )
        .map(MaintenanceAdmission::release_by_micros),
        Ok(Some(60))
    );
    assert_eq!(
        admit(active, &[until(RadioClient::Wifi, 1, 100)], 10),
        Err(ConcurrentPhyError::MissingQuiescence(
            PhyModemClient::Bluetooth
        ))
    );
    assert_eq!(
        admit(active, &[until(RadioClient::Wifi, 20, 100), stopped()], 10),
        Err(ConcurrentPhyError::ClockBehindProof)
    );
    assert_eq!(
        admit(active, &[until(RadioClient::Wifi, 1, 10), stopped()], 10),
        Err(ConcurrentPhyError::WindowClosed)
    );
    let wifi_stopped = ClientQuiescence::for_validation(RadioClient::Wifi, QuiescentSpan::Stopped);
    assert_eq!(
        admit(active, &[wifi_stopped, stopped()], 10).map(MaintenanceAdmission::release_by_micros),
        Ok(None)
    );
}

#[test]
fn a_closed_domain_admits_no_client_until_rf_wakes() {
    let radio = rf_closed_arbiter();
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
    assert!(lease.attachment().rf_closed());
    assert_eq!(
        lease
            .attachment()
            .client_snapshot()
            .map(|set| set.is_empty()),
        Some(true)
    );
    assert_eq!(
        acquire_client(&mut lease, PhyModemClient::Ieee802154, &mut Clock(0)),
        Err(ConcurrentPhyError::RfClosed)
    );
    assert_eq!(
        evaluate_periodic_tracking(&mut lease, &mut Clock(0)),
        Err(ConcurrentPhyError::RfClosed)
    );
    assert_eq!(
        admit_maintenance(&lease, &[], 0),
        Err(ConcurrentPhyError::RfClosed)
    );
    // RF close takes only an open domain; the closed one stays closed.
    assert!(matches!(
        lease.attachment_mut().idle_domain(),
        Err(ConcurrentPhyError::RfClosed)
    ));
    assert!(lease.attachment().rf_closed());
    assert!(lease.attachment_mut().closed_domain().is_ok());
}

#[test]
fn rf_closes_only_after_the_last_client_left() {
    let radio = registered_arbiter();
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
    assert!(matches!(
        lease.attachment_mut().closed_domain(),
        Err(ConcurrentPhyError::RfOpen)
    ));
    assert_eq!(
        acquire_client(&mut lease, PhyModemClient::Bluetooth, &mut Clock(0)),
        Ok(ConcurrentAcquire::Settled)
    );
    assert!(matches!(
        lease.attachment_mut().idle_domain(),
        Err(ConcurrentPhyError::ClientsActive)
    ));
    // The rejected close leaves the client in the open domain.
    assert_eq!(
        release_client(&mut lease, PhyModemClient::Bluetooth),
        Ok(true)
    );
    assert!(lease.attachment_mut().idle_domain().is_ok());
}
