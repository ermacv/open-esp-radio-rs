use super::{
    BLUETOOTH_APB_CLOCKS, BLUETOOTH_CONTROLLER_CLOCKS, BluetoothClockBaseline,
    BluetoothClockTransition, BluetoothModemSysconClockState, BluetoothPhysicalClock,
    ModemSysconBluetoothClock, UnbalancedBluetoothClockRelease,
};

#[test]
fn overlapping_apb_release_keeps_controller_dependencies_retained() {
    let mut state = BluetoothModemSysconClockState::new();
    for clock in BLUETOOTH_CONTROLLER_CLOCKS {
        assert_eq!(
            state.retain(clock, Some(BluetoothClockBaseline::default())),
            BluetoothClockTransition::Enable
        );
    }
    for clock in BLUETOOTH_APB_CLOCKS {
        assert_eq!(
            state.retain(clock, None),
            BluetoothClockTransition::NoChange
        );
    }
    for clock in BLUETOOTH_APB_CLOCKS {
        assert_eq!(state.release(clock), Ok(BluetoothClockTransition::NoChange));
    }
    for clock in BLUETOOTH_CONTROLLER_CLOCKS {
        assert_eq!(
            state.release(clock),
            Ok(BluetoothClockTransition::Restore(
                BluetoothClockBaseline::default()
            ))
        );
    }
}

#[test]
fn preexisting_clock_is_restored_instead_of_disabled() {
    let mut state = BluetoothModemSysconClockState::new();
    let clock = ModemSysconBluetoothClock::BluetoothMac;
    let mut baseline = BluetoothClockBaseline::default();
    baseline.record(BluetoothPhysicalClock::BluetoothMac, true);
    assert_eq!(
        state.retain(clock, Some(baseline)),
        BluetoothClockTransition::NoChange
    );
    assert_eq!(state.release(clock), Ok(BluetoothClockTransition::NoChange));
}

#[test]
fn unbalanced_release_is_rejected_without_mutating_the_epoch() {
    let mut state = BluetoothModemSysconClockState::new();
    let clock = ModemSysconBluetoothClock::Etm;
    assert_eq!(state.release(clock), Err(UnbalancedBluetoothClockRelease));
    assert_eq!(
        state.retain(clock, Some(BluetoothClockBaseline::default())),
        BluetoothClockTransition::Enable
    );
}

#[test]
fn partial_logical_group_baseline_is_restored_exactly() {
    let mut state = BluetoothModemSysconClockState::new();
    let clock = ModemSysconBluetoothClock::BluetoothPeripheral;
    let mut baseline = BluetoothClockBaseline::default();
    baseline.record(BluetoothPhysicalClock::ModemSecurity, true);
    baseline.record(BluetoothPhysicalClock::ModemSecurityCcm, true);
    baseline.record(BluetoothPhysicalClock::BleTimer, true);
    assert_eq!(
        state.retain(clock, Some(baseline)),
        BluetoothClockTransition::Enable
    );
    assert_eq!(
        state.release(clock),
        Ok(BluetoothClockTransition::Restore(baseline))
    );
}
