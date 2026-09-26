use oer_esp32s31_hal::{
    ieee80211::client::{WifiClocked, WifiCold},
    owner::PhyInitializationAccess,
    root::RadioHardware,
    shared_radio::SharedRadio,
};

use super::*;
use crate::{
    PhyConfig, PhyState, RegisteredPhyState,
    domain::PhyDomain,
    state::client::{DEFAULT_PLL_TRACK_PERIOD_MICROS, PhyClientState},
};

struct Clock(u64);

impl PhyPllTrackClock for Clock {
    fn now_micros(&mut self) -> u64 {
        self.0
    }
}

fn registered() -> (SharedRadio<ConcurrentPhy>, WifiClocked) {
    let (radio, partitions) = RadioHardware::for_validation().into_concurrent(ConcurrentPhy::new());
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
    (
        radio,
        WifiClocked::for_validation(WifiCold::from_partition(partitions.wifi)),
    )
}

#[test]
fn wifi_joins_and_leaves_the_shared_domain_as_its_last_client() {
    let (radio, clocked) = registered();
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
    let (membership, acquired) = join_wifi(&mut lease, &clocked, &mut Clock(0))
        .unwrap_or_else(|error| panic!("join failed: {error:?}"));
    assert_eq!(acquired, ConcurrentAcquire::Settled);
    assert!(
        lease
            .attachment()
            .client_snapshot()
            .is_some_and(|clients| clients.contains(PhyModemClient::Wifi))
    );
    // A second membership would double the client bit.
    assert!(matches!(
        join_wifi(&mut lease, &clocked, &mut Clock(0)),
        Err(ConcurrentPhyError::Acquire(_))
    ));
    assert_eq!(
        leave_wifi(&mut lease, &clocked, membership).ok(),
        Some(true)
    );
}

#[test]
fn an_unregistered_domain_admits_no_wifi_client() {
    let (radio, partitions) = RadioHardware::for_validation().into_concurrent(ConcurrentPhy::new());
    let clocked = WifiClocked::for_validation(WifiCold::from_partition(partitions.wifi));
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
    assert!(matches!(
        join_wifi(&mut lease, &clocked, &mut Clock(0)),
        Err(ConcurrentPhyError::NotRegistered)
    ));
}
