//! Executor-neutral Controller worker for the software-only HCI bootstrap.

use core::{fmt, future::Future};

use bt_hci::PacketKind;
use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::RawMutex;

use crate::{
    BootstrapCommandCompleteEvent, HciChannelError, HciCommandPacket, HostToControllerFrame,
    InProcessHciControllerEndpoint, LeControllerBootstrap,
};

/// Terminal reason returned by [`LeControllerBootstrapWorker::run_until`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootstrapWorkerExit {
    /// Shutdown won at an idle receive boundary.
    StoppedIdle,
    /// A command was accepted, but its response could not be published before
    /// shutdown won.
    ///
    /// The response remains owned by the worker. Calling [`LeControllerBootstrapWorker::process_one`]
    /// or [`LeControllerBootstrapWorker::run_until`] again retries that exact
    /// response before receiving another Host packet.
    StoppedWithPendingResponse,
}

/// Fail-closed terminal error from the bootstrap worker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootstrapWorkerError {
    /// The bounded HCI channel rejected or could not decode a packet.
    Channel(HciChannelError),
    /// Host data reached the bootstrap-only worker before a Link Layer owned
    /// that packet class.
    LinkLayerPacketBeforeReady(PacketKind),
}

impl fmt::Display for BootstrapWorkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl core::error::Error for BootstrapWorkerError {}

impl From<HciChannelError> for BootstrapWorkerError {
    fn from(error: HciChannelError) -> Self {
        Self::Channel(error)
    }
}

/// One complete Controller packet produced by an HCI command dispatcher.
pub trait HciControllerResponse {
    /// HCI packet class published toward the Host.
    fn kind(&self) -> PacketKind;

    /// Complete packet body without an H4 indicator.
    fn as_bytes(&self) -> &[u8];
}

/// Closed synchronous command policy driven by [`HciCommandWorker`].
pub trait HciCommandDispatcher {
    /// Owned response retained across output backpressure and cancellation.
    type Response: HciControllerResponse;

    /// Execute one already validated HCI command and produce its exact response.
    fn dispatch(&mut self, command: HciCommandPacket<'_>) -> Self::Response;
}

impl<D> HciCommandDispatcher for &mut D
where
    D: HciCommandDispatcher,
{
    type Response = D::Response;

    fn dispatch(&mut self, command: HciCommandPacket<'_>) -> Self::Response {
        D::dispatch(self, command)
    }
}

impl HciControllerResponse for BootstrapCommandCompleteEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl HciCommandDispatcher for LeControllerBootstrap {
    type Response = BootstrapCommandCompleteEvent;

