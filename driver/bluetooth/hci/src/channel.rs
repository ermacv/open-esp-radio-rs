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

use crate::{ControllerToHostQueueError, PacketSlot, decode_complete_packet};

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
    host_to_controller: &'channel AsyncPacketQueue<M, HOST_TO_CONTROLLER_DEPTH, PACKET_CAPACITY>,
    controller_to_host: &'channel AsyncPacketQueue<M, CONTROLLER_TO_HOST_DEPTH, PACKET_CAPACITY>,
}

impl<
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    InProcessHciControllerEndpoint<
        '_,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
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

    /// Validate and asynchronously publish one Controller packet.
    pub async fn publish(&self, kind: PacketKind, bytes: &[u8]) -> Result<(), HciChannelError> {
        let slot = controller_slot::<PACKET_CAPACITY>(kind, bytes)?;
        self.controller_to_host.send(slot).await;
        Ok(())
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
        PacketKind::Cmd => {
            let raw = u16::from_le_bytes([bytes[0], bytes[1]]);
            let opcode = Opcode::new(OpcodeGroup::new((raw >> 10) as u8), raw & 0x03ff);
            Ok(HostToControllerFrame::Command(HciCommandPacket {
                opcode,
                parameters: &bytes[3..],
            }))
        }
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
        cmd::{Cmd, SyncCmd, controller_baseband::Reset},
        controller::{Controller, ExternalController},
        transport::Transport,
    };
    use embassy_futures::{
        block_on,
        join::{join, join3},
    };
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;

    use super::{HciChannelError, HostToControllerFrame, InProcessHciChannel};

    const RESET_COMMAND_COMPLETE: [u8; 6] = [0x0e, 0x04, 0x01, 0x03, 0x0c, 0x00];
    const HARDWARE_ERROR: [u8; 3] = [0x10, 0x01, 0x42];

    type TestChannel = InProcessHciChannel<NoopRawMutex, 1, 1, 16>;

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
