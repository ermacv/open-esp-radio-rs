//! HIL owns the command lease and UART; the production composition owns RF.

use bt_hci::{
    ControllerToHostPacket,
    cmd::{
        SyncCmd,
        controller_baseband::{
            HostBufferSize, Reset, SetControllerToHostFlowControl, SetEventMask,
        },
        info::ReadBdAddr,
        le::{
            LeReceiverTestV2, LeSetAdvData, LeSetAdvEnable, LeSetAdvParams, LeSetEventMask,
            LeTestEnd, LeTransmitterTestV2,
        },
        link_control::Disconnect,
    },
    controller::Controller,
    data::{AclBroadcastFlag, AclPacket, AclPacketBoundary},
    event::{Event as HciEvent, le::LeEvent},
    param::{
        ConnHandle, ConnHandleCompletedPackets, ControllerToHostFlowControl, DisconnectReason,
        EventMask, LeConnRole, LeEventMask, Status,
    },
};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use embedded_io_async::Read as _;
use esp_hal::{Async, usb::usb_serial_jtag::UsbSerialJtag};
use oer_esp32s31_bluetooth::{
    le::{
        dtm::{DtmDefaultTxPowerDbm, DtmRuntimeConfig},
        peripheral::PeripheralConnectionRuntimeConfig,
        scanning::PassiveScanRuntimeConfig,
    },
    resources::BluetoothRadioHardware,
};
use oer_esp32s31_bluetooth_embassy::controller::DtmRecheckPeriod;
use oer_esp32s31_bluetooth_integration::{
    BluetoothColdStartConfig, BluetoothHostAclCredits, BluetoothHostController, BluetoothSystem,
    BluetoothSystemStorage, start_esp32s31_bluetooth,
};
use oer_esp32s31_bluetooth_memory::{
    DtmSchedulerAllocationConfig, PassiveScanDefaultTxPowerDbm,
    PassiveScanSchedulerAllocationConfig, PeripheralConnectionDefaultTxPowerDbm,
};
use oer_esp32s31_radio_platform_esp_hal::{EspHalBluetoothPlatform, EspHalRadioPlatform};
use open_esp_radio_hil_protocol::{
    BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES, BLUETOOTH_PERIPHERAL_UPDATED_INTERVAL_MILLIS,
    BluetoothDtmEvidence as Evidence, BluetoothDtmOperation as Operation,
    BluetoothDtmResult as Outcome, BluetoothPeripheralOperation as PeripheralOperation,
    BluetoothPeripheralResult as PeripheralResult,
    BluetoothPeripheralTermination as PeripheralTermination, Capabilities, Command, Envelope,
    Event, FeatureCapabilities, FrameDecoder, FrameEncoder, LinkHealth, RejectReason,
    bluetooth_peripheral_acl_payload, bluetooth_peripheral_acl_payload_for_sequence,
};
use static_cell::StaticCell;

type Host = BluetoothHostController<4, 4, 258>;
type HostAclCredits = BluetoothHostAclCredits<4, 258>;
static STORAGE: BluetoothSystemStorage<EspHalBluetoothPlatform<'static>, 4, 1, 4, 4, 258> =
    BluetoothSystemStorage::new();
static PLATFORM: StaticCell<EspHalRadioPlatform> = StaticCell::new();
static PERIPHERAL_HOST_EVENTS: PeripheralHostEvents = PeripheralHostEvents::new();
static PERIPHERAL_TERMINATION: AtomicU32 = AtomicU32::new(0);
static PERIPHERAL_TERMINATION_HOLD_MILLIS: AtomicU32 = AtomicU32::new(0);

const NO_DISCONNECT_REASON: u32 = u8::MAX as u32 + 1;
const HOST_ACL_BACKPRESSURE_HOLD_MILLIS: u64 = 300;
const HOST_ACL_PACKET_BYTES: u16 = 27;
const HOST_ACL_PACKET_CREDITS: u16 = 1;
const PERIPHERAL_ACL_PAYLOAD: [u8; BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES] =
    bluetooth_peripheral_acl_payload();
const PERIPHERAL_POST_UPDATE_ACL_PAYLOAD: [u8; BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES] =
    bluetooth_peripheral_acl_payload_for_sequence(1);

struct PeripheralHostEvents {
    connection_live: AtomicBool,
    connection_updated: AtomicBool,
    connections: AtomicU32,
    disconnections: AtomicU32,
    connection_updates: AtomicU32,
    target_disconnect_commands: AtomicU32,
    target_reset_commands: AtomicU32,
    faults: AtomicU32,
    acl_received: AtomicU32,
    acl_queued: AtomicU32,
    acl_transmitted: AtomicU32,
    acl_completed: AtomicU32,
    acl_backpressure_holds: AtomicU32,
    acl_faults: AtomicU32,
    last_disconnect_reason: AtomicU32,
}

