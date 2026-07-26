//! One-attempt atomic claims for strict producer paths.
//!
//! LLVM currently lowers both strong and weak Rust compare/exchange operations
//! on RV32 to an `lr.w`/`sc.w` retry loop. That is correct for the language
//! operation but violates the strict runtime's no-wait contract. These helpers
//! issue exactly one reservation/store-conditional pair. A failed `sc.w` is
//! reported as contention; the caller retains ownership and may arrange an
//! asynchronous retry.

use core::sync::atomic::AtomicUsize;

#[cfg(target_arch = "riscv32")]
use core::arch::asm;
#[cfg(not(target_arch = "riscv32"))]
use core::sync::atomic::Ordering;

#[cfg(target_arch = "riscv32")]
#[inline(always)]
unsafe fn compare_exchange_once(
    atomic: &AtomicUsize,
    current: usize,
    new: usize,
    acquire: bool,
) -> Result<usize, usize> {
    let observed: usize;
    let status: usize;

    if acquire {
        unsafe {
            asm!(
                "lr.w.aq {observed}, 0({address})",
                "bne {observed}, {current}, 2f",
                "sc.w {status}, {new}, 0({address})",
                "j 3f",
                "2:",
                "li {status}, 1",
                "3:",
                address = in(reg) atomic.as_ptr(),
                current = in(reg) current,
                new = in(reg) new,
                observed = out(reg) observed,
                status = out(reg) status,
                options(nostack),
            );
        }
    } else {
        unsafe {
            asm!(
                "lr.w {observed}, 0({address})",
                "bne {observed}, {current}, 2f",
                "sc.w {status}, {new}, 0({address})",
                "j 3f",
                "2:",
                "li {status}, 1",
                "3:",
                address = in(reg) atomic.as_ptr(),
                current = in(reg) current,
                new = in(reg) new,
                observed = out(reg) observed,
                status = out(reg) status,
                options(nostack),
            );
        }
    }

    if observed == current && status == 0 {
        Ok(observed)
    } else {
        Err(observed)
    }
}

/// Compare and exchange with relaxed ordering and exactly one RV32 `sc.w`.
#[inline(always)]
pub(crate) fn compare_exchange_once_relaxed(
    atomic: &AtomicUsize,
    current: usize,
    new: usize,
) -> Result<usize, usize> {
    #[cfg(target_arch = "riscv32")]
    {
        // SAFETY: `AtomicUsize` provides a word-aligned atomic address. The
        // inline sequence reads and conditionally writes only that word.
        unsafe { compare_exchange_once(atomic, current, new, false) }
    }

    #[cfg(not(target_arch = "riscv32"))]
    {
        atomic.compare_exchange_weak(current, new, Ordering::Relaxed, Ordering::Relaxed)
    }
}

/// Compare and exchange with acquire success and exactly one RV32 `sc.w`.
#[inline(always)]
pub(crate) fn compare_exchange_once_acquire(
    atomic: &AtomicUsize,
    current: usize,
    new: usize,
) -> Result<usize, usize> {
    #[cfg(target_arch = "riscv32")]
    {
        // SAFETY: `AtomicUsize` provides a word-aligned atomic address. The
        // acquire reservation and conditional store touch only that word.
        unsafe { compare_exchange_once(atomic, current, new, true) }
    }

    #[cfg(not(target_arch = "riscv32"))]
    {
        atomic.compare_exchange_weak(current, new, Ordering::Acquire, Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relaxed_claim_changes_only_the_expected_word() {
        let value = AtomicUsize::new(7);
        assert_eq!(compare_exchange_once_relaxed(&value, 7, 9), Ok(7));
        assert_eq!(value.load(Ordering::Relaxed), 9);
        assert_eq!(compare_exchange_once_relaxed(&value, 7, 11), Err(9));
        assert_eq!(value.load(Ordering::Relaxed), 9);
    }

    #[test]
    fn acquire_claim_reports_mismatch_without_mutation() {
        let value = AtomicUsize::new(3);
        assert_eq!(compare_exchange_once_acquire(&value, 4, 5), Err(3));
        assert_eq!(value.load(Ordering::Relaxed), 3);
    }
}
