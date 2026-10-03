//! Statics in regions the boot zeroes.
//!
//! A board's linker script places some statics, by section name, in regions
//! the boot clears instead of loading (`NOLOAD`): their initializer never
//! reaches the device. Such a static must therefore start as all zero bytes,
//! and those bytes must be a valid value that means what the constructor
//! means. [`Zeroable`] (bytemuck's marker, the one esp-hal's
//! `#[ram(zeroed)]` requires) marks the types for which that holds;
//! `#[derive(bytemuck::Zeroable)]` accepts a struct whose fields all are, or
//! an enum with an integer `repr` and a zero discriminant. [`zeroed`] is the
//! one initializer [`zeroed_static!`](crate::zeroed_static) gives such a
//! static. The image linker checks the result: a zeroed input section that
//! carries an initializer fails the link.

use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, Ordering},
};

pub use bytemuck::Zeroable;

/// The all-zero value of `T`, its initial state; usable in a `static`
/// initializer, where `Zeroable::zeroed` is not.
#[allow(
    unsafe_code,
    reason = "Zeroable guarantees the zero bit pattern is valid"
)]
pub const fn zeroed<T: Zeroable>() -> T {
    // SAFETY: `T: Zeroable` guarantees that all-zero bytes are a valid `T`.
    unsafe { core::mem::zeroed() }
}

/// A static, in a region the boot zeroes, that one owner claims once.
///
/// It starts as [`zeroed`]; [`Self::take`] hands out the value the first
/// time. A value set at runtime is a `ZeroedStatic<MaybeUninit<T>>` that the
/// claimant writes.
pub struct ZeroedStatic<T> {
    taken: AtomicBool,
    value: UnsafeCell<T>,
}

#[allow(unsafe_code, reason = "the cell hands its value to one claimant")]
// SAFETY: the cell offers no access but `take`, which gives the value to one
// caller only (`taken` swaps once). Only that caller's thread ever touches
// it: the zero value was made by no thread and a runtime value is written
// by the claimant; handing the `&'static mut T` on needs `T: Send` by the
// ordinary rules. `static_cell::StaticCell` rests on the same argument.
unsafe impl<T> Sync for ZeroedStatic<T> {}

#[allow(unsafe_code, reason = "a flag and a zero-valid value")]
// SAFETY: `taken` is false at zero and `value` is zero-valid.
unsafe impl<T: Zeroable> Zeroable for ZeroedStatic<T> {}

impl<T: Zeroable> ZeroedStatic<T> {
    /// The unclaimed cell with the zero value.
    pub const fn new() -> Self {
        zeroed()
    }
}

impl<T: Zeroable> Default for ZeroedStatic<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> ZeroedStatic<T> {
    /// The value, the first time; panics on a second claim.
    pub fn take(&'static self) -> &'static mut T {
        self.try_take().expect("a zeroed static is claimed once")
    }

    /// The value, the first time; `None` once claimed.
    #[allow(unsafe_code, reason = "the first claimant receives the only reference")]
    #[allow(
        clippy::mut_from_ref,
        reason = "a take-once cell: the swap of `taken` makes this the only reference"
    )]
    pub fn try_take(&'static self) -> Option<&'static mut T> {
        if self.taken.swap(true, Ordering::AcqRel) {
            return None;
        }
        // SAFETY: the swap above returned false exactly once, so no other
        // reference to `value` exists or will be created.
        Some(unsafe { &mut *self.value.get() })
    }
}

impl<T> ZeroedStatic<MaybeUninit<T>> {
    /// Claim the storage once and move `value` into it; panics on a second
    /// claim.
    pub fn init(&'static self, value: T) -> &'static mut T {
        self.take().write(value)
    }
}

/// Declare statics in a region the boot zeroes: each starts as
/// [`zeroed`](crate::zeroed::zeroed), so its type must be
/// [`Zeroable`](crate::zeroed::Zeroable) and no non-zero initializer can be
/// written. The macro names the section, so the caller writes no `unsafe`
/// attribute; `static mut` is accepted for storage handed to hardware or
/// assembly by address.
///
/// ```ignore
/// oer_memory::zeroed_static! {
///     /// Placement: packet payloads live in the PSRAM tier.
///     static POOL: ZeroedStatic<Storage> = zeroed in ".psram.bss.pool";
/// }
/// ```
#[macro_export]
macro_rules! zeroed_static {
    () => {};
    (
        $(#[$meta:meta])*
        $vis:vis static mut $name:ident: $ty:ty = zeroed in $section:literal;
        $($rest:tt)*
    ) => {
        $(#[$meta])*
        #[unsafe(link_section = $section)]
        $vis static mut $name: $ty = $crate::zeroed::zeroed();
        $crate::zeroed_static! { $($rest)* }
    };
    (
        $(#[$meta:meta])*
        $vis:vis static $name:ident: $ty:ty = zeroed in $section:literal;
        $($rest:tt)*
    ) => {
        $(#[$meta])*
        #[unsafe(link_section = $section)]
        $vis static $name: $ty = $crate::zeroed::zeroed();
        $crate::zeroed_static! { $($rest)* }
    };
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::AtomicU32;

    use super::*;

    #[test]
    fn a_zeroed_static_starts_unclaimed_and_hands_its_zero_value_once() {
        static CELL: ZeroedStatic<[AtomicU32; 4]> = ZeroedStatic::new();
        let value = CELL.take();
        assert!(value.iter().all(|word| word.load(Ordering::Relaxed) == 0));
        assert!(CELL.try_take().is_none());
    }

    #[test]
    fn a_runtime_value_is_written_into_uninitialized_zeroed_storage() {
        static CELL: ZeroedStatic<MaybeUninit<[u8; 3]>> = ZeroedStatic::new();
        let value = CELL.take().write([1, 2, 3]);
        assert_eq!(*value, [1, 2, 3]);
    }

    #[test]
    fn the_zero_initializer_is_all_zero_bytes() {
        let cell: ZeroedStatic<[u8; 16]> = ZeroedStatic::new();
        assert!(!cell.taken.load(Ordering::Relaxed));
        assert_eq!(cell.value.into_inner(), [0; 16]);
    }
}
