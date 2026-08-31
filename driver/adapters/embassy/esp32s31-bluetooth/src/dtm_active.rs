//! Cancellation-safe Embassy waits for one borrowed active DTM session.
//!
//! No future in this module owns the affine session. Dropping a wait therefore
//! cancels only readiness observation; every radio and HCI owner remains in the
//! caller's `BluetoothDtmActiveSession`.

#![forbid(unsafe_code)]

use core::future::Future;

use embassy_futures::select::{Either, select};

#[cfg(target_arch = "riscv32")]
use embassy_sync::blocking_mutex::raw::RawMutex;
#[cfg(target_arch = "riscv32")]
use open_esp_radio_bluetooth_hci::LeControllerCommandEndpoint;
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_bluetooth::{
    BluetoothDtmActiveRadioWait, BluetoothDtmActiveSession, BluetoothDtmOrderReady,
    BluetoothDtmResponsePending, BluetoothSchedulerRunInterruptStorage,
};

#[cfg(target_arch = "riscv32")]
use crate::{EmbassyBluetoothPostUnlinkSignal, EmbassyBluetoothRuntimeWakers};

/// Exact radio-side reason an active-session wait completed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbassyBluetoothDtmActiveRadioSignal {
    /// The durable scheduler wake cell became non-empty.
    Scheduler,
    /// The exact post-unlink mailbox wake became durable.
    PostUnlink,
    /// The caller-provided absolute recheck future completed.
    ControllerTime,
}

/// Next signal for an active session whose command response is still pending.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbassyBluetoothDtmActivePendingSignal {
    /// Radio progress won the wait, including a simultaneous-ready tie.
    Radio(EmbassyBluetoothDtmActiveRadioSignal),
    /// The matching Controller-to-Host queue reported a capacity hint.
    ResponseCapacity,
}

/// Next signal after the previous DTM response entered HCI.
///
/// Host readiness is deliberately non-consuming. The sole task owner performs
/// a later synchronous receive, so a radio-first tie, cancellation, or this
/// signal itself can neither classify nor borrow the oldest packet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "route the radio edge or synchronously receive the ready Host packet"]
pub enum EmbassyBluetoothDtmActiveCommandSignal {
    /// Radio progress won the wait, including a simultaneous-ready tie.
    Radio(EmbassyBluetoothDtmActiveRadioSignal),
    /// Host-to-Controller storage was observed non-empty without consuming it.
    HostReady,
}

/// Failure to compose a pending response or command intake with an HCI endpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbassyBluetoothDtmActiveWaitError {
    /// The endpoint belongs to another live Controller epoch.
    EndpointMismatch,
}

#[cfg(target_arch = "riscv32")]
/// Borrowed wait view over one parked active DTM session.
///
/// Construction succeeds only while [`BluetoothDtmActiveSession::radio_wait`]
/// exposes a real radio wait. The view owns neither axis and may be dropped at
/// any await point without dropping or changing the session.
#[must_use = "borrow this view only while waiting, then resume the separately owned session"]
pub struct EmbassyBluetoothDtmActiveWait<'borrow, 'runtime, S, const CAPACITY: usize, Order, M>
where
    S: BluetoothSchedulerRunInterruptStorage,
    M: RawMutex,
{
    session: &'borrow BluetoothDtmActiveSession<'runtime, S, CAPACITY, Order>,
    wakers: &'borrow EmbassyBluetoothRuntimeWakers<M>,
}

#[cfg(target_arch = "riscv32")]
impl<'borrow, 'runtime, S, const CAPACITY: usize, Order, M>
    EmbassyBluetoothDtmActiveWait<'borrow, 'runtime, S, CAPACITY, Order, M>
