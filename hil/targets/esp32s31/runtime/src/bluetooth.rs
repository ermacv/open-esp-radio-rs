//! Bluetooth LE Direct Test Mode over the production radio composition.
//!
//! The console answers the typed HIL protocol. A separate task owns the DTM
//! session: it plans each test event against a fresh radio time, submits it to
//! the radio runtime and accounts the outcomes. The composition's runner
//! drives the radio on its own task.

use embassy_futures::select::{Either3, select3};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::{Duration, Timer, with_timeout};
use embedded_io_async::Read;
use esp_hal::{Async, usb::usb_serial_jtag::UsbSerialJtag};
use oer_bluetooth_ll::dtm::{
    DTM_MAX_PAYLOAD, DtmPayloadPattern, DtmSession, DtmStop, DtmTest,
};
use oer_bluetooth_radio::{RadioRequest, TestChannel, TestPhy, TxPower};
use oer_esp32s31_bluetooth::resources::BluetoothRadioHardware;
use oer_esp32s31_bluetooth_system::{
    BluetoothRunner, BluetoothSystemRuntime, start_esp32s31_bluetooth,
};
use oer_esp32s31_radio_esp_hal::EspHalRadioPlatform;
use oer_hil_protocol::{
    BluetoothDtmEvidence, BluetoothDtmOperation as Operation, BluetoothDtmResult,
    BluetoothDtmRxDiagnostics, Capabilities, Command, Envelope, Event, FeatureCapabilities,
    FrameDecoder, FrameEncoder, LinkHealth, RejectReason,
};
use static_cell::StaticCell;

/// DTM on LE 1M, channel 0, with 37-byte PRBS9 payloads.
const CHANNEL: u8 = 0;
const PAYLOAD_BYTES: u8 = 37;
/// Bound on one DTM operation, including draining the event in progress.
const OPERATION_TIMEOUT: Duration = Duration::from_secs(2);
/// Delay before planning again after the radio refused an event.
const REPLAN_DELAY: Duration = Duration::from_millis(1);

static PLATFORM: StaticCell<EspHalRadioPlatform> = StaticCell::new();
static OPERATIONS: Channel<CriticalSectionRawMutex, Operation, 1> = Channel::new();
static RESULTS: Channel<CriticalSectionRawMutex, (BluetoothDtmResult, BluetoothDtmRxDiagnostics), 1> =
    Channel::new();

pub(super) fn start(
    executor: &'static mut super::Executor<0>,
    platform: EspHalRadioPlatform,
    usb: esp_hal::peripherals::USB_DEVICE<'static>,
    rng: esp_hal::peripherals::RNG<'static>,
) -> ! {
    let platform = PLATFORM.init(platform);
    let entropy = esp_hal::rng::TrngSource::new(rng);
    let rng = esp_hal::rng::Trng::try_new().expect("HIL boot entropy");
    let boot = ((u64::from(rng.random()) << 32) | u64::from(rng.random())).max(1);
    drop(rng);
    drop(entropy);
    let hardware = BluetoothRadioHardware::take().expect("unique Bluetooth hardware");
    executor.run(|spawner| {
        spawner.spawn(main(spawner, platform, hardware, usb, boot).expect("Bluetooth task"));
    })
}

#[embassy_executor::task]
#[allow(
    large_assignments,
    reason = "the cold-start result crosses one poll boundary; the linked-image stack-frame audit bounds this task"
)]
async fn main(
    spawner: embassy_executor::Spawner,
    platform: &'static EspHalRadioPlatform,
    hardware: BluetoothRadioHardware,
    usb: esp_hal::peripherals::USB_DEVICE<'static>,
    boot: u64,
) {
    let system = match start_esp32s31_bluetooth(platform, hardware, None).await {
        Ok(system) => system,
        Err(_) => super::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-cold-start\r\n"),
    };
    spawner.spawn(runner(system.runner).expect("Bluetooth runner task"));
    spawner.spawn(dtm(system.runtime).expect("Bluetooth DTM task"));
    console(usb, boot).await;
}

