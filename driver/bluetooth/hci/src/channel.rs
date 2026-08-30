//! Bounded, packet-oriented, in-process HCI transport.

use core::{cell::RefCell, fmt, future::poll_fn, task::Poll};

use bt_hci::{
    ControllerToHostPacket, FromHciBytes, FromHciBytesError,
    HostToControllerPacket as HostToControllerPacketContract, PacketKind,
    cmd::{Opcode, OpcodeGroup},
    data::{AclPacket, IsoPacket, SyncPacket},
    transport::Transport,
};
use embassy_sync::{
    blocking_mutex::{Mutex, raw::RawMutex},
    waitqueue::WakerRegistration,
};
use embedded_io::{ErrorKind, ErrorType, Write};

use crate::{
    ControllerToHostQueueError, LeControllerCommandClassification, PacketSlot,
    classify_le_controller_command, decode_complete_packet,
};

/// Opaque identity of one live in-process HCI resource epoch.
///
/// The marker can be copied for affinity checks but cannot be constructed by
/// callers. Its borrow prevents identity from outliving the backing channel.
#[derive(Clone, Copy)]
pub struct HciEpochIdentity<'epoch> {
    marker: &'epoch u8,
}

impl HciEpochIdentity<'_> {
    /// Whether two endpoints originate from the same live channel object.
    pub fn same_epoch(self, other: HciEpochIdentity<'_>) -> bool {
        core::ptr::eq(self.marker, other.marker)
    }
}

/// A semantic Controller value proven to originate from one live HCI epoch.
///
/// Only [`InProcessHciControllerEndpoint`] can mint this token, immediately
/// after consuming and classifying a packet from its Host-to-Controller queue.
/// The epoch marker remains opaque while [`Self::map`] and [`Self::try_map`]
/// permit ownership-preserving semantic refinement without reconstructing it.
#[must_use = "the semantic value and its HCI origin proof must remain paired"]
pub struct HciEpochBound<'epoch, T> {
    hci_epoch: HciEpochIdentity<'epoch>,
    value: T,
}

impl<'epoch, T> HciEpochBound<'epoch, T> {
    fn bind(hci_epoch: HciEpochIdentity<'epoch>, value: T) -> Self {
        Self { hci_epoch, value }
    }

    /// Borrow the semantic value without releasing its origin proof.
    pub const fn value(&self) -> &T {
        &self.value
    }

    /// Transform the semantic owner while preserving its exact origin epoch.
    pub fn map<U>(self, map: impl FnOnce(T) -> U) -> HciEpochBound<'epoch, U> {
        HciEpochBound {
            hci_epoch: self.hci_epoch,
            value: map(self.value),
        }
    }

    /// Attempt one consuming semantic refinement without losing either branch.
    pub fn try_map<U>(
        self,
        map: impl FnOnce(T) -> Result<U, T>,
    ) -> Result<HciEpochBound<'epoch, U>, Self> {
        let Self { hci_epoch, value } = self;
        match map(value) {
            Ok(value) => Ok(HciEpochBound { hci_epoch, value }),
            Err(value) => Err(Self { hci_epoch, value }),
        }
    }

    /// Whether this value originated from the supplied live Controller endpoint.
    pub fn originates_from<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &InProcessHciControllerEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> bool {
        self.hci_epoch.same_epoch(controller.epoch_identity())
    }

    /// Release the semantic owner only to its matching live Controller endpoint.
    pub fn try_into_for_endpoint<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        self,
        controller: &InProcessHciControllerEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> Result<T, Self> {
        if self.originates_from(controller) {
            Ok(self.value)
        } else {
            Err(self)
        }
    }
}

/// An error at the bounded in-process HCI boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HciChannelError {
    /// A non-blocking send found no free packet slot.
    Full,
    /// A non-blocking receive found no published packet.
    Empty,
    /// A packet was offered on the wrong directional half of HCI.
    InvalidDirection,
    /// The packet body exceeds the statically selected storage profile.
    PacketTooLong {
        /// Supplied or declared packet length.
        length: usize,
        /// Maximum packet length retained by this channel.
        capacity: usize,
    },
    /// The caller buffer cannot retain this channel's complete packet profile.
    DestinationTooSmall {
        /// Required caller-buffer length.
        required: usize,
        /// Supplied caller-buffer length.
        available: usize,
    },
    /// A packet header or value is not valid HCI.
    InvalidPacket(FromHciBytesError),
    /// Bytes exist after the payload length declared by the HCI header.
    TrailingBytes,
    /// A `HostToControllerPacket` wrote beyond the selected storage profile.
    SerializationOverflow {
        /// Maximum writable packet body length.
        capacity: usize,
    },
    /// A packet's `WriteHci::size` contract disagreed with the bytes it wrote.
    SerializationLengthMismatch {
        /// Length reported before serialization.
        declared: usize,
        /// Bytes actually serialized.
        written: usize,
    },
    /// Safe code observed a retained packet that no longer satisfies admission.
    CorruptRetainedPacket,
}

impl fmt::Display for HciChannelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl core::error::Error for HciChannelError {}

impl embedded_io::Error for HciChannelError {
    fn kind(&self) -> ErrorKind {
        match self {
            Self::Full | Self::Empty => ErrorKind::Other,
            Self::InvalidDirection => ErrorKind::InvalidInput,
            Self::PacketTooLong { .. }
            | Self::DestinationTooSmall { .. }
            | Self::SerializationOverflow { .. } => ErrorKind::OutOfMemory,
            Self::InvalidPacket(_)
            | Self::TrailingBytes
            | Self::SerializationLengthMismatch { .. }
            | Self::CorruptRetainedPacket => ErrorKind::InvalidData,
        }
    }
}

impl From<FromHciBytesError> for HciChannelError {
    fn from(error: FromHciBytesError) -> Self {
        Self::InvalidPacket(error)
    }
}

/// A generic HCI command decoded at the Controller side of the boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HciCommandPacket<'packet> {
    opcode: Opcode,
    parameters: &'packet [u8],
}

impl HciCommandPacket<'_> {
    /// Command opcode, including its OGF and OCF fields.
    pub const fn opcode(&self) -> Opcode {
        self.opcode
    }

    /// Complete command parameter bytes following the three-byte HCI header.
    pub const fn parameters(&self) -> &[u8] {
        self.parameters
    }

    #[cfg(test)]
    pub(crate) const fn for_test(opcode: Opcode, parameters: &[u8]) -> HciCommandPacket<'_> {
        HciCommandPacket { opcode, parameters }
    }
}

