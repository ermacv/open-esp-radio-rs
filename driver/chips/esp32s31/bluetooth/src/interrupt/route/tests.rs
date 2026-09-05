use super::{
    BluetoothCpuInterruptRoutePolicy, BluetoothCpuInterruptSource,
    BluetoothInterruptHandlerResidency,
};

#[test]
fn primary_route_is_source_124_level_three_and_iram() {
    let policy = BluetoothCpuInterruptRoutePolicy::PRIMARY;
    assert_eq!(policy.source(), BluetoothCpuInterruptSource::PrimaryBtMac);
    assert_eq!(policy.source().number(), 124);
    assert_eq!(policy.priority_level(), 3);
    assert_eq!(
        policy.residency(),
        BluetoothInterruptHandlerResidency::IramRequired
    );
    assert!(policy.pinned_to_controller_core());
}

#[test]
fn nrt_route_is_distinct_source_133_without_iram_request() {
    let policy = BluetoothCpuInterruptRoutePolicy::NRT;
    assert_eq!(policy.source(), BluetoothCpuInterruptSource::NrtBtMacInt1);
    assert_eq!(policy.source().number(), 133);
    assert_eq!(policy.priority_level(), 3);
    assert_eq!(
        policy.residency(),
        BluetoothInterruptHandlerResidency::IramNotRequested
    );
    assert!(policy.pinned_to_controller_core());
}

#[test]
fn modem_lp_timer_route_is_source_127_level_three_and_iram() {
    let policy = BluetoothCpuInterruptRoutePolicy::MODEM_LP_TIMER;
    assert_eq!(policy.source(), BluetoothCpuInterruptSource::ModemLpTimer);
    assert_eq!(policy.source().number(), 127);
    assert_eq!(policy.priority_level(), 3);
    assert_eq!(
        policy.residency(),
        BluetoothInterruptHandlerResidency::IramRequired
    );
    assert!(policy.pinned_to_controller_core());
}
