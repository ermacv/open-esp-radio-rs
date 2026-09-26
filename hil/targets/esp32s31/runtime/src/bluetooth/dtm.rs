//! Direct Test Mode through the HCI Controller.
//!
//! One task owns the Host end of the transport. It resets the Controller at
//! boot, then turns every HIL DTM operation into the standard HCI command and
//! waits for its Command Complete: LE Transmitter Test v1 on channel 0 with
//! 37-byte PRBS9 payloads, LE Receiver Test v1 on channel 0, LE Test End and
//! HCI Reset. The Controller core runs the test over the radio runtime.

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::{Duration, with_timeout};
use oer_bluetooth_hci::bt_hci::{
    ControllerToHostPacket, PacketKind,
    cmd::{Opcode, OpcodeGroup},
    transport::{PacketToController, Transport},
};
use oer_esp32s31_bluetooth_system::BluetoothHostTransport;
use oer_hil_protocol::{
    BluetoothDtmEvidence, BluetoothDtmOperation as Operation, BluetoothDtmResult,
    BluetoothDtmRxDiagnostics, Command, Event, FeatureCapabilities, RejectReason,
};

use super::console;

const RESET: Opcode = Opcode::new(OpcodeGroup::CONTROL_BASEBAND, 0x0003);
const RECEIVER_TEST: Opcode = Opcode::new(OpcodeGroup::LE, 0x001d);
const TRANSMITTER_TEST: Opcode = Opcode::new(OpcodeGroup::LE, 0x001e);
const TEST_END: Opcode = Opcode::new(OpcodeGroup::LE, 0x001f);
/// DTM on channel 0 (2402 MHz) with 37-byte PRBS9 payloads.
const CHANNEL: u8 = 0;
const PAYLOAD_BYTES: u8 = 37;
const PRBS9: u8 = 0x00;
/// Bound on one DTM operation, including draining the event in progress.
const OPERATION_TIMEOUT: Duration = Duration::from_secs(2);

static OPERATIONS: Channel<CriticalSectionRawMutex, Operation, 1> = Channel::new();
static RESULTS: Channel<CriticalSectionRawMutex, (BluetoothDtmResult, u16), 1> = Channel::new();

pub(super) async fn run(
    spawner: embassy_executor::Spawner,
    host: BluetoothHostTransport,
    usb: esp_hal::peripherals::USB_DEVICE<'static>,
    boot: u64,
) -> ! {
    spawner.spawn(tester(host).expect("Bluetooth DTM task"));
    console::run(usb, boot, &Profile).await
}

struct Profile;

impl console::Profile for Profile {
    fn features(&self) -> FeatureCapabilities {
        FeatureCapabilities {
            bluetooth_dtm: true,
            phy_rx_hot_sram: cfg!(feature = "phy-rx-hot-sram"),
            ..FeatureCapabilities::default()
        }
    }

    fn maximum_payload_bytes(&self) -> u16 {
        u16::from(PAYLOAD_BYTES)
    }

    async fn command(&self, command: Command) -> Event {
        let Command::BluetoothDtm(operation) = command else {
            return Event::Rejected(RejectReason::InvalidState);
        };
        OPERATIONS.send(operation).await;
        let (result, received) = RESULTS.receive().await;
        Event::BluetoothDtm(BluetoothDtmEvidence {
            reset_reason: crate::system::boot_evidence().reset_reason,
            operation,
            result,
            rx_diagnostics: BluetoothDtmRxDiagnostics {
                counted_packets: u32::from(received),
                ..BluetoothDtmRxDiagnostics::default()
            },
        })
    }
}

#[embassy_executor::task]
async fn tester(host: BluetoothHostTransport) {
    if command(&host, RESET, &[]).await != Some((0, None)) {
        crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-hci-reset\r\n");
    }
    let mut received = 0;
    loop {
        let operation = OPERATIONS.receive().await;
        let result = with_timeout(OPERATION_TIMEOUT, execute(&host, operation))
            .await
            .unwrap_or(BluetoothDtmResult::Timeout);
        if let BluetoothDtmResult::Complete {
            received_packets: Some(count),
        } = result
        {
            received = count;
        }
        RESULTS.send((result, received)).await;
    }
}

async fn execute(host: &BluetoothHostTransport, operation: Operation) -> BluetoothDtmResult {
    let (opcode, parameters): (Opcode, &[u8]) = match operation {
        Operation::Transmit => (TRANSMITTER_TEST, &[CHANNEL, PAYLOAD_BYTES, PRBS9]),
        Operation::Receive => (RECEIVER_TEST, &[CHANNEL]),
        Operation::End => (TEST_END, &[]),
        Operation::Reset => (RESET, &[]),
    };
    match command(host, opcode, parameters).await {
        Some((0, count)) => BluetoothDtmResult::Complete {
            received_packets: (operation == Operation::End).then(|| count.unwrap_or(0)),
        },
        _ => BluetoothDtmResult::HciRejected,
    }
}

/// Send one command and wait for its Command Complete: the status and, for
/// LE Test End, the packet count. `None` when the transport failed.
async fn command(
    host: &BluetoothHostTransport,
    opcode: Opcode,
    parameters: &[u8],
) -> Option<(u8, Option<u16>)> {
    host.write(&RawCommand { opcode, parameters }).await.ok()?;
    let mut buffer = [0; oer_esp32s31_bluetooth_system::PACKET];
    loop {
        let ControllerToHostPacket::Event(event) = host.read(&mut buffer).await.ok()? else {
            continue;
        };
        // Command Complete: packets, opcode, status, return parameters.
        let data = event.data;
        if event.kind.0 == 0x0e
            && data.len() >= 4
            && u16::from_le_bytes([data[1], data[2]]) == opcode.to_raw()
        {
            let count = (data.len() >= 6).then(|| u16::from_le_bytes([data[4], data[5]]));
            return Some((data[3], count));
        }
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
