//! Direct Test Mode and raw HCI exchanges through the HCI Controller.
//!
//! One task owns the Host end of the transport. It resets the Controller at
//! boot, then serves the console: every HIL DTM operation becomes the standard
//! HCI command and waits for its Command Complete (LE Transmitter Test v1 on
//! channel 0 with 37-byte PRBS9 payloads, LE Receiver Test v1 on channel 0,
//! LE Test End and HCI Reset), and every raw HCI request sends one command
//! and returns its Command Complete or Command Status (Host Number Of
//! Completed Packets, which has neither, returns once written), sends one ACL
//! data
//! packet, or returns the oldest queued Controller packet. Events and ACL data
//! that arrive while a command waits for its completion join one bounded
//! queue in arrival order; when it is full the oldest packet is dropped and
//! counted. The image is only a passthrough: the host runner is the HCI Host,
//! including Controller-to-Host flow control. The Controller core runs every
//! role over the radio runtime. `PhyTracking` reports and suspends the
//! image's periodic PHY tracking.

use core::cell::RefCell;

use embassy_sync::{
    blocking_mutex::{Mutex, raw::CriticalSectionRawMutex},
    channel::Channel,
};
use embassy_time::{Duration, with_timeout};
use heapless::{Deque, Vec};
use oer_bluetooth_hci::bt_hci::{
    ControllerToHostPacket, PacketKind,
    cmd::{Opcode, OpcodeGroup},
    transport::{PacketToController, Transport},
};
use oer_esp32s31_bluetooth_system::BluetoothHostTransport;
use oer_hil_protocol::{
    RequestIdentity,
    base::RejectReason,
    bluetooth::{
        BLUETOOTH_HCI_ACL_BYTES, BLUETOOTH_HCI_EVENT_BYTES, BluetoothDtmEvidence,
        BluetoothDtmOperation as Operation, BluetoothDtmResult, BluetoothDtmRxDiagnostics,
        BluetoothHciRequest, BluetoothHciResponse, DtmResult, ExchangeHci, HciResponse, RunDtm,
    },
    phy::ControlTracking,
};

use super::console;

#[cfg(feature = "bluetooth-hci-lifecycle")]
mod lifecycle;

const RESET: Opcode = Opcode::new(OpcodeGroup::CONTROL_BASEBAND, 0x0003);
const RECEIVER_TEST: Opcode = Opcode::new(OpcodeGroup::LE, 0x001d);
const TRANSMITTER_TEST: Opcode = Opcode::new(OpcodeGroup::LE, 0x001e);
const TEST_END: Opcode = Opcode::new(OpcodeGroup::LE, 0x001f);
/// Host Number Of Completed Packets: the one command the Controller answers
/// with no event unless it fails.
const HOST_NUMBER_OF_COMPLETED_PACKETS: Opcode = Opcode::new(OpcodeGroup::CONTROL_BASEBAND, 0x0035);
/// DTM on channel 0 (2402 MHz) with 37-byte PRBS9 payloads.
const CHANNEL: u8 = 0;
const PAYLOAD_BYTES: u8 = 37;
const PRBS9: u8 = 0x00;
/// Bound on one DTM operation or raw command, including draining the event in
/// progress.
const OPERATION_TIMEOUT: Duration = Duration::from_secs(2);
/// Controller packets kept for [`BluetoothHciRequest::NextPacket`].
const PACKET_QUEUE: usize = 16;
const COMMAND_COMPLETE: u8 = 0x0e;
const COMMAND_STATUS: u8 = 0x0f;

enum Request {
    Dtm(Operation),
    Hci(BluetoothHciRequest),
}

enum Reply {
    Dtm(BluetoothDtmResult, u16),
    Hci(BluetoothHciResponse),
}

static REQUESTS: Channel<CriticalSectionRawMutex, Request, 1> = Channel::new();
static REPLIES: Channel<CriticalSectionRawMutex, Reply, 1> = Channel::new();

type EventPacket = Vec<u8, BLUETOOTH_HCI_EVENT_BYTES>;
type AclPacket = Vec<u8, BLUETOOTH_HCI_ACL_BYTES>;

/// One Controller-to-Host packet in its HCI encoding without the packet
/// indicator.
enum Packet {
    Event(EventPacket),
    Acl(AclPacket),
}

/// Controller packets no request has returned yet, with the count of those
/// dropped since the last returned one. Static, so that the task future
/// stays small.
struct Queue {
    packets: Deque<Packet, PACKET_QUEUE>,
    dropped: u16,
}

static QUEUE: Mutex<CriticalSectionRawMutex, RefCell<Queue>> = Mutex::new(RefCell::new(Queue {
    packets: Deque::new(),
    dropped: 0,
}));

