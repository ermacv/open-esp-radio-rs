use core::{
    future::Future,
    task::{Context, Poll, Waker},
};

use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use open_esp_radio_esp32s31_ieee802154_irq::{
    Ieee802154Event, Ieee802154EventMask, Ieee802154EventObservationError,
    acknowledged_interrupt_for_validation,
};
use open_esp_radio_esp32s31_ieee802154_mac::{MacNoDmaResources, MacReady};
use open_esp_radio_esp32s31_ieee802154_runtime::{MacRuntime, ValidationMacCommandExecutor};

use super::*;

fn publish<M: RawMutex, const DEPTH: usize>(
    irq: &EmbassyIeee802154IrqRuntime<M, DEPTH>,
    events: Ieee802154EventMask,
    ed_rss: i8,
) -> Result<(), Ieee802154AcknowledgedInterrupt> {
    irq.post(acknowledged_interrupt_for_validation(
        Ok(events),
        None,
        None,
        ed_rss,
        false,
    ))
}

fn publish_unclassified<M: RawMutex, const DEPTH: usize>(
    irq: &EmbassyIeee802154IrqRuntime<M, DEPTH>,
) -> Result<(), Ieee802154AcknowledgedInterrupt> {
    irq.post(acknowledged_interrupt_for_validation(
        Err(Ieee802154EventObservationError),
        None,
        None,
        0,
        false,
    ))
}

fn active_cca() -> MacRuntimeActive<MacNoDmaResources, ValidationMacCommandExecutor> {
    MacRuntime::for_validation()
        .start(MacReady::new().request_clear_channel_assessment())
        .unwrap()
}

#[test]
fn acknowledged_values_cross_the_async_handoff_in_order() {
    let irq = EmbassyIeee802154IrqRuntime::<NoopRawMutex, 2>::new();
    publish(&irq, Ieee802154Event::TxSfdDone.mask(), -20).unwrap();
    publish(&irq, Ieee802154Event::TxDone.mask(), -21).unwrap();

    let first = block_on(irq.wait()).unwrap();
    let second = block_on(irq.wait()).unwrap();
    assert_eq!(
        first.event_classification(),
        Ok(Ieee802154Event::TxSfdDone.mask())
    );
    assert_eq!(first.ed_rss_code(), -20);
    assert_eq!(
        second.event_classification(),
        Ok(Ieee802154Event::TxDone.mask())
    );
    assert_eq!(second.ed_rss_code(), -21);
}

#[test]
fn overflow_is_a_fail_closed_operation_error() {
    let irq = EmbassyIeee802154IrqRuntime::<NoopRawMutex, 1>::new();
    publish(&irq, Ieee802154Event::TxSfdDone.mask(), 0).unwrap();
    let rejected = publish(&irq, Ieee802154Event::TxDone.mask(), 0)
        .expect_err("the full handoff returns the exact rejected token");
    assert_eq!(
        rejected.event_classification(),
        Ok(Ieee802154Event::TxDone.mask())
    );

    assert!(matches!(block_on(irq.wait()), Err(Ieee802154IrqOverflow)));
    assert_eq!(irq.drain(), Ieee802154IrqDrain::default());
}

#[test]
fn cancellation_before_an_irq_keeps_the_exact_active_owner_recoverable() {
    let irq = EmbassyIeee802154IrqRuntime::<NoopRawMutex, 1>::new();
    let mut operation = EmbassyIeee802154Operation::new(active_cca());
    let mut future = std::boxed::Box::pin(operation.advance(&irq));
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(future.as_mut().poll(&mut context), Poll::Pending));
    drop(future);

    assert!(operation.is_active());
    assert!(!operation.is_quarantined());
    assert!(operation.into_active().is_some());
}

#[test]
fn consumed_overflow_decode_and_rejection_errors_quarantine_the_owner() {
    let overflow_irq = EmbassyIeee802154IrqRuntime::<NoopRawMutex, 1>::new();
    publish(&overflow_irq, Ieee802154Event::TxSfdDone.mask(), 0).unwrap();
    let rejected = publish(&overflow_irq, Ieee802154Event::TxDone.mask(), 0)
        .expect_err("the full handoff returns the exact rejected token");
    assert_eq!(
        rejected.event_classification(),
        Ok(Ieee802154Event::TxDone.mask())
    );
    let mut overflow = EmbassyIeee802154Operation::new(active_cca());
    assert!(matches!(
        block_on(overflow.advance(&overflow_irq)),
        Err(EmbassyIeee802154OperationError::IrqOverflow)
    ));
    assert!(!overflow.is_active());
    assert!(overflow.is_quarantined());
    assert!(overflow.into_active().is_none());

    let decode_irq = EmbassyIeee802154IrqRuntime::<NoopRawMutex, 1>::new();
    publish_unclassified(&decode_irq).unwrap();
    let mut decode = EmbassyIeee802154Operation::new(active_cca());
    assert!(matches!(
        block_on(decode.advance(&decode_irq)),
        Err(EmbassyIeee802154OperationError::Interrupt(
            MacInterruptBatchError::UnclassifiedEvents(_)
        ))
    ));
    assert!(!decode.is_active());
    assert!(decode.is_quarantined());
    assert!(decode.into_active().is_none());

    let rejected_irq = EmbassyIeee802154IrqRuntime::<NoopRawMutex, 1>::new();
    publish(&rejected_irq, Ieee802154Event::TxDone.mask(), 0).unwrap();
    let mut rejected = EmbassyIeee802154Operation::new(active_cca());
    assert!(matches!(
        block_on(rejected.advance(&rejected_irq)),
        Err(EmbassyIeee802154OperationError::Rejected(_))
    ));
    assert!(!rejected.is_active());
    assert!(rejected.is_quarantined());
    assert!(rejected.into_active().is_none());
}

#[test]
fn quiesced_epoch_drain_reports_every_stale_value() {
    let irq = EmbassyIeee802154IrqRuntime::<NoopRawMutex, 2>::new();
    publish(&irq, Ieee802154Event::RxSfdDone.mask(), 0).unwrap();
    publish(&irq, Ieee802154Event::RxDone.mask(), 0).unwrap();

    assert_eq!(
        irq.drain(),
        Ieee802154IrqDrain {
            acknowledged_events: 2,
            overflowed: false,
        }
    );
    assert!(irq.try_take().is_none());
}
