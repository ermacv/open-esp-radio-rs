use super::*;

#[test]
fn checksum_accepts_odd_boundaries_across_parts() {
    let contiguous = internet_checksum(&[&[1, 2, 3, 4, 5]]);
    assert_eq!(internet_checksum(&[&[1], &[2, 3], &[4, 5]]), contiguous);
}
