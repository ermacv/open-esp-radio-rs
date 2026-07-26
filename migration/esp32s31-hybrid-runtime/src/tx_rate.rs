#[cfg(target_arch = "riscv32")]
use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicU32, Ordering},
};

#[cfg(target_arch = "riscv32")]
const RATE_CONTEXT_PRIMARY_RATE_OFFSET: usize = 0x08;
#[cfg(target_arch = "riscv32")]
const RATE_CONTEXT_SECONDARY_RATE_OFFSET: usize = 0x09;
#[cfg(target_arch = "riscv32")]
const RATE_CONTEXT_MODE_OFFSET: usize = 0x0c;
#[cfg(target_arch = "riscv32")]
const RATE_CONTEXT_PRIMARY_SCHEDULE_OFFSET: usize = 0x64;
#[cfg(target_arch = "riscv32")]
const RATE_CONTEXT_SECONDARY_SCHEDULE_OFFSET: usize = 0x68;
#[cfg(target_arch = "riscv32")]
const DESCRIPTOR_SELECTED_RATE_OFFSET: usize = 0x0c;
#[cfg(target_arch = "riscv32")]
const DESCRIPTOR_SCHEDULE_OFFSET: usize = 0x1c;
#[cfg(target_arch = "riscv32")]
const DESCRIPTOR_RATE_CLASS_OFFSET: usize = 0x2f;

#[cfg(target_arch = "riscv32")]
struct ScheduleCell(UnsafeCell<[u8; 12]>);

#[cfg(target_arch = "riscv32")]
unsafe impl Sync for ScheduleCell {}

#[cfg(target_arch = "riscv32")]
#[used]
#[link_section = ".critical.data.wifi_strict.basic_secondary_schedule"]
static BASIC_SECONDARY_SCHEDULE: ScheduleCell = ScheduleCell(UnsafeCell::new([
    0x00, 0x02, 0x00, 0x02, 0x00, 0x03, 0x00, 0x19, 0x20, 0x1e, 0x00, 0x00,
]));

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FixedRateScheduleSnapshot {
    pub primary_fixed: u32,
    pub secondary_stateless: u32,
    pub vendor_fallbacks: u32,
}

#[cfg(target_arch = "riscv32")]
static FIXED_RATE_PRIMARY: AtomicU32 = AtomicU32::new(0);
#[cfg(target_arch = "riscv32")]
static FIXED_RATE_SECONDARY: AtomicU32 = AtomicU32::new(0);
#[cfg(target_arch = "riscv32")]
static DYNAMIC_RATE_FALLBACKS: AtomicU32 = AtomicU32::new(0);

#[cfg(target_arch = "riscv32")]
pub fn fixed_rate_schedule_snapshot() -> FixedRateScheduleSnapshot {
    FixedRateScheduleSnapshot {
        primary_fixed: FIXED_RATE_PRIMARY.load(Ordering::Acquire),
        secondary_stateless: FIXED_RATE_SECONDARY.load(Ordering::Acquire),
        vendor_fallbacks: DYNAMIC_RATE_FALLBACKS.load(Ordering::Acquire),
    }
}

/// Record use of the bounded snapshot of an adaptive-rate schedule.
///
/// The schedule itself remains vendor-initialized fixed SRAM. Strict runtime
/// deliberately omits the stateful `rcUpdateRate` mutation and consumes the
/// current schedule head in one finite action.
#[cfg(target_arch = "riscv32")]
fn record_dynamic_rate_schedule_fallback() {
    DYNAMIC_RATE_FALLBACKS.fetch_add(1, Ordering::Relaxed);
}

const fn rate_class_bits(previous: u8, rate: u8) -> u8 {
    if rate <= 7 {
        previous & !0x78
    } else if rate <= 15 {
        (previous & !0x78) | 0x08
    } else if rate <= 40 {
        previous
    } else {
        (previous & !0x78) | 0x50
    }
}

