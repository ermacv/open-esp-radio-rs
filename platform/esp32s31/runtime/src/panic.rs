//! The image's one panic entry.
//!
//! The handler records the panic in `.rtc_fast.persistent`, RTC fast memory no
//! reset entry initializes, without formatting its message: the location's file
//! bytes (the last [`FILE_BYTES`]), line and column, the hart, whether it ran on
//! the interrupt stack and, there, the interrupted PC. Every step is a bounded
//! copy, so the path stays a short leaf in every context's stack bound.
//!
//! A product image then resets the chip; the next boot takes the record with
//! [`take_previous`] and reports it in thread context. A diagnostic image
//! (feature `panic-diagnostics`) instead calls
//! `oer_platform_panic_diagnostics`, which it defines, to report and halt.
use core::cell::UnsafeCell;

/// Bytes of the location's file path kept: its end, which names the file.
pub const FILE_BYTES: usize = 48;
const MAGIC: u32 = 0x5045_524f; // "OREP"

/// One panic, as the next boot reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanicRecord {
    pub hart: u8,
    /// The panic ran on the hart's interrupt stack.
    pub in_interrupt: bool,
    /// `mepc` when [`Self::in_interrupt`]: the interrupted instruction.
    pub interrupted_pc: Option<u32>,
    pub line: u32,
    pub column: u32,
    file: [u8; FILE_BYTES],
    file_length: u8,
}

impl PanicRecord {
    /// The end of the location's file path, as recorded.
    pub fn file(&self) -> &str {
        let bytes = &self.file[..usize::from(self.file_length)];
        // The kept suffix may start inside a UTF-8 sequence; skip to the
        // first boundary.
        let start = (0..bytes.len())
            .find(|&i| core::str::from_utf8(&bytes[i..]).is_ok())
            .unwrap_or(bytes.len());
        core::str::from_utf8(&bytes[start..]).unwrap_or("")
    }
}

impl core::fmt::Display for PanicRecord {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{}:{}:{} on hart {}",
            self.file(),
            self.line,
            self.column,
            self.hart
        )?;
        match self.interrupted_pc {
            Some(pc) => write!(f, " in an interrupt of {pc:#010x}"),
            None => Ok(()),
        }
    }
}

/// The retained words: a magic, the record and a checksum over both.
#[repr(C)]
struct Slot {
    magic: u32,
    hart: u32,
    in_interrupt: u32,
    interrupted_pc: u32,
    line: u32,
    column: u32,
    file_length: u32,
    file: [u8; FILE_BYTES],
    checksum: u32,
}

impl Slot {
    const EMPTY: Self = Self {
        magic: 0,
        hart: 0,
        in_interrupt: 0,
        interrupted_pc: 0,
        line: 0,
        column: 0,
        file_length: 0,
        file: [0; FILE_BYTES],
        checksum: 0,
    };

    fn sum(&self) -> u32 {
        let mut sum = 0x811c_9dc5_u32;
        let mut mix = |word: u32| sum = (sum ^ word).wrapping_mul(0x0100_0193);
        for word in [
            self.magic,
            self.hart,
            self.in_interrupt,
            self.interrupted_pc,
            self.line,
            self.column,
            self.file_length,
        ] {
            mix(word);
        }
        for byte in self.file {
            mix(u32::from(byte));
        }
        sum
    }
}

struct Retained(UnsafeCell<Slot>);

// SAFETY: the panic handler writes the slot on the panicking hart while the
// image stops; `take_previous` runs once at boot, before any panic can.
unsafe impl Sync for Retained {}

// Every bit pattern is a `Slot`; the NOLOAD section never loads the initializer.
#[unsafe(link_section = ".rtc_fast.persistent")]
static SLOT: Retained = Retained(UnsafeCell::new(Slot::EMPTY));

