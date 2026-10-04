//! The Espressif station's TX Block Ack originator policy: the TIDs it
//! negotiates once connected and its Dialog Token sequence.

/// TX Block Ack TIDs the vendor station negotiates when its connection
/// completes, in order.
///
/// SOURCE(esp32s31): complete `libnet80211.a[wl_cnx.o]::cnx_auth_done`
/// invokes `ieee80211_ampdu_request` for TIDs 0, 7 and 5 in this order.
pub const STA_TX_BLOCK_ACK_TIDS: [u8; 3] = [0, 7, 5];

/// The first Dialog Token of the vendor's sequence.
pub const FIRST_DIALOG_TOKEN: u8 = 1;

/// The Dialog Token that follows `current` in the vendor's sequence: one
/// archive-static token shared by every agreement, modulo 63.
///
/// SOURCE(esp32s31): complete `libnet80211.a[ieee80211_ht.o]::ieee80211_ampdu_request`
/// increments one archive-static token modulo 63.
pub const fn next_dialog_token(current: u8) -> u8 {
    if current >= 62 { 0 } else { current + 1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dialog_token_wraps_modulo_sixty_three() {
        assert_eq!(next_dialog_token(FIRST_DIALOG_TOKEN), 2);
        assert_eq!(next_dialog_token(61), 62);
        assert_eq!(next_dialog_token(62), 0);
        assert_eq!(next_dialog_token(0), 1);
    }
}
