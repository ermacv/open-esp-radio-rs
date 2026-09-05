use open_esp_radio_esp32s31_hal::Radio;

use super::*;
use crate::{PhyConfig, state::client::DEFAULT_PLL_TRACK_PERIOD_MICROS};

#[derive(Debug)]
struct TestPlatform;

struct FixedClock(u64);

impl PhyPllTrackClock for FixedClock {
    fn now_micros(&mut self) -> u64 {
        self.0
    }
}

fn registered_radio() -> RegisteredPhyRadio<TestPlatform> {
    let radio = Radio::claim_for_validation(TestPlatform);
    let radio = radio.assume_powered_for_validation();
    RegisteredPhyRadio {
        radio,
        phy: RegisteredPhyState::from_wrapper_test_model(PhyState::new(PhyConfig::production())),
        clients: PhyClientState::for_registered_epoch(DEFAULT_PLL_TRACK_PERIOD_MICROS),
    }
}

fn settle_acquire(
    owner: RegisteredPhyRadio<TestPlatform>,
    client: PhyModemClient,
    now_micros: u64,
) -> RegisteredPhyRadio<TestPlatform> {
    let acquired = match owner.acquire_client(client, &mut FixedClock(now_micros)) {
        Ok(acquired) => acquired,
        Err(_) => panic!("fresh acquisition must succeed"),
    };
    match acquired.into_owner() {
        Ok(owner) => owner,
        Err(_) => panic!("fresh timestamp must not request tracking"),
    }
}

#[test]
fn registered_client_acquire_release_never_separates_radio_and_phy_state() {
    let owner = registered_radio();
    assert!(owner.client_snapshot().is_empty());

    let owner = settle_acquire(owner, PhyModemClient::Ieee802154, 0);
    assert!(owner.client_snapshot().contains(PhyModemClient::Ieee802154));

    let failure = match owner.acquire_client(PhyModemClient::Ieee802154, &mut FixedClock(0)) {
        Ok(_) => panic!("duplicate client acquisition must fail"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        PhyClientAcquireError::AlreadyAcquired(PhyModemClient::Ieee802154)
    );
    let owner = failure.into_owner();

    let released = match owner.release_client(PhyModemClient::Ieee802154) {
        Ok(released) => released,
        Err(_) => panic!("owned client must release"),
    };
    assert!(released.is_last());
    assert!(released.into_owner().client_snapshot().is_empty());
}

#[test]
fn periodic_request_retains_complete_registered_epoch_until_poison_or_success() {
    let owner = settle_acquire(registered_radio(), PhyModemClient::Ieee802154, 0);
    let evaluation = match owner.evaluate_periodic_tracking(&mut FixedClock(1)) {
        Ok(evaluation) => evaluation,
        Err(_) => panic!("monotonic callback must evaluate"),
    };
    let pending = match evaluation.into_owner() {
        Ok(_) => panic!("an active periodic class must retain the owner"),
        Err(pending) => pending,
    };
    assert!(!pending.request().wifi());
    assert!(pending.request().bluetooth_ieee802154());
    assert!(
        pending
            .client_snapshot()
            .contains(PhyModemClient::Ieee802154)
    );

    let tracking = pending.begin_tracking();
    assert_eq!(tracking.action(), PhyParamTrackingAction::EnterCritical);
    let poisoned = tracking.fail();
    assert!(poisoned.request().bluetooth_ieee802154());
    assert!(
        poisoned
            .client_snapshot()
            .contains(PhyModemClient::Ieee802154)
    );
}
