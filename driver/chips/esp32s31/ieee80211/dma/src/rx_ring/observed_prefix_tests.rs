use super::{RxObservedMask, observed_prefix_len, ring_distance, wrap_add};

#[test]
fn non_power_of_two_ring_arithmetic_wraps_without_remainder() {
    assert_eq!(wrap_add::<96>(0, 0), 0);
    assert_eq!(wrap_add::<96>(63, 32), 95);
    assert_eq!(wrap_add::<96>(95, 1), 0);
    assert_eq!(wrap_add::<96>(80, 96), 80);
    assert_eq!(ring_distance::<96>(80, 80), 0);
    assert_eq!(ring_distance::<96>(80, 95), 15);
    assert_eq!(ring_distance::<96>(80, 12), 28);
}

#[test]
fn counts_linear_wrapped_full_and_holed_prefixes() {
    let mask = |word| RxObservedMask {
        words: [word, 0, 0],
    };
    assert_eq!(observed_prefix_len::<8>(mask(0b0000_0111), 0), 3);
    assert_eq!(observed_prefix_len::<8>(mask(0b1100_0001), 6), 3);
    assert_eq!(observed_prefix_len::<8>(mask(0b1100_0101), 6), 3);
    assert_eq!(observed_prefix_len::<8>(mask(0b1010_0001), 6), 0);
    assert_eq!(observed_prefix_len::<8>(mask(u32::MAX), 5), 8);
    assert_eq!(
        observed_prefix_len::<64>(
            RxObservedMask {
                words: [u32::MAX, u32::MAX, 0],
            },
            63,
        ),
        64
    );
    assert_eq!(
        observed_prefix_len::<96>(
            RxObservedMask {
                words: [u32::MAX; 3],
            },
            80,
        ),
        96
    );
}
