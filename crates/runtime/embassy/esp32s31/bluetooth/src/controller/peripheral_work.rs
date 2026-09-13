//! Borrowed waits: radio readiness takes priority over response publication.

use core::future::Future;
use embassy_futures::select::{Either, select};

pub(super) enum Work<Response> {
    Radio,
    Response(Response),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HciWork {
    None,
    OrderedResponse,
    HostEvent,
    Command,
}

/// Preserve an older command completion ahead of an unsolicited connection event.
pub(super) const fn select_hci_work(
    response_pending: bool,
    host_event_pending: bool,
    command_ready: bool,
) -> HciWork {
    if response_pending {
        HciWork::OrderedResponse
    } else if host_event_pending {
        HciWork::HostEvent
    } else if command_ready {
        HciWork::Command
    } else {
        HciWork::None
    }
}

/// Select active-connection HCI work while preserving ordered output.
///
/// A flow-controlled Controller ACL packet must keep command intake live even
/// while the sole Host ACL owner is occupied. That intake is the only path by
/// which Host Number Of Completed Packets can return the blocked Controller
/// credit. If the Host violates its advertised packet credit and supplies a
/// second ACL packet instead, the active ACL owner consumes and completes that
/// packet without replacing the retained first packet.
pub(super) const fn select_active_hci_work(
    response_pending: bool,
    host_event_pending: bool,
    host_event_flow_controlled: bool,
    can_accept_host_packet: bool,
) -> HciWork {
    select_hci_work(
        response_pending,
        host_event_pending && !host_event_flow_controlled,
        !response_pending
            && (!host_event_pending || host_event_flow_controlled)
            && (can_accept_host_packet || host_event_flow_controlled),
    )
}

/// An absent pending response cannot synthesize work or spin a command-ready actor.
/// Both futures borrow retained owners; cancelling this wait consumes no authority.
pub(super) async fn wait<R: Future<Output = ()>, H: Future>(
    radio: R,
    response: Option<H>,
) -> Work<H::Output> {
    match response {
        None => {
            radio.await;
            Work::Radio
        }
        Some(response) => match select(radio, response).await {
            Either::First(()) => Work::Radio,
            Either::Second(response) => Work::Response(response),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::{
        future::{pending, ready},
        pin::pin,
        task::{Context, Poll, Waker},
    };

    #[test]
    fn response_backpressure_does_not_block_radio() {
        let mut future = pin!(wait(ready(()), Some(pending::<()>())));
        assert!(matches!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Work::Radio)
        ));
    }

    #[test]
    fn radio_wait_does_not_block_response() {
        let mut future = pin!(wait(pending(), Some(ready(7))));
        assert!(matches!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Work::Response(7))
        ));
    }

    #[test]
    fn simultaneous_readiness_prioritizes_radio() {
        let mut future = pin!(wait(ready(()), Some(ready(()))));
        assert!(matches!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Work::Radio)
        ));
    }

    #[test]
    fn command_ready_waits_for_radio() {
        let mut future = pin!(wait(pending(), None::<core::future::Ready<()>>));
        assert!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
    }

    #[test]
    fn response_endpoint_error_reaches_the_actor() {
        let mut future = pin!(wait(pending(), Some(ready(Err::<(), _>(7)))));
        assert!(matches!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Work::Response(Err(7)))
        ));
    }

    #[test]
    fn cancelled_selection_retains_both_owners_for_resume() {
        use core::cell::Cell;
        let radio_ready = Cell::new(false);
        let response_ready = Cell::new(false);
        let radio_owner = super::super::owner::ControllerOwnerSlot::new(41_u8);
        let response_owner = super::super::owner::ControllerOwnerSlot::new(43_u8);
        let borrow_radio = || {
            core::future::poll_fn(|_| {
                assert_eq!(*radio_owner.current(), 41);
                if radio_ready.get() {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
        };
        let borrow_response = || {
            core::future::poll_fn(|_| {
                assert_eq!(*response_owner.current(), 43);
                if response_ready.get() {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
        };
        {
            let mut future = pin!(wait(borrow_radio(), Some(borrow_response())));
            assert!(
                future
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()))
                    .is_pending()
            );
        }
        assert_eq!(*radio_owner.current(), 41);
        assert_eq!(*response_owner.current(), 43);
        radio_ready.set(true);
        let mut resumed = pin!(wait(borrow_radio(), Some(borrow_response())));
        assert!(matches!(
            resumed
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Work::Radio)
        ));
    }

    #[test]
    fn ordered_response_precedes_unsolicited_connection_event() {
        assert_eq!(select_hci_work(true, true, true), HciWork::OrderedResponse);
        assert_eq!(select_hci_work(false, true, true), HciWork::HostEvent);
        assert_eq!(select_hci_work(false, false, true), HciWork::Command);
        assert_eq!(select_hci_work(false, false, false), HciWork::None);
    }

    #[test]
    fn flow_control_keeps_credit_command_intake_live_with_an_occupied_host_acl_owner() {
        assert_eq!(
            select_active_hci_work(false, true, true, false),
            HciWork::Command
        );
        assert_eq!(
            select_active_hci_work(false, true, true, true),
            HciWork::Command
        );
        assert_eq!(
            select_active_hci_work(false, true, false, true),
            HciWork::HostEvent
        );
        assert_eq!(
            select_active_hci_work(true, true, true, true),
            HciWork::OrderedResponse
        );
    }
}
