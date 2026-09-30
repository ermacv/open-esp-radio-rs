//! The target core's HIL console on this chip: its USB Serial/JTAG endpoint,
//! its ROM line output and its platform. Every HIL image of this runtime
//! serves the host through [`serve`].

use core::ffi::CStr;

use esp_hal::usb::usb_serial_jtag::UsbSerialJtag;
use oer_hil_agent::base::{Intake, Platform, Requests};
use oer_hil_agent::console::{Console, Logger};
use oer_hil_protocol::RequestIdentity;
use oer_hil_protocol::base::{BootEvidence, ImageKeySet, PostMortemCheckpoints};

// Images with the Wi-Fi system's diagnostics (every driver-observation image
// and the station-exit diagnostic) log whole Debug records of driver state,
// up to about 900 bytes; production-like images keep the smaller footprint.
#[cfg(feature = "station-exit-evidence")]
const LINE: usize = 1024;
#[cfg(not(feature = "station-exit-evidence"))]
const LINE: usize = 384;
// Diagnostic role transitions emit synchronous register/status bursts before
// the console task can run. Reserve bounded burst storage only in observer
// images; production-like measurements retain the smaller footprint.
#[cfg(feature = "station-exit-evidence")]
const LINES: usize = 32;
#[cfg(not(feature = "station-exit-evidence"))]
const LINES: usize = 8;
const MESSAGES: usize = 8;

unsafe extern "C" {
    fn ets_printf(format: *const core::ffi::c_char, ...) -> i32;
}

/// Writes one line through the ROM, independently of the USB driver.
fn rom_line(line: &CStr) {
    // SAFETY: the format consumes exactly one NUL-terminated string argument.
    unsafe {
        ets_printf(c"%s\r\n".as_ptr(), line.as_ptr());
    }
}

#[unsafe(link_section = ".critical.data.logging")]
pub(crate) static CONSOLE: Console<LINE, LINES, MESSAGES> = Console::new(rom_line);

/// Installs the `log` facade over [`CONSOLE`]. Calling this more than once
/// is harmless.
pub(crate) fn init_logger() {
    static LOGGER: Logger<LINE, LINES, MESSAGES> = Logger(&CONSOLE);
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}

struct Chip;

impl Platform for Chip {
    fn boot_evidence(&self) -> BootEvidence {
        crate::system::boot_evidence()
    }

    fn post_mortem_checkpoints(&self, first: u8) -> PostMortemCheckpoints {
        crate::system::post_mortem_checkpoints(first)
    }
}

/// Serves the host over `usb` until the chip resets: the base module here,
/// the image's own requests through `serve`, which must not block. The boot
/// must have started the console (`CONSOLE.start`) before any task
/// publishes.
pub(crate) async fn serve<C: Requests>(
    usb: esp_hal::peripherals::USB_DEVICE<'static>,
    boot: u64,
    maximum_payload_bytes: u16,
    serve: impl FnMut(RequestIdentity, C),
) -> ! {
    let intake = Intake::new(
        boot,
        ImageKeySet::new(crate::image_features::keys()),
        maximum_payload_bytes,
    );
    let (rx, tx) = UsbSerialJtag::new(usb).into_async().split();
    CONSOLE.run(rx, tx, intake, &Chip, serve).await
}
