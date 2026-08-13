//! Deterministic byte-stream evidence shared by host and target HIL peers.

const STREAM_PATTERN_PERIOD: usize = 256;

const fn stream_pattern_period() -> [u8; STREAM_PATTERN_PERIOD] {
    let mut pattern = [0; STREAM_PATTERN_PERIOD];
    let mut index = 0;
    while index < pattern.len() {
        pattern[index] = stream_pattern_byte(index as u64);
        index += 1;
    }
    pattern
}

const STREAM_PATTERN: [u8; STREAM_PATTERN_PERIOD] = stream_pattern_period();

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
    let mut written = 0;
    let mut phase = offset as usize % STREAM_PATTERN_PERIOD;
    while written < buffer.len() {
        let length = (STREAM_PATTERN_PERIOD - phase).min(buffer.len() - written);
        buffer[written..written + length].copy_from_slice(&STREAM_PATTERN[phase..phase + length]);
        written += length;
        phase = 0;
    }
}

pub fn stream_pattern_matches(buffer: &[u8], offset: u64) -> bool {
    let mut compared = 0;
    let mut phase = offset as usize % STREAM_PATTERN_PERIOD;
    while compared < buffer.len() {
        let length = (STREAM_PATTERN_PERIOD - phase).min(buffer.len() - compared);
        if buffer[compared..compared + length] != STREAM_PATTERN[phase..phase + length] {
            return false;
        }
        compared += length;
        phase = 0;
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

    #[test]
    fn bulk_pattern_crosses_multiple_periods_at_an_arbitrary_phase() {
        let mut buffer = [0; 1_025];
        fill_stream_pattern(&mut buffer, 173);
        assert!(stream_pattern_matches(&buffer, 173));
        assert_eq!(buffer[0], stream_pattern_byte(173));
        assert_eq!(buffer[1_024], stream_pattern_byte(1_197));
    }
}