const fn stateless_schedule_source(
    primary: bool,
    mode: u16,
    descriptor_flags: u32,
    descriptor_control: u32,
) -> u8 {
    if primary {
        return if mode & 0x01 != 0 { 1 } else { 0 };
    }
    if mode & 0x02 != 0 {
        return 1;
    }
    if mode & 0x80 != 0 {
        // `.LANCHOR23 + 0x24` is the immutable 12-byte basic-rate schedule
        // selected by the pinned bit-7 branch when none of its PHY/NVS
        // overrides apply.
        return if descriptor_control & 0x00c3_0000 == 0 {
            3
        } else {
            0
        };
    }
    if descriptor_flags & 0x0020_0800 == 0 {
        2
    } else {
        0
    }
}

/// Recovered bounded branches of the pinned `rcGetSched` implementation.
///
/// Fixed-rate branches use their configured rate. An adaptive branch uses the
/// already initialized SRAM schedule without calling stateful `rcUpdateRate`;
/// ACK/CTS retry accounting remains owned by the Rust LMAC state machine.
///
/// # Safety
///
/// `rate_context` and `descriptor` must point to live pinned vendor objects
/// exclusively owned by the current run-to-completion TX submission.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.tx_rate_schedule"]
pub unsafe fn try_fixed_rate_schedule(rate_context: *mut u8, descriptor: *mut u8) -> bool {
    if rate_context.is_null() || descriptor.is_null() {
        return false;
    }

    let descriptor_flags = descriptor.cast::<u32>().read_unaligned();
    let primary = descriptor_flags & 0x0200_0008 == 0x0000_0008;
    let mode = rate_context
        .add(RATE_CONTEXT_MODE_OFFSET)
        .cast::<u16>()
        .read_unaligned();
    let descriptor_control = descriptor.add(0x10).cast::<u32>().read_unaligned();
    let source = stateless_schedule_source(primary, mode, descriptor_flags, descriptor_control);
    let (schedule, rate) = match source {
        1 => {
            let (rate_offset, schedule_offset) = if primary {
                (
                    RATE_CONTEXT_PRIMARY_RATE_OFFSET,
                    RATE_CONTEXT_PRIMARY_SCHEDULE_OFFSET,
                )
            } else {
                (
                    RATE_CONTEXT_SECONDARY_RATE_OFFSET,
                    RATE_CONTEXT_SECONDARY_SCHEDULE_OFFSET,
                )
            };
            (
                rate_context
                    .add(schedule_offset)
                    .cast::<*mut u8>()
                    .read_unaligned(),
                rate_context.add(rate_offset).read(),
            )
        }
        2 => {
            let schedule = rate_context
                .add(RATE_CONTEXT_SECONDARY_SCHEDULE_OFFSET)
                .cast::<*mut u8>()
                .read_unaligned();
            (
                schedule,
                if schedule.is_null() {
                    0
                } else {
                    schedule.read()
                },
            )
        }
        3 => {
            let schedule = BASIC_SECONDARY_SCHEDULE.0.get().cast::<u8>();
            (schedule, schedule.read())
        }
        0 => {
            let schedule_offset = if primary {
                RATE_CONTEXT_PRIMARY_SCHEDULE_OFFSET
            } else {
                RATE_CONTEXT_SECONDARY_SCHEDULE_OFFSET
            };
            let schedule = rate_context
                .add(schedule_offset)
                .cast::<*mut u8>()
                .read_unaligned();
            if schedule.is_null() {
                return false;
            }
            record_dynamic_rate_schedule_fallback();
            (schedule, schedule.read())
        }
        _ => return false,
    };
    if schedule.is_null() {
        return false;
    }
    let class = descriptor.add(DESCRIPTOR_RATE_CLASS_OFFSET).read();

    descriptor
        .add(DESCRIPTOR_SCHEDULE_OFFSET)
        .cast::<*mut u8>()
        .write_unaligned(schedule);
    descriptor.add(DESCRIPTOR_SELECTED_RATE_OFFSET).write(rate);
    descriptor
        .add(DESCRIPTOR_RATE_CLASS_OFFSET)
        .write(rate_class_bits(class, rate));

    if primary {
        FIXED_RATE_PRIMARY.fetch_add(1, Ordering::Relaxed);
    } else {
        FIXED_RATE_SECONDARY.fetch_add(1, Ordering::Relaxed);
    }
    true
}

