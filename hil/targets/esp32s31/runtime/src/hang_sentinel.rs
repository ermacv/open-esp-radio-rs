//! A diagnostic that turns a silent stall of the protocol executor into a
//! panic report.
//!
//! A task on the core 0 protocol executor advances a heartbeat every 100 ms.
//! A periodic TIMG0 timer 1 interrupt at the highest priority checks it every
//! 500 ms and panics after three seconds without progress. The panic report
//! then carries `mepc`, the instruction the interrupt preempted, and the
//! pending interrupt sources, which tell a busy loop, an interrupt storm and
//! a wait apart. A core that spins with its interrupts masked never takes the
//! sentinel interrupt, which is itself evidence.

use core::cell::RefCell;
use core::sync::atomic::{AtomicU32, Ordering};

use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
use esp_hal::{
    Blocking,
    interrupt::Priority,
    time::Duration,
    timer::{PeriodicTimer, timg::Timer},
};

static HEARTBEAT: AtomicU32 = AtomicU32::new(0);
static LAST_SEEN: AtomicU32 = AtomicU32::new(u32::MAX);
static STALE_CHECKS: AtomicU32 = AtomicU32::new(0);
static TIMER: Mutex<CriticalSectionRawMutex, RefCell<Option<PeriodicTimer<'static, Blocking>>>> =
    Mutex::new(RefCell::new(None));

const CHECK_PERIOD_MILLIS: u64 = 500;
const STALE_CHECK_LIMIT: u32 = 6;

/// Start the sentinel on TIMG0 timer 1; its interrupt runs on this core.
pub(super) fn start(timer: Timer<'static>) {
    let mut periodic = PeriodicTimer::new(timer);
    periodic.set_interrupt_handler(check);
    periodic
        .start(Duration::from_millis(CHECK_PERIOD_MILLIS))
        .expect("the sentinel period fits the timer");
    periodic.listen();
    TIMER.lock(|cell| cell.borrow_mut().replace(periodic));
}

/// The heartbeat of the core 0 protocol executor.
#[embassy_executor::task]
pub(super) async fn heartbeat_task() {
    loop {
        HEARTBEAT.fetch_add(1, Ordering::Relaxed);
        embassy_time::Timer::after_millis(100).await;
    }
}

#[esp_hal::handler(priority = Priority::max())]
#[allow(
    unsafe_code,
    reason = "esp-hal requires an unsafe link_section attribute for an IRAM ISR declaration"
)]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
fn check() {
    TIMER.lock(|cell| {
        if let Some(timer) = cell.borrow_mut().as_mut() {
            timer.clear_interrupt();
        }
    });
    let beat = HEARTBEAT.load(Ordering::Relaxed);
    if LAST_SEEN.swap(beat, Ordering::Relaxed) != beat {
        STALE_CHECKS.store(0, Ordering::Relaxed);
        return;
    }
    if STALE_CHECKS.fetch_add(1, Ordering::Relaxed) + 1 == STALE_CHECK_LIMIT {
        panic!("hang sentinel: the core 0 protocol executor made no progress for 3 s");
    }
}
