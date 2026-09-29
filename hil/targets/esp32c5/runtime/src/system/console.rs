//! Sole transport owner of the radio-free SoC image: the HIL protocol over
//! the USB Serial/JTAG console.
use embedded_io_async::Read;
use esp_hal::{Async, timer::timg::TimerGroup, usb::usb_serial_jtag::UsbSerialJtag};
use oer_esp32c5_soc_esp_hal::watchdog::{DeadlineBudget, DeadlineWatchdog};
use oer_hil_protocol::{
    Capabilities, Command, Envelope, Event, FeatureCapabilities, FrameDecoder, FrameEncoder,
    LinkHealth, RejectReason, WatchdogTestMode,
};
use static_cell::StaticCell;

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
                system_watchdog: true,
                structured_evidence: true,
                ..FeatureCapabilities::default()
            },
            maximum_payload_bytes: 0,
            maximum_wire_frame_bytes: oer_hil_protocol::MAX_WIRE_FRAME_BYTES as u16,
        }
    }

    #[inline(never)]
    fn send(&mut self, request: u32, event: Event) -> impl Future<Output = ()> + '_ {
        let bytes = self
            .encoder
            .encode(&Envelope::new(self.boot, self.sequence, 0, request, event))
            .expect("bounded system response");
        let usb = &mut self.usb;
        let sequence = &mut self.sequence;
        async move {
            // A transport failure cannot fabricate watchdog completion or a
            // software-reset substitute for the expected hardware reset.
            if oer_hil_protocol::write_frame(usb, bytes).await.is_err() {
                core::future::pending::<()>().await;
            }
            *sequence = sequence.checked_add(1).expect("HIL sequence exhausted");
        }
    }

    /// The next complete command, or the request of an intact frame this
    /// image cannot decode: a command of a radio family it leaves out.
    #[inline(never)]
    fn decode(
        &mut self,
        byte: &[u8],
    ) -> Option<Result<Envelope<Command>, oer_hil_protocol::RequestIdentity>> {
        let mut command = None;
        self.decoder.feed::<Command>(byte, |frame| {
            command = match frame {
                Ok(frame) => Some(Ok(frame)),
                Err(oer_hil_protocol::DecodeError::UndecodableBody(request)) => Some(Err(request)),
                Err(_) => None,
            }
        });
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

/// Serve the host until the chip resets.
pub(crate) async fn run(peripherals: esp_hal::peripherals::Peripherals) -> ! {
    // The console opens before the scheduler starts, so a fault while it
    // starts still leaves the lines before it on the console.
    let usb = UsbSerialJtag::new(peripherals.USB_DEVICE).into_async();
    static SERVICE: StaticCell<DeadlineWatchdog> = StaticCell::new();
    let service: &'static DeadlineWatchdog = SERVICE.init(DeadlineWatchdog::new(peripherals.TIMG1));
    // The boot identity only has to differ between boots.
    let rng = esp_hal::rng::Rng::new();
    let boot = ((u64::from(rng.random()) << 32) | u64::from(rng.random())).max(1);
    let timers = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timers.timer0);

    static CONSOLE: StaticCell<Console> = StaticCell::new();
    let console = CONSOLE.init(Console {
        usb,
        boot,
        sequence: 0,
        decoder: FrameDecoder::new(),
        encoder: FrameEncoder::new(),
    });
    console.send(0, Event::Hello(Console::capabilities())).await;
    loop {
        let mut byte = [0];
        if !matches!(console.usb.read(&mut byte).await, Ok(1)) {
            core::future::pending::<()>().await;
        }
        let command = match console.decode(&byte) {
            None => continue,
            Some(Err(request)) => {
                console
                    .send(
                        request.request_id,
                        Event::Rejected(RejectReason::Unsupported),
                    )
                    .await;
                continue;
            }
            Some(Ok(command)) => command,
        };
        let response = match command.validate_target(console.boot) {
            Err(reason) => Event::Rejected(reason),
            Ok(()) if command.session_id != 0 => Event::Rejected(RejectReason::InvalidState),
            Ok(()) => match command.body {
                Command::GetCapabilities => Event::Hello(Console::capabilities()),
                Command::GetBootStatus => Event::BootStatus(super::boot_evidence()),
                Command::GetPostMortemCheckpoints { first } => {
                    Event::PostMortemCheckpoints(super::post_mortem_checkpoints(first))
                }
                Command::QueryLinkHealth => console.health(),
                Command::SystemWatchdogTest(mode) => {
                    let budget =
                        DeadlineBudget::from_micros(core::num::NonZeroU32::new(1_000_000).unwrap());
                    match service.arm(budget) {
                        Err(_) => Event::Rejected(RejectReason::InvalidState),
                        Ok(lease) => {
                            // The host finds this in the next boot's post-mortem.
                            super::postmortem::checkpoint("watchdog.arm", mode as u32);
                            if mode == WatchdogTestMode::Complete {
                                lease.complete().expect("immediate diagnostic completion");
                                Event::SystemWatchdogTest(mode)
                            } else {
                                console
                                    .send(command.request_id, Event::SystemWatchdogTest(mode))
                                    .await;
                                super::watchdog::inject(mode, lease).await
                            }
                        }
                    }
                }
                _ => Event::Rejected(RejectReason::InvalidState),
            },
        };
        console.send(command.request_id, response).await;
    }
}