/// One complete packet emitted by the Host and consumed by the Controller.
#[derive(Debug)]
pub enum HostToControllerFrame<'packet> {
    /// A generic HCI command for dispatch by opcode.
    Command(HciCommandPacket<'packet>),
    /// Host ACL data.
    Acl(AclPacket<'packet>),
    /// Host synchronous data.
    Sync(SyncPacket<'packet>),
    /// Host isochronous data.
    Iso(IsoPacket<'packet>),
}

/// Failure to consume one Host packet as an epoch-bound classified command.
#[must_use = "a non-command frame retains its origin proof and borrowed packet"]
pub enum HciEpochBoundCommandReceiveError<'epoch, 'packet> {
    /// The queue or packet boundary failed before an epoch token was created.
    Channel(HciChannelError),
    /// The oldest packet was data rather than a command and remains bound to its epoch.
    NonCommand(HciEpochBound<'epoch, HostToControllerFrame<'packet>>),
}

/// One non-blocking classified intake that preserves receive-buffer ownership.
///
/// Command and stale-readiness branches return the caller's buffer so an event
/// loop can immediately reuse it. Only a non-command frame borrows the buffer,
/// transferring that exact packet to the outer data router. A malformed
/// retained packet fails closed and ends the intake call.
#[must_use = "route the packet or recover the receive buffer"]
pub enum HciClassifiedCommandIntake<'epoch, 'packet> {
    /// An owned command classification plus the reusable receive buffer.
    Command {
        /// Semantic command bound to the source Controller epoch.
        command: HciEpochBound<'epoch, LeControllerCommandClassification>,
        /// Scratch storage no longer borrowed by the owned classification.
        buffer: &'packet mut [u8],
    },
    /// A readiness hint became stale before the sole intake owner consumed it.
    Empty {
        /// Scratch storage remains available for the replacement wait.
        buffer: &'packet mut [u8],
    },
    /// A non-retryable packet boundary failure.
    Channel(HciChannelError),
    /// The oldest Host packet is data and retains its buffer borrow.
    NonCommand(HciEpochBound<'epoch, HostToControllerFrame<'packet>>),
}

struct AsyncPacketQueue<M, const DEPTH: usize, const PACKET_CAPACITY: usize>
where
    M: RawMutex,
{
    state: Mutex<M, RefCell<AsyncPacketQueueState<DEPTH, PACKET_CAPACITY>>>,
}

impl<M, const DEPTH: usize, const PACKET_CAPACITY: usize>
    AsyncPacketQueue<M, DEPTH, PACKET_CAPACITY>
where
    M: RawMutex,
{
    const fn new() -> Self {
        Self {
            state: Mutex::new(RefCell::new(AsyncPacketQueueState::new())),
        }
    }

    async fn send(&self, packet: PacketSlot<PACKET_CAPACITY>) {
        poll_fn(|context| {
            self.state.lock(|state| {
                let mut state = state.borrow_mut();
                if state.try_send(packet) {
                    Poll::Ready(())
                } else {
                    state.sender_waker.register(context.waker());
                    Poll::Pending
                }
            })
        })
        .await
    }

    async fn wait_send_ready(&self) {
        poll_fn(|context| {
            self.state.lock(|state| {
                let mut state = state.borrow_mut();
                if state.length < DEPTH {
                    Poll::Ready(())
                } else {
                    state.sender_waker.register(context.waker());
                    Poll::Pending
                }
            })
        })
        .await
    }

    fn try_send(&self, packet: PacketSlot<PACKET_CAPACITY>) -> Result<(), ()> {
        self.state.lock(|state| {
            if state.borrow_mut().try_send(packet) {
                Ok(())
            } else {
                Err(())
            }
        })
    }

    async fn receive(&self) -> PacketSlot<PACKET_CAPACITY> {
        poll_fn(|context| {
            self.state.lock(|state| {
                let mut state = state.borrow_mut();
                if let Some(packet) = state.try_receive() {
                    Poll::Ready(packet)
                } else {
                    state.receiver_waker.register(context.waker());
                    Poll::Pending
                }
            })
        })
        .await
    }

    async fn wait_receive_ready(&self) {
        poll_fn(|context| {
            self.state.lock(|state| {
                let mut state = state.borrow_mut();
                if state.length > 0 {
                    Poll::Ready(())
                } else {
                    state.receiver_waker.register(context.waker());
                    Poll::Pending
                }
            })
        })
        .await
    }

    fn try_receive(&self) -> Result<PacketSlot<PACKET_CAPACITY>, ()> {
        self.state
            .lock(|state| state.borrow_mut().try_receive().ok_or(()))
    }

    fn is_empty(&self) -> bool {
        self.state.lock(|state| state.borrow().length == 0)
    }

    fn is_pristine(&self) -> bool {
        self.state.lock(|state| {
            let state = state.borrow();
            state.length == 0 && !state.has_published_packet
        })
    }

    #[cfg(test)]
    fn vacant_storage_is_zeroed(&self) -> bool {
        self.state.lock(|state| {
            let state = state.borrow();
            state
                .slots
                .iter()
                .all(|slot| slot.length != 0 || slot.bytes.iter().all(|byte| *byte == 0))
        })
    }
}

struct AsyncPacketQueueState<const DEPTH: usize, const PACKET_CAPACITY: usize> {
    slots: [PacketSlot<PACKET_CAPACITY>; DEPTH],
    head: usize,
    length: usize,
    has_published_packet: bool,
    receiver_waker: WakerRegistration,
    sender_waker: WakerRegistration,
}

impl<const DEPTH: usize, const PACKET_CAPACITY: usize>
    AsyncPacketQueueState<DEPTH, PACKET_CAPACITY>
{
    const fn new() -> Self {
        Self {
            slots: [PacketSlot::EMPTY; DEPTH],
            head: 0,
            length: 0,
            has_published_packet: false,
            receiver_waker: WakerRegistration::new(),
            sender_waker: WakerRegistration::new(),
        }
    }

    fn try_send(&mut self, packet: PacketSlot<PACKET_CAPACITY>) -> bool {
        if self.length == DEPTH {
            return false;
        }
        let tail = (self.head + self.length) % DEPTH;
        self.slots[tail] = packet;
        self.length += 1;
        self.has_published_packet = true;
        self.receiver_waker.wake();
        true
    }

    fn try_receive(&mut self) -> Option<PacketSlot<PACKET_CAPACITY>> {
        if self.length == 0 {
            return None;
        }
        let packet = self.slots[self.head];
        self.slots[self.head].bytes.fill(0);
        self.slots[self.head].length = 0;
        self.head = (self.head + 1) % DEPTH;
        self.length -= 1;
        self.sender_waker.wake();
        Some(packet)
    }
}

impl HostToControllerFrame<'_> {
    /// Packet kind carried separately from the in-process packet body.
    pub const fn kind(&self) -> PacketKind {
        match self {
            Self::Command(_) => PacketKind::Cmd,
            Self::Acl(_) => PacketKind::AclData,
            Self::Sync(_) => PacketKind::SyncData,
            Self::Iso(_) => PacketKind::IsoData,
        }
    }
}

/// Two bounded packet queues joining an HCI Host and one raw Controller owner.
///
/// Packet indicators are retained as typed [`PacketKind`] values and are never
/// serialized as UART/H4 bytes. Calling [`Self::split`] requires exclusive
/// access, so safe code cannot manufacture a second endpoint pair while the
/// first pair is alive. `M` selects the synchronization domain; a platform may
/// use a critical-section mutex for IRQ/task handoff without introducing an
/// RTOS.
pub struct InProcessHciChannel<
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    identity: u8,
    host_to_controller: AsyncPacketQueue<M, HOST_TO_CONTROLLER_DEPTH, PACKET_CAPACITY>,
    controller_to_host: AsyncPacketQueue<M, CONTROLLER_TO_HOST_DEPTH, PACKET_CAPACITY>,
}

impl<
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> InProcessHciChannel<M, HOST_TO_CONTROLLER_DEPTH, CONTROLLER_TO_HOST_DEPTH, PACKET_CAPACITY>
where
    M: RawMutex,
{
    /// Construct an empty channel without allocator or runtime registration.
    pub const fn new() -> Self {
        assert!(
            HOST_TO_CONTROLLER_DEPTH > 0,
            "the Host-to-Controller channel needs a packet slot"
        );
        assert!(
            CONTROLLER_TO_HOST_DEPTH > 0,
            "the Controller-to-Host channel needs a packet slot"
        );
        assert!(
            PACKET_CAPACITY > 0,
            "an HCI channel packet slot must retain at least one byte"
        );
        Self {
            identity: 0,
            host_to_controller: AsyncPacketQueue::new(),
            controller_to_host: AsyncPacketQueue::new(),
        }
    }

    /// Split into the Host transport and sole raw Controller endpoint.
    pub fn split(
        &mut self,
    ) -> (
        InProcessHciHostTransport<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        InProcessHciControllerEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) {
        (
            InProcessHciHostTransport {
                host_to_controller: &self.host_to_controller,
                controller_to_host: &self.controller_to_host,
            },
            InProcessHciControllerEndpoint {
                identity: HciEpochIdentity {
                    marker: &self.identity,
                },
                host_to_controller: &self.host_to_controller,
                controller_to_host: &self.controller_to_host,
            },
        )
    }

    /// Whether neither direction retains a packet.
    ///
    /// This observation is intended for an owning lifecycle before endpoints
    /// are published. It does not reserve the channel against concurrent use.
    pub fn is_empty(&self) -> bool {
        self.host_to_controller.is_empty() && self.controller_to_host.is_empty()
    }

    /// Whether no packet has ever entered either direction of this channel.
    ///
    /// Unlike [`Self::is_empty`], draining a packet cannot make the channel
    /// pristine again. Lifecycle owners use this monotonic observation before
    /// binding the channel to a powered Controller epoch.
    pub fn is_pristine(&self) -> bool {
        self.host_to_controller.is_pristine() && self.controller_to_host.is_pristine()
    }
}

impl<
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> Default
    for InProcessHciChannel<M, HOST_TO_CONTROLLER_DEPTH, CONTROLLER_TO_HOST_DEPTH, PACKET_CAPACITY>
where
    M: RawMutex,
{
    fn default() -> Self {
        Self::new()
    }
}

/// Host-facing packet transport accepted by `bt_hci::ExternalController`.
///
/// Writes await bounded capacity and reads await a Controller publication.
/// Dropping either pending future leaves both queues unchanged.
pub struct InProcessHciHostTransport<
    'channel,
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    host_to_controller: &'channel AsyncPacketQueue<M, HOST_TO_CONTROLLER_DEPTH, PACKET_CAPACITY>,
    controller_to_host: &'channel AsyncPacketQueue<M, CONTROLLER_TO_HOST_DEPTH, PACKET_CAPACITY>,
}

