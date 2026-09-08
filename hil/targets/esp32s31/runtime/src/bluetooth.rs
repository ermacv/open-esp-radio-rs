//! HIL owns the command lease and UART; the production composition owns RF.

use bt_hci::{
    cmd::{
        SyncCmd,
        controller_baseband::Reset,
        info::ReadBdAddr,
        le::{
            LeReceiverTestV2, LeSetAdvData, LeSetAdvEnable, LeSetAdvParams, LeTestEnd,
            LeTransmitterTestV2,
        },
    },
    controller::Controller,
};
use embassy_time::{Duration, Instant, with_timeout};
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
    BluetoothColdStartConfig, BluetoothHostController, BluetoothSystem, BluetoothSystemStorage,
    start_esp32s31_bluetooth,
};
use oer_esp32s31_bluetooth_memory::{
    DtmSchedulerAllocationConfig, PassiveScanDefaultTxPowerDbm,
    PassiveScanSchedulerAllocationConfig, PeripheralConnectionDefaultTxPowerDbm,
};
use oer_esp32s31_radio_platform_esp_hal::{EspHalBluetoothPlatform, EspHalRadioPlatform};
use open_esp_radio_hil_protocol::{
    BluetoothDtmEvidence as Evidence, BluetoothDtmOperation as Operation,
    BluetoothDtmResult as Outcome, BluetoothPeripheralOperation as PeripheralOperation,
    BluetoothPeripheralResult as PeripheralResult, Capabilities, Command, Envelope, Event,
    FeatureCapabilities, FrameDecoder, FrameEncoder, LinkHealth, RejectReason,
};
use static_cell::StaticCell;

type Host = BluetoothHostController<4, 4, 258>;
static STORAGE: BluetoothSystemStorage<EspHalBluetoothPlatform<'static>, 4, 1, 4, 4, 258> =
    BluetoothSystemStorage::new();
static PLATFORM: StaticCell<EspHalRadioPlatform> = StaticCell::new();

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
    let BluetoothSystem { hci, runners } = output.system;
    let mut hardware = core::pin::pin!(runners.hardware.run());
    let mut console = Console {
        usb: UsbSerialJtag::new(usb).into_async(),
        boot_id,
        sequence: 0,
        decoder: FrameDecoder::new(),
        encoder: FrameEncoder::new(),
        peripheral_probe: false,
    };
    // Keep the radio actor's large ownership transitions out of the joined
    // console poll frame, including when diagnostic formatting changes inlining.
    let (_, _, never) = embassy_futures::join::join3(
        console.run(&hci),
        pump(&hci),
        poll_with_stack_boundary(hardware.as_mut()),
    )
    .await;
    match never {}
}

async fn pump(hci: &Host) {
    let mut buffer = hci
        .alloc_buf()
        .unwrap_or_else(|_| panic!("HCI receive buffer"));
    loop {
        if hci.read(&mut buffer).await.is_err() {
            panic!("HCI transport failed");
        }
    }
}

struct Console {
    usb: UsbSerialJtag<'static, Async>,
    boot_id: u64,
    sequence: u32,
    decoder: FrameDecoder,
    encoder: FrameEncoder,
    peripheral_probe: bool,
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
                    if operation == PeripheralOperation::StartAdvertising
                        && (active || self.peripheral_probe)
                    {
                        Event::Rejected(RejectReason::InvalidState)
                    } else {
                        let result = if operation == PeripheralOperation::Snapshot {
                            PeripheralResult::Snapshot
                        } else {
                            self.peripheral_probe = true;
                            lease = Some(Instant::now() + Duration::from_secs(30));
                            match with_timeout(Duration::from_secs(5), start_advertising(hci)).await
                            {
                                Ok(Ok(address)) => PeripheralResult::Started { address },
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
    use core::fmt::Write as _;
    let mut detail = heapless::String::<128>::new();
    let truncated = if snapshot.terminal {
        detail.push_str(snapshot.detail()).is_err() || snapshot.detail_truncated
    } else if snapshot.peripheral_runs > 0 {
        let ll = oer_esp32s31_bluetooth::le::peripheral::diagnostics::snapshot();
        write!(
            detail,
            "ll rx={} drop={} ctrl={} queued={} done={} op={:?}",
            ll.received, ll.discarded, ll.control, ll.queued, ll.completed, ll.last_opcode
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

async fn start_advertising(hci: &Host) -> Result<[u8; 6], PeripheralResult> {
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
    command!(Reset::new(), 0);
    let address = command!(ReadBdAddr::new(), 1);
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
        2
    );
    let payload = b"\x02\x01\x06\x08\x09OER-HIL";
    let mut data = [0; 31];
    data[..payload.len()].copy_from_slice(payload);
    command!(LeSetAdvData::new(payload.len() as u8, data), 3);
    command!(LeSetAdvEnable::new(true), 4);
    Ok(address.into_inner())
}
