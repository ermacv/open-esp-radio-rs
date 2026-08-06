//! Deterministic byte-stream evidence shared by host and target HIL peers.

/// Return the byte expected at one absolute stream offset.
///
/// The full-period affine map catches dropped, duplicated and shifted bytes
/// without storing a second reference stream. TCP still supplies ordering and
/// retransmission; this pattern proves that both applications observed the
/// same byte positions rather than merely the same total length.
pub const fn stream_pattern_byte(offset: u64) -> u8 {
    (offset as u8).wrapping_mul(31).wrapping_add(0x5a)
}

pub fn fill_stream_pattern(buffer: &mut [u8], offset: u64) {
    let mut expected = stream_pattern_byte(offset);
    for byte in buffer {
        *byte = expected;
        expected = expected.wrapping_add(31);
    }
}

pub fn stream_pattern_matches(buffer: &[u8], offset: u64) -> bool {
    let mut expected = stream_pattern_byte(offset);
    for byte in buffer {
        if *byte != expected {
            return false;
        }
        expected = expected.wrapping_add(31);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_are_independent_of_read_boundaries() {
        let mut whole = [0; 257];
        fill_stream_pattern(&mut whole, 0);
        assert!(stream_pattern_matches(&whole[..113], 0));
        assert!(stream_pattern_matches(&whole[113..], 113));
        whole[200] ^= 1;
        assert!(!stream_pattern_matches(&whole[113..], 113));
    }
}