impl<
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> ErrorType
    for InProcessHciHostTransport<
        '_,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    type Error = HciChannelError;
}

impl<
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> Transport
    for InProcessHciHostTransport<
        '_,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    async fn read<'buffer>(
        &self,
        buffer: &'buffer mut [u8],
    ) -> Result<ControllerToHostPacket<'buffer>, Self::Error> {
        require_profile_buffer::<PACKET_CAPACITY>(buffer.len())?;
        let slot = self.controller_to_host.receive().await;
        decode_controller_slot(slot, buffer)
    }

    async fn write<T: HostToControllerPacketContract>(&self, value: &T) -> Result<(), Self::Error> {
        let slot = encode_host_packet::<T, PACKET_CAPACITY>(value)?;
        self.host_to_controller.send(slot).await;
        Ok(())
    }
}

/// Raw Controller half of the bounded in-process HCI boundary.
///
/// The future hardware/Link-Layer owner receives Host commands and data here,
/// then publishes only complete validated events or data. The endpoint does
/// not implement radio, Link Layer or HCI command semantics by itself.
pub struct InProcessHciControllerEndpoint<
    'channel,
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    identity: HciEpochIdentity<'channel>,
    host_to_controller: &'channel AsyncPacketQueue<M, HOST_TO_CONTROLLER_DEPTH, PACKET_CAPACITY>,
    controller_to_host: &'channel AsyncPacketQueue<M, CONTROLLER_TO_HOST_DEPTH, PACKET_CAPACITY>,
}

