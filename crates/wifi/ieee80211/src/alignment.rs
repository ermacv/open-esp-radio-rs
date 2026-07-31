//! Stateless ESF alignment policy recovered from ESP32-S31 net80211.
//!
//! Raw ESF ownership and memory movement remain in the target adapter. This
//! module contains only the checked pointer/length arithmetic and descriptor
//! word transformation from the pinned `ieee80211_align_eb` leaf.

pub const MAX_ENCODED_MPDU_LEN: usize = 0x3fff;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AlignPlan {
    pub reserved_start: usize,
    pub aligned_start: usize,
    pub move_len: usize,
    pub storage_word: u32,
}

pub const fn plan(
    data_address: usize,
    reserve: usize,
    header_len: u16,
    remaining_len: u16,
    storage_word: u32,
) -> Option<AlignPlan> {
    if reserve != header_len as usize || !matches!(reserve, 24 | 26) {
        return None;
    }
    let move_len = match (header_len as usize).checked_add(remaining_len as usize) {
        Some(length) if length <= MAX_ENCODED_MPDU_LEN => length,
        _ => return None,
    };
    let reserved_start = match data_address.checked_sub(reserve) {
        Some(address) => address,
        None => return None,
    };
    let alignment_delta = reserved_start & 3;
    let aligned_start = match reserved_start.checked_sub(alignment_delta) {
        Some(address) => address,
        None => return None,
    };
    let storage_word = (storage_word & 0x1000_3fff) | 0xc000_0000 | (move_len as u32) << 14;
    Some(AlignPlan {
        reserved_start,
        aligned_start,
        move_len,
        storage_word,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligned_header_reservation_matches_the_pinned_leaf() {
        assert_eq!(
            plan(0x2000, 24, 24, 100, 0x3fff_8123),
            Some(AlignPlan {
                reserved_start: 0x1fe8,
                aligned_start: 0x1fe8,
                move_len: 124,
                storage_word: 0xd01f_0123,
            })
        );
    }

    #[test]
    fn qos_header_moves_at_most_three_alignment_bytes() {
        assert_eq!(
            plan(0x2000, 26, 26, 100, 0x2000_0007),
            Some(AlignPlan {
                reserved_start: 0x1fe6,
                aligned_start: 0x1fe4,
                move_len: 126,
                storage_word: 0xc01f_8007,
            })
        );
    }

    #[test]
    fn only_the_recovered_finite_layout_is_admitted() {
        assert_eq!(plan(0x2000, 25, 25, 100, 0), None);
        assert_eq!(plan(0x2000, 24, 26, 100, 0), None);
        assert_eq!(plan(20, 24, 24, 100, 0), None);
        assert_eq!(plan(0x5000, 24, 24, 0x3fff, 0), None);
    }
}