#[derive(Clone, Copy)]
struct PeripheralHostEventSnapshot {
    connections: u32,
    disconnections: u32,
    connection_updates: u32,
    target_disconnect_commands: u32,
    target_reset_commands: u32,
    faults: u32,
    acl_received: u32,
    acl_queued: u32,
    acl_transmitted: u32,
    acl_completed: u32,
    acl_backpressure_holds: u32,
    acl_faults: u32,
    last_disconnect_reason: Option<u8>,
}

impl PeripheralHostEvents {
    const fn new() -> Self {
        Self {
            connection_live: AtomicBool::new(false),
            connection_updated: AtomicBool::new(false),
            connections: AtomicU32::new(0),
            disconnections: AtomicU32::new(0),
            connection_updates: AtomicU32::new(0),
            target_disconnect_commands: AtomicU32::new(0),
            target_reset_commands: AtomicU32::new(0),
            faults: AtomicU32::new(0),
            acl_received: AtomicU32::new(0),
            acl_queued: AtomicU32::new(0),
            acl_transmitted: AtomicU32::new(0),
            acl_completed: AtomicU32::new(0),
            acl_backpressure_holds: AtomicU32::new(0),
            acl_faults: AtomicU32::new(0),
            last_disconnect_reason: AtomicU32::new(NO_DISCONNECT_REASON),
        }
    }

