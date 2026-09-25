//! Framed application control: never issues an HCI command or an ATT reply.
use core::convert::Infallible;
use embedded_io_async::Read;
use esp_hal::{Async, usb::usb_serial_jtag::UsbSerialJtag};
use oer_hil_protocol::{
    Capabilities, Command, Envelope, Event, FeatureCapabilities, FrameDecoder, FrameEncoder,
    LinkHealth, RejectReason,
};
use static_cell::StaticCell;

struct Console {
    usb: UsbSerialJtag<'static, Async>,
    boot: u64,
    sequence: u32,
    decoder: FrameDecoder,
    encoder: FrameEncoder,
}

pub(super) trait Profile {
    fn features(&self) -> FeatureCapabilities;
    /// Called only after exact boot and session-zero validation.
    fn command(&self, command: Command) -> Event;
}

impl Console {
    fn capabilities(profile: &impl Profile) -> Capabilities {
        Capabilities {
            features: FeatureCapabilities {
                structured_evidence: true,
                psram_task_stack: true,
                ..profile.features()
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
            .expect("bounded GATT observation");
        let usb = &mut self.usb;
        let sequence = &mut self.sequence;
        async move {
            if oer_hil_protocol::write_frame(usb, bytes).await.is_err() {
                core::future::pending::<()>().await;
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

pub(super) async fn run(
    usb: esp_hal::peripherals::USB_DEVICE<'static>,
    boot: u64,
    profile: &impl Profile,
) -> Infallible {
    static CONSOLE: StaticCell<Console> = StaticCell::new();
    let console = CONSOLE.init_with(|| Console {
        usb: UsbSerialJtag::new(usb).into_async(),
        boot,
        sequence: 0,
        decoder: FrameDecoder::new(),
        encoder: FrameEncoder::new(),
    });
    console
        .send(0, Event::Hello(Console::capabilities(profile)))
        .await;
    loop {
        let mut byte = [0];
        if !matches!(console.usb.read(&mut byte).await, Ok(1)) {
            core::future::pending::<()>().await;
        }
        let Some(command) = console.decode(&byte) else {
            continue;
        };
        let response = match command.validate_target(console.boot) {
            Err(reason) => Event::Rejected(reason),
            Ok(()) if command.session_id != 0 => Event::Rejected(RejectReason::InvalidState),
            Ok(()) => match command.body {
                Command::GetCapabilities => Event::Hello(Console::capabilities(profile)),
                Command::GetBootStatus => Event::BootStatus(crate::system::boot_evidence()),
                Command::QueryLinkHealth => console.health(),
                Command::QueryInterruptStackUsage => Event::InterruptStackUsage {
                    cpu0: crate::stack_evidence::current_irq_snapshot(),
                    cpu1: None,
                },
                command => profile.command(command),
            },
        };
        console.send(command.request_id, response).await;
    }
}
