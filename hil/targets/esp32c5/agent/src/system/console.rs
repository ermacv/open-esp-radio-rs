//! The radio-free SoC image's HIL console: the target core's console over
//! this chip's USB Serial/JTAG endpoint, serving the SoC deadline watchdog.
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use esp_hal::{timer::timg::TimerGroup, usb::usb_serial_jtag::UsbSerialJtag};
use oer_esp32c5_soc_esp_hal::watchdog::{DeadlineBudget, DeadlineWatchdog};
use oer_hil_agent::base::{Intake, Platform, WatchdogRequest};
use oer_hil_agent::console::Console;
use oer_hil_image_keys::{ImageKeys, image_keys};
use oer_hil_protocol::RequestIdentity;
use oer_hil_protocol::base::{
    BootEvidence, ImageKeySet, PostMortemCheckpoints, RejectReason, Rejected,
};
use oer_hil_protocol::system::{WatchdogArmed, WatchdogTest, WatchdogTestMode};
use static_cell::StaticCell;

unsafe extern "C" {
    fn ets_printf(format: *const core::ffi::c_char, ...) -> i32;
}

/// Writes one line through the ROM, independently of the USB driver.
fn rom_line(line: &core::ffi::CStr) {
    // SAFETY: the format consumes exactly one NUL-terminated string argument.
    unsafe {
        ets_printf(c"%s\r\n".as_ptr(), line.as_ptr());
    }
}

pub(crate) static CONSOLE: Console<256, 8, 8> = Console::new(rom_line);

/// Watchdog tests the console handed over, one at a time.
static TESTS: Channel<CriticalSectionRawMutex, (RequestIdentity, WatchdogTestMode), 1> =
    Channel::new();

struct Chip;

impl Platform for Chip {
    fn boot_evidence(&self) -> BootEvidence {
        super::boot_evidence()
    }

    fn post_mortem_checkpoints(&self, first: u8) -> PostMortemCheckpoints {
        super::post_mortem_checkpoints(first)
    }
}

/// Serve the host until the chip resets.
pub(crate) async fn run(peripherals: esp_hal::peripherals::Peripherals) -> ! {
    let usb = UsbSerialJtag::new(peripherals.USB_DEVICE).into_async();
    static SERVICE: StaticCell<DeadlineWatchdog> = StaticCell::new();
    let service: &'static DeadlineWatchdog = SERVICE.init(DeadlineWatchdog::new(peripherals.TIMG1));
    // The boot identity only has to differ between boots.
    let rng = esp_hal::rng::Rng::new();
    let boot = ((u64::from(rng.random()) << 32) | u64::from(rng.random())).max(1);
    let timers = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timers.timer0);
    CONSOLE.start(boot);

    // This image's keys: the same function of its Cargo features the host
    // derives the system-watchdog class's keys with.
    static KEYS: StaticCell<ImageKeys> = StaticCell::new();
    let keys = KEYS.init(image_keys(&|feature| feature == "system-watchdog"));
    let intake = Intake::new(boot, ImageKeySet::new(keys), 0);
    let (rx, tx) = usb.split();
    let console = CONSOLE.run(
        rx,
        tx,
        intake,
        &Chip,
        |request, WatchdogRequest::Test(WatchdogTest(mode))| {
            if TESTS.try_send((request, mode)).is_err() {
                CONSOLE.publish(
                    request.session_id,
                    request.request_id,
                    &Rejected(RejectReason::Busy),
                );
            }
        },
    );
    embassy_futures::join::join(console, test_watchdog(service))
        .await
        .0
}

/// Arms the SoC deadline watchdog for each test and misbehaves as asked.
async fn test_watchdog(service: &'static DeadlineWatchdog) -> ! {
    loop {
        let (request, mode) = TESTS.receive().await;
        let budget = DeadlineBudget::from_micros(core::num::NonZeroU32::new(1_000_000).unwrap());
        let Ok(lease) = service.arm(budget) else {
            let refused = Rejected(RejectReason::InvalidState);
            CONSOLE
                .publish_reliably(request.session_id, request.request_id, &refused)
                .await;
            continue;
        };
        // The host finds this in the next boot's post-mortem.
        super::postmortem::checkpoint("watchdog.arm", mode as u32);
        let armed = WatchdogArmed(mode);
        if mode == WatchdogTestMode::Complete {
            lease.complete().expect("immediate diagnostic completion");
            CONSOLE
                .publish_reliably(request.session_id, request.request_id, &armed)
                .await;
        } else {
            let sequence = CONSOLE
                .publish_reliably(request.session_id, request.request_id, &armed)
                .await;
            // The host must see the acknowledgement before the reset.
            CONSOLE.written(sequence).await;
            super::watchdog::inject(mode, lease).await;
        }
    }
}
