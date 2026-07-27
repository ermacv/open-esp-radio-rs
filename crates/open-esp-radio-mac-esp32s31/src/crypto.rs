//! Typed ESP32-S31 hardware-key transactions.
//!
//! This is the allocation-free subset recovered from
//! `hal_crypto_set_key_entry` and `hal_crypto_enable`. It owns only MMIO
//! publication; WPA2 derivation and long-lived secret ownership remain in the
//! protocol crate.

use open_esp_radio_ieee80211::ccmp::{advance_vendor_tx_pn, ccmp_header};
use open_esp_radio_pac_esp32s31::mac;

use crate::registers::Mmio;

const STA_INTERFACE: usize = 0;
const STA_PAIRWISE_HARDWARE_INDEX: u8 = 4;
const STA_GROUP_HARDWARE_INDEX: u8 = 1;
const MAX_WPA2_GTK_ID: u8 = 3;
const PAIRWISE_LOGICAL_KEY_INDEX: u32 = 0;
const CCMP_ALGORITHM: u32 = 3;
const CCMP_KEY_BYTES: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CryptoKeyError {
    InvalidGroupKeyId,
    Occupied,
    HardwareRejected,
}

/// Authority for the one STA pairwise CCMP slot installed by this module.
///
/// The token deliberately contains no key bytes. Consuming it through
/// [`StaPairwiseCcmpSlot::clear`] clears the hardware entry.
#[must_use = "the installed hardware key must remain owned until it is explicitly cleared"]
pub struct StaPairwiseCcmpSlot {
    peer: [u8; 6],
    tx_pn_low: u32,
    tx_pn_high: u32,
}

/// Authority for the one active STA group CCMP slot recovered from the
/// migration backend.
#[must_use = "the installed hardware key must remain owned until it is explicitly cleared"]
pub struct StaGroupCcmpSlot {
    key_id: u8,
}

impl StaGroupCcmpSlot {
    pub const fn hardware_index(&self) -> u8 {
        STA_GROUP_HARDWARE_INDEX
    }

    pub const fn key_id(&self) -> u8 {
        self.key_id
    }

    pub fn clear<M: Mmio>(self, mmio: &mut M) {
        clear_entry(mmio, STA_GROUP_HARDWARE_INDEX);
        mmio.fence();
    }
}

impl StaPairwiseCcmpSlot {
    pub const fn hardware_index(&self) -> u8 {
        STA_PAIRWISE_HARDWARE_INDEX
    }

    pub const fn peer(&self) -> &[u8; 6] {
        &self.peer
    }

    /// Advances the vendor-compatible 48-bit TX packet number and returns the
    /// eight-byte CCMP header for one pairwise MPDU.
    ///
    /// The pinned S31 net80211 implementation advances by three. A newly
    /// installed key therefore emits PN 3 first. Pairwise traffic uses key ID
    /// zero, so only the ExtIV bit is present in byte three.
    pub fn next_tx_ccmp_header(&mut self) -> [u8; 8] {
        (self.tx_pn_low, self.tx_pn_high) = advance_vendor_tx_pn(self.tx_pn_low, self.tx_pn_high);
        ccmp_header(self.tx_pn_low, self.tx_pn_high, 0)
    }

    pub fn clear<M: Mmio>(self, mmio: &mut M) {
        clear_entry(mmio, STA_PAIRWISE_HARDWARE_INDEX);
        mmio.fence();
    }
}

/// Installs the WPA2 temporal key into the recovered STA pairwise slot.
///
/// The slot must be invalid on entry. This fail-closed rule prevents an open
/// driver instance from overwriting a key owned by retained vendor state or a
/// different interface. The key is borrowed only for the bounded MMIO
/// transaction and is never retained by the returned ownership token.
pub fn install_sta_pairwise_ccmp<M: Mmio>(
    mmio: &mut M,
    peer: [u8; 6],
    temporal_key: &[u8; CCMP_KEY_BYTES],
) -> Result<StaPairwiseCcmpSlot, CryptoKeyError> {
    let validity = mmio.read32(mac::CRYPTO_KEY_VALID_BITMAP);
    let valid_bit = 1_u32 << STA_PAIRWISE_HARDWARE_INDEX;
    if validity & valid_bit != 0 {
        return Err(CryptoKeyError::Occupied);
    }

    clear_entry_words(mmio, STA_PAIRWISE_HARDWARE_INDEX);

    let peer_low = u32::from_le_bytes(peer[..4].try_into().expect("fixed peer low word"));
    let peer_high = u16::from_le_bytes(peer[4..].try_into().expect("fixed peer high word"));
    // Recovered `hal_crypto_set_key_entry` encoding:
    // CCMP cipher selector 3, pairwise direction 3, logical key index 0.
    let cipher = CCMP_ALGORITHM << 18;
    let direction = 3_u32;
    let control = (direction << 5)
        | (u32::from(PAIRWISE_LOGICAL_KEY_INDEX != 3) << 11)
        | (PAIRWISE_LOGICAL_KEY_INDEX << 14)
        | ((cipher >> 16) & 0x341f);

    write_entry(mmio, STA_PAIRWISE_HARDWARE_INDEX, 0, peer_low);
    write_entry(
        mmio,
        STA_PAIRWISE_HARDWARE_INDEX,
        1,
        u32::from(peer_high) | (control << 16),
    );
    for (word, bytes) in temporal_key.chunks_exact(4).enumerate() {
        write_entry(
            mmio,
            STA_PAIRWISE_HARDWARE_INDEX,
            word as u8 + 2,
            u32::from_le_bytes(bytes.try_into().expect("four-byte TK word")),
        );
    }

    mmio.write32(mac::CRYPTO_KEY_VALID_BITMAP, validity | valid_bit);
    enable_sta_ccmp(mmio);
    mmio.fence();

    if mmio.read32(mac::CRYPTO_KEY_VALID_BITMAP) & valid_bit == 0 {
        return Err(CryptoKeyError::HardwareRejected);
    }
    Ok(StaPairwiseCcmpSlot {
        peer,
        tx_pn_low: 0,
        tx_pn_high: 0,
    })
}

