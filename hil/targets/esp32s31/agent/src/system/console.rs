//! The radio-free SoC diagnostic image: the HIL console serving the SoC
//! deadline watchdog.
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use oer_esp32s31_soc_esp_hal::watchdog::{DeadlineBudget, DeadlineWatchdog};
use oer_hil_protocol::RequestIdentity;
use oer_hil_protocol::base::{RejectReason, Rejected};
use oer_hil_protocol::system::{WatchdogArmed, WatchdogTest, WatchdogTestMode};
use static_cell::StaticCell;

use crate::transport::CONSOLE;
use oer_hil_agent::base::WatchdogRequest;

/// Watchdog tests the console handed over, one at a time.
static TESTS: Channel<CriticalSectionRawMutex, (RequestIdentity, WatchdogTestMode), 1> =
    Channel::new();

pub(crate) fn start(
    executor: &'static mut crate::Executor<0>,
    usb: esp_hal::peripherals::USB_DEVICE<'static>,
    rng: esp_hal::peripherals::RNG<'static>,
    timer: esp_hal::peripherals::TIMG1<'static>,
) -> ! {
    static SERVICE: StaticCell<DeadlineWatchdog> = StaticCell::new();
    let service = SERVICE.init(DeadlineWatchdog::new(timer));
    let entropy = esp_hal::rng::TrngSource::new(rng);
    let rng = esp_hal::rng::Trng::try_new().expect("HIL boot entropy");
    let boot = ((u64::from(rng.random()) << 32) | u64::from(rng.random())).max(1);
    drop(rng);
    drop(entropy);
    CONSOLE.start(boot);
    crate::transport::init_logger();
    executor.run(|spawner| {
        spawner.spawn(test_watchdog(service).expect("system watchdog test task"));
        spawner.spawn(run(usb, boot).expect("system console task"));
    });
}

#[embassy_executor::task]
async fn run(usb: esp_hal::peripherals::USB_DEVICE<'static>, boot: u64) {
    crate::transport::serve(
        usb,
        boot,
        0,
        |request, WatchdogRequest::Test(WatchdogTest(mode))| {
            if TESTS.try_send((request, mode)).is_err() {
                CONSOLE.publish(
                    request.session_id,
                    request.request_id,
                    &Rejected(RejectReason::Busy),
                );
            }
        },
    )
    .await
}

/// Arms the SoC deadline watchdog for each test and misbehaves as asked.
#[embassy_executor::task]
async fn test_watchdog(service: &'static DeadlineWatchdog) {
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