where
    S: BluetoothSchedulerRunInterruptStorage,
    M: RawMutex,
{
    /// Borrow a session only if its radio axis is currently parked.
    pub fn from_waiting(
        session: &'borrow BluetoothDtmActiveSession<'runtime, S, CAPACITY, Order>,
        wakers: &'borrow EmbassyBluetoothRuntimeWakers<M>,
    ) -> Option<Self> {
        session.radio_wait().map(|_| Self { session, wakers })
    }

    async fn wait_radio<R>(
        &self,
        controller_time_recheck: R,
    ) -> EmbassyBluetoothDtmActiveRadioSignal
    where
        R: Future<Output = ()>,
    {
        match self
            .session
            .radio_wait()
            .expect("the borrowed wait view retains an unchanged parked session")
        {
            BluetoothDtmActiveRadioWait::Scheduler(wake) => {
                self.wakers.wait_scheduler_ready(wake).await;
                EmbassyBluetoothDtmActiveRadioSignal::Scheduler
            }
            BluetoothDtmActiveRadioWait::PostUnlink(wake) => {
                match self
                    .wakers
                    .wait_post_unlink_or_recheck(wake, controller_time_recheck)
                    .await
                {
                    EmbassyBluetoothPostUnlinkSignal::Mailbox => {
                        EmbassyBluetoothDtmActiveRadioSignal::PostUnlink
                    }
                    EmbassyBluetoothPostUnlinkSignal::Recheck => {
                        EmbassyBluetoothDtmActiveRadioSignal::ControllerTime
                    }
                }
            }
            BluetoothDtmActiveRadioWait::ControllerTime => {
                controller_time_recheck.await;
                EmbassyBluetoothDtmActiveRadioSignal::ControllerTime
            }
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<'borrow, 'runtime, S, const CAPACITY: usize, M>
    EmbassyBluetoothDtmActiveWait<
        'borrow,
        'runtime,
        S,
        CAPACITY,
        BluetoothDtmResponsePending<'runtime>,
        M,
    >
where
    S: BluetoothSchedulerRunInterruptStorage,
    M: RawMutex,
{
    /// Borrow both readiness sources and return whichever completes first.
    ///
    /// The radio future is the first `select` operand and therefore wins a
    /// simultaneous-ready tie. `controller_time_recheck` must be anchored to a
    /// caller-owned absolute deadline; rebuilding a relative delay after HCI
    /// wakeups would incorrectly extend the Controller-time wait.
    pub async fn wait_next<
        HciMutex: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
        R,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<
            '_,
            HciMutex,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        controller_time_recheck: R,
    ) -> Result<EmbassyBluetoothDtmActivePendingSignal, EmbassyBluetoothDtmActiveWaitError>
    where
        R: Future<Output = ()>,
    {
        if !self.session.matches_hci_endpoint(controller) {
            return Err(EmbassyBluetoothDtmActiveWaitError::EndpointMismatch);
        }

        Ok(
            match select_radio_first(
                self.wait_radio(controller_time_recheck),
                self.session.wait_response_capacity(controller),
            )
            .await
            {
                RadioFirst::Radio(signal) => EmbassyBluetoothDtmActivePendingSignal::Radio(signal),
                RadioFirst::Other(Ok(())) => {
                    EmbassyBluetoothDtmActivePendingSignal::ResponseCapacity
                }
                RadioFirst::Other(Err(_)) => {
                    return Err(EmbassyBluetoothDtmActiveWaitError::EndpointMismatch);
                }
            },
        )
    }
}

#[cfg(target_arch = "riscv32")]
impl<'borrow, 'runtime, S, const CAPACITY: usize, M>
    EmbassyBluetoothDtmActiveWait<
        'borrow,
        'runtime,
        S,
        CAPACITY,
        BluetoothDtmOrderReady<'runtime>,
        M,
    >
where
    S: BluetoothSchedulerRunInterruptStorage,
    M: RawMutex,
{
    /// Race radio progress against non-consuming Host packet readiness.
    ///
    /// Radio remains the first `select` operand and wins a simultaneous-ready
    /// tie. The readiness future neither borrows a packet buffer nor consumes,
    /// classifies or reserves the oldest packet. After `HostReady`, the sole
    /// task owner finishes with a synchronous receive and handles `Empty`
    /// losslessly because readiness is only a hint.
    pub async fn wait_next<
        HciMutex: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
        R,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<
            '_,
            HciMutex,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        controller_time_recheck: R,
    ) -> Result<EmbassyBluetoothDtmActiveCommandSignal, EmbassyBluetoothDtmActiveWaitError>
    where
        R: Future<Output = ()>,
    {
        if !self.session.accepts_hci_endpoint(controller) {
            return Err(EmbassyBluetoothDtmActiveWaitError::EndpointMismatch);
        }

        Ok(
            match select_radio_first(
                self.wait_radio(controller_time_recheck),
                self.session.wait_command_available(controller),
            )
            .await
            {
                RadioFirst::Radio(signal) => EmbassyBluetoothDtmActiveCommandSignal::Radio(signal),
                RadioFirst::Other(Ok(())) => EmbassyBluetoothDtmActiveCommandSignal::HostReady,
                RadioFirst::Other(Err(_)) => {
                    return Err(EmbassyBluetoothDtmActiveWaitError::EndpointMismatch);
                }
            },
        )
    }
}

enum RadioFirst<Radio, Other> {
    Radio(Radio),
    Other(Other),
}

async fn select_radio_first<R, O>(radio: R, other: O) -> RadioFirst<R::Output, O::Output>
where
    R: Future,
    O: Future,
{
    match select(radio, other).await {
        Either::First(radio) => RadioFirst::Radio(radio),
        Either::Second(other) => RadioFirst::Other(other),
    }
}

#[cfg(test)]
mod tests {
    use core::{
        future::{Future, pending, ready},
        task::{Context, Poll},
    };

    use bt_hci::{
        cmd::le::LeTestEnd,
        data::{AclBroadcastFlag, AclPacket, AclPacketBoundary},
        param::ConnHandle,
        transport::Transport,
    };
    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use open_esp_radio_bluetooth_hci::{
        BluetoothPublicDeviceAddress, HostToControllerFrame, LeControllerBootstrapConfig,
        LeControllerCommandIntake, LeControllerCommandReadyClaim, LeControllerHciResources,
        LeControllerIdleClassifiedCommandRoute,
    };
    use std::{boxed::Box, task::Waker};

    use super::{EmbassyBluetoothDtmActiveRadioSignal, RadioFirst, select_radio_first};

    type TestResources = LeControllerHciResources<NoopRawMutex, 2, 1, 16>;

    fn test_resources() -> TestResources {
        let config = LeControllerBootstrapConfig::new(
            BluetoothPublicDeviceAddress::from_canonical_bytes([2, 3, 5, 7, 11, 13]),
            12,
            1,
        )
        .expect("the test profile has one nonzero ACL credit");
        TestResources::new(config).expect("the test profile fits its owned transport")
    }

    #[test]
    fn radio_wins_a_simultaneous_ready_tie() {
        assert!(matches!(
            block_on(select_radio_first(ready(7_u8), ready(9_u8))),
            RadioFirst::Radio(7)
        ));
    }

    #[test]
    fn capacity_wins_only_while_radio_is_pending() {
        assert!(matches!(
            block_on(select_radio_first(pending::<()>(), ready(9_u8))),
            RadioFirst::Other(9)
        ));
    }

    #[test]
    fn production_readiness_keeps_command_on_radio_tie_then_sync_routes_it() {
        let mut resources = test_resources();
        let mut endpoints = resources.split();
        let LeControllerCommandReadyClaim::Ready(command_ready) =
            endpoints.controller.claim_initial_command_ready(())
        else {
            panic!("the fresh test epoch exposes initial command authority")
        };
        block_on(endpoints.host.write(&LeTestEnd::new())).unwrap();

        let first = block_on(select_radio_first(
            ready(EmbassyBluetoothDtmActiveRadioSignal::Scheduler),
            endpoints.controller.wait_command_available(&command_ready),
        ));
        assert!(matches!(
            first,
            RadioFirst::Radio(EmbassyBluetoothDtmActiveRadioSignal::Scheduler)
        ));

        let second = block_on(select_radio_first(
            pending::<EmbassyBluetoothDtmActiveRadioSignal>(),
            endpoints.controller.wait_command_available(&command_ready),
        ));
        assert!(matches!(second, RadioFirst::Other(Ok(()))));

        let mut packet = [0; 16];
        let command = match endpoints
            .controller
            .try_receive_classified_command_with_buffer(command_ready, &mut packet)
        {
            LeControllerCommandIntake::Command { command, .. } => command,
            _ => panic!("Host readiness must leave Test End for synchronous classification"),
        };
        assert!(matches!(
            endpoints.controller.route_idle_classified_command(command),
            LeControllerIdleClassifiedCommandRoute::ResponsePending(_)
        ));
    }

    #[test]
    fn production_readiness_leaves_exact_acl_for_synchronous_receive() {
        let mut resources = test_resources();
        let mut endpoints = resources.split();
        let LeControllerCommandReadyClaim::Ready(command_ready) =
            endpoints.controller.claim_initial_command_ready(())
        else {
            panic!("the fresh test epoch exposes initial command authority")
        };
        let acl = AclPacket::new(
            ConnHandle::new(1),
            AclPacketBoundary::Complete,
            AclBroadcastFlag::PointToPoint,
            &[7, 8],
        );
        block_on(endpoints.host.write(&acl)).unwrap();

        let selected = block_on(select_radio_first(
            pending::<EmbassyBluetoothDtmActiveRadioSignal>(),
            endpoints.controller.wait_command_available(&command_ready),
        ));
        assert!(matches!(selected, RadioFirst::Other(Ok(()))));

        let mut packet = [0; 16];
        let LeControllerCommandIntake::NonCommand { frame, .. } = endpoints
            .controller
            .try_receive_classified_command_with_buffer(command_ready, &mut packet)
        else {
            panic!("Host readiness must leave ACL for the synchronous router");
        };
        let HostToControllerFrame::Acl(received) = frame.value() else {
            panic!("the exact Host ACL packet changed kind")
        };
        assert_eq!(received.handle(), ConnHandle::new(1));
        assert_eq!(received.boundary_flag(), AclPacketBoundary::Complete);
        assert_eq!(received.broadcast_flag(), AclBroadcastFlag::PointToPoint);
        assert_eq!(received.data(), &[7, 8]);
    }

    #[test]
    fn cancelling_notified_production_readiness_leaves_exact_command() {
        let mut resources = test_resources();
        let mut endpoints = resources.split();
        let LeControllerCommandReadyClaim::Ready(command_ready) =
            endpoints.controller.claim_initial_command_ready(())
        else {
            panic!("the fresh test epoch exposes initial command authority")
        };
        let mut selected = Box::pin(select_radio_first(
            pending::<EmbassyBluetoothDtmActiveRadioSignal>(),
            endpoints.controller.wait_command_available(&command_ready),
        ));
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(
            selected.as_mut().poll(&mut context),
            Poll::Pending
        ));

        block_on(endpoints.host.write(&LeTestEnd::new())).unwrap();
        drop(selected);

        assert!(matches!(
            block_on(select_radio_first(
                pending::<EmbassyBluetoothDtmActiveRadioSignal>(),
                endpoints.controller.wait_command_available(&command_ready),
            )),
            RadioFirst::Other(Ok(()))
        ));
        let mut packet = [0; 16];
        let command = match endpoints
            .controller
            .try_receive_classified_command_with_buffer(command_ready, &mut packet)
        {
            LeControllerCommandIntake::Command { command, .. } => command,
            _ => panic!("cancelling readiness must retain the exact queued command"),
        };
        assert!(matches!(
            endpoints.controller.route_idle_classified_command(command),
            LeControllerIdleClassifiedCommandRoute::ResponsePending(_)
        ));
    }
}