    fn increment(counter: &AtomicU32) {
        let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_add(1)
        });
    }

    fn add(counter: &AtomicU32, delta: u16) {
        let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_add(u32::from(delta))
        });
    }

    fn observe(&self, packet: ControllerToHostPacket<'_>) -> PeripheralHostObservation {
        let ControllerToHostPacket::Event(packet) = packet else {
            return PeripheralHostObservation::None;
        };
        let event = match HciEvent::try_from(packet) {
            Ok(event) => event,
            Err(_) => {
                Self::increment(&self.faults);
                return PeripheralHostObservation::None;
            }
        };
        match event {
            HciEvent::Le(LeEvent::LeConnectionComplete(event)) => {
                if event.status == Status::SUCCESS
                    && event.handle.raw() == 1
                    && event.role == LeConnRole::Peripheral
                    && self
                        .connection_live
                        .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
                        .is_ok()
                {
                    self.connection_updated.store(false, Ordering::Relaxed);
                    Self::increment(&self.connections);
                    PeripheralHostObservation::Connection
                } else {
                    Self::increment(&self.faults);
                    PeripheralHostObservation::None
                }
            }
            HciEvent::DisconnectionComplete(event) => {
                if event.status == Status::SUCCESS
                    && event.handle.raw() == 1
                    && self.connection_live.swap(false, Ordering::Relaxed)
                {
                    self.connection_updated.store(false, Ordering::Relaxed);
                    self.last_disconnect_reason
                        .store(u32::from(event.reason.into_inner()), Ordering::Relaxed);
                    Self::increment(&self.disconnections);
                } else {
                    Self::increment(&self.faults);
                }
                PeripheralHostObservation::None
            }
            HciEvent::Le(LeEvent::LeConnectionUpdateComplete(event)) => {
                if event.status == Status::SUCCESS
                    && event.handle.raw() == 1
                    && event.conn_interval
                        == bt_hci::param::Duration::from_millis(u32::from(
                            BLUETOOTH_PERIPHERAL_UPDATED_INTERVAL_MILLIS,
                        ))
                    && event.peripheral_latency == 0
                    && event.supervision_timeout == bt_hci::param::Duration::from_millis(2_000)
                    && self.connection_live.load(Ordering::Relaxed)
                    && self
                        .connection_updated
                        .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
                        .is_ok()
                {
                    Self::increment(&self.connection_updates);
                    PeripheralHostObservation::ConnectionUpdate
                } else {
                    Self::increment(&self.faults);
                    PeripheralHostObservation::None
                }
            }
            HciEvent::NumberOfCompletedPackets(event) => {
                let mut completed = 0_u16;
                for entry in event.completed_packets {
                    let Ok(handle) = entry.handle() else {
                        Self::increment(&self.faults);
                        return PeripheralHostObservation::None;
                    };
                    let Ok(packets) = entry.num_completed_packets() else {
                        Self::increment(&self.faults);
                        return PeripheralHostObservation::None;
                    };
                    if handle.raw() != 1 || packets == 0 {
                        Self::increment(&self.faults);
                        return PeripheralHostObservation::None;
                    }
                    let Some(updated) = completed.checked_add(packets) else {
                        Self::increment(&self.faults);
                        return PeripheralHostObservation::None;
                    };
                    completed = updated;
                }
                if completed == 0 {
                    Self::increment(&self.faults);
                    PeripheralHostObservation::None
                } else {
                    Self::add(&self.acl_transmitted, completed);
                    PeripheralHostObservation::AclCompleted(completed)
                }
            }
            _ => PeripheralHostObservation::None,
        }
    }

    fn observe_target_reset(&self) {
        if !self.connection_live.swap(false, Ordering::Relaxed) {
            Self::increment(&self.faults);
        }
        self.connection_updated.store(false, Ordering::Relaxed);
        Self::increment(&self.target_reset_commands);
    }

    fn snapshot(&self) -> PeripheralHostEventSnapshot {
        let last_disconnect_reason = self.last_disconnect_reason.load(Ordering::Relaxed);
        PeripheralHostEventSnapshot {
            connections: self.connections.load(Ordering::Relaxed),
            disconnections: self.disconnections.load(Ordering::Relaxed),
            connection_updates: self.connection_updates.load(Ordering::Relaxed),
            target_disconnect_commands: self.target_disconnect_commands.load(Ordering::Relaxed),
            target_reset_commands: self.target_reset_commands.load(Ordering::Relaxed),
            faults: self.faults.load(Ordering::Relaxed),
            acl_received: self.acl_received.load(Ordering::Relaxed),
            acl_queued: self.acl_queued.load(Ordering::Relaxed),
            acl_transmitted: self.acl_transmitted.load(Ordering::Relaxed),
            acl_completed: self.acl_completed.load(Ordering::Relaxed),
            acl_backpressure_holds: self.acl_backpressure_holds.load(Ordering::Relaxed),
            acl_faults: self.acl_faults.load(Ordering::Relaxed),
            last_disconnect_reason: u8::try_from(last_disconnect_reason).ok(),
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum PeripheralHostObservation {
    None,
    Connection,
    ConnectionUpdate,
    AclCompleted(u16),
}

struct PeripheralAclEcho {
    bytes: [u8; BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES],
    len: usize,
}

impl PeripheralAclEcho {
    const fn new() -> Self {
        Self {
            bytes: [0; BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES],
            len: 0,
        }
    }

    fn receive(
        &mut self,
        packet: AclPacket<'_>,
        expected: &[u8; BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES],
    ) -> Result<bool, ()> {
        if packet.handle().raw() != 1
            || packet.broadcast_flag() != AclBroadcastFlag::PointToPoint
            || packet.data().is_empty()
        {
            return Err(());
        }
        match packet.boundary_flag() {
            AclPacketBoundary::FirstFlushable if self.len == 0 => {}
            AclPacketBoundary::Continuing if self.len != 0 => {}
            _ => return Err(()),
        }
        let end = self
            .len
            .checked_add(packet.data().len())
            .filter(|end| *end <= self.bytes.len())
            .ok_or(())?;
        if packet.data() != &expected[self.len..end] {
            return Err(());
        }
        self.bytes[self.len..end].copy_from_slice(packet.data());
        self.len = end;
        Ok(self.len == self.bytes.len())
    }

    fn clear(&mut self) {
        self.len = 0;
    }
}

pub(super) fn start(
    executor: &'static mut super::Executor<0>,
    platform: EspHalRadioPlatform,
    usb: esp_hal::peripherals::USB_DEVICE<'static>,
    rng: esp_hal::peripherals::RNG<'static>,
) -> ! {
    let platform = PLATFORM.init(platform);
    let entropy = esp_hal::rng::TrngSource::new(rng);
    let rng = esp_hal::rng::Trng::try_new().expect("HIL boot entropy");
    let boot_id = ((u64::from(rng.random()) << 32) | u64::from(rng.random())).max(1);
    drop(rng);
    drop(entropy);
    let hardware = BluetoothRadioHardware::take().expect("unique Bluetooth hardware");
    executor.run(|spawner| {
        spawner.spawn(task(platform, hardware, usb, boot_id).expect("Bluetooth task allocation"));
    })
}

#[embassy_executor::task]
async fn task(
    platform: &'static EspHalRadioPlatform,
    hardware: BluetoothRadioHardware,
    usb: esp_hal::peripherals::USB_DEVICE<'static>,
    boot_id: u64,
) {
    // Diagnostic assumption: the board's retained main XTAL meets the BLE
    // 500-ppm limit. Use the broadest permitted bound, including margin over
    // ESP-IDF's CONFIG_BT_LE_LL_SCA default of 60 ppm. This is not a measured
    // PHY-calibration result or a qualification of oscillator accuracy.
    let peripheral_connection =
        PeripheralConnectionRuntimeConfig::new(PeripheralConnectionDefaultTxPowerDbm::new(0))
            .with_software_recurring_timing(500)
            .expect("BLE maximum local sleep-clock error")
            // Explicit, unassigned development identity for the open Controller.
            .with_version_information(oer_bluetooth_ll::control::LeVersionInformation::new(
                0x0d, 0xffff, 1,
            ));
    let config = BluetoothColdStartConfig::new(
        251,
        4,
        None,
        DtmRuntimeConfig::new(
            DtmSchedulerAllocationConfig::new(0, 0, 0),
            DtmDefaultTxPowerDbm::new(0),
        ),
        PassiveScanRuntimeConfig::new(
            PassiveScanSchedulerAllocationConfig::new(0, 0).expect("scan allocation"),
            PassiveScanDefaultTxPowerDbm::new(0),
        ),
        DtmRecheckPeriod::from_duration(Duration::from_micros(50)).expect("nonzero recheck"),
    )
    .with_peripheral_connection(peripheral_connection);
    let mut startup = core::pin::pin!(start_esp32s31_bluetooth(
        platform, hardware, &STORAGE, config
    ));
    let output = match startup.as_mut().await {
        Ok(output) => output,
        Err(_error) => super::fail(c"OPEN_RADIO_HIL Bluetooth cold start failed\r\n"),
    };
    let BluetoothSystem {
        hci,
        host_acl_credits,
        runners,
    } = output.system;
    let mut hardware = core::pin::pin!(runners.hardware.run());
    let mut console = Console {
        usb: UsbSerialJtag::new(usb).into_async(),
        boot_id,
        sequence: 0,
        decoder: FrameDecoder::new(),
        encoder: FrameEncoder::new(),
        peripheral_probe: false,
        peripheral_closed_at_start: 0,
        peripheral_reset_at_start: 0,
    };
    // Keep the radio actor's large ownership transitions out of the joined
    // console poll frame, including when diagnostic formatting changes inlining.
    let (_, _, never) = embassy_futures::join::join3(
        console.run(&hci),
        pump(&hci, &host_acl_credits),
        poll_with_stack_boundary(hardware.as_mut()),
    )
    .await;
    match never {}
}

async fn pump(hci: &Host, host_acl_credits: &HostAclCredits) {
    let mut buffer = hci
        .alloc_buf()
        .unwrap_or_else(|_| panic!("HCI receive buffer"));
    let mut assembly = PeripheralAclEcho::new();
    let mut hold_next_acl_credit = false;
    let mut echoes_in_connection = 0_u8;
    let mut acl_completions_in_connection = 0_u16;
    loop {
        let packet = hci
            .read(&mut buffer)
            .await
            .unwrap_or_else(|_| panic!("HCI transport failed"));
        match packet {
            ControllerToHostPacket::Acl(packet) => {
                let credit_handle = packet.handle();
                let expected = match echoes_in_connection {
                    0 => &PERIPHERAL_ACL_PAYLOAD,
                    1 => &PERIPHERAL_POST_UPDATE_ACL_PAYLOAD,
                    _ => {
                        PeripheralHostEvents::increment(&PERIPHERAL_HOST_EVENTS.acl_faults);
                        continue;
                    }
                };
                let complete = match assembly.receive(packet, expected) {
                    Ok(complete) => complete,
                    Err(()) => {
                        assembly.clear();
                        PeripheralHostEvents::increment(&PERIPHERAL_HOST_EVENTS.acl_faults);
                        let completed = [ConnHandleCompletedPackets::new(credit_handle, 1)];
                        let _ = host_acl_credits.return_completed_packets(&completed).await;
                        continue;
                    }
                };
                PeripheralHostEvents::increment(&PERIPHERAL_HOST_EVENTS.acl_received);
                if hold_next_acl_credit {
                    hold_next_acl_credit = false;
                    Timer::after(Duration::from_millis(HOST_ACL_BACKPRESSURE_HOLD_MILLIS)).await;
                    PeripheralHostEvents::increment(&PERIPHERAL_HOST_EVENTS.acl_backpressure_holds);
                }
                let completed = [ConnHandleCompletedPackets::new(credit_handle, 1)];
                if host_acl_credits
                    .return_completed_packets(&completed)
                    .await
                    .is_err()
                {
                    PeripheralHostEvents::increment(&PERIPHERAL_HOST_EVENTS.acl_faults);
                    assembly.clear();
                    continue;
                }
                PeripheralHostEvents::increment(&PERIPHERAL_HOST_EVENTS.acl_completed);
                if !complete {
                    continue;
                }
                let echo = AclPacket::new(
                    ConnHandle::new(1),
                    AclPacketBoundary::FirstNonFlushable,
                    AclBroadcastFlag::PointToPoint,
                    &assembly.bytes,
                );
                if hci.write_acl_data(&echo).await.is_ok() {
                    PeripheralHostEvents::increment(&PERIPHERAL_HOST_EVENTS.acl_queued);
                    echoes_in_connection = echoes_in_connection.saturating_add(1);
                } else {
                    PeripheralHostEvents::increment(&PERIPHERAL_HOST_EVENTS.acl_faults);
                }
                assembly.clear();
            }
            packet => match PERIPHERAL_HOST_EVENTS.observe(packet) {
                PeripheralHostObservation::None => {}
                PeripheralHostObservation::Connection => {
                    hold_next_acl_credit = true;
                    echoes_in_connection = 0;
                    acl_completions_in_connection = 0;
                }
                PeripheralHostObservation::ConnectionUpdate => {}
                PeripheralHostObservation::AclCompleted(completed) => {
                    let Some(updated) = acl_completions_in_connection.checked_add(completed) else {
                        PeripheralHostEvents::increment(&PERIPHERAL_HOST_EVENTS.faults);
                        continue;
                    };
                    acl_completions_in_connection = updated;
                    if acl_completions_in_connection == 2 && echoes_in_connection == 2 {
                        execute_peripheral_termination(hci).await;
                    } else if acl_completions_in_connection > 2 {
                        PeripheralHostEvents::increment(&PERIPHERAL_HOST_EVENTS.faults);
                    }
                }
            },
        }
    }
}

async fn execute_peripheral_termination(hci: &Host) {
    let termination = PERIPHERAL_TERMINATION.swap(0, Ordering::AcqRel);
    if termination == 0 {
        return;
    }
    let hold_millis = PERIPHERAL_TERMINATION_HOLD_MILLIS.swap(0, Ordering::AcqRel);
    if hold_millis != 0 {
        Timer::after(Duration::from_millis(u64::from(hold_millis))).await;
    }
    let accepted = match termination {
        1 => Disconnect::new(
            ConnHandle::new(1),
            DisconnectReason::RemoteUserTerminatedConn,
        )
        .exec(hci)
        .await
        .map(|()| {
            PeripheralHostEvents::increment(&PERIPHERAL_HOST_EVENTS.target_disconnect_commands)
        })
        .is_ok(),
        2 => Reset::new()
            .exec(hci)
            .await
            .map(|()| PERIPHERAL_HOST_EVENTS.observe_target_reset())
            .is_ok(),
        _ => false,
    };
    if !accepted {
        PeripheralHostEvents::increment(&PERIPHERAL_HOST_EVENTS.faults);
    }
}

struct Console {
    usb: UsbSerialJtag<'static, Async>,
    boot_id: u64,
    sequence: u32,
    decoder: FrameDecoder,
    encoder: FrameEncoder,
    peripheral_probe: bool,
    peripheral_closed_at_start: u32,
    peripheral_reset_at_start: u32,
}

impl Console {
    // Encode before constructing the async write future. Retaining the large
    // Event enum across each write await inflates the joined console poll frame.
    #[inline(never)]
    fn send<'a>(
        &'a mut self,
        hci: &'a Host,
        request_id: u32,
        body: Event,
    ) -> impl core::future::Future<Output = ()> + 'a {
        let envelope = Envelope::new(self.boot_id, self.sequence, 0, request_id, body);
        let bytes = self
            .encoder
            .encode(&envelope)
            .expect("bounded HIL response");
        let usb = &mut self.usb;
        let sequence = &mut self.sequence;
        let peripheral_probe = self.peripheral_probe;
        async move {
            if !matches!(
                with_timeout(
                    Duration::from_secs(2),
                    open_esp_radio_hil_protocol::write_frame(usb, bytes)
                )
                .await,
                Ok(Ok(()))
            ) {
                if peripheral_probe {
                    esp_hal::system::software_reset();
                }
                let _ = execute(hci, Operation::Reset).await;
                core::future::pending::<()>().await;
            }
            *sequence = sequence.checked_add(1).expect("HIL sequence exhausted");
        }
    }

    #[inline(never)]
    fn send_peripheral<'a>(
        &'a mut self,
        hci: &'a Host,
        request: u32,
        operation: PeripheralOperation,
        result: PeripheralResult,
    ) -> impl core::future::Future<Output = ()> + 'a {
        self.send(hci, request, peripheral_evidence(operation, result))
    }

    #[inline(never)]
    fn decode_command(&mut self, byte: &[u8]) -> Option<Envelope<Command>> {
        let mut received = None;
        self.decoder
            .feed::<Command>(byte, |frame| received = frame.ok());
        received
    }

    async fn run(&mut self, hci: &Host) {
        let capabilities = Capabilities {
            features: FeatureCapabilities {
                bluetooth_dtm: true,
                bluetooth_peripheral: true,
                structured_evidence: true,
                psram_task_stack: true,
                ..FeatureCapabilities::default()
            },
            maximum_payload_bytes: 37,
            maximum_wire_frame_bytes: open_esp_radio_hil_protocol::MAX_WIRE_FRAME_BYTES as u16,
        };
        self.send(hci, 0, Event::Hello(capabilities)).await;
        let mut lease: Option<Instant> = None;
        let mut active = false;

        loop {
            if lease.is_some_and(|deadline| Instant::now() >= deadline) && self.peripheral_probe {
                self.send_peripheral(
                    hci,
                    0,
                    PeripheralOperation::Snapshot,
                    PeripheralResult::LeaseExpired,
                )
                .await;
                // This ends the whole diagnostic boot, not a logical HCI Reset.
                esp_hal::system::software_reset();
            }
            if lease.is_some_and(|deadline| Instant::now() >= deadline) {
                let _ = execute(hci, Operation::Reset).await;
                self.send(
                    hci,
                    0,
                    Event::BluetoothDtm(Evidence {
                        operation: Operation::Reset,
                        result: Outcome::LeaseExpired,
                        rx_diagnostics: rx_diagnostics(),
                    }),
                )
                .await;
                core::future::pending::<()>().await;
            }
            let mut byte = [0];
            match with_timeout(Duration::from_millis(20), self.usb.read(&mut byte)).await {
                Err(_) => continue,
                Ok(Err(_)) => {
                    if self.peripheral_probe {
                        esp_hal::system::software_reset();
                    }
                    let _ = execute(hci, Operation::Reset).await;
                    core::future::pending::<()>().await;
                    continue;
                }
                Ok(Ok(0)) => continue,
                Ok(Ok(_)) => {}
            }
            let Some(command) = self.decode_command(&byte) else {
                continue;
            };
            let request = command.request_id;
            if let Err(reason) = command.validate_target(self.boot_id) {
                self.send(hci, request, Event::Rejected(reason)).await;
                continue;
            }
            if command.session_id != 0 {
                self.send(hci, request, Event::Rejected(RejectReason::InvalidState))
                    .await;
                continue;
            }
            let response = match command.body {
                Command::GetCapabilities => Event::Hello(capabilities),
                Command::QueryLinkHealth => {
                    let c = self.decoder.counters();
                    Event::LinkHealth(LinkHealth {
                        rx_frames: c.frames,
                        rx_cobs_errors: c.cobs_errors,
                        rx_checksum_errors: c.checksum_errors,
                        rx_decode_errors: c.deserialize_errors
                            + c.header_errors
                            + c.protocol_version_errors
                            + c.framing_version_errors
                            + c.message_kind_errors
                            + c.payload_length_errors
                            + c.too_short,
                        rx_overflows: c.overflows,
                        tx_frames: self.sequence,
                        tx_dropped: 0,
                        text_dropped: 0,
                        text_truncated: 0,
                    })
                }
                Command::BluetoothPeripheral(operation) => {
                    let execution = oer_esp32s31_bluetooth_integration::diagnostics::snapshot();
                    let host_events = PERIPHERAL_HOST_EVENTS.snapshot();
                    let start = match operation {
                        PeripheralOperation::StartAdvertising {
                            termination,
                            hold_millis,
                        } => Some((termination, hold_millis)),
                        PeripheralOperation::Snapshot => None,
                    };
                    if start.is_some()
                        && (active
                            || execution.terminal
                            || execution.saturated
                            || (self.peripheral_probe
                                && execution.peripheral_disconnections
                                    == self.peripheral_closed_at_start
                                && host_events.target_reset_commands
                                    == self.peripheral_reset_at_start))
                    {
                        Event::Rejected(RejectReason::InvalidState)
                    } else if start.is_some_and(|(_, hold_millis)| hold_millis > 5_000) {
                        Event::Rejected(RejectReason::InvalidConfiguration)
                    } else if start.is_some_and(|(termination, _)| {
                        termination == PeripheralTermination::LegacyPeerPowerOff
                    }) {
                        Event::Rejected(RejectReason::InvalidConfiguration)
                    } else {
                        let result = if operation == PeripheralOperation::Snapshot {
                            PeripheralResult::Snapshot
                        } else {
                            let initialize = !self.peripheral_probe
                                || host_events.target_reset_commands
                                    != self.peripheral_reset_at_start;
                            self.peripheral_closed_at_start = execution.peripheral_disconnections;
                            self.peripheral_reset_at_start = host_events.target_reset_commands;
                            self.peripheral_probe = true;
                            lease = Some(Instant::now() + Duration::from_secs(30));
                            match with_timeout(
                                Duration::from_secs(5),
                                start_advertising(hci, initialize),
                            )
                            .await
                            {
                                Ok(Ok(address)) => {
                                    let (termination, hold_millis) = start
                                        .expect("non-snapshot operation retains its start mode");
                                    let termination = match termination {
                                        PeripheralTermination::PeerReset
                                        | PeripheralTermination::PeerRfkill => 0,
                                        PeripheralTermination::TargetDisconnect => 1,
                                        PeripheralTermination::TargetReset => 2,
                                        PeripheralTermination::LegacyPeerPowerOff => unreachable!(),
                                    };
                                    PERIPHERAL_TERMINATION_HOLD_MILLIS
                                        .store(u32::from(hold_millis), Ordering::Release);
                                    PERIPHERAL_TERMINATION.store(termination, Ordering::Release);
                                    PeripheralResult::Started { address }
                                }
                                Ok(Err(result)) => result,
                                Err(_) => PeripheralResult::Timeout,
                            }
                        };
                        self.send_peripheral(hci, request, operation, result).await;
                        continue;
                    }
                }
                Command::BluetoothDtm(_) if self.peripheral_probe => {
                    Event::Rejected(RejectReason::InvalidState)
                }
                Command::BluetoothDtm(operation) => {
                    if (active && matches!(operation, Operation::Receive | Operation::Transmit))
                        || (!active && operation == Operation::End)
                    {
                        Event::Rejected(RejectReason::InvalidState)
                    } else {
                        let result = execute(hci, operation).await;
                        match result {
                            Outcome::Complete { .. } => {
                                active =
                                    matches!(operation, Operation::Receive | Operation::Transmit);
                                lease = active.then(|| Instant::now() + Duration::from_secs(30));
                            }
                            _ => {
                                // A cancelled HCI exchange has uncertain state. Attempt Reset,
                                // then retain the composition until the host resets the board.
                                let _ = execute(hci, Operation::Reset).await;
                                self.send(
                                    hci,
                                    request,
                                    Event::BluetoothDtm(Evidence {
                                        operation,
                                        result,
                                        rx_diagnostics: rx_diagnostics(),
                                    }),
                                )
                                .await;
                                core::future::pending::<()>().await;
                            }
                        }
                        Event::BluetoothDtm(Evidence {
                            operation,
                            result,
                            rx_diagnostics: rx_diagnostics(),
                        })
                    }
                }
                _ => Event::Rejected(RejectReason::InvalidState),
            };
            self.send(hci, request, response).await;
        }
    }
}

