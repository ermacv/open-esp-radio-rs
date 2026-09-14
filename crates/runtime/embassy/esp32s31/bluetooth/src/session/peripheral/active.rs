//! Executable work selection for an active peripheral connection.
//!
//! This module is part of the production runtime on the chip and is also built
//! on the host. It owns the cooperative progress policy while the chip driver
//! retains every affine radio, Link Layer, ACL, and HCI-order owner.

#![forbid(unsafe_code)]
#![cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]

use core::future::Future;

use embassy_futures::select::{Either, select};

/// Readiness source that won one borrowed active-connection wait.
pub(crate) enum PeripheralConnectionWork<Hci> {
    Radio,
    Hci(Hci),
}

/// Source which released a CPU-owned completion from RX backpressure.
pub(crate) enum PeripheralConnectionBackpressureWork<Capacity, Recheck> {
    HostCapacity(Capacity),
    ControllerTime(Recheck),
}

/// Ordered HCI work that may run beside the radio axis.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PeripheralConnectionHciWork {
    OrderedResponse,
    HostEvent,
    Command,
}

/// One immutable polling plan derived from the retained production state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PeripheralConnectionWorkPlan {
    hci: PeripheralConnectionHciWork,
    poll_radio: bool,
}

impl PeripheralConnectionWorkPlan {
    pub(crate) const fn hci(self) -> PeripheralConnectionHciWork {
        self.hci
    }

    pub(crate) const fn poll_radio(self) -> bool {
        self.poll_radio
    }
}

/// Select active-connection work while preserving Controller output order.
///
/// Command intake remains live whenever no older ordered response or
/// publishable Host event owns the HCI axis. The intake itself classifies ACL
/// and commands, so an occupied Host ACL owner must not hide Disconnect, Reset,
/// or Host Number Of Completed Packets behind the retained data packet.
///
/// Radio backpressure waits on the actual Host-output capacity and a periodic
/// Controller-time recheck. The latter keeps supervision and procedure
/// deadlines live when Host credits are exhausted, so it remains a genuine
/// asynchronous radio source beside command intake.
pub(crate) const fn plan_peripheral_connection_work(
    response_pending: bool,
    host_event_pending: bool,
    host_event_flow_controlled: bool,
) -> PeripheralConnectionWorkPlan {
    let hci = if response_pending {
        PeripheralConnectionHciWork::OrderedResponse
    } else if host_event_pending && !host_event_flow_controlled {
        PeripheralConnectionHciWork::HostEvent
    } else {
        PeripheralConnectionHciWork::Command
    };
    PeripheralConnectionWorkPlan {
        hci,
        poll_radio: true,
    }
}

/// Race only the readiness sources selected by the production work plan.
///
/// Both futures borrow retained owners. Cancelling this wait therefore consumes
/// no radio or HCI authority. Radio is the first `select` operand and wins a
/// simultaneous-ready tie.
pub(crate) async fn wait_peripheral_connection_work<R, H>(
    radio: Option<R>,
    hci: H,
) -> PeripheralConnectionWork<H::Output>
where
    R: Future<Output = ()>,
    H: Future,
{
    match radio {
        Some(radio) => match select(radio, hci).await {
            Either::First(()) => PeripheralConnectionWork::Radio,
            Either::Second(hci) => PeripheralConnectionWork::Hci(hci),
        },
        None => PeripheralConnectionWork::Hci(hci.await),
    }
}