    fn dispatch(&mut self, command: HciCommandPacket<'_>) -> Self::Response {
        self.dispatch(command)
    }
}

/// Sole executor-neutral owner of the conservative HCI bootstrap endpoint.
///
/// The worker has no timer, interrupt, Link Layer, radio or executor policy. It
/// serializes one Host command at a time, retains an accepted command response
/// across backpressure or cancellation, and refuses ACL/Synchronous/ISO data
/// until a future dataplane owner replaces this bootstrap-only boundary.
pub struct HciCommandWorker<
    'channel,
    M,
    D,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
    D: HciCommandDispatcher,
{
    endpoint: InProcessHciControllerEndpoint<
        'channel,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    dispatcher: D,
    command_buffer: [u8; PACKET_CAPACITY],
    pending_response: Option<D::Response>,
}

impl<
    'channel,
    M,
    D,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    HciCommandWorker<
        'channel,
        M,
        D,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
    D: HciCommandDispatcher,
{
    /// Bind the unique Controller endpoint to one command dispatcher.
    pub const fn new(
        endpoint: InProcessHciControllerEndpoint<
            'channel,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        dispatcher: D,
    ) -> Self {
        Self {
            endpoint,
            dispatcher,
            command_buffer: [0; PACKET_CAPACITY],
            pending_response: None,
        }
    }

    /// Current command dispatcher state owned by this worker.
    pub const fn dispatcher(&self) -> &D {
        &self.dispatcher
    }

    /// Whether an accepted command still owns an unpublished response.
    pub const fn has_pending_response(&self) -> bool {
        self.pending_response.is_some()
    }

    /// Run one complete command transaction.
    ///
    /// If a previous call was cancelled while publishing, its retained response
    /// is retried first. Cancelling while waiting for a command does not consume
    /// the queue. Cancelling while publishing leaves the response in this
    /// worker, so resumption cannot execute the command twice.
    pub async fn process_one(&mut self) -> Result<(), BootstrapWorkerError> {
        self.publish_pending_response().await?;

        let frame = self.endpoint.receive(&mut self.command_buffer).await?;
        let response = match frame {
            HostToControllerFrame::Command(command) => self.dispatcher.dispatch(command),
            HostToControllerFrame::Acl(_) => {
                return Err(BootstrapWorkerError::LinkLayerPacketBeforeReady(
                    PacketKind::AclData,
                ));
            }
            HostToControllerFrame::Sync(_) => {
                return Err(BootstrapWorkerError::LinkLayerPacketBeforeReady(
                    PacketKind::SyncData,
                ));
            }
            HostToControllerFrame::Iso(_) => {
                return Err(BootstrapWorkerError::LinkLayerPacketBeforeReady(
                    PacketKind::IsoData,
                ));
            }
        };

        // There is no await between consuming the command and retaining its
        // response. Every later cancellation therefore leaves a resumable
        // transaction in `pending_response`.
        self.pending_response = Some(response);
        self.publish_pending_response().await
    }

    /// Run until a command/transport failure.
    pub async fn run(&mut self) -> Result<(), BootstrapWorkerError> {
        loop {
            self.process_one().await?;
        }
    }

    /// Run until `stop` resolves or a command/transport failure occurs.
    ///
    /// `stop` is polled first. If stop and a new command become ready together,
    /// the command remains queued. If stop wins after a command was accepted,
    /// the worker retains its exact response and reports
    /// [`BootstrapWorkerExit::StoppedWithPendingResponse`].
    pub async fn run_until<S>(
        &mut self,
        stop: S,
    ) -> Result<BootstrapWorkerExit, BootstrapWorkerError>
    where
        S: Future<Output = ()>,
    {
        let mut stop = core::pin::pin!(stop);
        loop {
            match select(stop.as_mut(), self.process_one()).await {
                Either::First(()) => {
                    return Ok(if self.has_pending_response() {
                        BootstrapWorkerExit::StoppedWithPendingResponse
                    } else {
                        BootstrapWorkerExit::StoppedIdle
                    });
                }
                Either::Second(result) => result?,
            }
        }
    }

    /// Recover the endpoint, dispatcher state and any retained response.
    ///
    /// A caller must not discard a non-`None` response while claiming clean HCI
    /// shutdown: it belongs to a command already consumed from the Host queue.
    pub fn into_parts(
        self,
    ) -> (
        InProcessHciControllerEndpoint<
            'channel,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        D,
        Option<D::Response>,
    ) {
        (self.endpoint, self.dispatcher, self.pending_response)
    }

    async fn publish_pending_response(&mut self) -> Result<(), BootstrapWorkerError> {
        if let Some(response) = self.pending_response.as_ref() {
            self.endpoint
                .publish(response.kind(), response.as_bytes())
                .await?;
            self.pending_response = None;
        }
        Ok(())
    }
}

/// Bootstrap-specialized command worker retained as the initial profile.
pub type LeControllerBootstrapWorker<
    'channel,
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> = HciCommandWorker<
    'channel,
    M,
    LeControllerBootstrap,
    HOST_TO_CONTROLLER_DEPTH,
    CONTROLLER_TO_HOST_DEPTH,
    PACKET_CAPACITY,
>;

impl<
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    HciCommandWorker<
        '_,
        M,
        LeControllerBootstrap,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    /// Current pure bootstrap state owned by this specialized worker.
    pub const fn bootstrap(&self) -> &LeControllerBootstrap {
        self.dispatcher()
    }
}

#[cfg(test)]
mod tests {
    use core::{
        future::{Future, ready},
        pin::Pin,
        task::{Context, Poll},
    };

    use bt_hci::{
        ControllerToHostPacket, FromHciBytes, PacketKind,
        cmd::{Cmd, Opcode, controller_baseband::Reset},
        data::AclPacket,
        event::{CommandComplete, CommandCompleteWithStatus, EventKind},
        param::Status,
        transport::Transport,
    };
    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;

    use crate::{
        BluetoothPublicDeviceAddress, BootstrapPhase, HostToControllerFrame, InProcessHciChannel,
        LeControllerBootstrap, LeControllerBootstrapConfig,
    };

    use super::{BootstrapWorkerError, BootstrapWorkerExit, LeControllerBootstrapWorker};

    const HARDWARE_ERROR: [u8; 3] = [0x10, 0x01, 0x42];

    type TestChannel = InProcessHciChannel<NoopRawMutex, 1, 1, 32>;