#[embassy_executor::task]
async fn runner(runner: BluetoothRunner) {
    let _fault = runner.run().await;
    super::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-runner-fault\r\n");
}

#[derive(Default)]
struct Diagnostics {
    refused: u32,
}

#[embassy_executor::task]
async fn dtm(runtime: &'static BluetoothSystemRuntime) {
    let mut session = DtmSession::new(TxPower::from_dbm(0), 1);
    let mut payload = [0; DTM_MAX_PAYLOAD];
    let mut diagnostics = Diagnostics::default();
    let mut retry = false;
    loop {
        if !retry {
            retry = !plan(runtime, &mut session, &mut payload, &mut diagnostics).await;
        }
        let delay = async {
            if retry {
                Timer::after(REPLAN_DELAY).await;
            } else {
                core::future::pending::<()>().await;
            }
        };
        match select3(OPERATIONS.receive(), runtime.next_outcome(), delay).await {
            Either3::First(operation) => {
                let result = with_timeout(
                    OPERATION_TIMEOUT,
                    execute(runtime, &mut session, operation),
                )
                .await
                .unwrap_or(BluetoothDtmResult::Timeout);
                RESULTS
                    .send((result, rx_diagnostics(&session, &diagnostics)))
                    .await;
                retry = false;
            }
            Either3::Second(Ok(outcome)) => session.observe(outcome.portable()),
            Either3::Second(Err(_lost)) => {}
            Either3::Third(()) => retry = false,
        }
    }
}

/// Submit the session's next event. `false` when the radio refused it.
async fn plan(
    runtime: &BluetoothSystemRuntime,
    session: &mut DtmSession,
    payload: &mut [u8; DTM_MAX_PAYLOAD],
    diagnostics: &mut Diagnostics,
) -> bool {
    if !session.is_active() {
        return true;
    }
    let Ok((now, timing)) = runtime.clock().await else {
        return false;
    };
    let Some(request) = session.next_request(now, timing, payload) else {
        return true;
    };
    if runtime.request(request).await.is_ok() {
        return true;
    }
    session.refused();
    diagnostics.refused += 1;
    false
}

async fn execute(
    runtime: &BluetoothSystemRuntime,
    session: &mut DtmSession,
    operation: Operation,
) -> BluetoothDtmResult {
    let channel = TestChannel::new(CHANNEL).expect("channel zero");
    let started = match operation {
        Operation::Transmit => session.start(DtmTest::Transmit {
            channel,
            phy: TestPhy::Le1M,
            pattern: DtmPayloadPattern::Prbs9,
            length: PAYLOAD_BYTES,
        }),
        Operation::Receive => session.start(DtmTest::Receive {
            channel,
            phy: TestPhy::Le1M,
        }),
        Operation::End | Operation::Reset => {
            let received = stop(runtime, session).await;
            return BluetoothDtmResult::Complete {
                received_packets: (operation == Operation::End).then_some(received),
            };
        }
    };
    match started {
        Ok(()) => BluetoothDtmResult::Complete {
            received_packets: None,
        },
        Err(_) => BluetoothDtmResult::HciRejected,
    }
}

/// End the test, draining the event in progress, and release the DTM role.
async fn stop(runtime: &BluetoothSystemRuntime, session: &mut DtmSession) -> u16 {
    let received = match session.end() {
        DtmStop::Stopped { received } => received,
        DtmStop::Draining(id) => {
            // The event may already have ended; its outcome still arrives.
            let _ = runtime.request(RadioRequest::Cancel(id)).await;
            loop {
                if let Some(received) = session.drained() {
                    break received;
                }
                if let Ok(outcome) = runtime.next_outcome().await {
                    session.observe(outcome.portable());
                }
            }
        }
    };
    // No DTM role is held when no event was ever admitted.
    let _ = runtime.request(RadioRequest::EndTest).await;
    received
}

