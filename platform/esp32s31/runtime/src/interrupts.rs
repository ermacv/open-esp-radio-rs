//! The image's interrupt table on the ESP32-S31 matrix.
//!
//! Every image declares its peripheral interrupt sources once with
//! [`interrupt_table!`](crate::interrupt_table): source, handler, level and
//! core. The runtime silences each source of a hart's entries when it installs
//! that hart's interrupt stack and checks every vector slot against the table;
//! [`enable_interrupts_after_handoff`](crate::enable_interrupts_after_handoff)
//! checks again. An owner routes its source with [`enable`] and its token, and
//! silences it with [`disable`].

use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

use esp_hal::{interrupt::Priority, peripherals::Interrupt, system::Cpu};
use oer_interrupt_table::{Binding, Entry, Matrix, MatrixError, Table};

/// The ESP32-S31 interrupt matrix, as esp-hal drives it.
pub struct EspHalMatrix;

impl Matrix for EspHalMatrix {
    type Source = Interrupt;
    type Level = Priority;
    type Core = Cpu;

    fn current_core(&self) -> Cpu {
        Cpu::current()
    }

    fn route(&mut self, source: Interrupt, level: Priority) {
        esp_hal::interrupt::enable(source, level);
    }

    fn silence(&mut self, core: Cpu, source: Interrupt) {
        esp_hal::interrupt::disable(core, source);
    }

    fn routed(&self, core: Cpu, source: Interrupt) -> Option<Priority> {
        esp_hal::interrupt::mapped_to(core, source).map(|line| line.priority())
    }

    fn slot(&self, source: Interrupt) -> usize {
        esp_hal::interrupt::bound_handler(source).map_or(0, |handler| handler.address())
    }
}

/// An ESP32-S31 table error.
pub type Error = MatrixError<EspHalMatrix>;

/// The image's table, which [`adopt`] stores once: its first entry and length.
static TABLE: AtomicPtr<Binding<Interrupt, Priority, Cpu>> = AtomicPtr::new(core::ptr::null_mut());
static TABLE_LENGTH: AtomicUsize = AtomicUsize::new(0);

/// Keep the image's `INTERRUPT_TABLE` for both harts.
pub(crate) fn adopt(table: &'static Table<EspHalMatrix>) {
    TABLE_LENGTH.store(table.len(), Ordering::Relaxed);
    TABLE.store(table.as_ptr().cast_mut(), Ordering::Release);
}

/// The image's interrupt table; empty before
/// [`adopt_psram`](crate::adopt_psram).
pub fn table() -> &'static Table<EspHalMatrix> {
    let first = TABLE.load(Ordering::Acquire);
    if first.is_null() {
        return &[];
    }
    // SAFETY: `adopt` stored the start of a `&'static` slice and, before it,
    // that slice's length; nothing writes either again.
    unsafe { core::slice::from_raw_parts(first, TABLE_LENGTH.load(Ordering::Relaxed)) }
}

/// Route the token's source to its table level; on its table core only.
///
/// # Errors
///
/// [`oer_interrupt_table::Error::WrongCore`] on another core.
pub fn enable<E>(token: &E) -> Result<(), Error>
where
    E: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
{
    oer_interrupt_table::enable(&mut EspHalMatrix, table(), token)
}

/// Silence the token's source on its table core.
pub fn disable<E>(token: &E)
where
    E: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
{
    oer_interrupt_table::disable(&mut EspHalMatrix, token);
}

/// Silence the current hart's sources and check every vector slot.
pub(crate) fn install_current_hart() {
    if let Err(error) = oer_interrupt_table::install(&mut EspHalMatrix, table()) {
        panic!("the interrupt matrix disagrees with the image's table: {error:?}");
    }
}

/// Check the current hart's sources and every vector slot against the table.
pub(crate) fn verify_current_hart() {
    if let Err(error) = oer_interrupt_table::verify(&EspHalMatrix, table()) {
        panic!("the interrupt matrix disagrees with the image's table: {error:?}");
    }
}

/// Declare the image's interrupt table, once per image, and hand its
/// `INTERRUPT_TABLE` to [`adopt_psram`](crate::adopt_psram):
///
/// ```ignore
/// oer_esp32s31_platform_runtime::interrupt_table! {
///     /// The Embassy time driver's alarm.
///     timer: TimerToken = TG0_T0_LEVEL => crate::time::on_alarm, Priority1, ProCpu;
/// }
/// ```
///
/// Each entry names the token field, the token type, the `Interrupt` source,
/// the handler (`fn()`), the `Priority` and the `Cpu`. The macro defines the
/// source's handler symbol in SRAM, which the source's vector slot holds from
/// the link on, the token type, and `Interrupts::take` that hands out each
/// token once. The image needs `esp-hal` as a dependency.
#[macro_export]
macro_rules! interrupt_table {
    ($($entries:tt)*) => {
        $crate::__interrupt_table::interrupt_table! {
            source: esp_hal::peripherals::Interrupt = esp_hal::peripherals::Interrupt,
            level: esp_hal::interrupt::Priority = esp_hal::interrupt::Priority,
            core: esp_hal::system::Cpu = esp_hal::system::Cpu,
            handler_attributes: [#[unsafe(link_section = ".rwtext.open_radio_irq")]];
            $($entries)*
        }
    };
}
