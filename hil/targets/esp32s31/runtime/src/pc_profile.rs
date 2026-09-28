//! Statistical program-counter profile of both harts during one traffic
//! window.
//!
//! SYSTIMER alarm 1 interrupts core 0 at the highest priority every
//! [`PERIOD_MICROS`]. Its handler records the instruction core 0 was
//! interrupted at (`mepc`) and the interrupted code's return address, then
//! raises FROM_CPU_INTR3, whose handler bound on core 1 records core 1's pair
//! the same way. Samples cover interrupt handlers of lower priority, executor
//! tasks and the idle wait alike, so the profile divides each hart's whole
//! time, not only task polls.
//!
//! Only an armed window records. After it the report task sorts each hart's
//! samples and prints run-length `pc:ra:count` records (`OPROF`/`OPROFS`);
//! the host symbolizes them against the image ELF. The return address names
//! the caller only while the sampled function has not yet made a call of its
//! own; it is exact for leaf functions such as copies and checksums.

use core::cell::{RefCell, UnsafeCell};
use core::fmt::Write as _;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use critical_section::Mutex;
use embassy_time::Timer;
use esp_hal::{
    Blocking,
    interrupt::Priority,
    time::Duration,
    timer::{PeriodicTimer, systimer::Alarm},
};

use crate::console::runtime_log_reliably;

/// Sampling period: a prime number of microseconds, so the samples do not
/// lock to the 1024-us TU cadence of beacons and power-save timers.
const PERIOD_MICROS: u64 = 1999;
/// Samples retained per hart: a 16-second window at [`PERIOD_MICROS`].
const CAPACITY: usize = 8192;
/// `pc:ra:count` records per printed line, within one console record.
const RECORDS_PER_LINE: usize = 12;

/// One hart's samples, `mepc << 32 | ra`.
struct Samples(UnsafeCell<[u64; CAPACITY]>);

// SAFETY: during an armed window only the sampling handler of the owning
// hart writes, each to a slot it claimed through `CLAIMED`; the report task
// reads only after disarming and waiting out a sampling period.
#[allow(
    unsafe_code,
    reason = "sample slots are claimed atomically by one hart"
)]
unsafe impl Sync for Samples {}

#[allow(
    unsafe_code,
    reason = "the sample buffers are placed in the PSRAM tier to keep internal SRAM for the measured path"
)]
#[unsafe(link_section = ".psram.bss.open_radio_pc_profile")]
static SAMPLES: [Samples; 2] = [const { Samples(UnsafeCell::new([0; CAPACITY])) }; 2];
/// Slots each hart claimed in the current window, possibly above capacity.
static CLAIMED: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];
static ARMED: AtomicBool = AtomicBool::new(false);
static TIMER: Mutex<RefCell<Option<PeriodicTimer<'static, Blocking>>>> =
    Mutex::new(RefCell::new(None));

/// Start the sampling timer on this core (core 0).
pub(crate) fn start(alarm: Alarm<'static>) {
    let mut periodic = PeriodicTimer::new(alarm);
    periodic.set_interrupt_handler(sample_core0);
    periodic
        .start(Duration::from_micros(PERIOD_MICROS))
        .expect("the profile period fits the timer");
    periodic.listen();
    critical_section::with(|cs| TIMER.borrow_ref_mut(cs).replace(periodic));
}

/// Bind the core 1 sampling interrupt; call on core 1 before its executor
/// enables interrupts.
pub(crate) fn bind_core1_sampler() {
    let mut interrupt = crate::software_interrupt::profiler();
    interrupt.set_interrupt_handler(sample_core1);
}

/// Begin a profile window, discarding the previous one.
pub(crate) fn arm() {
    for claimed in &CLAIMED {
        claimed.store(0, Ordering::Relaxed);
    }
    ARMED.store(true, Ordering::Release);
}

/// End the profile window.
pub(crate) fn disarm() {
    ARMED.store(false, Ordering::Release);
}

fn record(hart: usize) {
    if !ARMED.load(Ordering::Acquire) {
        return;
    }
    let mepc: usize;
    // SAFETY: reading a machine CSR has no side effect.
    #[allow(unsafe_code, reason = "reading a CSR requires a CSR instruction")]
    unsafe {
        core::arch::asm!("csrr {0}, mepc", out(reg) mepc);
    }
    let ra = esp_hal::interrupt::interrupted_context()
        .map(|context| context.ra)
        .unwrap_or(0);
    let index = CLAIMED[hart].fetch_add(1, Ordering::Relaxed) as usize;
    if index < CAPACITY {
        // SAFETY: `index` was claimed by this hart's only sampling handler.
        #[allow(unsafe_code, reason = "a claimed slot has one writer")]
        unsafe {
            (*SAMPLES[hart].0.get())[index] = (mepc as u64) << 32 | ra as u64;
        }
    }
}

#[esp_hal::handler(priority = Priority::max())]
#[allow(
    unsafe_code,
    reason = "esp-hal requires an unsafe link_section attribute for an IRAM ISR declaration"
)]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
fn sample_core0() {
    critical_section::with(|cs| {
        if let Some(timer) = TIMER.borrow_ref_mut(cs).as_mut() {
            timer.clear_interrupt();
        }
    });
    record(0);
    if ARMED.load(Ordering::Relaxed) {
        crate::software_interrupt::profiler().raise();
    }
}

#[esp_hal::handler(priority = Priority::max())]
#[allow(
    unsafe_code,
    reason = "esp-hal requires an unsafe link_section attribute for an IRAM ISR declaration"
)]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
fn sample_core1() {
    crate::software_interrupt::profiler().reset();
    record(1);
}

/// Print the disarmed window's profile of both harts.
pub(crate) async fn report() {
    // A sampling handler that read `ARMED` before `disarm` may still write.
    Timer::after_micros(4 * PERIOD_MICROS).await;
    for hart in 0..2 {
        let claimed = CLAIMED[hart].load(Ordering::Acquire) as usize;
        let retained = claimed.min(CAPACITY);
        // SAFETY: the window is disarmed and its last handler has returned.
        #[allow(unsafe_code, reason = "the disarmed window has no writer")]
        let samples = unsafe { &mut (&mut *SAMPLES[hart].0.get())[..retained] };
        samples.sort_unstable();
        runtime_log_reliably(format_args!(
            "OPROF hart={hart} period_us={PERIOD_MICROS} samples={retained} overflow={}",
            claimed - retained
        ))
        .await;
        let mut line = heapless::String::<384>::new();
        let mut entries = 0;
        let mut index = 0;
        while index < retained {
            let sample = samples[index];
            let run = samples[index..]
                .iter()
                .take_while(|&&other| other == sample)
                .count();
            index += run;
            let _ = write!(line, " {:x}:{:x}:{run}", sample >> 32, sample as u32);
            entries += 1;
            if entries == RECORDS_PER_LINE || index == retained {
                runtime_log_reliably(format_args!("OPROFS hart={hart}{line}")).await;
                line.clear();
                entries = 0;
            }
        }
    }
}