fn rx_diagnostics(session: &DtmSession, diagnostics: &Diagnostics) -> BluetoothDtmRxDiagnostics {
    let counters = session.counters();
    BluetoothDtmRxDiagnostics {
        successful_events: counters.executed,
        failed_events: counters.failed,
        empty_events: counters.empty,
        counted_packets: counters.received,
        rejected_packets: diagnostics.refused,
        ..BluetoothDtmRxDiagnostics::default()
    }
}

struct Console {
    usb: UsbSerialJtag<'static, Async>,
    boot: u64,
    sequence: u32,
    decoder: FrameDecoder,
    encoder: FrameEncoder,
}

impl Console {
    fn capabilities() -> Capabilities {
        Capabilities {
            features: FeatureCapabilities {
                bluetooth_dtm: true,
                phy_rx_hot_sram: cfg!(feature = "phy-rx-hot-sram"),
                structured_evidence: true,
                psram_task_stack: true,
                ..FeatureCapabilities::default()
            },
            maximum_payload_bytes: u16::from(PAYLOAD_BYTES),
            maximum_wire_frame_bytes: oer_hil_protocol::MAX_WIRE_FRAME_BYTES as u16,
        }
    }

    #[inline(never)]
    fn send(&mut self, request: u32, event: Event) -> impl Future<Output = ()> + '_ {
        let bytes = self
            .encoder
            .encode(&Envelope::new(self.boot, self.sequence, 0, request, event))
            .expect("bounded Bluetooth response");
        let usb = &mut self.usb;
        let sequence = &mut self.sequence;
        async move {
            if oer_hil_protocol::write_frame(usb, bytes).await.is_err() {
                esp_hal::system::software_reset();
            }
            *sequence = sequence.checked_add(1).expect("HIL sequence exhausted");
        }
    }

    #[inline(never)]
    fn decode(&mut self, byte: &[u8]) -> Option<Envelope<Command>> {
        let mut command = None;
        self.decoder
            .feed::<Command>(byte, |frame| command = frame.ok());
        command
    }

    fn health(&self) -> Event {
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
}

async fn console(usb: esp_hal::peripherals::USB_DEVICE<'static>, boot: u64) -> ! {
    static CONSOLE: StaticCell<Console> = StaticCell::new();
    let console = CONSOLE.init_with(|| Console {
        usb: UsbSerialJtag::new(usb).into_async(),
        boot,
        sequence: 0,
        decoder: FrameDecoder::new(),
        encoder: FrameEncoder::new(),
    });
    console.send(0, Event::Hello(Console::capabilities())).await;
    loop {
        let mut byte = [0];
        if !matches!(console.usb.read(&mut byte).await, Ok(1)) {
            continue;
        }
        let Some(command) = console.decode(&byte) else {
            continue;
        };
        let response = match command.validate_target(console.boot) {
            Err(reason) => Event::Rejected(reason),
            Ok(()) if command.session_id != 0 => Event::Rejected(RejectReason::InvalidState),
            Ok(()) => match command.body {
                Command::GetCapabilities => Event::Hello(Console::capabilities()),
                Command::GetBootStatus => Event::BootStatus(super::system::boot_evidence()),
                Command::QueryLinkHealth => console.health(),
                Command::BluetoothDtm(operation) => {
                    OPERATIONS.send(operation).await;
                    let (result, rx_diagnostics) = RESULTS.receive().await;
                    Event::BluetoothDtm(BluetoothDtmEvidence {
                        reset_reason: super::system::boot_evidence().reset_reason,
                        operation,
                        result,
                        rx_diagnostics,
                    })
                }
                _ => Event::Rejected(RejectReason::InvalidState),
            },
        };
        console.send(command.request_id, response).await;
    }
}
