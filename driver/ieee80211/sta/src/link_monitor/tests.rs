use super::*;

const BEACON: StaBeaconObservation = StaBeaconObservation {
    timestamp_tsf: 10,
    interval_tu: 100,
    capability_information: 0,
    tim: None,
};

#[test]
fn exact_deadline_is_lost_but_an_observation_refreshes_the_window() {
    let config = StaBeaconLossConfig::new(100, 3).unwrap();
    let mut monitor = StaBeaconMonitor::new(config);
    monitor.arm(1_000).unwrap();
    assert_eq!(monitor.deadline_micros(), Some(308_200));
    assert!(!monitor.expired(308_199));

    monitor.observe(308_200, BEACON).unwrap();
    assert!(!monitor.expired(308_200));
    assert_eq!(monitor.deadline_micros(), Some(615_400));
    assert_eq!(monitor.observed(), 1);
    assert_eq!(monitor.last_observation(), Some(BEACON));
    assert!(monitor.expired(615_400));
}

#[test]
fn construction_rejects_unbounded_or_vacuous_policy() {
    assert_eq!(
        StaBeaconLossConfig::new(0, 3),
        Err(StaBeaconLossConfigError::ZeroInterval)
    );
    assert_eq!(
        StaBeaconLossConfig::new(100, 0),
        Err(StaBeaconLossConfigError::ZeroMissLimit)
    );
}

#[test]
fn active_reachability_refresh_does_not_fabricate_a_beacon() {
    let config = StaBeaconLossConfig::new(100, 3).unwrap();
    let mut monitor = StaBeaconMonitor::new(config);
    monitor.arm(1_000).unwrap();
    monitor.observe_reachability(308_200).unwrap();

    assert_eq!(monitor.deadline_micros(), Some(615_400));
    assert_eq!(monitor.observed(), 0);
    assert_eq!(monitor.last_observation(), None);
}