/// Fail-closed final-link replacement for the measured `rcGetSched` domain.
///
/// Every admitted branch is a finite SRAM-resident load/store sequence. An
/// A missing/invalid schedule traps before the descriptor can enter hardware;
/// the leaf never delegates to vendor rate control.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.tx_rate_schedule"]
pub unsafe fn strict_rate_schedule(rate_context: *mut u8, descriptor: *mut u8) {
    if !try_fixed_rate_schedule(rate_context, descriptor) {
        record_dynamic_rate_schedule_fallback();
        core::arch::asm!("ebreak", options(noreturn));
    }
}

/// Recovered finite body of the vendor `mac_tx_get_rts_rate` leaf for every
/// non-HE rate admitted by the strict runtime.
pub(crate) const fn basic_non_he_rts_rate(rate: u8) -> Option<u8> {
    if rate <= 7 {
        return Some(match rate {
            0 | 4 => 0,
            1..=3 => 1,
            _ => 5,
        });
    }
    if rate <= 15 {
        return Some(match rate {
            8 | 9 | 12 | 13 => 9,
            10 | 14 => 10,
            _ => 11,
        });
    }
    if rate > 35 {
        return None;
    }
    let mcs = (rate - 16) % 10;
    Some(if mcs == 0 {
        11
    } else if mcs <= 2 {
        10
    } else {
        9
    })
}

#[cfg(test)]
mod tests {
    use super::{basic_non_he_rts_rate, rate_class_bits, stateless_schedule_source};

    #[test]
    fn admits_only_recovered_stateless_schedule_branches() {
        assert_eq!(stateless_schedule_source(true, 1, 0x2009, 0x304), 1);
        assert_eq!(stateless_schedule_source(true, 0, 0x2009, 0x304), 0);
        assert_eq!(stateless_schedule_source(false, 0, 0, 0), 2);
        assert_eq!(stateless_schedule_source(false, 0x80, 0x0200_200c, 0), 3);
        assert_eq!(stateless_schedule_source(false, 2, 0, 0), 1);
        assert_eq!(stateless_schedule_source(false, 0x80, 0, 0x10000), 0);
        assert_eq!(stateless_schedule_source(false, 0, 0x0000_0800, 0), 0);
    }

    #[test]
    fn reproduces_rc_get_sched_rate_classes() {
        assert_eq!(rate_class_bits(0xff, 7), 0x87);
        assert_eq!(rate_class_bits(0xff, 8), 0x8f);
        assert_eq!(rate_class_bits(0x35, 16), 0x35);
        assert_eq!(rate_class_bits(0x35, 40), 0x35);
        assert_eq!(rate_class_bits(0xff, 41), 0xd7);
    }

    #[test]
    fn reproduces_every_legacy_rate() {
        let expected = [0, 1, 1, 1, 0, 5, 5, 5, 9, 9, 10, 11, 9, 9, 10, 11];
        for (rate, expected_rate) in expected.into_iter().enumerate() {
            assert_eq!(basic_non_he_rts_rate(rate as u8), Some(expected_rate));
        }
    }

    #[test]
    fn reproduces_every_strict_basic_ht_rate() {
        let expected = [
            11, 10, 10, 9, 9, 9, 9, 9, 9, 9, 11, 10, 10, 9, 9, 9, 9, 9, 9, 9,
        ];

        for (offset, expected_rate) in expected.into_iter().enumerate() {
            let rate = 16 + offset as u8;
            assert_eq!(basic_non_he_rts_rate(rate), Some(expected_rate));
        }
    }

    #[test]
    fn rejects_he_and_invalid_rates() {
        for rate in 36..=u8::MAX {
            assert_eq!(basic_non_he_rts_rate(rate), None);
        }
    }
}