async fn execute(hci: &Host, operation: Operation) -> Outcome {
    let command = async {
        match operation {
            Operation::Reset => Reset::new().exec(hci).await.map(|_| None),
            Operation::Receive => LeReceiverTestV2::new(0, 1, 0).exec(hci).await.map(|_| None),
            Operation::Transmit => LeTransmitterTestV2::new(0, 37, 0, 1)
                .exec(hci)
                .await
                .map(|_| None),
            Operation::End => LeTestEnd::new().exec(hci).await.map(Some),
        }
    };
    match with_timeout(Duration::from_secs(2), command).await {
        Ok(Ok(received_packets)) => Outcome::Complete { received_packets },
        Ok(Err(_)) => Outcome::HciRejected,
        Err(_) => Outcome::Timeout,
    }
}

fn rx_diagnostics() -> open_esp_radio_hil_protocol::BluetoothDtmRxDiagnostics {
    let d = oer_esp32s31_bluetooth::le::dtm::diagnostics::snapshot();
    open_esp_radio_hil_protocol::BluetoothDtmRxDiagnostics {
        sequence_checks: d.sequence_checks,
        sequence_deadline_rejections: d.sequence_deadline_rejections,
        last_sequence_lead_ticks: d.last_sequence_lead_ticks,
        successful_events: d.successful_events,
        failed_events: d.failed_events,
        last_failure_status: d.last_failure_status,
        empty_events: d.empty_events,
        counted_packets: d.counted_packets,
        rejected_packets: d.rejected_packets,
    }
}