/// Take the record the previous boot's panic left, and clear it. Call once,
/// early at boot and before anything can panic; `None` when the previous
/// boot did not panic or the chip lost power.
pub fn take_previous() -> Option<PanicRecord> {
    // SAFETY: called once at boot, before any panic can write the slot.
    let slot = unsafe { &mut *SLOT.0.get() };
    let valid = slot.magic == MAGIC
        && slot.checksum == slot.sum()
        && slot.file_length as usize <= FILE_BYTES;
    let record = valid.then(|| PanicRecord {
        hart: slot.hart as u8,
        in_interrupt: slot.in_interrupt != 0,
        interrupted_pc: (slot.in_interrupt != 0).then_some(slot.interrupted_pc),
        line: slot.line,
        column: slot.column,
        file: slot.file,
        file_length: slot.file_length as u8,
    });
    slot.magic = 0;
    record
}

/// Record `info` in the retained slot without formatting.
fn record(info: &core::panic::PanicInfo<'_>) {
    let hart: usize;
    let sp: usize;
    let mepc: usize;
    // SAFETY: CSR and register reads only.
    unsafe {
        core::arch::asm!(
            "csrr {hart}, mhartid",
            "mv {sp}, sp",
            "csrr {mepc}, mepc",
            hart = out(reg) hart,
            sp = out(reg) sp,
            mepc = out(reg) mepc,
            options(nomem, nostack),
        )
    };
    let in_interrupt = crate::stacks::on_interrupt_stack(hart, sp);
    // SAFETY: the other hart may hold any lock while this one panics, so the
    // slot is written without one; the image stops after this write.
    let slot = unsafe { &mut *SLOT.0.get() };
    let (file, line, column) = info
        .location()
        .map_or(("", 0, 0), |l| (l.file(), l.line(), l.column()));
    let bytes = file.as_bytes();
    let kept = &bytes[bytes.len().saturating_sub(FILE_BYTES)..];
    slot.file = [0; FILE_BYTES];
    slot.file[..kept.len()].copy_from_slice(kept);
    slot.file_length = kept.len() as u32;
    slot.hart = hart as u32;
    slot.in_interrupt = u32::from(in_interrupt);
    slot.interrupted_pc = if in_interrupt { mepc as u32 } else { 0 };
    slot.line = line;
    slot.column = column;
    slot.magic = MAGIC;
    slot.checksum = slot.sum();
}

#[cfg(feature = "panic-diagnostics")]
unsafe extern "Rust" {
    /// The diagnostic image's report after the record is written.
    fn oer_platform_panic_diagnostics(info: &core::panic::PanicInfo<'_>) -> !;
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    record(info);
    #[cfg(feature = "panic-diagnostics")]
    // SAFETY: the diagnostic image defines this function with the declared
    // signature; the link fails without it.
    unsafe {
        oer_platform_panic_diagnostics(info)
    }
    #[cfg(not(feature = "panic-diagnostics"))]
    esp_hal::system::software_reset()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record_with(file: &[u8]) -> PanicRecord {
        let mut stored = [0; FILE_BYTES];
        stored[..file.len()].copy_from_slice(file);
        PanicRecord {
            hart: 0,
            in_interrupt: false,
            interrupted_pc: None,
            line: 1,
            column: 1,
            file: stored,
            file_length: file.len() as u8,
        }
    }

    #[test]
    fn a_kept_suffix_starting_inside_a_character_skips_to_its_end() {
        assert_eq!(record_with(b"src/lib.rs").file(), "src/lib.rs");
        // "é" is 0xc3 0xa9; a suffix may start at its second byte.
        assert_eq!(record_with(b"\xa9/lib.rs").file(), "/lib.rs");
    }

    #[test]
    fn the_checksum_covers_every_field() {
        let mut slot = Slot::EMPTY;
        slot.magic = MAGIC;
        let empty = slot.sum();
        slot.file[FILE_BYTES - 1] = 1;
        assert_ne!(slot.sum(), empty);
        slot.file[FILE_BYTES - 1] = 0;
        slot.line = 7;
        assert_ne!(slot.sum(), empty);
    }
}
