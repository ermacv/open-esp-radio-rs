//! HIL owns the command lease and UART; the production composition owns RF.

use bt_hci::{
    cmd::{
        SyncCmd,
        controller_baseband::Reset,
        le::{LeReceiverTestV2, LeTestEnd, LeTransmitterTestV2},
    },
    controller::Controller,
};
use embassy_time::{Duration, Instant, with_timeout};
use embedded_io_async::{Read as _, Write as _};
use esp_hal::{Async, usb::usb_serial_jtag::UsbSerialJtag};
use oer_esp32s31_bluetooth::{
    le::{
        dtm::{DtmDefaultTxPowerDbm, DtmRuntimeConfig},
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
    PassiveScanSchedulerAllocationConfig,
};
use oer_esp32s31_radio_platform_esp_hal::{EspHalBluetoothPlatform, EspHalRadioPlatform};
use open_esp_radio_hil_protocol::{
    BluetoothDtmEvidence as Evidence, BluetoothDtmOperation as Operation,
    BluetoothDtmResult as Outcome, Capabilities, Command, Envelope, Event, FeatureCapabilities,
    FrameDecoder, FrameEncoder, LinkHealth, RejectReason,
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
    );
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
    };
    let (_, _, never) =
        embassy_futures::join::join3(console.run(&hci), pump(&hci), hardware.as_mut()).await;
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
}

impl Console {
    async fn send(&mut self, hci: &Host, request_id: u32, body: Event) {
        let envelope = Envelope::new(self.boot_id, self.sequence, 0, request_id, body);
        let bytes = self
            .encoder
            .encode(&envelope)
            .expect("bounded HIL response");
        // USB backpressure cannot keep a DTM transmitter alive indefinitely.
        if !matches!(
            with_timeout(Duration::from_secs(2), self.usb.write_all(bytes)).await,
            Ok(Ok(()))
        ) {
            let _ = execute(hci, Operation::Reset).await;
            core::future::pending::<()>().await;
        }
        self.sequence = self
            .sequence
            .checked_add(1)
            .expect("HIL sequence exhausted");
    }

    async fn run(&mut self, hci: &Host) {
        let capabilities = Capabilities {
            features: FeatureCapabilities {
                bluetooth_dtm: true,
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
                    let _ = execute(hci, Operation::Reset).await;
                    core::future::pending::<()>().await;
                    continue;
                }
                Ok(Ok(0)) => continue,
                Ok(Ok(_)) => {}
            }
            let mut received = None;
            self.decoder.feed::<Command>(&byte, |frame| {
                received = frame.ok();
            });
            let Some(command) = received else {
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
