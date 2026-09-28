//! The ESP32-S31 placement of the post-mortem record.
//!
//! The record lives in `.rtc_fast.persistent`, RTC fast memory that no reset
//! entry initializes, so what one boot writes is there for the next unless
//! the chip lost power. [`begin`] runs once, early in every boot: it takes
//! what the previous boot left and starts this boot's ring. The record is
//! written only inside the critical section, except by the panic handler,
//! which must not wait for another hart.

use core::cell::{RefCell, UnsafeCell};

use critical_section::Mutex;
use oer_hil_target_core::postmortem::{Previous, RateMonitor, Record};

struct Retained(UnsafeCell<Record>);

// SAFETY: every access goes through `with_record`, inside the critical
// section, or through the panic path documented at `record_panic`.
#[allow(
    unsafe_code,
    reason = "a static in retained memory needs interior mutability"
)]
unsafe impl Sync for Retained {}

// Every bit pattern is a `Record`, so memory no reset initialized is a value;
// the initializer is never loaded into this NOLOAD section.
#[allow(
    unsafe_code,
    reason = "placing the record in retained memory needs a link section"
)]
#[unsafe(link_section = ".rtc_fast.persistent")]
static RECORD: Retained = Retained(UnsafeCell::new(Record::EMPTY));

static PREVIOUS: Mutex<RefCell<Option<Previous>>> = Mutex::new(RefCell::new(None));
static RATE: Mutex<RefCell<RateMonitor>> = Mutex::new(RefCell::new(RateMonitor::new()));

fn with_record<T>(access: impl FnOnce(&mut Record) -> T) -> T {
    critical_section::with(|_| {
        // SAFETY: the critical section excludes every other access but the
        // panic path.
        #[allow(unsafe_code, reason = "the retained record is a raw static")]
        let record = unsafe { &mut *RECORD.0.get() };
        access(record)
    })
}

fn uptime_ms() -> u32 {
    esp_hal::time::Instant::now()
        .duration_since_epoch()
        .as_millis() as u32
}

fn hart() -> u8 {
    esp_hal::system::Cpu::current() as u8
}

/// Take what the previous boot left and start this boot's record.
pub(crate) fn begin() {
    let previous = with_record(Record::begin_boot);
    critical_section::with(|cs| *PREVIOUS.borrow_ref_mut(cs) = previous);
}

/// The previous boot's record, as boot evidence reports it.
pub(crate) fn previous<T>(read: impl FnOnce(Option<&Previous>) -> T) -> T {
    critical_section::with(|cs| read(PREVIOUS.borrow_ref(cs).as_ref()))
}

/// Record that this boot passed `name`. For named phase transitions, not
/// hot loops: debug builds log a second with too many checkpoints.
#[allow(dead_code, reason = "each image checkpoints its own phases")]
pub(crate) fn checkpoint(name: &str, arg: u32) {
    let now = uptime_ms();
    with_record(|record| record.checkpoint(name, arg, now, hart()));
    let excess = critical_section::with(|cs| RATE.borrow_ref_mut(cs).record(now));
    #[cfg(all(debug_assertions, feature = "open-radio-hil"))]
    if let Some(count) = excess {
        log::warn!("hil-postmortem: {count} checkpoints in one second; not for hot loops");
    }
    #[cfg(not(all(debug_assertions, feature = "open-radio-hil")))]
    let _ = excess;
}

/// Record the hang the watchdog found. The stalled hart may hold the
/// critical section, so, as for a panic, the record is written without it.
#[cfg(all(feature = "open-radio-hil", not(feature = "memory-benchmark")))]
pub(crate) fn record_hang(hang: &oer_hil_protocol::HangFault) {
    // SAFETY: see `record_panic`; the chip resets right after.
    #[allow(unsafe_code, reason = "a hang cannot wait for the critical section")]
    let record = unsafe { &mut *RECORD.0.get() };
    record.record_hang(hang);
}

/// Record the panic being handled. The other hart may hold the critical
/// section while this one panics, so the record is written without it; a
/// checkpoint racing this write can at worst fail its own CRC.
pub(crate) fn record_panic(info: &core::panic::PanicInfo<'_>) {
    use core::fmt::Write as _;
    let mut message = heapless::String::<96>::new();
    let _ = write!(message, "{}", info.message());
    let (file, line) = info
        .location()
        .map_or(("", 0), |location| (location.file(), location.line()));
    // SAFETY: see above; the halted boot never reads the record again.
    #[allow(unsafe_code, reason = "a panic cannot wait for the critical section")]
    let record = unsafe { &mut *RECORD.0.get() };
    record.record_panic(file, line, &message);
}
