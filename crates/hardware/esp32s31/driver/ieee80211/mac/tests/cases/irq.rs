use crate::{support::*, *};

#[test]
fn irq_state_coalesces_named_work_and_records_unhandled_causes() {
    let mut mmio = MockMmio {
        interrupt_status: MacInterruptObservation::from_semantic_events(
            MacInterruptEvents::TX_COMPLETE.union(MacInterruptEvents::RX_SUCCESS),
            true,
            true,
        ),
        ..MockMmio::default()
    };
    let state = IrqState::new();
    let (disposition, snapshot) = handle_mac_irq(&mut mmio, &state);

    assert_eq!(disposition, IrqDisposition::Posted);
    assert!(snapshot.had_auxiliary_event);
    assert!(snapshot.had_unhandled_event);
    assert!(state.observed_unhandled());
    let event = state.try_take().unwrap();
    assert_eq!(event.events, EVENT_TX_COMPLETE | EVENT_RX_SUCCESS);
    assert_eq!(mmio.operations().last(), Some(&Operation::Fence));
    assert!(mmio.operations().contains(&Operation::AcknowledgeInterrupt));
}

#[test]
fn irq_acknowledges_auxiliary_status_without_posting_independent_work() {
    let mut mmio = MockMmio {
        interrupt_status: MacInterruptObservation::from_semantic_events(
            MacInterruptEvents::empty(),
            true,
            false,
        ),
        ..MockMmio::default()
    };
    let state = IrqState::new();
    let (disposition, snapshot) = handle_mac_irq(&mut mmio, &state);

    assert_eq!(disposition, IrqDisposition::AcknowledgedOnly);
    assert_eq!(snapshot.posted_events, 0);
    assert!(snapshot.had_auxiliary_event);
    assert!(!snapshot.had_unhandled_event);
    assert_eq!(state.try_take(), None);
    assert!(!state.observed_unhandled());
    assert!(mmio.operations().contains(&Operation::AcknowledgeInterrupt));
}

#[test]
fn irq_state_exposes_vendor_run_to_completion_order() {
    let mut mmio = MockMmio {
        interrupt_status: MacInterruptObservation::from_semantic_events(
            MacInterruptEvents::COLLISION
                .union(MacInterruptEvents::TX_TIMEOUT)
                .union(MacInterruptEvents::TX_COMPLETE)
                .union(MacInterruptEvents::RX_SUCCESS),
            false,
            false,
        ),
        ..MockMmio::default()
    };
    let state = IrqState::new();
    assert_eq!(handle_mac_irq(&mut mmio, &state).0, IrqDisposition::Posted);

    assert_eq!(state.try_take_next(), Some(IrqWork::RxSuccess));
    assert_eq!(state.try_take_next(), Some(IrqWork::TxComplete));
    assert_eq!(state.try_take_next(), Some(IrqWork::TxTimeout));
    assert_eq!(state.try_take_next(), Some(IrqWork::Collision));
    assert_eq!(state.try_take_next(), None);
}
