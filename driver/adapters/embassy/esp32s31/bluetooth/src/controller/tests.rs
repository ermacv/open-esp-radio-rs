use core::{future::pending, pin::Pin, task::Context};

use std::boxed::Box;

use super::{
    ControllerCommandAction, ControllerCommandStimulus, ControllerOwnerSlot,
    EmbassyBluetoothControllerCommandPhase, reduce_controller_command_transition,
};

#[test]
fn reducer_closes_start_test_end_and_reset_paths_back_to_idle() {
    use ControllerCommandAction::{Advance, Retain, Terminal};
    use ControllerCommandStimulus::{
        Active, FirstEvent, IdleReset, IdleResponse, IdleRestored, LegacyAdvertisingActive,
        LegacyAdvertisingFirst, LegacyAdvertisingResponse, LegacyAdvertisingStopCompletion,
        LegacyConnectableAdvertisingActive, LegacyConnectableAdvertisingFirst,
        LegacyConnectableAdvertisingResponse, PassiveScanActive, PassiveScanFirst,
        PassiveScanResponse, PeripheralConnectionActive, PeripheralConnectionFirst,
        ResetCompletion, ResetResponse, ResetRestore, ResetStopping, UnownedFinishedList,
    };
    use EmbassyBluetoothControllerCommandPhase::{
        Active as ActivePhase, FirstEvent as FirstEventPhase, Idle, IdleReset as IdleResetPhase,
        IdleResponse as IdleResponsePhase, LegacyAdvertisingActive as LegacyAdvertisingActivePhase,
        LegacyAdvertisingFirst as LegacyAdvertisingFirstPhase,
        LegacyAdvertisingResponse as LegacyAdvertisingResponsePhase,
        LegacyAdvertisingStopCompletion as LegacyAdvertisingStopCompletionPhase,
        LegacyConnectableAdvertisingActive as LegacyConnectableAdvertisingActivePhase,
        LegacyConnectableAdvertisingFirst as LegacyConnectableAdvertisingFirstPhase,
        LegacyConnectableAdvertisingResponse as LegacyConnectableAdvertisingResponsePhase,
        PassiveScanActive as PassiveScanActivePhase, PassiveScanFirst as PassiveScanFirstPhase,
        PassiveScanResponse as PassiveScanResponsePhase,
        PeripheralConnectionActive as PeripheralConnectionActivePhase,
        PeripheralConnectionFirst as PeripheralConnectionFirstPhase,
        ResetCompletion as ResetCompletionPhase, ResetResponse as ResetResponsePhase,
        ResetRestore as ResetRestorePhase, ResetStopping as ResetStoppingPhase,
        UnownedFinishedList as UnownedFinishedListPhase,
    };

    assert_eq!(
        reduce_controller_command_transition(Idle, IdleResponse),
        Advance(IdleResponsePhase)
    );
    assert_eq!(
        reduce_controller_command_transition(IdleResponsePhase, IdleRestored),
        Advance(Idle)
    );
    assert_eq!(
        reduce_controller_command_transition(Idle, IdleReset),
        Advance(IdleResetPhase)
    );
    assert_eq!(
        reduce_controller_command_transition(IdleResetPhase, IdleResponse),
        Advance(IdleResponsePhase)
    );

    assert_eq!(
        reduce_controller_command_transition(Idle, FirstEvent),
        Advance(FirstEventPhase)
    );
    assert_eq!(
        reduce_controller_command_transition(Idle, Active),
        Advance(ActivePhase)
    );
    assert_eq!(
        reduce_controller_command_transition(FirstEventPhase, Active),
        Advance(ActivePhase)
    );
    assert_eq!(
        reduce_controller_command_transition(FirstEventPhase, IdleResponse),
        Advance(IdleResponsePhase)
    );
    assert_eq!(
        reduce_controller_command_transition(Idle, LegacyAdvertisingFirst),
        Advance(LegacyAdvertisingFirstPhase)
    );
    assert_eq!(
        reduce_controller_command_transition(
            LegacyAdvertisingFirstPhase,
            LegacyAdvertisingResponse,
        ),
        Advance(LegacyAdvertisingResponsePhase)
    );
    assert_eq!(
        reduce_controller_command_transition(
            LegacyAdvertisingResponsePhase,
            LegacyAdvertisingActive,
        ),
        Advance(LegacyAdvertisingActivePhase)
    );
    assert_eq!(
        reduce_controller_command_transition(LegacyAdvertisingFirstPhase, IdleResponse),
        Advance(IdleResponsePhase)
    );
    assert_eq!(
        reduce_controller_command_transition(Idle, LegacyConnectableAdvertisingFirst),
        Advance(LegacyConnectableAdvertisingFirstPhase)
    );
    assert_eq!(
        reduce_controller_command_transition(
            LegacyConnectableAdvertisingFirstPhase,
            LegacyConnectableAdvertisingResponse,
        ),
        Advance(LegacyConnectableAdvertisingResponsePhase)
    );
    assert_eq!(
        reduce_controller_command_transition(
            LegacyConnectableAdvertisingResponsePhase,
            LegacyConnectableAdvertisingActive,
        ),
        Advance(LegacyConnectableAdvertisingActivePhase)
    );
    assert_eq!(
        reduce_controller_command_transition(LegacyConnectableAdvertisingFirstPhase, IdleResponse,),
        Advance(IdleResponsePhase)
    );
    assert_eq!(
        reduce_controller_command_transition(
            LegacyConnectableAdvertisingActivePhase,
            ControllerCommandStimulus::Retain,
        ),
        Retain
    );
    assert_eq!(
        reduce_controller_command_transition(
            LegacyAdvertisingActivePhase,
            LegacyAdvertisingStopCompletion,
        ),
        Advance(LegacyAdvertisingStopCompletionPhase)
    );
    assert_eq!(
        reduce_controller_command_transition(
            LegacyConnectableAdvertisingActivePhase,
            LegacyAdvertisingStopCompletion,
        ),
        Advance(LegacyAdvertisingStopCompletionPhase)
    );
    assert_eq!(
        reduce_controller_command_transition(LegacyAdvertisingStopCompletionPhase, IdleRestored,),
        Advance(Idle)
    );
    assert_eq!(
        reduce_controller_command_transition(
            LegacyConnectableAdvertisingResponsePhase,
            PeripheralConnectionFirst,
        ),
        Advance(PeripheralConnectionFirstPhase)
    );
    assert_eq!(
        reduce_controller_command_transition(
            LegacyConnectableAdvertisingActivePhase,
            PeripheralConnectionFirst,
        ),
        Advance(PeripheralConnectionFirstPhase)
    );
    assert_eq!(
        reduce_controller_command_transition(
            PeripheralConnectionFirstPhase,
            PeripheralConnectionActive,
        ),
        Advance(PeripheralConnectionActivePhase)
    );
    assert_eq!(
        reduce_controller_command_transition(
            PeripheralConnectionActivePhase,
            ControllerCommandStimulus::Retain,
        ),
        Retain
    );
    assert_eq!(
        reduce_controller_command_transition(LegacyConnectableAdvertisingActivePhase, IdleReset,),
        Advance(IdleResetPhase)
    );
    assert_eq!(
        reduce_controller_command_transition(Idle, PassiveScanFirst),
        Advance(PassiveScanFirstPhase)
    );
    assert_eq!(
        reduce_controller_command_transition(PassiveScanFirstPhase, PassiveScanResponse),
        Advance(PassiveScanResponsePhase)
    );
    assert_eq!(
        reduce_controller_command_transition(PassiveScanResponsePhase, PassiveScanActive),
        Advance(PassiveScanActivePhase)
    );
    assert_eq!(
        reduce_controller_command_transition(PassiveScanActivePhase, IdleRestored),
        Advance(Idle)
    );
    assert_eq!(
        reduce_controller_command_transition(PassiveScanActivePhase, IdleResponse),
        Advance(IdleResponsePhase)
    );
    assert_eq!(
        reduce_controller_command_transition(PassiveScanActivePhase, IdleReset),
        Advance(IdleResetPhase)
    );
    assert_eq!(
        reduce_controller_command_transition(ActivePhase, IdleRestored),
        Advance(Idle)
    );
    assert_eq!(
        reduce_controller_command_transition(LegacyAdvertisingActivePhase, IdleRestored),
        Advance(Idle)
    );
    assert_eq!(
        reduce_controller_command_transition(ActivePhase, ResetStopping),
        Advance(ResetStoppingPhase)
    );
    assert_eq!(
        reduce_controller_command_transition(ActivePhase, UnownedFinishedList),
        Advance(UnownedFinishedListPhase)
    );
    assert_eq!(
        reduce_controller_command_transition(LegacyAdvertisingActivePhase, UnownedFinishedList,),
        Advance(UnownedFinishedListPhase)
    );
    assert_eq!(
        reduce_controller_command_transition(PassiveScanActivePhase, UnownedFinishedList),
        Advance(UnownedFinishedListPhase)
    );
    assert_eq!(
        reduce_controller_command_transition(ResetStoppingPhase, UnownedFinishedList),
        Advance(UnownedFinishedListPhase)
    );
    assert_eq!(
        reduce_controller_command_transition(UnownedFinishedListPhase, UnownedFinishedList),
        Retain
    );
    assert_eq!(
        reduce_controller_command_transition(ResetStoppingPhase, ResetRestore),
        Advance(ResetRestorePhase)
    );
    assert_eq!(
        reduce_controller_command_transition(ResetRestorePhase, ResetCompletion),
        Advance(ResetCompletionPhase)
    );
    assert_eq!(
        reduce_controller_command_transition(ResetCompletionPhase, ResetResponse),
        Advance(ResetResponsePhase)
    );
    assert_eq!(
        reduce_controller_command_transition(ResetResponsePhase, IdleRestored),
        Advance(Idle)
    );
    assert_eq!(
        reduce_controller_command_transition(FirstEventPhase, ControllerCommandStimulus::Terminal,),
        Terminal
    );
    assert_eq!(
        reduce_controller_command_transition(
            PassiveScanActivePhase,
            ControllerCommandStimulus::Terminal,
        ),
        Terminal
    );
}

