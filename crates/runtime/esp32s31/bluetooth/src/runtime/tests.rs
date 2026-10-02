use core::cell::Cell;

use embassy_futures::{block_on, join::join};

use super::{CONTINUE_BUDGET, ContinueBudget};

#[test]
fn a_runner_that_always_continues_lets_a_sibling_run() {
    let sibling_ran = Cell::new(false);
    let passes = Cell::new(0_u32);
    let runner = async {
        let mut budget = ContinueBudget::new();
        // An endless run of continued passes ends only once the sibling
        // on the same executor has made progress.
        while !sibling_ran.get() {
            passes.set(passes.get() + 1);
            budget.spend().await;
        }
    };
    let sibling = async { sibling_ran.set(true) };
    block_on(join(runner, sibling));
    assert!(sibling_ran.get());
    assert!(passes.get() <= u32::from(CONTINUE_BUDGET) + 1);
}

#[test]
fn awaiting_refills_the_budget() {
    block_on(async {
        let mut budget = ContinueBudget::new();
        for _ in 1..CONTINUE_BUDGET {
            budget.spend().await;
        }
        budget.refill();
        assert_eq!(budget.0, CONTINUE_BUDGET);
    });
}

/// The outcome queue reports a loss once, after the outcomes queued before
/// it and before those after it.
#[test]
fn an_outcome_loss_is_reported_in_its_place() {
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use oer_bluetooth_radio::{ConnectionId, EventsLost};

    use super::OutcomeQueue;
    use crate::BluetoothOutcome;

    let queue = OutcomeQueue::<NoopRawMutex, 2>::new();
    let acknowledged = |id| BluetoothOutcome::TransmitAcknowledged(ConnectionId::new(id));
    for id in 0..4 {
        queue.push(acknowledged(id));
    }
    queue.push(acknowledged(9));
    assert_eq!(queue.take(), Some(Ok(acknowledged(0))));
    assert_eq!(queue.take(), Some(Ok(acknowledged(1))));
    assert_eq!(queue.take(), Some(Err(EventsLost)));
    assert_eq!(queue.take(), None);
    queue.push(acknowledged(5));
    assert_eq!(queue.take(), Some(Ok(acknowledged(5))));
}

#[test]
fn not_installed_is_rejected_and_a_fault_poisons() {
    use oer_bluetooth_radio::{FailureClass, PortError, RequestError};

    use super::BluetoothRuntimeError;

    assert_eq!(
        BluetoothRuntimeError::NotInstalled.class(),
        FailureClass::Rejected
    );
    assert_eq!(
        BluetoothRuntimeError::Rejected(RequestError::Busy).class(),
        FailureClass::Rejected
    );
    assert!(BluetoothRuntimeError::Faulted.is_poisoned());
}
