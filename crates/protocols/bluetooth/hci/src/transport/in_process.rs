//! Bounded in-process Host/Controller endpoints and HCI epoch authority.

use core::{cell::RefCell, convert::Infallible, fmt, future::poll_fn, task::Poll};

use bt_hci::{
    FromHciBytes, FromHciBytesError, PacketKind, ReadHciError,
    data::{AclPacket, IsoPacket, SyncPacket},
    transport::{PacketToController, PacketToHost, Transport},
};

use embassy_sync::{
    blocking_mutex::{Mutex, raw::RawMutex},
    waitqueue::WakerRegistration,
};

use embedded_io::{ErrorKind, ErrorType, ReadExactError, Write};

use super::packet::{PacketSlot, decode_complete_packet};

mod codec;
mod queue;
#[cfg(test)]
use codec::decode_host_slot;
use codec::{
    controller_slot, decode_controller_slot, encode_host_packet, require_profile_buffer,
    validate_host_packet,
};
use queue::AsyncPacketQueue;

use crate::{
    ControllerToHostQueueError, HostToControllerFrame, LeControllerCommandClassification,
    classify_le_controller_command, wire::command_from_validated_bytes,
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
/// Only the private `InProcessHciControllerEndpoint` can mint this token, immediately
/// after consuming and classifying a packet from its Host-to-Controller queue.
/// The epoch marker remains opaque. Only crate-defined consuming refinements
/// may change the semantic type while retaining that proof.
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

    /// Attempt one consuming semantic refinement without losing either branch.
    pub(crate) fn try_map<U>(
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
    pub(crate) fn originates_from<
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
    pub(crate) fn try_into_for_endpoint<
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
    /// A `PacketToController` wrote beyond the selected storage profile.
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

impl From<ReadHciError<Infallible>> for HciChannelError {
    fn from(error: ReadHciError<Infallible>) -> Self {
        match error {
            ReadHciError::BufferTooSmall | ReadHciError::Read(ReadExactError::UnexpectedEof) => {
                Self::InvalidPacket(FromHciBytesError::InvalidSize)
            }
            ReadHciError::InvalidValue => Self::InvalidPacket(FromHciBytesError::InvalidValue),
            ReadHciError::Read(ReadExactError::Other(never)) => match never {},
        }
    }
}

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

/// Failure to consume one Host packet as an epoch-bound classified command.
#[cfg(test)]
#[expect(
    dead_code,
    reason = "the crate-private raw test harness retains its lossless data branch"
)]
#[must_use = "a non-command frame retains its origin proof and borrowed packet"]
pub(crate) enum HciEpochBoundCommandReceiveError<'epoch, 'packet> {
    /// The queue or packet boundary failed before an epoch token was created.
    Channel(HciChannelError),
    /// The oldest packet was data rather than a command and remains bound to its epoch.
    NonCommand(HciEpochBound<'epoch, HostToControllerFrame<'packet>>),
}

/// One non-blocking classified intake that preserves receive-buffer ownership.
///
/// Command, stale-readiness and channel-failure branches return the caller's
/// buffer so an event loop can immediately reuse or replace it. Only a
/// non-command frame borrows the buffer, transferring that exact packet to the
/// outer data router. A malformed retained packet fails closed without hiding
/// the caller's storage.
#[must_use = "route the packet or recover the receive buffer"]
pub(crate) enum HciClassifiedCommandIntake<'epoch, 'packet> {
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
    /// A packet-boundary failure plus reusable scratch storage.
    Channel {
        /// Exact transport failure.
        error: HciChannelError,
        /// Scratch storage available to the supervisor or corrected retry.
        buffer: &'packet mut [u8],
    },
    /// The oldest Host packet is data and retains its buffer borrow.
    NonCommand(HciEpochBound<'epoch, HostToControllerFrame<'packet>>),
}

