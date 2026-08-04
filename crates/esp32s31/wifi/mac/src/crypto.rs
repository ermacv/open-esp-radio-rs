//! Typed ESP32-S31 hardware-key transactions.
//!
//! This is the allocation-free subset recovered from
//! `hal_crypto_set_key_entry` and `hal_crypto_enable`. It owns only MMIO
//! publication; WPA2 derivation and long-lived secret ownership remain in the
//! protocol crate.

use open_esp_radio_esp32s31_pac::{MacKeyInstallOutcome, RadioRegisters};
use open_esp_radio_ieee80211::ccmp::{advance_vendor_tx_pn, ccmp_header};

const STA_PAIRWISE_HARDWARE_INDEX: u8 = 4;
const STA_GROUP_HARDWARE_INDEX: u8 = 1;
const MAX_WPA2_GTK_ID: u8 = 3;
const PAIRWISE_LOGICAL_KEY_INDEX: u32 = 0;
const CCMP_ALGORITHM: u32 = 3;
const CCMP_KEY_BYTES: usize = 16;
const CCMP_ENTRY_WORDS: usize = 6;

/// Finite hardware authority for the two open STA CCMP slots.
pub trait CcmpKeyHardware {
    fn install_sta_ccmp_entry(
        &mut self,
        index: u8,
        words: [u32; CCMP_ENTRY_WORDS],
    ) -> MacKeyInstallOutcome;
    fn clear_ccmp_entry(&mut self, index: u8);
}

impl CcmpKeyHardware for RadioRegisters {
    fn install_sta_ccmp_entry(
        &mut self,
        index: u8,
        words: [u32; CCMP_ENTRY_WORDS],
    ) -> MacKeyInstallOutcome {
        self.install_sta_ccmp_key_entry(index, words)
    }

    fn clear_ccmp_entry(&mut self, index: u8) {
        self.clear_mac_key_entry(index);
    }
}

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

/// Hardware indices cleared at the connected-to-disconnected boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaCcmpClearReport {
    pub pairwise_hardware_index: u8,
    pub group_hardware_index: u8,
}

impl StaGroupCcmpSlot {
    pub const fn hardware_index(&self) -> u8 {
        STA_GROUP_HARDWARE_INDEX
    }

    pub const fn key_id(&self) -> u8 {
        self.key_id
    }

    pub fn clear<H: CcmpKeyHardware>(self, hardware: &mut H) {
        hardware.clear_ccmp_entry(STA_GROUP_HARDWARE_INDEX);
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

    pub fn clear<H: CcmpKeyHardware>(self, hardware: &mut H) {
        hardware.clear_ccmp_entry(STA_PAIRWISE_HARDWARE_INDEX);
    }
}

/// Consume and clear both association-scoped station key authorities.
///
/// Group is cleared before pairwise to match the recovered station teardown
/// order. The returned indices are observations only; they cannot authorize a
/// later clear or key use.
pub fn clear_sta_ccmp_slots<H: CcmpKeyHardware>(
    hardware: &mut H,
    pairwise: StaPairwiseCcmpSlot,
    group: StaGroupCcmpSlot,
) -> StaCcmpClearReport {
    let report = StaCcmpClearReport {
        pairwise_hardware_index: pairwise.hardware_index(),
        group_hardware_index: group.hardware_index(),
    };
    group.clear(hardware);
    pairwise.clear(hardware);
    report
}

/// Installs the WPA2 temporal key into the recovered STA pairwise slot.
///
/// The slot must be invalid on entry. This fail-closed rule prevents an open
/// driver instance from overwriting a key owned by retained vendor state or a
/// different interface. The key is borrowed only for the bounded MMIO
/// transaction and is never retained by the returned ownership token.
pub fn install_sta_pairwise_ccmp<H: CcmpKeyHardware>(
    hardware: &mut H,
    peer: [u8; 6],
    temporal_key: &[u8; CCMP_KEY_BYTES],
) -> Result<StaPairwiseCcmpSlot, CryptoKeyError> {
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

    let mut words = [0_u32; CCMP_ENTRY_WORDS];
    words[0] = peer_low;
    words[1] = u32::from(peer_high) | (control << 16);
    for (word, bytes) in temporal_key.chunks_exact(4).enumerate() {
        words[word + 2] = u32::from_le_bytes(bytes.try_into().expect("four-byte TK word"));
    }

    match hardware.install_sta_ccmp_entry(STA_PAIRWISE_HARDWARE_INDEX, words) {
        MacKeyInstallOutcome::Installed => {}
        MacKeyInstallOutcome::Occupied => return Err(CryptoKeyError::Occupied),
        MacKeyInstallOutcome::Rejected => return Err(CryptoKeyError::HardwareRejected),
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
pub fn install_sta_group_ccmp<H: CcmpKeyHardware>(
    hardware: &mut H,
    key_id: u8,
    temporal_key: &[u8; CCMP_KEY_BYTES],
) -> Result<StaGroupCcmpSlot, CryptoKeyError> {
    if key_id > MAX_WPA2_GTK_ID {
        return Err(CryptoKeyError::InvalidGroupKeyId);
    }
    let cipher = CCMP_ALGORITHM << 18;
    let direction = 6_u32;
    let logical_key_index = u32::from(key_id);
    let control = (direction << 5)
        | (u32::from(logical_key_index != 3) << 11)
        | (logical_key_index << 14)
        | ((cipher >> 16) & 0x341f);

    let mut words = [0_u32; CCMP_ENTRY_WORDS];
    words[0] = u32::MAX;
    words[1] = u32::from(u16::MAX) | (control << 16);
    for (word, bytes) in temporal_key.chunks_exact(4).enumerate() {
        words[word + 2] = u32::from_le_bytes(bytes.try_into().expect("four-byte GTK word"));
    }

    match hardware.install_sta_ccmp_entry(STA_GROUP_HARDWARE_INDEX, words) {
        MacKeyInstallOutcome::Installed => {}
        MacKeyInstallOutcome::Occupied => return Err(CryptoKeyError::Occupied),
        MacKeyInstallOutcome::Rejected => return Err(CryptoKeyError::HardwareRejected),
    }
    Ok(StaGroupCcmpSlot { key_id })
}