impl<
    'channel,
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    InProcessHciControllerEndpoint<
        'channel,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    /// Identity shared only by endpoints split from this exact channel epoch.
    pub const fn epoch_identity(&self) -> HciEpochIdentity<'channel> {
        self.identity
    }

    /// Await and consume the oldest complete Host packet.
    pub async fn receive<'buffer>(
        &self,
        buffer: &'buffer mut [u8],
    ) -> Result<HostToControllerFrame<'buffer>, HciChannelError> {
        require_profile_buffer::<PACKET_CAPACITY>(buffer.len())?;
        let slot = self.host_to_controller.receive().await;
        decode_host_slot(slot, buffer)
    }

    /// Consume a Host packet immediately or return [`HciChannelError::Empty`].
    pub fn try_receive<'buffer>(
        &self,
        buffer: &'buffer mut [u8],
    ) -> Result<HostToControllerFrame<'buffer>, HciChannelError> {
        require_profile_buffer::<PACKET_CAPACITY>(buffer.len())?;
        let slot = self
            .host_to_controller
            .try_receive()
            .map_err(|()| HciChannelError::Empty)?;
        decode_host_slot(slot, buffer)
    }

    /// Wait until Host-to-Controller storage is observed with a packet.
    ///
    /// This operation neither borrows a packet buffer nor consumes or reserves
    /// the oldest packet. It is a cancellation-safe readiness hint: callers
    /// finish with [`Self::try_receive`] or
    /// [`Self::try_receive_classified_command`] and handle `Empty` losslessly.
    /// The affine Controller endpoint is designed for one logical intake waiter
    /// at a time.
    pub async fn wait_receive_ready(&self) {
        self.host_to_controller.wait_receive_ready().await;
    }

    /// Await, consume and production-classify the oldest Host command.
    ///
    /// Classification is synchronous after queue consumption, so cancellation
    /// while awaiting data cannot create a token or consume a packet. A data
    /// packet is returned with the same epoch proof rather than being discarded.
    pub async fn receive_classified_command<'buffer>(
        &self,
        buffer: &'buffer mut [u8],
    ) -> Result<
        HciEpochBound<'channel, LeControllerCommandClassification>,
        HciEpochBoundCommandReceiveError<'channel, 'buffer>,
    > {
        let frame = self
            .receive(buffer)
            .await
            .map_err(HciEpochBoundCommandReceiveError::Channel)?;
        self.bind_classified_command(frame)
    }

    /// Consume and production-classify the oldest Host command immediately.
    ///
    /// `Empty` and every pre-consumption boundary error create no epoch token.
    pub fn try_receive_classified_command<'buffer>(
        &self,
        buffer: &'buffer mut [u8],
    ) -> Result<
        HciEpochBound<'channel, LeControllerCommandClassification>,
        HciEpochBoundCommandReceiveError<'channel, 'buffer>,
    > {
        let frame = self
            .try_receive(buffer)
            .map_err(HciEpochBoundCommandReceiveError::Channel)?;
        self.bind_classified_command(frame)
    }

    /// Consume and classify one Host packet while returning reusable storage.
    ///
    /// This is the event-loop form of [`Self::try_receive_classified_command`].
    /// An owned command and a stale `Empty` hint return `buffer`; a data packet
    /// instead transfers its borrow to the outer packet router. Other channel
    /// failures are returned to the supervisor and end this intake call.
    pub fn try_receive_classified_command_with_buffer<'buffer>(
        &self,
        buffer: &'buffer mut [u8],
    ) -> HciClassifiedCommandIntake<'channel, 'buffer> {
        if let Err(error) = require_profile_buffer::<PACKET_CAPACITY>(buffer.len()) {
            return HciClassifiedCommandIntake::Channel(error);
        }
        let mut slot = match self.host_to_controller.try_receive() {
            Ok(slot) => slot,
            Err(()) => return HciClassifiedCommandIntake::Empty { buffer },
        };

        if slot.kind == PacketKind::Cmd {
            let bytes = &slot.bytes[..slot.length];
            if validate_host_packet(slot.kind, bytes).is_err() {
                slot.bytes[..slot.length].fill(0);
                return HciClassifiedCommandIntake::Channel(HciChannelError::CorruptRetainedPacket);
            }
            let command = command_from_validated_bytes(bytes);
            let command =
                HciEpochBound::bind(self.identity, classify_le_controller_command(command));
            slot.bytes[..slot.length].fill(0);
            return HciClassifiedCommandIntake::Command { command, buffer };
        }

        match decode_host_slot(slot, buffer) {
            Ok(frame) => {
                HciClassifiedCommandIntake::NonCommand(HciEpochBound::bind(self.identity, frame))
            }
            Err(error) => HciClassifiedCommandIntake::Channel(error),
        }
    }

    fn bind_classified_command<'buffer>(
        &self,
        frame: HostToControllerFrame<'buffer>,
    ) -> Result<
        HciEpochBound<'channel, LeControllerCommandClassification>,
        HciEpochBoundCommandReceiveError<'channel, 'buffer>,
    > {
        match frame {
            HostToControllerFrame::Command(command) => Ok(HciEpochBound::bind(
                self.identity,
                classify_le_controller_command(command),
            )),
            frame => Err(HciEpochBoundCommandReceiveError::NonCommand(
                HciEpochBound::bind(self.identity, frame),
            )),
        }
    }

    /// Validate and asynchronously publish one Controller packet.
    pub async fn publish(&self, kind: PacketKind, bytes: &[u8]) -> Result<(), HciChannelError> {
        let slot = controller_slot::<PACKET_CAPACITY>(kind, bytes)?;
        self.controller_to_host.send(slot).await;
        Ok(())
    }

    /// Wait until Controller-to-Host storage is observed with free capacity.
    ///
    /// This operation does not reserve a slot or retain packet bytes. It is a
    /// cancellation-safe readiness hint: after it returns, another producer
    /// may still win the slot, so callers must finish with [`Self::try_publish`]
    /// and handle `Full` losslessly. The affine Controller endpoint is designed
    /// for one logical publication waiter at a time.
    pub async fn wait_publish_ready(&self) {
        self.controller_to_host.wait_send_ready().await;
    }

    /// Publish immediately or return [`HciChannelError::Full`] without overwrite.
    pub fn try_publish(&self, kind: PacketKind, bytes: &[u8]) -> Result<(), HciChannelError> {
        let slot = controller_slot::<PACKET_CAPACITY>(kind, bytes)?;
        self.controller_to_host
            .try_send(slot)
            .map_err(|()| HciChannelError::Full)
    }
}

fn require_profile_buffer<const PACKET_CAPACITY: usize>(
    available: usize,
) -> Result<(), HciChannelError> {
    if available < PACKET_CAPACITY {
        Err(HciChannelError::DestinationTooSmall {
            required: PACKET_CAPACITY,
            available,
        })
    } else {
        Ok(())
    }
}

fn controller_slot<const PACKET_CAPACITY: usize>(
    kind: PacketKind,
    bytes: &[u8],
) -> Result<PacketSlot<PACKET_CAPACITY>, HciChannelError> {
    if kind == PacketKind::Cmd {
        return Err(HciChannelError::InvalidDirection);
    }
    if bytes.len() > PACKET_CAPACITY {
        return Err(HciChannelError::PacketTooLong {
            length: bytes.len(),
            capacity: PACKET_CAPACITY,
        });
    }
    decode_complete_packet(kind, bytes).map_err(map_controller_packet_error)?;

    let mut slot = PacketSlot::EMPTY;
    slot.kind = kind;
    slot.length = bytes.len();
    slot.bytes[..bytes.len()].copy_from_slice(bytes);
    Ok(slot)
}

fn decode_controller_slot<'buffer, const PACKET_CAPACITY: usize>(
    mut slot: PacketSlot<PACKET_CAPACITY>,
    buffer: &'buffer mut [u8],
) -> Result<ControllerToHostPacket<'buffer>, HciChannelError> {
    buffer[..slot.length].copy_from_slice(&slot.bytes[..slot.length]);
    slot.bytes[..slot.length].fill(0);
    decode_complete_packet(slot.kind, &buffer[..slot.length])
        .map_err(|_| HciChannelError::CorruptRetainedPacket)
}

