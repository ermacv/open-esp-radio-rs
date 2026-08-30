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
use open_esp_radio_bluetooth_hci::InProcessHciControllerEndpoint;
#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_bluetooth_hci::{
    HciChannelError, HciEpochBound, HciEpochBoundCommandReceiveError, HostToControllerFrame,
    LeControllerCommandClassification,
};
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_bluetooth::{
    BluetoothDtmActiveRadioWait, BluetoothDtmActiveSession, BluetoothDtmResponsePending,
    BluetoothDtmResponsePublished, BluetoothSchedulerRunInterruptStorage,
};

#[cfg(target_arch = "riscv32")]
use crate::EmbassyBluetoothRuntimeWakers;

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
/// Host packets remain bound to their exact Controller epoch. A non-command
/// frame is returned intact for the future Link Layer/ACL router rather than
/// being discarded by the DTM task.
#[must_use = "route the radio edge, classified command, retained frame, or channel fault"]
pub enum EmbassyBluetoothDtmActiveCommandSignal<'epoch, 'packet> {
    /// Radio progress won the wait, including a simultaneous-ready tie.
    Radio(EmbassyBluetoothDtmActiveRadioSignal),
    /// One command was consumed, classified and bound to its source HCI epoch.
    Command(HciEpochBound<'epoch, LeControllerCommandClassification>),
    /// The oldest Host packet is data and remains bound to its source epoch.
    NonCommand(HciEpochBound<'epoch, HostToControllerFrame<'packet>>),
    /// A packet boundary failed without creating a semantic command token.
    ChannelFault(HciChannelError),
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
                self.wakers.wait_post_unlink_ready(wake).await;
                EmbassyBluetoothDtmActiveRadioSignal::PostUnlink
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
        controller: &InProcessHciControllerEndpoint<
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
                controller.wait_publish_ready(),
            )
            .await
            {
                RadioFirst::Radio(signal) => EmbassyBluetoothDtmActivePendingSignal::Radio(signal),
                RadioFirst::Other(()) => EmbassyBluetoothDtmActivePendingSignal::ResponseCapacity,
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
        BluetoothDtmResponsePublished<'runtime>,
        M,
    >
where
    S: BluetoothSchedulerRunInterruptStorage,
    M: RawMutex,
{
    /// Race radio progress against endpoint-bound Host command intake.
    ///
    /// Radio remains the first `select` operand and wins a simultaneous-ready
    /// tie. Cancelling the receive future before it wins consumes no Host
    /// packet and creates no epoch token.
    pub async fn wait_next<
        'channel,
        'packet,
        HciMutex: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
        R,
    >(
        &self,
        controller: &InProcessHciControllerEndpoint<
            'channel,
            HciMutex,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        packet: &'packet mut [u8],
        controller_time_recheck: R,
    ) -> Result<
        EmbassyBluetoothDtmActiveCommandSignal<'channel, 'packet>,
        EmbassyBluetoothDtmActiveWaitError,
    >
    where
        R: Future<Output = ()>,
    {
        if !self.session.accepts_hci_endpoint(controller) {
            return Err(EmbassyBluetoothDtmActiveWaitError::EndpointMismatch);
        }

        Ok(into_active_command_signal(
            select_radio_first(
                self.wait_radio(controller_time_recheck),
                controller.receive_classified_command(packet),
            )
            .await,
        ))
    }
}

#[cfg(any(target_arch = "riscv32", test))]
fn into_active_command_signal<'epoch, 'packet>(
    selected: RadioFirst<
        EmbassyBluetoothDtmActiveRadioSignal,
        Result<
            HciEpochBound<'epoch, LeControllerCommandClassification>,
            HciEpochBoundCommandReceiveError<'epoch, 'packet>,
        >,
    >,
) -> EmbassyBluetoothDtmActiveCommandSignal<'epoch, 'packet> {
    match selected {
        RadioFirst::Radio(signal) => EmbassyBluetoothDtmActiveCommandSignal::Radio(signal),
        RadioFirst::Other(Ok(command)) => EmbassyBluetoothDtmActiveCommandSignal::Command(command),
        RadioFirst::Other(Err(HciEpochBoundCommandReceiveError::NonCommand(frame))) => {
            EmbassyBluetoothDtmActiveCommandSignal::NonCommand(frame)
        }
        RadioFirst::Other(Err(HciEpochBoundCommandReceiveError::Channel(error))) => {
            EmbassyBluetoothDtmActiveCommandSignal::ChannelFault(error)
        }
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
        pin::Pin,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
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
        HciChannelError, InProcessHciChannel, LeControllerCommandClassification, LeDtmCommand,
    };
    use std::{boxed::Box, task::Waker};

    use super::{
        EmbassyBluetoothDtmActiveCommandSignal, EmbassyBluetoothDtmActiveRadioSignal, RadioFirst,
        into_active_command_signal, select_radio_first,
    };

    type TestChannel = InProcessHciChannel<NoopRawMutex, 2, 1, 16>;

    struct BorrowedReadiness<'a> {
        ready: &'a AtomicBool,
        polls: &'a AtomicUsize,
    }

    impl Future for BorrowedReadiness<'_> {
        type Output = ();

        fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
            self.polls.fetch_add(1, Ordering::Relaxed);
            if self.ready.load(Ordering::Acquire) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }
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
    fn cancelling_a_borrowed_select_consumes_no_readiness() {
        let radio_ready = AtomicBool::new(false);
        let capacity_ready = AtomicBool::new(false);
        let radio_polls = AtomicUsize::new(0);
        let capacity_polls = AtomicUsize::new(0);
        let task_waker = Waker::noop();

        let radio = BorrowedReadiness {
            ready: &radio_ready,
            polls: &radio_polls,
        };
        let capacity = BorrowedReadiness {
            ready: &capacity_ready,
            polls: &capacity_polls,
        };
        let mut selected = Box::pin(select_radio_first(radio, capacity));
        let mut context = Context::from_waker(task_waker);
        assert!(selected.as_mut().poll(&mut context).is_pending());
        drop(selected);

        radio_ready.store(true, Ordering::Release);
        let replacement = BorrowedReadiness {
            ready: &radio_ready,
            polls: &radio_polls,
        };
        assert!(matches!(
            block_on(select_radio_first(replacement, pending::<()>())),
            RadioFirst::Radio(())
        ));
        assert!(radio_polls.load(Ordering::Relaxed) >= 2);
        assert_eq!(capacity_polls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn production_receive_keeps_command_on_radio_tie_then_classifies_it() {
        let mut channel = TestChannel::new();
        let (host, controller) = channel.split();
        block_on(host.write(&LeTestEnd::new())).unwrap();
        let mut packet = [0; 16];

        let first = block_on(select_radio_first(
            ready(EmbassyBluetoothDtmActiveRadioSignal::Scheduler),
            controller.receive_classified_command(&mut packet),
        ));
        assert!(matches!(
            into_active_command_signal(first),
            EmbassyBluetoothDtmActiveCommandSignal::Radio(
                EmbassyBluetoothDtmActiveRadioSignal::Scheduler
            )
        ));

        let second = block_on(select_radio_first(
            pending::<EmbassyBluetoothDtmActiveRadioSignal>(),
            controller.receive_classified_command(&mut packet),
        ));
        let EmbassyBluetoothDtmActiveCommandSignal::Command(command) =
            into_active_command_signal(second)
        else {
            panic!("the queued Test End must remain for replacement command intake");
        };
        assert!(matches!(
            command.value(),
            LeControllerCommandClassification::Dtm(LeDtmCommand::TestEnd(_))
        ));
        assert!(command.originates_from(&controller));
    }

    #[test]
    fn production_receive_retains_non_command_and_maps_preconsume_fault() {
        let mut channel = TestChannel::new();
        let (host, controller) = channel.split();
        let acl = AclPacket::new(
            ConnHandle::new(1),
            AclPacketBoundary::Complete,
            AclBroadcastFlag::PointToPoint,
            &[7, 8],
        );
        block_on(host.write(&acl)).unwrap();
        let mut packet = [0; 16];

        let selected = block_on(select_radio_first(
            pending::<EmbassyBluetoothDtmActiveRadioSignal>(),
            controller.receive_classified_command(&mut packet),
        ));
        let EmbassyBluetoothDtmActiveCommandSignal::NonCommand(frame) =
            into_active_command_signal(selected)
        else {
            panic!("ACL must leave DTM as an epoch-bound non-command frame");
        };
        assert!(frame.originates_from(&controller));

        let selected = block_on(select_radio_first(
            pending::<EmbassyBluetoothDtmActiveRadioSignal>(),
            controller.receive_classified_command(&mut []),
        ));
        assert!(matches!(
            into_active_command_signal(selected),
            EmbassyBluetoothDtmActiveCommandSignal::ChannelFault(
                HciChannelError::DestinationTooSmall { .. }
            )
        ));
    }
}