#[cfg(not(feature = "bluetooth-hci-lifecycle"))]
pub(super) async fn run(
    spawner: embassy_executor::Spawner,
    host: BluetoothHostTransport,
    usb: esp_hal::peripherals::USB_DEVICE<'static>,
    boot: u64,
) -> ! {
    spawner.spawn(tester(host).expect("Bluetooth HCI task"));
    spawner.spawn(requests().expect("Bluetooth request task"));
    console::serve_console(usb, boot, &Profile).await
}

/// Serve the console while the HCI task, the request task and the
/// lifecycle owner run as their own tasks; the owner lends the Controller and
/// its HCI service to their own tasks for each epoch and ends epochs on
/// request.
#[cfg(feature = "bluetooth-hci-lifecycle")]
pub(super) async fn run_with_lifecycle(
    spawner: embassy_executor::Spawner,
    radio: &'static super::Radio,
    system: oer_esp32s31_bluetooth_system::BluetoothSystem,
    hci: oer_esp32s31_bluetooth_system::BluetoothHci,
    public_address: oer_bluetooth_hci::BluetoothPublicDeviceAddress,
    usb: esp_hal::peripherals::USB_DEVICE<'static>,
    boot: u64,
) -> ! {
    let oer_esp32s31_bluetooth_system::BluetoothHci { host, service } = hci;
    spawner.spawn(tester(host).expect("Bluetooth HCI task"));
    spawner.spawn(requests().expect("Bluetooth request task"));
    spawner.spawn(lifecycle::runner(radio).expect("Bluetooth runner task"));
    spawner.spawn(lifecycle::service().expect("Bluetooth HCI service task"));
    spawner.spawn(
        lifecycle::task(radio, system, service, public_address).expect("Bluetooth lifecycle task"),
    );
    console::serve_console(usb, boot, &Profile).await
}

/// Serves the requests the console queued, in a task of its own.
#[embassy_executor::task]
async fn requests() {
    console::serve_requests(&Profile).await
}

struct Profile;

oer_hil_target_core::requests! {
    /// The HCI image's requests.
    pub(super) enum Served (sessions = false) {
        Dtm(RunDtm),
        Hci(ExchangeHci),
        Tracking(ControlTracking),
    }
}

impl console::Profile for Profile {
    type Request = Served;

    fn queue(&self) -> &'static console::Queue<Served> {
        static QUEUE: console::Queue<Served> = console::Queue::new();
        &QUEUE
    }

    fn maximum_payload_bytes(&self) -> u16 {
        u16::from(PAYLOAD_BYTES)
    }

    async fn serve(&self, identity: RequestIdentity, body: Served) {
        match body {
            Served::Dtm(RunDtm(operation)) => {
                REQUESTS.send(Request::Dtm(operation)).await;
                let reply = match REPLIES.receive().await {
                    Reply::Dtm(result, received) => Ok(DtmResult(BluetoothDtmEvidence {
                        reset_reason: crate::system::boot_evidence().reset_reason,
                        operation,
                        result,
                        rx_diagnostics: BluetoothDtmRxDiagnostics {
                            counted_packets: u32::from(received),
                            ..BluetoothDtmRxDiagnostics::default()
                        },
                    })),
                    Reply::Hci(_) => Err(RejectReason::InvalidState),
                };
                console::respond(identity, reply).await;
            }
            // Only an image that ends Controller epochs serves them.
            Served::Hci(ExchangeHci(BluetoothHciRequest::Lifecycle(_)))
                if !cfg!(feature = "bluetooth-hci-lifecycle") =>
            {
                console::respond::<HciResponse>(identity, Err(RejectReason::InvalidState)).await;
            }
            Served::Hci(ExchangeHci(request)) => {
                REQUESTS.send(Request::Hci(request)).await;
                let reply = match REPLIES.receive().await {
                    Reply::Hci(response) => Ok(HciResponse(response)),
                    Reply::Dtm(..) => Err(RejectReason::InvalidState),
                };
                console::respond(identity, reply).await;
            }
            Served::Tracking(ControlTracking(control)) => {
                console::respond(identity, Ok(crate::phy_tracking::control(control))).await;
            }
        }
    }
}

/// The Host end of the transport.
struct Host {
    transport: BluetoothHostTransport,
}