fn peripheral_evidence(operation: PeripheralOperation, result: PeripheralResult) -> Event {
    let snapshot = oer_esp32s31_bluetooth_integration::diagnostics::snapshot();
    let host_events = PERIPHERAL_HOST_EVENTS.snapshot();
    use core::fmt::Write as _;
    let mut detail = heapless::String::<128>::new();
    let truncated = if snapshot.terminal {
        detail.push_str(snapshot.detail()).is_err() || snapshot.detail_truncated
    } else if snapshot.peripheral_runs > 0 {
        let ll = oer_esp32s31_bluetooth::le::peripheral::diagnostics::snapshot();
        write!(
            detail,
            "ll rx={} drop={} ctrl={} queued={} done={} maps={} op={:?} closed={} reason={:?}",
            ll.received,
            ll.discarded,
            ll.control,
            ll.queued,
            ll.completed,
            ll.channel_map_updates,
            ll.last_opcode,
            snapshot.peripheral_disconnections,
            snapshot.last_disconnect_reason
        )
        .is_err()
    } else {
        let rx = oer_esp32s31_bluetooth::le::advertising::diagnostics::snapshot();
        let nodes = rx.last_progress.map(|nodes| {
            nodes.map(|node| {
                (
                    u8::from(node.completed),
                    u8::from(node.producer_updated),
                    u8::from(node.epoch_updated),
                    node.header,
                )
            })
        });
        write!(
            detail,
            "rx events={} scan={} conn={} unfinished={} last={:?}",
            rx.events, rx.scan_headers, rx.connect_headers, rx.unfinished_packets, nodes
        )
        .is_err()
    };
    Event::BluetoothPeripheral(open_esp_radio_hil_protocol::BluetoothPeripheralEvidence {
        operation,
        result,
        advertising_runs: snapshot.advertising_runs,
        peripheral_runs: snapshot.peripheral_runs,
        peripheral_disconnections: snapshot.peripheral_disconnections,
        connection_complete_events: host_events.connections,
        disconnection_complete_events: host_events.disconnections,
        connection_update_complete_events: host_events.connection_updates,
        channel_map_update_events: oer_esp32s31_bluetooth::le::peripheral::diagnostics::snapshot()
            .channel_map_updates,
        target_disconnect_commands: host_events.target_disconnect_commands,
        target_reset_commands: host_events.target_reset_commands,
        host_event_faults: host_events.faults,
        host_acl_received_packets: host_events.acl_received,
        host_acl_queued_packets: host_events.acl_queued,
        host_acl_transmitted_packets: host_events.acl_transmitted,
        host_acl_completed_packets: host_events.acl_completed,
        host_acl_backpressure_holds: host_events.acl_backpressure_holds,
        host_acl_faults: host_events.acl_faults,
        last_disconnect_reason: host_events.last_disconnect_reason,
        retries: snapshot.retries,
        terminal: snapshot.terminal,
        saturated: snapshot.saturated,
        detail_truncated: truncated,
        detail,
    })
}