fn map_controller_packet_error(error: ControllerToHostQueueError) -> HciChannelError {
    match error {
        ControllerToHostQueueError::InvalidDirection => HciChannelError::InvalidDirection,
        ControllerToHostQueueError::PacketTooLong { length, capacity } => {
            HciChannelError::PacketTooLong { length, capacity }
        }
        ControllerToHostQueueError::DestinationTooSmall {
            required,
            available,
        } => HciChannelError::DestinationTooSmall {
            required,
            available,
        },
        ControllerToHostQueueError::InvalidPacket(error) => HciChannelError::InvalidPacket(error),
        ControllerToHostQueueError::TrailingBytes => HciChannelError::TrailingBytes,
        ControllerToHostQueueError::Full
        | ControllerToHostQueueError::Empty
        | ControllerToHostQueueError::CorruptRetainedPacket => {
            HciChannelError::CorruptRetainedPacket
        }
    }
}

fn encode_host_packet<T: HostToControllerPacketContract, const PACKET_CAPACITY: usize>(
    value: &T,
) -> Result<PacketSlot<PACKET_CAPACITY>, HciChannelError> {
    if T::KIND == PacketKind::Event {
        return Err(HciChannelError::InvalidDirection);
    }

    let declared = value.size();
    if declared > PACKET_CAPACITY {
        return Err(HciChannelError::PacketTooLong {
            length: declared,
            capacity: PACKET_CAPACITY,
        });
    }

    let mut slot = PacketSlot::EMPTY;
    let written = {
        let mut writer = PacketWriter::new(&mut slot.bytes);
        value.write_hci(&mut writer)?;
        writer.written()
    };
    if written != declared {
        slot.bytes[..written].fill(0);
        return Err(HciChannelError::SerializationLengthMismatch { declared, written });
    }
    validate_host_packet(T::KIND, &slot.bytes[..written])?;
    slot.kind = T::KIND;
    slot.length = written;
    Ok(slot)
}

fn validate_host_packet(kind: PacketKind, bytes: &[u8]) -> Result<(), HciChannelError> {
    validate_host_declared_length(kind, bytes)?;
    match kind {
        PacketKind::Cmd => Ok(()),
        PacketKind::AclData => AclPacket::from_hci_bytes(bytes)
            .map_err(HciChannelError::InvalidPacket)
            .and_then(|(_, remaining)| require_no_remaining(remaining)),
        PacketKind::SyncData => SyncPacket::from_hci_bytes(bytes)
            .map_err(HciChannelError::InvalidPacket)
            .and_then(|(_, remaining)| require_no_remaining(remaining)),
        PacketKind::IsoData => IsoPacket::from_hci_bytes(bytes)
            .map_err(HciChannelError::InvalidPacket)
            .and_then(|(_, remaining)| require_no_remaining(remaining)),
        PacketKind::Event => Err(HciChannelError::InvalidDirection),
    }
}

fn validate_host_declared_length(kind: PacketKind, bytes: &[u8]) -> Result<(), HciChannelError> {
    let declared = match kind {
        PacketKind::Cmd => {
            let Some(length) = bytes.get(2) else {
                return Err(HciChannelError::InvalidPacket(
                    FromHciBytesError::InvalidSize,
                ));
            };
            3 + usize::from(*length)
        }
        PacketKind::AclData => declared_u16_length(bytes, 4, 2, 0xffff)?,
        PacketKind::SyncData => {
            let Some(length) = bytes.get(2) else {
                return Err(HciChannelError::InvalidPacket(
                    FromHciBytesError::InvalidSize,
                ));
            };
            3 + usize::from(*length)
        }
        PacketKind::IsoData => declared_u16_length(bytes, 4, 2, 0x3fff)?,
        PacketKind::Event => return Err(HciChannelError::InvalidDirection),
    };

    if bytes.len() < declared {
        Err(HciChannelError::InvalidPacket(
            FromHciBytesError::InvalidSize,
        ))
    } else if bytes.len() > declared {
        Err(HciChannelError::TrailingBytes)
    } else {
        Ok(())
    }
}

fn declared_u16_length(
    bytes: &[u8],
    header_length: usize,
    length_offset: usize,
    mask: u16,
) -> Result<usize, HciChannelError> {
    let Some(length) = bytes.get(length_offset..length_offset + 2) else {
        return Err(HciChannelError::InvalidPacket(
            FromHciBytesError::InvalidSize,
        ));
    };
    Ok(header_length + usize::from(u16::from_le_bytes([length[0], length[1]]) & mask))
}

fn require_no_remaining(remaining: &[u8]) -> Result<(), HciChannelError> {
    if remaining.is_empty() {
        Ok(())
    } else {
        Err(HciChannelError::TrailingBytes)
    }
}

fn decode_host_slot<'buffer, const PACKET_CAPACITY: usize>(
    mut slot: PacketSlot<PACKET_CAPACITY>,
    buffer: &'buffer mut [u8],
) -> Result<HostToControllerFrame<'buffer>, HciChannelError> {
    buffer[..slot.length].copy_from_slice(&slot.bytes[..slot.length]);
    slot.bytes[..slot.length].fill(0);
    let bytes = &buffer[..slot.length];
    validate_host_packet(slot.kind, bytes).map_err(|_| HciChannelError::CorruptRetainedPacket)?;

    match slot.kind {
        PacketKind::Cmd => Ok(HostToControllerFrame::Command(
            command_from_validated_bytes(bytes),
        )),
        PacketKind::AclData => AclPacket::from_hci_bytes(bytes)
            .map(|(packet, _)| HostToControllerFrame::Acl(packet))
            .map_err(|_| HciChannelError::CorruptRetainedPacket),
        PacketKind::SyncData => SyncPacket::from_hci_bytes(bytes)
            .map(|(packet, _)| HostToControllerFrame::Sync(packet))
            .map_err(|_| HciChannelError::CorruptRetainedPacket),
        PacketKind::IsoData => IsoPacket::from_hci_bytes(bytes)
            .map(|(packet, _)| HostToControllerFrame::Iso(packet))
            .map_err(|_| HciChannelError::CorruptRetainedPacket),
        PacketKind::Event => Err(HciChannelError::CorruptRetainedPacket),
    }
}

fn command_from_validated_bytes(bytes: &[u8]) -> HciCommandPacket<'_> {
    let raw = u16::from_le_bytes([bytes[0], bytes[1]]);
    HciCommandPacket {
        opcode: Opcode::new(OpcodeGroup::new((raw >> 10) as u8), raw & 0x03ff),
        parameters: &bytes[3..],
    }
}

struct PacketWriter<'buffer> {
    buffer: &'buffer mut [u8],
    written: usize,
}