impl Host {
    /// Read the next Controller event or ACL data packet; synchronous and
    /// isochronous data, which no LE peripheral role produces, are
    /// discarded.
    async fn read_packet(&self) -> Option<Packet> {
        let mut buffer = [0; oer_esp32s31_bluetooth_system::PACKET];
        loop {
            match self.transport.read(&mut buffer).await.ok()? {
                ControllerToHostPacket::Event(event) => {
                    let mut packet = EventPacket::new();
                    packet.push(event.kind.0).ok()?;
                    packet.push(event.data.len() as u8).ok()?;
                    packet.extend_from_slice(event.data).ok()?;
                    return Some(Packet::Event(packet));
                }
                ControllerToHostPacket::Acl(acl) => {
                    let header = acl.handle().into_inner()
                        | ((acl.boundary_flag() as u16) << 12)
                        | ((acl.broadcast_flag() as u16) << 14);
                    let mut packet = AclPacket::new();
                    packet.extend_from_slice(&header.to_le_bytes()).ok()?;
                    packet
                        .extend_from_slice(&(acl.data().len() as u16).to_le_bytes())
                        .ok()?;
                    packet.extend_from_slice(acl.data()).ok()?;
                    return Some(Packet::Acl(packet));
                }
                _ => {}
            }
        }
    }

    fn queue(&self, packet: Packet) {
        QUEUE.lock(|queue| {
            let mut queue = queue.borrow_mut();
            if queue.packets.is_full() {
                queue.packets.pop_front();
                queue.dropped = queue.dropped.saturating_add(1);
            }
            let _ = queue.packets.push_back(packet);
        });
    }

    /// Send one command and wait for its Command Complete or Command Status,
    /// queueing every other packet. `None` when the transport failed.
    async fn command(&self, opcode: Opcode, parameters: &[u8]) -> Option<EventPacket> {
        self.transport
            .write(&RawCommand { opcode, parameters })
            .await
            .ok()?;
        loop {
            let packet = match self.read_packet().await? {
                Packet::Event(packet) => packet,
                acl @ Packet::Acl(_) => {
                    self.queue(acl);
                    continue;
                }
            };
            let completes = match packet[0] {
                COMMAND_COMPLETE => {
                    packet.len() >= 5 && packet[3..5] == opcode.to_raw().to_le_bytes()
                }
                COMMAND_STATUS => {
                    packet.len() >= 6 && packet[4..6] == opcode.to_raw().to_le_bytes()
                }
                _ => false,
            };
            if completes {
                return Some(packet);
            }
            self.queue(Packet::Event(packet));
        }
    }

    /// Send one command that has no completion event; `false` when the
    /// transport failed.
    async fn unacknowledged(&self, opcode: Opcode, parameters: &[u8]) -> bool {
        self.transport
            .write(&RawCommand { opcode, parameters })
            .await
            .is_ok()
    }

    /// Send one ACL data packet; `false` when the transport failed.
    async fn acl(&self, packet: &[u8]) -> bool {
        self.transport.write(&RawAcl { packet }).await.is_ok()
    }

    async fn next_packet(&self, wait: Duration) -> BluetoothHciResponse {
        let queued = QUEUE.lock(|queue| queue.borrow_mut().packets.pop_front());
        let packet = match queued {
            Some(packet) => packet,
            None => match with_timeout(wait, self.read_packet()).await {
                Ok(Some(packet)) => packet,
                Ok(None) => return BluetoothHciResponse::TransportFailed,
                Err(_) => return BluetoothHciResponse::NoPacket,
            },
        };
        let dropped = QUEUE.lock(|queue| core::mem::take(&mut queue.borrow_mut().dropped));
        match packet {
            Packet::Event(packet) => BluetoothHciResponse::Event { packet, dropped },
            Packet::Acl(packet) => BluetoothHciResponse::Acl { packet, dropped },
        }
    }
}