#[test]
fn retained_observation_does_not_empty_or_replace_owner_slot() {
    let slot = ControllerOwnerSlot::new(37_u8);
    assert_eq!(*slot.current(), 37);
    assert!(!slot.is_empty());
    assert_eq!(
        reduce_controller_command_transition(
            EmbassyBluetoothControllerCommandPhase::FirstEvent,
            ControllerCommandStimulus::Retain,
        ),
        ControllerCommandAction::Retain
    );
    assert_eq!(
        reduce_controller_command_transition(
            EmbassyBluetoothControllerCommandPhase::Active,
            ControllerCommandStimulus::Retain,
        ),
        ControllerCommandAction::Retain
    );
}

#[test]
#[should_panic(expected = "invalid Controller command actor transition")]
fn response_backpressure_cannot_be_misclassified_as_terminal() {
    let _ = reduce_controller_command_transition(
        EmbassyBluetoothControllerCommandPhase::ResetResponse,
        ControllerCommandStimulus::Terminal,
    );
}

#[test]
#[should_panic(expected = "invalid Controller command actor transition")]
fn connectable_active_owner_cannot_be_fabricated_as_idle_completion() {
    let _ = reduce_controller_command_transition(
        EmbassyBluetoothControllerCommandPhase::LegacyConnectableAdvertisingActive,
        ControllerCommandStimulus::IdleRestored,
    );
}

#[test]
fn owner_slot_transfers_exactly_once() {
    let mut slot = ControllerOwnerSlot::new(41_u8);
    assert_eq!(slot.take(), 41);
    assert!(slot.is_empty());
    slot.store(43);
    assert_eq!(*slot.current_mut(), 43);
}

#[test]
fn cancelling_borrowed_wait_leaves_exact_actor_owner_in_slot() {
    async fn wait_forever(owner: &u8) {
        let _retained_owner = owner;
        pending::<()>().await;
    }

    let slot = ControllerOwnerSlot::new(47_u8);
    let mut future = Box::pin(wait_forever(slot.current()));
    let mut context = Context::from_waker(std::task::Waker::noop());
    assert!(Pin::as_mut(&mut future).poll(&mut context).is_pending());
    drop(future);

    assert_eq!(*slot.current(), 47);
    assert!(!slot.is_empty());
}
