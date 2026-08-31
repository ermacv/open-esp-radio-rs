//! In-place initialization for large statically owned arrays.

#![deny(unsafe_op_in_unsafe_fn)]

use core::mem::MaybeUninit;

/// Fill an uninitialized static array without constructing an array-sized
/// temporary on the caller's stack.
pub(crate) fn fill<T: Copy, const N: usize>(
    storage: &mut [MaybeUninit<T>; N],
    value: T,
) -> &mut [T; N] {
    for slot in storage.iter_mut() {
        slot.write(value);
    }

    // SAFETY: every one of the N elements was initialized above, and
    // `MaybeUninit<T>` has exactly the same layout as `T`. The returned borrow
    // is tied to the unique input borrow, so no alias to uninitialized storage
    // survives this conversion.
    unsafe { &mut *storage.as_mut_ptr().cast::<[T; N]>() }
}

#[cfg(test)]
mod tests {
    use super::fill;
    use core::mem::MaybeUninit;

    #[test]
    fn initializes_every_element_without_a_source_array() {
        let mut storage = [const { MaybeUninit::uninit() }; 8];
        assert_eq!(fill(&mut storage, 7_u32), &mut [7; 8]);
    }
}
