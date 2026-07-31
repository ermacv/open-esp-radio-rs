//! Early-console and runtime logging backend.
//!
//! Application code should use the macros from the `log` crate. Direct ROM
//! output remains available for the boot and panic paths, where the executor
//! and the asynchronous logging transport may not be running yet.

use core::{
    fmt::{Arguments, Write},
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embedded_io_async::Write as _;
use esp_hal::{
    Async,
    peripherals::USB_DEVICE,
    usb::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagTx},
};

const MESSAGE_CAPACITY: usize = 384;
const QUEUE_CAPACITY: usize = 8;
const DRAIN_BATCH: usize = 4;

#[unsafe(link_section = ".critical.data.logging")]
static WRITER_ACTIVE: AtomicBool = AtomicBool::new(false);
#[unsafe(link_section = ".critical.data.logging")]
static RUNTIME_ACTIVE: AtomicBool = AtomicBool::new(false);
#[unsafe(link_section = ".critical.data.logging")]
static DROPPED_RECORDS: AtomicU32 = AtomicU32::new(0);
#[unsafe(link_section = ".critical.data.logging")]
static TRUNCATED_RECORDS: AtomicU32 = AtomicU32::new(0);
#[unsafe(link_section = ".critical.data.logging")]
static RECORDS: Channel<CriticalSectionRawMutex, TextBuffer<MESSAGE_CAPACITY>, QUEUE_CAPACITY> =
    Channel::new();

unsafe extern "C" {
    fn ets_printf(format: *const u8, ...) -> i32;
}

struct TextBuffer<const N: usize> {
    bytes: [u8; N],
    len: usize,
    truncated: bool,
}

impl<const N: usize> TextBuffer<N> {
    const fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
            truncated: false,
        }
    }

    fn as_c_string(&self) -> *const u8 {
        self.bytes.as_ptr()
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    fn was_truncated(&self) -> bool {
        self.truncated
    }
}

impl<const N: usize> Write for TextBuffer<N> {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let available = self.bytes.len() - 1 - self.len;
        let length = text.len().min(available);
        self.bytes[self.len..self.len + length].copy_from_slice(&text.as_bytes()[..length]);
        self.len += length;
        self.truncated |= length != text.len();
        Ok(())
    }
}

/// Reports the minimum architectural state needed to diagnose a panic.
///
/// This deliberately bypasses the global logger: a panic can happen while the
/// logger is already active, before it is installed, or while normal memory
/// and executor services are unavailable.
#[unsafe(link_section = ".rwtext.logging")]
pub fn panic_report(mcause: usize, mepc: usize, mtval: usize) {
    unsafe {
        ets_printf(
            c"panic mcause=%08x mepc=%08x mtval=%08x\r\n"
                .as_ptr()
                .cast(),
            mcause,
            mepc,
            mtval,
        );
    }
}

/// Formats and writes one emergency line immediately.
///
/// This bypasses the queue and is intended only for early boot, panic, and
/// last-resort diagnostics.
pub fn emergency_log(args: Arguments<'_>) {
    write_line_immediate(args);
}

/// Returns the number of records discarded because the queue was full or
/// another core/interrupt was already using the immediate writer.
pub fn dropped_records() -> u32 {
    DROPPED_RECORDS.load(Ordering::Relaxed)
}

/// Returns the number of records whose text exceeded [`MESSAGE_CAPACITY`].
pub fn truncated_records() -> u32 {
    TRUNCATED_RECORDS.load(Ordering::Relaxed)
}

/// Runs the runtime transport worker.
///
/// Spawn this task once the Embassy executor starts. Before it starts, records
/// are written synchronously so early boot diagnostics remain visible. Once it
/// is active, normal `log` records use a bounded, non-blocking SRAM queue. The
/// worker sleeps while the USB endpoint is busy and resumes from its interrupt;
/// it never spins waiting for the host.
#[embassy_executor::task]
pub async fn logger_task(usb_device: USB_DEVICE<'static>) {
    let (_, mut tx) = UsbSerialJtag::new(usb_device).into_async().split();
    RUNTIME_ACTIVE.store(true, Ordering::Release);
    let mut reported_dropped = 0;
    let mut reported_truncated = 0;
    loop {
        let record = RECORDS.receive().await;
        write_record_async(&mut tx, &record).await;

        for _ in 1..DRAIN_BATCH {
            let Ok(record) = RECORDS.try_receive() else {
                break;
            };
            write_record_async(&mut tx, &record).await;
        }
        report_health_changes(&mut tx, &mut reported_dropped, &mut reported_truncated).await;
        embassy_futures::yield_now().await;
    }
}