impl<'buffer> PacketWriter<'buffer> {
    fn new(buffer: &'buffer mut [u8]) -> Self {
        Self { buffer, written: 0 }
    }

    fn written(&self) -> usize {
        self.written
    }
}

impl ErrorType for PacketWriter<'_> {
    type Error = HciChannelError;
}

impl Write for PacketWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error> {
        if bytes.len() > self.buffer.len().saturating_sub(self.written) {
            return Err(HciChannelError::SerializationOverflow {
                capacity: self.buffer.len(),
            });
        }
        let end = self.written + bytes.len();
        self.buffer[self.written..end].copy_from_slice(bytes);
        self.written = end;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use core::{
        future::Future,
        pin::{Pin, pin},
        task::{Context, Poll, Waker},
    };

    use bt_hci::{
        ControllerToHostPacket, PacketKind,
        cmd::{Cmd, SyncCmd, controller_baseband::Reset, le::LeTestEnd},
        controller::{Controller, ExternalController},
        data::{AclBroadcastFlag, AclPacket, AclPacketBoundary},
        param::ConnHandle,
        transport::Transport,
    };
    use embassy_futures::{
        block_on,
        join::{join, join3},
    };
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;

    use super::{
        HciChannelError, HciClassifiedCommandIntake, HciEpochBoundCommandReceiveError,
        HostToControllerFrame, InProcessHciChannel,
    };
    use crate::{LE_TEST_END_OPCODE, LeControllerCommandClassification, LeDtmCommand};

    const RESET_COMMAND_COMPLETE: [u8; 6] = [0x0e, 0x04, 0x01, 0x03, 0x0c, 0x00];
    const HARDWARE_ERROR: [u8; 3] = [0x10, 0x01, 0x42];

    type TestChannel = InProcessHciChannel<NoopRawMutex, 1, 1, 16>;

    #[test]
    fn controller_epoch_identity_distinguishes_live_channels() {
        let mut first = TestChannel::new();
        let mut second = TestChannel::new();
        let (_, first_controller) = first.split();
        let (_, second_controller) = second.split();

        let first_identity = first_controller.epoch_identity();
        assert!(first_identity.same_epoch(first_controller.epoch_identity()));
        assert!(!first_identity.same_epoch(second_controller.epoch_identity()));
    }

    #[test]
    fn epoch_bound_test_end_rejects_a_live_cross_wired_endpoint() {
        let mut first_channel = TestChannel::new();
        let (first_host, first_controller) = first_channel.split();
        let mut second_channel = TestChannel::new();
        let (_second_host, second_controller) = second_channel.split();

        block_on(first_host.write(&LeTestEnd::new())).expect("Test End enters its source queue");
        let mut buffer = [0; 16];
        let bound = match first_controller.try_receive_classified_command(&mut buffer) {
            Ok(bound) => bound,
            Err(_) => panic!("the source endpoint must classify its oldest Test End"),
        };
        let bound = match bound.try_into_dtm() {
            Ok(bound) => bound,
            Err(_) => panic!("Test End must retain an owned DTM command"),
        };
        let bound = match bound.try_into_test_end() {
            Ok(bound) => bound,
            Err(_) => panic!("the DTM command must retain semantic Test End ownership"),
        };

        assert!(bound.originates_from(&first_controller));
        assert!(!bound.originates_from(&second_controller));
        let bound = match bound.try_into_for_endpoint(&second_controller) {
            Ok(_) => panic!("a foreign live endpoint must not consume the semantic owner"),
            Err(bound) => bound,
        };
        let command = match bound.try_into_for_endpoint(&first_controller) {
            Ok(command) => command,
            Err(_) => panic!("the source endpoint must recover its semantic owner"),
        };
        let response = command.into_ended_command_complete(0x1234);
        assert_eq!(response.opcode(), LE_TEST_END_OPCODE);
    }

    #[test]
    fn empty_and_cancelled_receive_create_no_epoch_token_or_consumption() {
        let mut channel = TestChannel::new();
        let (host, controller) = channel.split();
        let mut buffer = [0; 16];

        assert!(matches!(
            controller.try_receive_classified_command(&mut buffer),
            Err(HciEpochBoundCommandReceiveError::Channel(
                HciChannelError::Empty
            ))
        ));

        {
            let mut cancelled = pin!(controller.receive_classified_command(&mut buffer));
            assert_pending(cancelled.as_mut());
        }

        block_on(host.write(&LeTestEnd::new()))
            .expect("the replacement receive gets one queued command");
        let bound = match block_on(controller.receive_classified_command(&mut buffer)) {
            Ok(bound) => bound,
            Err(_) => panic!("cancelled empty receive must leave the later packet available"),
        };
        assert_eq!(bound.value().opcode(), LE_TEST_END_OPCODE);
        assert!(bound.originates_from(&controller));
        assert!(matches!(
            controller.try_receive_classified_command(&mut buffer),
            Err(HciEpochBoundCommandReceiveError::Channel(
                HciChannelError::Empty
            ))
        ));
    }

    #[test]
    fn epoch_bound_classification_preserves_host_command_fifo() {
        type FifoChannel = InProcessHciChannel<NoopRawMutex, 2, 1, 16>;

        let mut channel = FifoChannel::new();
        let (host, controller) = channel.split();
        block_on(async {
            host.write(&LeTestEnd::new()).await.unwrap();
            host.write(&Reset::new()).await.unwrap();
        });

        let mut buffer = [0; 16];
        let first = match controller.try_receive_classified_command(&mut buffer) {
            Ok(bound) => bound,
            Err(_) => panic!("the oldest Test End must be classified first"),
        };
        assert_eq!(first.value().opcode(), LE_TEST_END_OPCODE);
        assert!(matches!(
            first.value(),
            LeControllerCommandClassification::Dtm(LeDtmCommand::TestEnd(_))
        ));

        let second = match controller.try_receive_classified_command(&mut buffer) {
            Ok(bound) => bound,
            Err(_) => panic!("Reset must remain second in the Host FIFO"),
        };
        assert_eq!(second.value().opcode(), Reset::OPCODE);
        assert!(matches!(
            second.value(),
            LeControllerCommandClassification::Bootstrap(_)
        ));
        assert!(first.originates_from(&controller));
        assert!(second.originates_from(&controller));
    }

    #[test]
    fn publish_readiness_wait_is_side_effect_free_and_wakes_after_drain() {
        let mut channel = TestChannel::new();
        let (host, controller) = channel.split();

        block_on(controller.wait_publish_ready());
        controller
            .try_publish(PacketKind::Event, &HARDWARE_ERROR)
            .unwrap();

        let mut wait = pin!(controller.wait_publish_ready());
        assert_pending(wait.as_mut());
        let mut event_buffer = [0; 16];
        block_on(host.read(&mut event_buffer)).unwrap();
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(wait.as_mut().poll(&mut context), Poll::Ready(())));