/// Two bounded packet queues joining an HCI Host and one raw Controller owner.
///
/// Packet indicators are retained as typed [`PacketKind`] values and are never
/// serialized as UART/H4 bytes. Calling [`Self::split`] requires exclusive
/// access, so safe code cannot manufacture a second endpoint pair while the
/// first pair is alive. `M` selects the synchronization domain; a platform may
/// use a critical-section mutex for IRQ/task handoff without introducing an
/// RTOS.
pub(crate) struct InProcessHciChannel<
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
    pub(crate) const fn new() -> Self {
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

    /// Split into the Host transport and crate-private raw Controller endpoint.
    pub(crate) fn split(
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

    /// Whether no packet has ever entered either direction of this channel.
    ///
    /// Draining a packet cannot make the channel pristine again. Lifecycle
    /// owners use this monotonic observation before binding the channel to a
    /// powered Controller epoch.
    pub(crate) fn is_pristine(&self) -> bool {
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
    async fn read<'buffer, P: PacketToHost<'buffer>>(
        &self,
        buffer: &'buffer mut [u8],
    ) -> Result<P, Self::Error> {
        require_profile_buffer::<PACKET_CAPACITY>(buffer.len())?;
        let slot = self.controller_to_host.receive().await;
        decode_controller_slot(slot, buffer)
    }

    async fn write<T: PacketToController>(&self, value: &T) -> Result<(), Self::Error> {
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
pub(crate) struct InProcessHciControllerEndpoint<
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
    pub(crate) const fn epoch_identity(&self) -> HciEpochIdentity<'channel> {
        self.identity
    }

    /// Await and consume the oldest complete Host packet.
    #[cfg(test)]
    pub(crate) async fn receive<'buffer>(
        &self,
        buffer: &'buffer mut [u8],
    ) -> Result<HostToControllerFrame<'buffer>, HciChannelError> {
        require_profile_buffer::<PACKET_CAPACITY>(buffer.len())?;
        let slot = self.host_to_controller.receive().await;
        decode_host_slot(slot, buffer)
    }

    /// Consume a Host packet immediately or return [`HciChannelError::Empty`].
    #[cfg(test)]
    pub(crate) fn try_receive<'buffer>(
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
    /// finish with [`Self::try_receive_classified_command_with_buffer`] and
    /// handle `Empty` losslessly.
    /// The affine Controller endpoint is designed for one logical intake waiter
    /// at a time.
    pub(crate) async fn wait_receive_ready(&self) {
        self.host_to_controller.wait_receive_ready().await;
    }

    /// Await, consume and production-classify the oldest Host command.
    ///
    /// Classification is synchronous after queue consumption, so cancellation
    /// while awaiting data cannot create a token or consume a packet. A data
    /// packet is returned with the same epoch proof rather than being discarded.
    #[cfg(test)]
    pub(crate) async fn receive_classified_command<'buffer>(
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
    #[cfg(test)]
    pub(crate) fn try_receive_classified_command<'buffer>(
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
    /// failures return the scratch storage to the supervisor.
    pub(crate) fn try_receive_classified_command_with_buffer<'buffer>(
        &self,
        buffer: &'buffer mut [u8],
    ) -> HciClassifiedCommandIntake<'channel, 'buffer> {
        if let Err(error) = require_profile_buffer::<PACKET_CAPACITY>(buffer.len()) {
            return HciClassifiedCommandIntake::Channel { error, buffer };
        }
        let mut slot = match self.host_to_controller.try_receive() {
            Ok(slot) => slot,
            Err(()) => return HciClassifiedCommandIntake::Empty { buffer },
        };

        if slot.kind == PacketKind::Cmd {
            let bytes = &slot.bytes[..slot.length];
            if validate_host_packet(slot.kind, bytes).is_err() {
                slot.bytes[..slot.length].fill(0);
                return HciClassifiedCommandIntake::Channel {
                    error: HciChannelError::CorruptRetainedPacket,
                    buffer,
                };
            }
            let command = command_from_validated_bytes(bytes);
            let command =
                HciEpochBound::bind(self.identity, classify_le_controller_command(command));
            slot.bytes[..slot.length].fill(0);
            return HciClassifiedCommandIntake::Command { command, buffer };
        }

        let bytes = &slot.bytes[..slot.length];
        if validate_host_packet(slot.kind, bytes).is_err() {
            slot.bytes[..slot.length].fill(0);
            return HciClassifiedCommandIntake::Channel {
                error: HciChannelError::CorruptRetainedPacket,
                buffer,
            };
        }
        buffer[..slot.length].copy_from_slice(bytes);
        slot.bytes[..slot.length].fill(0);
        let bytes = &buffer[..slot.length];
        let frame = match slot.kind {
            PacketKind::AclData => {
                let (packet, _) = AclPacket::from_hci_bytes(bytes)
                    .unwrap_or_else(|_| unreachable!("validated retained ACL must decode"));
                HostToControllerFrame::Acl(packet)
            }
            PacketKind::SyncData => {
                let (packet, _) = SyncPacket::from_hci_bytes(bytes)
                    .unwrap_or_else(|_| unreachable!("validated retained Sync must decode"));
                HostToControllerFrame::Sync(packet)
            }
            PacketKind::IsoData => {
                let (packet, _) = IsoPacket::from_hci_bytes(bytes)
                    .unwrap_or_else(|_| unreachable!("validated retained ISO must decode"));
                HostToControllerFrame::Iso(packet)
            }
            PacketKind::Cmd | PacketKind::Event => {
                unreachable!("command and invalid-direction kinds returned above")
            }
        };
        HciClassifiedCommandIntake::NonCommand(HciEpochBound::bind(self.identity, frame))
    }

    #[cfg(test)]
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
    #[cfg(test)]
    pub(crate) async fn publish(
        &self,
        kind: PacketKind,
        bytes: &[u8],
    ) -> Result<(), HciChannelError> {
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
    pub(crate) async fn wait_publish_ready(&self) {
        self.controller_to_host.wait_send_ready().await;
    }

    /// Publish immediately or return [`HciChannelError::Full`] without overwrite.
    pub(crate) fn try_publish(
        &self,
        kind: PacketKind,
        bytes: &[u8],
    ) -> Result<(), HciChannelError> {
        let slot = controller_slot::<PACKET_CAPACITY>(kind, bytes)?;
        self.controller_to_host
            .try_send(slot)
            .map_err(|()| HciChannelError::Full)
    }
}

#[cfg(test)]
mod tests;