async fn report_health_changes(
    tx: &mut UsbSerialJtagTx<'static, Async>,
    reported_dropped: &mut u32,
    reported_truncated: &mut u32,
) {
    let dropped = dropped_records();
    let truncated = truncated_records();
    if dropped == *reported_dropped && truncated == *reported_truncated {
        return;
    }

    let record = format_record(format_args!(
        "[WARN logger] dropped_total={dropped} truncated_total={truncated}"
    ));
    write_record_async(tx, &record).await;
    *reported_dropped = dropped;
    *reported_truncated = truncated;
}

fn format_record(args: Arguments<'_>) -> TextBuffer<MESSAGE_CAPACITY> {
    let mut message = TextBuffer::<MESSAGE_CAPACITY>::new();
    let _ = message.write_fmt(args);
    if message.was_truncated() {
        TRUNCATED_RECORDS.fetch_add(1, Ordering::Relaxed);
    }
    message
}

fn submit_line(args: Arguments<'_>) {
    if RUNTIME_ACTIVE.load(Ordering::Acquire) {
        // Under sustained pressure, avoid paying even the formatting cost for
        // a record that cannot enter the bounded queue. This observation is a
        // best-effort fast path; try_send below remains the authoritative race-
        // safe capacity check.
        if RECORDS.is_full() {
            DROPPED_RECORDS.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let record = format_record(args);
        if RECORDS.try_send(record).is_err() {
            DROPPED_RECORDS.fetch_add(1, Ordering::Relaxed);
        }
    } else {
        let record = format_record(args);
        write_record_immediate(&record);
    }
}

fn write_line_immediate(args: Arguments<'_>) {
    let record = format_record(args);
    write_record_immediate(&record);
}

fn write_record_immediate(message: &TextBuffer<MESSAGE_CAPACITY>) {
    let Ok(_guard) = WriterGuard::acquire() else {
        DROPPED_RECORDS.fetch_add(1, Ordering::Relaxed);
        return;
    };

    unsafe {
        ets_printf(c"%s\r\n".as_ptr().cast(), message.as_c_string());
    }
}

async fn write_record_async(
    tx: &mut UsbSerialJtagTx<'static, Async>,
    message: &TextBuffer<MESSAGE_CAPACITY>,
) {
    let Ok(_guard) = WriterGuard::acquire() else {
        DROPPED_RECORDS.fetch_add(1, Ordering::Relaxed);
        return;
    };

    // The HAL submits at most one 64-byte USB packet at a time. If the endpoint
    // is busy, this await parks the task until SERIAL_IN_EMPTY wakes it.
    let _ = tx.write_all(message.as_bytes()).await;
    let _ = tx.write_all(b"\r\n").await;
}

struct WriterGuard;

impl WriterGuard {
    fn acquire() -> Result<Self, ()> {
        WRITER_ACTIVE
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map(|_| Self)
            .map_err(|_| ())
    }
}

impl Drop for WriterGuard {
    fn drop(&mut self) {
        WRITER_ACTIVE.store(false, Ordering::Release);
    }
}

struct ConsoleLogger;

impl ::log::Log for ConsoleLogger {
    fn enabled(&self, metadata: &::log::Metadata<'_>) -> bool {
        metadata.level() <= ::log::STATIC_MAX_LEVEL
    }

    fn log(&self, record: &::log::Record<'_>) {
        if self.enabled(record.metadata()) {
            submit_line(format_args!(
                "[{} {}] {}",
                record.level(),
                record.target(),
                record.args()
            ));
        }
    }

    fn flush(&self) {}
}

/// Installs the firmware logger. Calling this more than once is harmless.
pub fn init_logger() {
    static LOGGER: ConsoleLogger = ConsoleLogger;
    if ::log::set_logger(&LOGGER).is_ok() {
        ::log::set_max_level(::log::STATIC_MAX_LEVEL);
    }
}

#[cfg(test)]
mod tests {
    use super::TextBuffer;
    use core::fmt::Write;

    #[test]
    fn text_buffer_keeps_space_for_nul() {
        let mut buffer = TextBuffer::<5>::new();
        write!(&mut buffer, "abcdef").unwrap();

        assert_eq!(&buffer.bytes, b"abcd\0");
        assert!(buffer.was_truncated());
    }

    #[test]
    fn exact_fit_is_not_truncated() {
        let mut buffer = TextBuffer::<5>::new();
        write!(&mut buffer, "abcd").unwrap();

        assert_eq!(&buffer.bytes, b"abcd\0");
        assert!(!buffer.was_truncated());
    }
}