        controller
            .try_publish(PacketKind::Event, &RESET_COMMAND_COMPLETE)
            .expect("readiness did not manufacture or reserve a packet");
    }

    #[test]
    fn receive_readiness_wait_is_side_effect_free_and_wakes_after_publish() {
        let mut channel = TestChannel::new();
        let (host, controller) = channel.split();

        {
            let mut wait = pin!(controller.wait_receive_ready());
            assert_pending(wait.as_mut());
            block_on(host.write(&LeTestEnd::new())).unwrap();
            let mut context = Context::from_waker(Waker::noop());
            assert!(matches!(wait.as_mut().poll(&mut context), Poll::Ready(())));
        }

        let mut buffer = [0; 16];
        let command = match controller.try_receive_classified_command(&mut buffer) {
            Ok(command) => command,
            Err(_) => panic!("readiness must leave the exact oldest packet queued"),
        };
        assert_eq!(command.value().opcode(), LE_TEST_END_OPCODE);
        assert!(matches!(
            controller.try_receive_classified_command(&mut buffer),
            Err(HciEpochBoundCommandReceiveError::Channel(
                HciChannelError::Empty
            ))
        ));
    }

    #[test]
    fn cancelled_receive_readiness_wait_consumes_and_reserves_nothing() {
        let mut channel = TestChannel::new();
        let (host, controller) = channel.split();

        {
            let mut cancelled = pin!(controller.wait_receive_ready());
            assert_pending(cancelled.as_mut());
        }

        block_on(host.write(&LeTestEnd::new())).unwrap();
        block_on(controller.wait_receive_ready());
        block_on(controller.wait_receive_ready());

        let mut buffer = [0; 16];
        let command = match controller.try_receive_classified_command(&mut buffer) {
            Ok(command) => command,
            Err(_) => {
                panic!("cancelled and repeated readiness waits must not consume the packet")
            }
        };
        assert_eq!(command.value().opcode(), LE_TEST_END_OPCODE);
    }

    #[test]
    fn event_loop_intake_returns_buffer_for_commands_and_stale_empty() {
        type FifoChannel = InProcessHciChannel<NoopRawMutex, 2, 1, 16>;

        let mut channel = FifoChannel::new();
        let (host, controller) = channel.split();
        block_on(async {
            host.write(&LeTestEnd::new()).await.unwrap();
            host.write(&Reset::new()).await.unwrap();
        });

        let mut storage = [0; 16];
        let (first, buffer) =
            match controller.try_receive_classified_command_with_buffer(&mut storage) {
                HciClassifiedCommandIntake::Command { command, buffer } => (command, buffer),
                _ => panic!("the first command must return reusable storage"),
            };
        assert_eq!(first.value().opcode(), LE_TEST_END_OPCODE);

        let (second, buffer) = match controller.try_receive_classified_command_with_buffer(buffer) {
            HciClassifiedCommandIntake::Command { command, buffer } => (command, buffer),
            _ => panic!("the second command must reuse the same storage"),
        };
        assert_eq!(second.value().opcode(), Reset::OPCODE);

        let buffer = match controller.try_receive_classified_command_with_buffer(buffer) {
            HciClassifiedCommandIntake::Empty { buffer } => buffer,
            _ => panic!("stale readiness must return storage for another wait"),
        };
        assert_eq!(buffer.len(), storage.len());
    }

    #[test]
    fn event_loop_intake_transfers_exact_data_frame_to_outer_router() {
        let mut channel = TestChannel::new();
        let (host, controller) = channel.split();
        let acl = AclPacket::new(
            ConnHandle::new(7),
            AclPacketBoundary::Complete,
            AclBroadcastFlag::PointToPoint,
            &[11, 13],
        );
        block_on(host.write(&acl)).unwrap();

        let mut storage = [0; 16];
        let frame = match controller.try_receive_classified_command_with_buffer(&mut storage) {
            HciClassifiedCommandIntake::NonCommand(frame) => frame,
            _ => panic!("the data packet must transfer its exact borrowed frame"),
        };
        assert!(frame.originates_from(&controller));
        let HostToControllerFrame::Acl(received) = frame.value() else {
            panic!("the outer router must receive the original ACL kind");
        };
        assert_eq!(received.handle(), ConnHandle::new(7));
        assert_eq!(received.data(), &[11, 13]);
    }

    #[test]
    fn cancelled_publish_readiness_wait_leaves_capacity_for_replacement() {
        let mut channel = TestChannel::new();
        let (host, controller) = channel.split();
        controller
            .try_publish(PacketKind::Event, &HARDWARE_ERROR)
            .unwrap();

        {
            let mut cancelled = pin!(controller.wait_publish_ready());
            assert_pending(cancelled.as_mut());
        }

        let mut event_buffer = [0; 16];
        block_on(host.read(&mut event_buffer)).unwrap();
        block_on(controller.wait_publish_ready());
        controller
            .try_publish(PacketKind::Event, &RESET_COMMAND_COMPLETE)
            .expect("cancelled readiness did not consume the released slot");
    }

    #[test]
    fn typed_reset_and_event_cross_the_direct_hci_boundary() {
        let mut channel = TestChannel::new();
        let (host, controller) = channel.split();

        block_on(async {
            host.write(&Reset::new()).await.unwrap();
            let mut command_buffer = [0; 16];
            let HostToControllerFrame::Command(command) =
                controller.receive(&mut command_buffer).await.unwrap()
            else {
                panic!("Reset changed packet kind");
            };
            assert_eq!(command.opcode(), Reset::OPCODE);
            assert!(command.parameters().is_empty());
            assert!(controller.host_to_controller.vacant_storage_is_zeroed());

            controller
                .publish(PacketKind::Event, &HARDWARE_ERROR)
                .await
                .unwrap();
            let mut event_buffer = [0; 16];
            let ControllerToHostPacket::Event(event) = host.read(&mut event_buffer).await.unwrap()
            else {
                panic!("Hardware Error changed packet kind");
            };
            assert_eq!(event.data, &[0x42]);
            assert!(controller.controller_to_host.vacant_storage_is_zeroed());
        });
    }

    #[test]
    fn external_controller_completes_a_command_via_the_same_event_loop() {
        let mut channel = TestChannel::new();
        let (host, controller) = channel.split();
        let external = ExternalController::<_, 1>::new(host);

        block_on(async {
            let reset = Reset::new();
            let mut event_buffer = [0; 16];
            let worker = async {
                let mut command_buffer = [0; 16];
                let HostToControllerFrame::Command(command) =
                    controller.receive(&mut command_buffer).await.unwrap()
                else {
                    panic!("Reset changed packet kind");
                };
                assert_eq!(command.opcode(), Reset::OPCODE);
                controller
                    .publish(PacketKind::Event, &RESET_COMMAND_COMPLETE)
                    .await
                    .unwrap();
                controller
                    .publish(PacketKind::Event, &HARDWARE_ERROR)
                    .await
                    .unwrap();
            };

            let (completed, received, ()) = join3(
                reset.exec(&external),
                external.read(&mut event_buffer),
                worker,
            )
            .await;
            completed.unwrap();
            assert!(matches!(
                received.unwrap(),
                ControllerToHostPacket::Event(_)
            ));
        });
    }

    #[test]
    fn both_async_directions_wake_without_polling_or_an_rtos() {
        let mut channel = TestChannel::new();
        let (host, controller) = channel.split();

        block_on(async {
            let mut command_buffer = [0; 16];
            let (received, sent) = join(
                controller.receive(&mut command_buffer),
                host.write(&Reset::new()),
            )
            .await;
            sent.unwrap();
            assert!(matches!(
                received.unwrap(),
                HostToControllerFrame::Command(_)
            ));

            let mut event_buffer = [0; 16];
            let (received, sent) = join(
                host.read(&mut event_buffer),
                controller.publish(PacketKind::Event, &HARDWARE_ERROR),
            )
            .await;
            sent.unwrap();
            assert!(matches!(
                received.unwrap(),
                ControllerToHostPacket::Event(_)
            ));
        });
    }

    #[test]
    fn cancelled_backpressure_waits_never_publish_a_packet() {
        let mut channel = TestChannel::new();
        let (host, controller) = channel.split();
        let reset = Reset::new();

        block_on(async {
            host.write(&reset).await.unwrap();
            {
                let mut second_write = pin!(host.write(&reset));
                assert_pending(second_write.as_mut());
            }

            let mut command_buffer = [0; 16];
            assert!(controller.receive(&mut command_buffer).await.is_ok());
            assert!(matches!(
                controller.try_receive(&mut command_buffer),
                Err(HciChannelError::Empty)
            ));

            controller
                .publish(PacketKind::Event, &HARDWARE_ERROR)
                .await
                .unwrap();
            {
                let mut second_publish =
                    pin!(controller.publish(PacketKind::Event, &RESET_COMMAND_COMPLETE));
                assert_pending(second_publish.as_mut());
            }

            let mut event_buffer = [0; 16];
            assert!(host.read(&mut event_buffer).await.is_ok());
            controller
                .try_publish(PacketKind::Event, &RESET_COMMAND_COMPLETE)
                .unwrap();
            assert!(host.read(&mut event_buffer).await.is_ok());
        });
    }

    #[test]
    fn short_profile_buffers_fail_before_consuming_either_direction() {
        let mut channel = TestChannel::new();
        let (host, controller) = channel.split();

        block_on(async {
            host.write(&Reset::new()).await.unwrap();
            let mut short = [0; 15];
            assert!(matches!(
                controller.receive(&mut short).await,
                Err(HciChannelError::DestinationTooSmall {
                    required: 16,
                    available: 15,
                })
            ));
            let mut complete = [0; 16];
            assert!(controller.receive(&mut complete).await.is_ok());

            controller
                .publish(PacketKind::Event, &HARDWARE_ERROR)
                .await
                .unwrap();
            assert!(matches!(
                host.read(&mut short).await,
                Err(HciChannelError::DestinationTooSmall {
                    required: 16,
                    available: 15,
                })
            ));
            assert!(host.read(&mut complete).await.is_ok());
        });
    }

    #[test]
    fn try_publication_rejects_direction_length_and_overwrite() {
        let mut channel = TestChannel::new();
        let (_host, controller) = channel.split();

        assert_eq!(
            controller.try_publish(PacketKind::Cmd, &[0x03, 0x0c, 0x00]),
            Err(HciChannelError::InvalidDirection)
        );
        assert_eq!(
            controller.try_publish(PacketKind::Event, &[0x10, 0x00, 0xff]),
            Err(HciChannelError::TrailingBytes)
        );
        controller
            .try_publish(PacketKind::Event, &HARDWARE_ERROR)
            .unwrap();
        assert_eq!(
            controller.try_publish(PacketKind::Event, &RESET_COMMAND_COMPLETE),
            Err(HciChannelError::Full)
        );
    }

    #[test]
    fn async_queue_preserves_fifo_across_ring_wrap() {
        type RingChannel = InProcessHciChannel<NoopRawMutex, 2, 2, 16>;
        let mut channel = RingChannel::new();
        let (host, controller) = channel.split();
        let first = [0x10, 0x01, 0x11];
        let second = [0x10, 0x01, 0x22];
        let third = [0x10, 0x01, 0x33];

        block_on(async {
            controller.publish(PacketKind::Event, &first).await.unwrap();
            controller
                .publish(PacketKind::Event, &second)
                .await
                .unwrap();
            let mut buffer = [0; 16];
            assert_eq!(event_parameter(host.read(&mut buffer).await.unwrap()), 0x11);
            controller.publish(PacketKind::Event, &third).await.unwrap();
            assert_eq!(event_parameter(host.read(&mut buffer).await.unwrap()), 0x22);
            assert_eq!(event_parameter(host.read(&mut buffer).await.unwrap()), 0x33);
            assert!(controller.controller_to_host.vacant_storage_is_zeroed());
        });
    }

    #[test]
    fn every_host_packet_header_rejects_declared_length_mismatch() {
        for (kind, bytes) in [
            (PacketKind::Cmd, &[0x03, 0x0c, 0x00, 0xaa][..]),
            (PacketKind::AclData, &[0x01, 0x00, 0x00, 0x00, 0xaa][..]),
            (PacketKind::SyncData, &[0x01, 0x00, 0x00, 0xbb][..]),
            (PacketKind::IsoData, &[0x01, 0x00, 0x00, 0x00, 0xcc][..]),
        ] {
            assert_eq!(
                super::validate_host_packet(kind, bytes),
                Err(HciChannelError::TrailingBytes)
            );
        }
        assert_eq!(
            super::validate_host_packet(PacketKind::Cmd, &[0x03, 0x0c, 0x01]),
            Err(HciChannelError::InvalidPacket(
                bt_hci::FromHciBytesError::InvalidSize
            ))
        );
        assert_eq!(
            super::validate_host_packet(PacketKind::Event, &HARDWARE_ERROR),
            Err(HciChannelError::InvalidDirection)
        );
    }

    fn event_parameter(packet: ControllerToHostPacket<'_>) -> u8 {
        let ControllerToHostPacket::Event(event) = packet else {
            panic!("event changed packet kind");
        };
        event.data[0]
    }

    fn assert_pending<F: Future>(mut future: Pin<&mut F>) {
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(future.as_mut().poll(&mut context), Poll::Pending));
    }
}
