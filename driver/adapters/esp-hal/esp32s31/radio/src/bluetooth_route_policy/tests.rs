use super::{
    BluetoothInterruptRouteState, BluetoothModemLpTimerInterruptAdmission,
    BluetoothModemLpTimerStoragePhase, BluetoothModemLpTimerTaskTakeAdmission,
    EspHalBluetoothInterruptRouteError, EspHalBluetoothInterruptStorageError,
    classify_modem_lp_timer_interrupt, classify_modem_lp_timer_task_take,
    ready_owner_restore_is_admitted, service_stable_owner, validate_interrupt_storage,
};

#[test]
fn route_epoch_binds_once_and_dispatches_only_while_live() {
    let mut state = BluetoothInterruptRouteState::from_bound_core(None);
    assert!(!state.dispatch_is_live());
    assert_eq!(state.bind(0_u8), Ok(()));
    assert!(state.dispatch_is_live());
    assert_eq!(
        state.bind(0),
        Err(EspHalBluetoothInterruptRouteError::AlreadyBound)
    );
}

#[test]
fn route_epoch_disable_is_same_core_and_consumes_live_dispatch() {
    let mut state = BluetoothInterruptRouteState::inactive();
    assert_eq!(state.bind(1_u8), Ok(()));
    assert_eq!(
        state.disable(0),
        Err(EspHalBluetoothInterruptRouteError::WrongCore)
    );
    assert!(state.dispatch_is_live());
    assert_eq!(state.disable(1), Ok(()));
    assert!(!state.dispatch_is_live());
    assert_eq!(
        state.disable(1),
        Err(EspHalBluetoothInterruptRouteError::Inactive)
    );
}

#[test]
fn both_isr_owner_slots_publish_or_reject_as_one_invariant() {
    assert_eq!(validate_interrupt_storage(false, false), Ok(()));
    assert_eq!(
        validate_interrupt_storage(true, true),
        Err(EspHalBluetoothInterruptStorageError::AlreadyPublished)
    );
    assert_eq!(
        validate_interrupt_storage(true, false),
        Err(EspHalBluetoothInterruptStorageError::StorageInvariant)
    );
    assert_eq!(
        validate_interrupt_storage(false, true),
        Err(EspHalBluetoothInterruptStorageError::StorageInvariant)
    );
}

#[test]
fn source_127_register_entry_stops_while_software_owns_the_timer() {
    assert_eq!(
        classify_modem_lp_timer_interrupt(BluetoothModemLpTimerStoragePhase::Missing),
        BluetoothModemLpTimerInterruptAdmission::Unavailable
    );
    assert_eq!(
        classify_modem_lp_timer_interrupt(BluetoothModemLpTimerStoragePhase::Ready),
        BluetoothModemLpTimerInterruptAdmission::ServiceRegisters
    );
    assert_eq!(
        classify_modem_lp_timer_interrupt(BluetoothModemLpTimerStoragePhase::SoftwarePending),
        BluetoothModemLpTimerInterruptAdmission::AwaitSoftware
    );
}

#[test]
fn source_127_task_can_take_only_pending_work_and_restore_only_into_the_empty_slot() {
    assert_eq!(
        classify_modem_lp_timer_task_take(BluetoothModemLpTimerStoragePhase::Missing),
        BluetoothModemLpTimerTaskTakeAdmission::Missing
    );
    assert_eq!(
        classify_modem_lp_timer_task_take(BluetoothModemLpTimerStoragePhase::Ready),
        BluetoothModemLpTimerTaskTakeAdmission::NotSoftwarePending
    );
    assert_eq!(
        classify_modem_lp_timer_task_take(BluetoothModemLpTimerStoragePhase::SoftwarePending),
        BluetoothModemLpTimerTaskTakeAdmission::Acquire
    );
    assert!(ready_owner_restore_is_admitted(false));
    assert!(!ready_owner_restore_is_admitted(true));
}

#[test]
fn shared_interrupt_service_retains_and_reuses_the_stable_owner() {
    let mut slot = Some(0_u8);

    assert_eq!(
        service_stable_owner(&mut slot, |owner| {
            *owner += 1;
            *owner
        }),
        Some(1)
    );
    assert_eq!(
        service_stable_owner(&mut slot, |owner| {
            *owner += 1;
            *owner
        }),
        Some(2)
    );
    assert_eq!(slot, Some(2));

    let mut missing: Option<u8> = None;
    assert_eq!(
        service_stable_owner(&mut missing, |_| panic!(
            "missing owner must not be serviced"
        )),
        None
    );
}