/// Installs the recovered STA GTK into hardware slot 1.
///
/// ESP32-S31 exposes one active station group slot regardless of the logical
/// RSN key ID. The logical ID is still encoded in the hardware control word,
/// exactly as in `migration::wpa2_s31::install_ccmp`.
pub fn install_sta_group_ccmp<M: Mmio>(
    mmio: &mut M,
    key_id: u8,
    temporal_key: &[u8; CCMP_KEY_BYTES],
) -> Result<StaGroupCcmpSlot, CryptoKeyError> {
    if key_id > MAX_WPA2_GTK_ID {
        return Err(CryptoKeyError::InvalidGroupKeyId);
    }
    let validity = mmio.read32(mac::CRYPTO_KEY_VALID_BITMAP);
    let valid_bit = 1_u32 << STA_GROUP_HARDWARE_INDEX;
    if validity & valid_bit != 0 {
        return Err(CryptoKeyError::Occupied);
    }

    clear_entry_words(mmio, STA_GROUP_HARDWARE_INDEX);

    let cipher = CCMP_ALGORITHM << 18;
    let direction = 6_u32;
    let logical_key_index = u32::from(key_id);
    let control = (direction << 5)
        | (u32::from(logical_key_index != 3) << 11)
        | (logical_key_index << 14)
        | ((cipher >> 16) & 0x341f);

    write_entry(mmio, STA_GROUP_HARDWARE_INDEX, 0, u32::MAX);
    write_entry(
        mmio,
        STA_GROUP_HARDWARE_INDEX,
        1,
        u32::from(u16::MAX) | (control << 16),
    );
    for (word, bytes) in temporal_key.chunks_exact(4).enumerate() {
        write_entry(
            mmio,
            STA_GROUP_HARDWARE_INDEX,
            word as u8 + 2,
            u32::from_le_bytes(bytes.try_into().expect("four-byte GTK word")),
        );
    }

    mmio.write32(mac::CRYPTO_KEY_VALID_BITMAP, validity | valid_bit);
    enable_sta_ccmp(mmio);
    mmio.fence();

    if mmio.read32(mac::CRYPTO_KEY_VALID_BITMAP) & valid_bit == 0 {
        return Err(CryptoKeyError::HardwareRejected);
    }
    Ok(StaGroupCcmpSlot { key_id })
}

fn enable_sta_ccmp<M: Mmio>(mmio: &mut M) {
    // Exact reachable `hal_crypto_enable(interface=STA, algorithm=CCMP,
    // enable=1, spp=0)` image.
    let first_control = 0x0003_0103;
    mmio.write32(mac::CRYPTO_INTERFACE_CONTROL[STA_INTERFACE], first_control);
    let policy = mmio.read32(mac::CRYPTO_POLICY_CONTROL);
    mmio.write32(mac::CRYPTO_POLICY_CONTROL, policy & 0xffc0_003f);
    let control = mmio.read32(mac::CRYPTO_INTERFACE_CONTROL[STA_INTERFACE]);
    mmio.write32(
        mac::CRYPTO_INTERFACE_CONTROL[STA_INTERFACE],
        control & 0x3fff_ffff,
    );
}

fn clear_entry<M: Mmio>(mmio: &mut M, index: u8) {
    let validity = mmio.read32(mac::CRYPTO_KEY_VALID_BITMAP);
    mmio.write32(mac::CRYPTO_KEY_VALID_BITMAP, validity & !(1_u32 << index));
    clear_entry_words(mmio, index);
}

fn clear_entry_words<M: Mmio>(mmio: &mut M, index: u8) {
    for word in 0..mac::CRYPTO_KEY_ENTRY_WORDS {
        write_entry(mmio, index, word, 0);
    }
}

fn write_entry<M: Mmio>(mmio: &mut M, index: u8, word: u8, value: u32) {
    let register =
        mac::crypto_key_entry_word(index, word).expect("fixed hardware key slot geometry");
    mmio.write32(register, value);
}