    #[test]
    fn stop_wins_a_ready_tie_without_consuming_the_queued_command() {
        let mut channel = TestChannel::new();
        let (host, controller) = channel.split();
        block_on(host.write(&Reset::new())).unwrap();
        let mut worker = LeControllerBootstrapWorker::new(controller, bootstrap());

        assert_eq!(
            block_on(worker.run_until(ready(()))).unwrap(),
            BootstrapWorkerExit::StoppedIdle
        );
        assert_eq!(worker.bootstrap().phase(), BootstrapPhase::AwaitingReset);

        let (controller, _, pending) = worker.into_parts();
        assert_eq!(pending, None);
        let mut command_buffer = [0; 32];
        let HostToControllerFrame::Command(command) =
            controller.try_receive(&mut command_buffer).unwrap()
        else {
            panic!("queued Reset changed packet kind");
        };
        assert_eq!(command.opcode(), Reset::OPCODE);
    }

    #[test]
    fn shutdown_retains_a_backpressured_response_and_resume_publishes_it_once() {
        let mut channel = TestChannel::new();
        let (host, controller) = channel.split();
        controller
            .try_publish(PacketKind::Event, &HARDWARE_ERROR)
            .unwrap();
        block_on(host.write(&Reset::new())).unwrap();
        let mut worker = LeControllerBootstrapWorker::new(controller, bootstrap());

        assert_eq!(
            block_on(worker.run_until(StopAfterOnePendingPoll::new())).unwrap(),
            BootstrapWorkerExit::StoppedWithPendingResponse
        );
        assert!(worker.has_pending_response());
        assert_eq!(worker.bootstrap().phase(), BootstrapPhase::Configuring);

        let mut event_buffer = [0; 32];
        let ControllerToHostPacket::Event(hardware_error) =
            block_on(host.read(&mut event_buffer)).unwrap()
        else {
            panic!("prefilled Hardware Error changed packet kind");
        };
        assert_eq!(hardware_error.data, &[0x42]);

        assert_eq!(
            block_on(worker.run_until(StopAfterOnePendingPoll::new())).unwrap(),
            BootstrapWorkerExit::StoppedIdle
        );
        assert!(!worker.has_pending_response());
        assert_command_complete(
            block_on(host.read(&mut event_buffer)).unwrap(),
            Reset::OPCODE,
            Status::SUCCESS,
        );

        // A further stop at the receive boundary cannot replay the completed
        // response or consume another command.
        assert_eq!(
            block_on(worker.run_until(ready(()))).unwrap(),
            BootstrapWorkerExit::StoppedIdle
        );
        assert!(!worker.has_pending_response());
    }

    #[test]
    fn acl_data_is_terminal_before_a_link_layer_owner_exists() {
        let mut channel = TestChannel::new();
        let (host, controller) = channel.split();
        let acl = AclPacket::from_hci_bytes_complete(&[1, 0, 0, 0]).unwrap();
        block_on(host.write(&acl)).unwrap();
        let mut worker = LeControllerBootstrapWorker::new(controller, bootstrap());

        assert_eq!(
            block_on(worker.process_one()),
            Err(BootstrapWorkerError::LinkLayerPacketBeforeReady(
                PacketKind::AclData
            ))
        );
        assert_eq!(worker.bootstrap().phase(), BootstrapPhase::AwaitingReset);
        assert!(!worker.has_pending_response());
    }

    fn bootstrap() -> LeControllerBootstrap {
        let config = LeControllerBootstrapConfig::new(
            BluetoothPublicDeviceAddress::from_canonical_bytes([0; 6]),
            27,
            1,
        )
        .unwrap();
        LeControllerBootstrap::new(config)
    }

    fn assert_command_complete(packet: ControllerToHostPacket<'_>, opcode: Opcode, status: Status) {
        let ControllerToHostPacket::Event(event) = packet else {
            panic!("Command Complete changed packet kind");
        };
        assert_eq!(event.kind, EventKind::CommandComplete);
        let complete = CommandComplete::from_hci_bytes_complete(event.data).unwrap();
        let complete: CommandCompleteWithStatus<'_> = complete.try_into().unwrap();
        assert_eq!(complete.cmd_opcode, opcode);
        assert_eq!(complete.status, status);
    }

    struct StopAfterOnePendingPoll {
        pending_returned: bool,
    }

    impl StopAfterOnePendingPoll {
        const fn new() -> Self {
            Self {
                pending_returned: false,
            }
        }
    }

    impl Future for StopAfterOnePendingPoll {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
            if self.pending_returned {
                Poll::Ready(())
            } else {
                self.pending_returned = true;
                context.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }
}
