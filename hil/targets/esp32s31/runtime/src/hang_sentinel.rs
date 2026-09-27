//! A diagnostic that turns a silent stall of the protocol executor into a
//! panic report.
//!
//! A task on the core 0 protocol executor advances a heartbeat every 100 ms.
//! A periodic SYSTIMER alarm 0 interrupt at the highest priority checks it every
//! 250 ms. After 1.5 s without progress it samples `mepc`, the instruction
//! the interrupt preempted, at each of 16 further checks and panics with the
//! samples; the panic report adds the
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
    timer::{PeriodicTimer, systimer::SystemTimer},
};

static HEARTBEAT: AtomicU32 = AtomicU32::new(0);
static LAST_SEEN: AtomicU32 = AtomicU32::new(u32::MAX);
static STALE_CHECKS: AtomicU32 = AtomicU32::new(0);
static TIMER: Mutex<CriticalSectionRawMutex, RefCell<Option<PeriodicTimer<'static, Blocking>>>> =
    Mutex::new(RefCell::new(None));

const CHECK_PERIOD_MILLIS: u64 = 250;
const STALE_CHECK_LIMIT: u32 = 6;

/// Start the sentinel on SYSTIMER alarm 0; its interrupt runs on this core.
pub(super) fn start(systimer: esp_hal::peripherals::SYSTIMER<'static>) {
    let mut periodic = PeriodicTimer::new(SystemTimer::new(systimer).alarm0);
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
    let mut beats = 0_u32;
    loop {
        HEARTBEAT.fetch_add(1, Ordering::Relaxed);
        beats += 1;
        if beats.is_multiple_of(10) {
            let (point, count) =
                oer_esp32s31_ieee80211_runtime::diagnostics::runner_await::current();
            log::info!("hil-hang: radio loop await point={point} entered={count}");
        }
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
    let stale = STALE_CHECKS.fetch_add(1, Ordering::Relaxed) + 1;
    if stale < STALE_CHECK_LIMIT {
        return;
    }
    // A stalled executor loops through more than one instruction: sample
    // the preempted instruction at every later check to map the loop.
    let index = (stale - STALE_CHECK_LIMIT) as usize;
    SAMPLES[index].store(preempted_instruction(), Ordering::Relaxed);
    if index + 1 == SAMPLES.len() {
        let mut samples = [0_u32; SAMPLE_COUNT];
        for (sample, stored) in samples.iter_mut().zip(&SAMPLES) {
            *sample = stored.load(Ordering::Relaxed);
        }
        panic!(
            "hang sentinel: the core 0 protocol executor made no progress; preempted at {:08x?}",
            samples
        );
    }
}

const SAMPLE_COUNT: usize = 16;
static SAMPLES: [AtomicU32; SAMPLE_COUNT] = [const { AtomicU32::new(0) }; SAMPLE_COUNT];

/// `mepc`: the instruction this interrupt preempted.
fn preempted_instruction() -> u32 {
    let mepc: usize;
    // SAFETY: reading a machine CSR has no side effect.
    #[allow(unsafe_code, reason = "reading mepc requires a CSR instruction")]
    unsafe {
        core::arch::asm!("csrr {0}, mepc", out(reg) mepc);
    }
    mepc as u32
}
