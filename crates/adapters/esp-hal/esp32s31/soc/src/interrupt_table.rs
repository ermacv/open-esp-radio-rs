//! The image's interrupt table (`oer-interrupt-table`) on the ESP32-S31
//! matrix.
//!
//! Every image declares its peripheral interrupt sources once with the
//! platform runtime's `interrupt_table!` and hands the table to the runtime,
//! which [`adopt`]s it, silences each source of a hart's entries when it
//! installs that hart's interrupt stack ([`install_current_hart`]) and checks
//! again before it enables interrupts ([`verify_current_hart`]). An owner routes
//! its source with [`enable`] and its token, and silences it with [`disable`].

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

// The stack analysis reads the table from the image by these offsets: a
// change of esp-hal's or the PAC's representation of a field must fail here.
const _: () = {
    use core::mem::{align_of, offset_of, size_of};
    type Entry = Binding<Interrupt, Priority, Cpu>;
    assert!(size_of::<Interrupt>() == 2 && size_of::<Priority>() == 1);
    assert!(size_of::<Cpu>() == 4);
    assert!(offset_of!(Entry, source) == 0);
    assert!(offset_of!(Entry, level) == 2);
    assert!(offset_of!(Entry, core) == 4);
    assert!(offset_of!(Entry, handler) == 8);
    assert!(size_of::<Entry>() == 12 && align_of::<Entry>() == 4);
    assert!(Cpu::ProCpu as u32 == 0 && Cpu::AppCpu as u32 == 1);
    assert!(Priority::Priority1 as u8 == 1);
};

/// An ESP32-S31 table error.
pub type Error = MatrixError<EspHalMatrix>;

/// The image's table, which [`adopt`] stores once: its first entry and length.
static TABLE: AtomicPtr<Binding<Interrupt, Priority, Cpu>> = AtomicPtr::new(core::ptr::null_mut());
static TABLE_LENGTH: AtomicUsize = AtomicUsize::new(0);

/// Keep the image's `INTERRUPT_TABLE` for both harts; the platform runtime
/// calls this once, before either hart installs its interrupt stack.
pub fn adopt(table: &'static Table<EspHalMatrix>) {
    TABLE_LENGTH.store(table.len(), Ordering::Relaxed);
    TABLE.store(table.as_ptr().cast_mut(), Ordering::Release);
}

/// The image's interrupt table; empty before [`adopt`].
pub fn table() -> &'static Table<EspHalMatrix> {
    let first = TABLE.load(Ordering::Acquire);
    if first.is_null() {
        return &[];
    }
    // SAFETY: `adopt` stored the start of a `&'static` slice and, before it,
    // that slice's length; nothing writes either again.
    #[allow(unsafe_code, reason = "the stored table is a `&'static` slice")]
    unsafe {
        core::slice::from_raw_parts(first, TABLE_LENGTH.load(Ordering::Relaxed))
    }
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
///
/// # Panics
///
/// When the matrix disagrees with the table.
pub fn install_current_hart() {
    if let Err(error) = oer_interrupt_table::install(&mut EspHalMatrix, table()) {
        panic!("the interrupt matrix disagrees with the image's table: {error:?}");
    }
}

/// Check the current hart's sources and every vector slot against the table.
///
/// # Panics
///
/// When the matrix disagrees with the table.
pub fn verify_current_hart() {
    if let Err(error) = oer_interrupt_table::verify(&EspHalMatrix, table()) {
        panic!("the interrupt matrix disagrees with the image's table: {error:?}");
    }
}