// Keep each HCI exchange's poll frame separate from the framed-console parser.
async fn poll_with_stack_boundary<F: core::future::Future>(future: F) -> F::Output {
    let mut future = core::pin::pin!(future);
    core::future::poll_fn(|cx| poll_pinned_future(future.as_mut(), cx)).await
}

#[inline(never)]
fn poll_pinned_future<F: core::future::Future>(
    future: core::pin::Pin<&mut F>,
    cx: &mut core::task::Context<'_>,
) -> core::task::Poll<F::Output> {
    future.poll(cx)
}

async fn start_advertising(hci: &Host, initialize: bool) -> Result<[u8; 6], PeripheralResult> {
    use bt_hci::param::{AddrKind, AdvChannelMap, AdvFilterPolicy, AdvKind, BdAddr};
    macro_rules! command {
        ($value:expr, $stage:expr) => {
            poll_with_stack_boundary($value.exec(hci))
                .await
                .map_err(|_| PeripheralResult::HciRejected {
                    command_stage: $stage,
                })?
        };
    }
    if initialize {
        command!(Reset::new(), 0);
        command!(
            SetEventMask::new(
                EventMask::new()
                    .enable_le_meta(true)
                    .enable_hardware_error(true)
                    .enable_disconnection_complete(true),
            ),
            1
        );
        command!(
            LeSetEventMask::new(
                LeEventMask::new()
                    .enable_le_conn_complete(true)
                    .enable_le_conn_update_complete(true),
            ),
            2
        );
        command!(
            HostBufferSize::new(HOST_ACL_PACKET_BYTES, 0, HOST_ACL_PACKET_CREDITS, 0,),
            3
        );
        command!(
            SetControllerToHostFlowControl::new(ControllerToHostFlowControl::AclOnSyncOff),
            4
        );
    }
    let address = command!(ReadBdAddr::new(), 5);
    command!(
        LeSetAdvParams::new(
            bt_hci::param::Duration::from_millis(100),
            bt_hci::param::Duration::from_millis(100),
            AdvKind::AdvInd,
            AddrKind::PUBLIC,
            AddrKind::PUBLIC,
            BdAddr::new([0; 6]),
            AdvChannelMap::CHANNEL_37,
            AdvFilterPolicy::Unfiltered,
        ),
        6
    );
    let payload = b"\x02\x01\x06\x08\x09OER-HIL";
    let mut data = [0; 31];
    data[..payload.len()].copy_from_slice(payload);
    command!(LeSetAdvData::new(payload.len() as u8, data), 7);
    command!(LeSetAdvEnable::new(true), 8);
    Ok(address.into_inner())
}
