use super::RegisteredBluetoothPhy;
use crate::{
    PhyConfig, PhyState, RegisteredPhyState,
    state::client::{
        DEFAULT_PLL_TRACK_PERIOD_MICROS, PhyClientState, PhyModemClient, PhyPllTrackClock,
    },
};

struct FixedClock(u64);

impl PhyPllTrackClock for FixedClock {
    fn now_micros(&mut self) -> u64 {
        self.0
    }
}

fn registered_phy() -> RegisteredBluetoothPhy {
    RegisteredBluetoothPhy {
        registered: RegisteredPhyState::from_wrapper_test_model(PhyState::new(
            PhyConfig::production(),
        )),
        clients: PhyClientState::for_registered_epoch(DEFAULT_PLL_TRACK_PERIOD_MICROS),
    }
}

#[test]
fn bluetooth_client_acquisition_retains_registration_and_exact_role() {
    let acquisition = registered_phy()
        .acquire_phy_client(&mut FixedClock(0))
        .unwrap_or_else(|_| panic!("fresh Bluetooth client acquisition must succeed"));
    assert!(acquisition.request().is_none());

    let owner = acquisition
        .into_owner()
        .unwrap_or_else(|_| panic!("fresh timestamp must not require tracking"));
    assert!(owner.phy_state().phy_registered());
    assert!(owner.client_snapshot().contains(PhyModemClient::Bluetooth));
    assert!(!owner.client_snapshot().contains(PhyModemClient::Wifi));
    assert!(!owner.client_snapshot().contains(PhyModemClient::Ieee802154));
}

#[test]
fn due_initial_tracking_cannot_be_skipped_or_recovered_after_fail_stop() {
    let acquisition = registered_phy()
        .acquire_phy_client(&mut FixedClock(
            DEFAULT_PLL_TRACK_PERIOD_MICROS.saturating_add(1),
        ))
        .unwrap_or_else(|_| panic!("Bluetooth client acquisition must succeed"));
    assert!(acquisition.request().is_some());

    let pending = match acquisition.into_owner() {
        Ok(_) => panic!("due tracking must retain pending ownership"),
        Err(pending) => pending,
    };
    let poisoned = pending.fail();
    assert!(poisoned.phy_state().phy_registered());
    assert!(
        poisoned
            .client_snapshot()
            .contains(PhyModemClient::Bluetooth)
    );
}

#[test]
fn due_initial_tracking_uses_registered_epoch_policy() {
    let acquisition = registered_phy()
        .acquire_phy_client(&mut FixedClock(
            DEFAULT_PLL_TRACK_PERIOD_MICROS.saturating_add(1),
        ))
        .unwrap_or_else(|_| panic!("Bluetooth client acquisition must succeed"));
    let pending = match acquisition.into_owner() {
        Ok(_) => panic!("due tracking must retain the registered epoch"),
        Err(pending) => pending,
    };
    let mut tracking = pending.begin_tracking();

    assert_eq!(
        tracking.action(),
        crate::tracking::parameters::PhyParamTrackingAction::EnterCritical
    );
    tracking
        .pending
        .advance(crate::tracking::parameters::PhyParamTrackingCompletion::EnteredCritical)
        .unwrap();
    assert_eq!(
        tracking.action(),
        crate::tracking::parameters::PhyParamTrackingAction::BluetoothIeee802154TxPowerTrack {
            enabled: true,
            diagnostics: crate::tracking::parameters::PhyTrackingDiagnostics::Disabled,
        }
    );
}