/// Race the two real predicates which may advance RX-backpressured ownership.
///
/// Both futures are borrowed. Cancellation therefore leaves the Host-capacity
/// predicate and the absolute Controller-time recheck available to a replacement
/// wait without consuming either underlying owner.
pub(crate) async fn wait_peripheral_connection_backpressure<C, T>(
    capacity: C,
    recheck: T,
) -> PeripheralConnectionBackpressureWork<C::Output, T::Output>
where
    C: Future,
    T: Future,
{
    match select(capacity, recheck).await {
        Either::First(capacity) => PeripheralConnectionBackpressureWork::HostCapacity(capacity),
        Either::Second(recheck) => PeripheralConnectionBackpressureWork::ControllerTime(recheck),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::{
        cell::Cell,
        future::{pending, ready},
        pin::pin,
        task::{Context, Poll, Waker},
    };
    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use oer_bluetooth_hci::{
        BluetoothPublicDeviceAddress, LeControllerBootstrapConfig, LeControllerCommandIntake,
        LeControllerCommandReadyClaim, LeControllerHciResources,
        bt_hci::{cmd::controller_baseband::Reset, transport::Transport},
    };
    use std::rc::Rc;

    struct BorrowedOwner {
        value: u8,
        drops: Rc<Cell<u8>>,
    }

    impl Drop for BorrowedOwner {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
        }
    }

    #[test]
    fn occupied_host_acl_owner_does_not_disable_command_intake() {
        let plan = plan_peripheral_connection_work(false, false, false);
        assert_eq!(plan.hci(), PeripheralConnectionHciWork::Command);
        assert!(plan.poll_radio());
    }

    #[test]
    fn host_credit_backpressure_keeps_the_deadline_recheck_live() {
        let plan = plan_peripheral_connection_work(false, true, true);
        assert_eq!(plan.hci(), PeripheralConnectionHciWork::Command);
        assert!(plan.poll_radio());

        let mut waiting = pin!(wait_peripheral_connection_work(
            plan.poll_radio().then_some(pending::<()>()),
            pending::<u8>(),
        ));
        assert!(
            waiting
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
    }

    #[test]
    fn returned_host_credit_wakes_the_hci_axis_without_a_radio_spin() {
        let plan = plan_peripheral_connection_work(false, true, true);
        let mut waiting = pin!(wait_peripheral_connection_work(
            plan.poll_radio().then_some(pending::<()>()),
            ready(7_u8),
        ));
        assert!(matches!(
            waiting
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(PeripheralConnectionWork::Hci(7))
        ));
    }

    #[test]
    fn exhausted_credits_still_wake_on_the_controller_time_deadline() {
        let signal = block_on(wait_peripheral_connection_backpressure(
            pending::<()>(),
            ready(17_u8),
        ));
        assert!(matches!(
            signal,
            PeripheralConnectionBackpressureWork::ControllerTime(17)
        ));
    }

    #[test]
    fn restored_output_capacity_precedes_a_pending_deadline() {
        let signal = block_on(wait_peripheral_connection_backpressure(
            ready(19_u8),
            pending::<()>(),
        ));
        assert!(matches!(
            signal,
            PeripheralConnectionBackpressureWork::HostCapacity(19)
        ));
    }

    #[test]
    fn flow_controlled_radio_wait_keeps_real_reset_intake_live() {
        let config = LeControllerBootstrapConfig::new(
            BluetoothPublicDeviceAddress::from_canonical_bytes([2, 3, 5, 7, 11, 13]),
            27,
            1,
        )
        .unwrap();
        let mut resources =
            LeControllerHciResources::<NoopRawMutex, 1, 1, 80>::new(config).unwrap();
        let mut endpoints = resources.split();
        let LeControllerCommandReadyClaim::Ready(ready) =
            endpoints.controller.claim_initial_command_ready(())
        else {
            panic!("the fresh HCI epoch exposes command authority")
        };
        block_on(endpoints.host.write(&Reset::new())).unwrap();

        let plan = plan_peripheral_connection_work(false, true, true);
        let signal = block_on(wait_peripheral_connection_work(
            plan.poll_radio().then_some(core::future::pending::<()>()),
            endpoints.controller.wait_command_available(&ready),
        ));
        assert!(matches!(signal, PeripheralConnectionWork::Hci(Ok(()))));

        let mut storage = [0; 80];
        assert!(matches!(
            endpoints
                .controller
                .try_receive_classified_command_with_buffer(ready, &mut storage),
            LeControllerCommandIntake::Command { .. }
        ));
    }

    #[test]
    fn pending_response_precedes_host_event_and_command() {
        let plan = plan_peripheral_connection_work(true, true, false);
        assert_eq!(plan.hci(), PeripheralConnectionHciWork::OrderedResponse);
        assert!(plan.poll_radio());
    }

    #[test]
    fn publishable_host_event_precedes_new_command() {
        let plan = plan_peripheral_connection_work(false, true, false);
        assert_eq!(plan.hci(), PeripheralConnectionHciWork::HostEvent);
    }

    #[test]
    fn response_backpressure_does_not_block_radio() {
        let mut future = pin!(wait_peripheral_connection_work(
            Some(ready(())),
            pending::<()>()
        ));
        assert!(matches!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(PeripheralConnectionWork::Radio)
        ));
    }

    #[test]
    fn simultaneous_readiness_prioritizes_radio() {
        let mut future = pin!(wait_peripheral_connection_work(Some(ready(())), ready(())));
        assert!(matches!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(PeripheralConnectionWork::Radio)
        ));
    }

    #[test]
    fn cancelled_selection_retains_borrowed_owners_for_resume() {
        let radio_ready = Cell::new(false);
        let hci_ready = Cell::new(false);
        let drops = Rc::new(Cell::new(0));
        let radio_owner = BorrowedOwner {
            value: 41,
            drops: Rc::clone(&drops),
        };
        let hci_owner = BorrowedOwner {
            value: 43,
            drops: Rc::clone(&drops),
        };
        let borrow_radio = || {
            core::future::poll_fn(|_| {
                assert_eq!(radio_owner.value, 41);
                if radio_ready.get() {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
        };
        let borrow_hci = || {
            core::future::poll_fn(|_| {
                assert_eq!(hci_owner.value, 43);
                if hci_ready.get() {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
        };
        {
            let mut future = pin!(wait_peripheral_connection_work(
                Some(borrow_radio()),
                borrow_hci()
            ));
            assert!(
                future
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()))
                    .is_pending()
            );
        }
        assert_eq!((radio_owner.value, hci_owner.value), (41, 43));
        assert_eq!(drops.get(), 0);

        hci_ready.set(true);
        let mut resumed = pin!(wait_peripheral_connection_work(
            Some(borrow_radio()),
            borrow_hci()
        ));
        assert!(matches!(
            resumed
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(PeripheralConnectionWork::Hci(()))
        ));
        assert_eq!(drops.get(), 0);
    }
}