#[embassy_executor::task]
async fn tester(transport: BluetoothHostTransport) {
    let host = Host { transport };
    if host
        .command(RESET, &[])
        .await
        .and_then(|packet| packet.get(5).copied())
        != Some(0)
    {
        crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-hci-reset\r\n");
    }
    let mut received = 0;
    // `None` once the Controller was retired.
    #[cfg_attr(not(feature = "bluetooth-hci-lifecycle"), allow(unused_mut))]
    let mut current = Some(host);
    loop {
        let request = REQUESTS.receive().await;
        #[cfg(feature = "bluetooth-hci-lifecycle")]
        if let Request::Hci(BluetoothHciRequest::Lifecycle(operation)) = request {
            let response = lifecycle::end_epoch(&mut current, operation).await;
            REPLIES.send(Reply::Hci(response)).await;
            continue;
        }
        let Some(host) = current.as_ref() else {
            REPLIES
                .send(match request {
                    Request::Dtm(_) => Reply::Dtm(BluetoothDtmResult::HciRejected, received),
                    Request::Hci(_) => Reply::Hci(BluetoothHciResponse::TransportFailed),
                })
                .await;
            continue;
        };
        let reply = match request {
            Request::Dtm(operation) => {
                let result = with_timeout(OPERATION_TIMEOUT, execute(host, operation))
                    .await
                    .unwrap_or(BluetoothDtmResult::Timeout);
                if let BluetoothDtmResult::Complete {
                    received_packets: Some(count),
                } = result
                {
                    received = count;
                }
                Reply::Dtm(result, received)
            }
            Request::Hci(BluetoothHciRequest::Command { opcode, parameters }) => {
                let opcode = Opcode::new(OpcodeGroup::new((opcode >> 10) as u8), opcode & 0x03ff);
                Reply::Hci(if opcode == HOST_NUMBER_OF_COMPLETED_PACKETS {
                    match with_timeout(OPERATION_TIMEOUT, host.unacknowledged(opcode, &parameters))
                        .await
                    {
                        Ok(true) => BluetoothHciResponse::Accepted,
                        Ok(false) => BluetoothHciResponse::TransportFailed,
                        Err(_) => BluetoothHciResponse::Timeout,
                    }
                } else {
                    match with_timeout(OPERATION_TIMEOUT, host.command(opcode, &parameters)).await {
                        Ok(Some(packet)) => BluetoothHciResponse::Completed(packet),
                        Ok(None) => BluetoothHciResponse::TransportFailed,
                        Err(_) => BluetoothHciResponse::Timeout,
                    }
                })
            }
            Request::Hci(BluetoothHciRequest::Acl { packet }) => Reply::Hci(
                match with_timeout(OPERATION_TIMEOUT, host.acl(&packet)).await {
                    Ok(true) => BluetoothHciResponse::Accepted,
                    Ok(false) => BluetoothHciResponse::TransportFailed,
                    Err(_) => BluetoothHciResponse::Timeout,
                },
            ),
            Request::Hci(BluetoothHciRequest::NextPacket { wait_ms }) => Reply::Hci(
                host.next_packet(Duration::from_millis(u64::from(wait_ms)))
                    .await,
            ),
            // Served above by an image that ends Controller epochs; the
            // console rejects it for every other image.
            Request::Hci(BluetoothHciRequest::Lifecycle(_)) => {
                Reply::Hci(BluetoothHciResponse::TransportFailed)
            }
        };
        REPLIES.send(reply).await;
    }
}

async fn execute(host: &Host, operation: Operation) -> BluetoothDtmResult {
    let (opcode, parameters): (Opcode, &[u8]) = match operation {
        Operation::Transmit => (TRANSMITTER_TEST, &[CHANNEL, PAYLOAD_BYTES, PRBS9]),
        Operation::Receive => (RECEIVER_TEST, &[CHANNEL]),
        Operation::End => (TEST_END, &[]),
        Operation::Reset => (RESET, &[]),
    };
    // Command Complete: code, length, packets, opcode, status, count.
    match host.command(opcode, parameters).await {
        Some(packet) if packet[0] == COMMAND_COMPLETE && packet.get(5) == Some(&0) => {
            BluetoothDtmResult::Complete {
                received_packets: (operation == Operation::End).then(|| {
                    packet
                        .get(6..8)
                        .map_or(0, |count| u16::from_le_bytes([count[0], count[1]]))
                }),
            }
        }
        _ => BluetoothDtmResult::HciRejected,
    }
}

/// One HCI command with its parameters, written as the transport expects.
struct RawCommand<'a> {
    opcode: Opcode,
    parameters: &'a [u8],
}

impl PacketToController for RawCommand<'_> {
    const KIND: PacketKind = PacketKind::Cmd;

    fn size(&self) -> usize {
        3 + self.parameters.len()
    }

    fn write_hci<W: embedded_io::Write>(&self, mut writer: W) -> Result<(), W::Error> {
        let opcode = self.opcode.to_raw().to_le_bytes();
        writer.write_all(&[opcode[0], opcode[1], self.parameters.len() as u8])?;
        writer.write_all(self.parameters)
    }

    async fn write_hci_async<W: embedded_io_async::Write>(
        &self,
        mut writer: W,
    ) -> Result<(), W::Error> {
        let opcode = self.opcode.to_raw().to_le_bytes();
        writer
            .write_all(&[opcode[0], opcode[1], self.parameters.len() as u8])
            .await?;
        writer.write_all(self.parameters).await
    }
}

/// One ACL data packet, header included, written as the transport expects.
struct RawAcl<'a> {
    packet: &'a [u8],
}

impl PacketToController for RawAcl<'_> {
    const KIND: PacketKind = PacketKind::AclData;

    fn size(&self) -> usize {
        self.packet.len()
    }

    fn write_hci<W: embedded_io::Write>(&self, mut writer: W) -> Result<(), W::Error> {
        writer.write_all(self.packet)
    }

    async fn write_hci_async<W: embedded_io_async::Write>(
        &self,
        mut writer: W,
    ) -> Result<(), W::Error> {
        writer.write_all(self.packet).await
    }
}
