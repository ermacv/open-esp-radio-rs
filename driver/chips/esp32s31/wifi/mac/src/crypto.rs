//! Typed ESP32-S31 hardware-key transactions.
//!
//! This is the allocation-free subset recovered from
//! `hal_crypto_set_key_entry` and `hal_crypto_enable`. It owns only MMIO
//! publication; WPA2 derivation and long-lived secret ownership remain in the
//! protocol crate.

use open_esp_radio_esp32s31_registers::{MacKeyInstallOutcome, RadioRegisters};
use open_esp_radio_ieee80211::ccmp::ccmp_header;

const STA_PAIRWISE_HARDWARE_INDEX: u8 = 4;
const STA_GROUP_HARDWARE_INDEX: u8 = 1;
// The first statically provisioned vendor AP peer used pairwise slot 8.
// AP GTK key id one maps to `1 + key_id`, therefore slot 2.
const AP_PAIRWISE_HARDWARE_INDEX: u8 = 8;
const AP_GROUP_HARDWARE_INDEX_BASE: u8 = 1;
const MAX_WPA2_GTK_ID: u8 = 3;
const PAIRWISE_LOGICAL_KEY_INDEX: u32 = 0;
const CCMP_ALGORITHM: u32 = 3;
const CCMP_KEY_BYTES: usize = 16;
const CCMP_ENTRY_WORDS: usize = 6;

const fn advance_esp32s31_tx_pn(low: u32, high: u32) -> (u32, u32) {
    let next_low = low.wrapping_add(3);
    let carry = (next_low < low) as u32;
    (next_low, high.wrapping_add(carry))
}

/// Finite hardware authority for the two open STA CCMP slots.
pub trait CcmpKeyHardware {
    fn install_sta_ccmp_entry(
        &mut self,
        index: u8,
        words: [u32; CCMP_ENTRY_WORDS],
    ) -> MacKeyInstallOutcome;
    fn install_ap_ccmp_entry(
        &mut self,
        index: u8,
        words: [u32; CCMP_ENTRY_WORDS],
    ) -> MacKeyInstallOutcome {
        self.install_sta_ccmp_entry(index, words)
    }
    fn clear_ccmp_entry(&mut self, index: u8);

    /// Query one hardware entry when the implementation exposes validity.
    fn ccmp_entry_is_valid(&self, _index: u8) -> Option<bool> {
        None
    }
}

impl CcmpKeyHardware for RadioRegisters {
    fn install_sta_ccmp_entry(
        &mut self,
        index: u8,
        words: [u32; CCMP_ENTRY_WORDS],
    ) -> MacKeyInstallOutcome {
        self.install_sta_ccmp_key_entry(index, words)
    }

    fn install_ap_ccmp_entry(
        &mut self,
        index: u8,
        words: [u32; CCMP_ENTRY_WORDS],
    ) -> MacKeyInstallOutcome {
        self.install_ap_ccmp_key_entry(index, words)
    }

    fn clear_ccmp_entry(&mut self, index: u8) {
        self.clear_mac_key_entry(index);
    }

    fn ccmp_entry_is_valid(&self, index: u8) -> Option<bool> {
        self.mac_key_entry_is_valid(index)
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
    installed: bool,
}

/// Authority for the first AP peer pairwise CCMP entry.
#[must_use = "the installed hardware key must remain owned until it is explicitly cleared"]
pub struct ApPairwiseCcmpSlot {
    peer: [u8; 6],
    tx_pn_low: u32,
    tx_pn_high: u32,
}

/// Authority for the one group key exposed by the first AP service.
#[must_use = "the installed hardware key must remain owned until it is explicitly cleared"]
pub struct ApGroupCcmpSlot {
    key_id: u8,
    hardware_index: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApCcmpClearReport {
    pub pairwise_hardware_index: u8,
    pub group_hardware_index: u8,
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
        if self.installed {
            hardware.clear_ccmp_entry(STA_GROUP_HARDWARE_INDEX);
        }
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
        (self.tx_pn_low, self.tx_pn_high) = advance_esp32s31_tx_pn(self.tx_pn_low, self.tx_pn_high);
        ccmp_header(self.tx_pn_low, self.tx_pn_high, 0)
    }

    pub fn clear<H: CcmpKeyHardware>(self, hardware: &mut H) {
        hardware.clear_ccmp_entry(STA_PAIRWISE_HARDWARE_INDEX);
    }
}

impl ApPairwiseCcmpSlot {
    pub const fn hardware_index(&self) -> u8 {
        AP_PAIRWISE_HARDWARE_INDEX
    }

    pub const fn peer(&self) -> &[u8; 6] {
        &self.peer
    }

    pub fn next_tx_ccmp_header(&mut self) -> [u8; 8] {
        (self.tx_pn_low, self.tx_pn_high) = advance_esp32s31_tx_pn(self.tx_pn_low, self.tx_pn_high);
        ccmp_header(self.tx_pn_low, self.tx_pn_high, 0)
    }

    pub fn clear<H: CcmpKeyHardware>(self, hardware: &mut H) {
        hardware.clear_ccmp_entry(AP_PAIRWISE_HARDWARE_INDEX);
    }
}

impl ApGroupCcmpSlot {
    pub const fn hardware_index(&self) -> u8 {
        self.hardware_index
    }

    pub const fn key_id(&self) -> u8 {
        self.key_id
    }

    pub fn clear<H: CcmpKeyHardware>(self, hardware: &mut H) {
        hardware.clear_ccmp_entry(self.hardware_index);
    }
}

pub fn clear_ap_ccmp_slots<H: CcmpKeyHardware>(
    hardware: &mut H,
    pairwise: ApPairwiseCcmpSlot,
    group: ApGroupCcmpSlot,
) -> ApCcmpClearReport {
    let report = ApCcmpClearReport {
        pairwise_hardware_index: pairwise.hardware_index(),
        group_hardware_index: group.hardware_index(),
    };
    group.clear(hardware);
    pairwise.clear(hardware);
    report
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
    Ok(StaGroupCcmpSlot {
        key_id,
        installed: true,
    })
}

/// Install the first AP client's pairwise CCMP key into hardware slot 8.
///
/// The slot and direction encoding are evidenced by the retained vendor AP
/// qualification: AP peer identity one selected descriptor key byte `0x48`,
/// and `hal_crypto_set_key_entry` uses pairwise direction three for entries
/// above the four group-key slots.
pub fn install_ap_pairwise_ccmp<H: CcmpKeyHardware>(
    hardware: &mut H,
    peer: [u8; 6],
    temporal_key: &[u8; CCMP_KEY_BYTES],
) -> Result<ApPairwiseCcmpSlot, CryptoKeyError> {
    let peer_low = u32::from_le_bytes(peer[..4].try_into().expect("fixed peer low word"));
    let peer_high = u16::from_le_bytes(peer[4..].try_into().expect("fixed peer high word"));
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

    match hardware.install_ap_ccmp_entry(AP_PAIRWISE_HARDWARE_INDEX, words) {
        MacKeyInstallOutcome::Installed => {}
        MacKeyInstallOutcome::Occupied => return Err(CryptoKeyError::Occupied),
        MacKeyInstallOutcome::Rejected => return Err(CryptoKeyError::HardwareRejected),
    }
    Ok(ApPairwiseCcmpSlot {
        peer,
        tx_pn_low: 0,
        tx_pn_high: 0,
    })
}

/// Install one AP GTK. Vendor AP group slots map directly to `1 + key_id`;
/// the first service provisions key id one and therefore owns slot 2.
pub fn install_ap_group_ccmp<H: CcmpKeyHardware>(
    hardware: &mut H,
    key_id: u8,
    temporal_key: &[u8; CCMP_KEY_BYTES],
) -> Result<ApGroupCcmpSlot, CryptoKeyError> {
    if key_id > MAX_WPA2_GTK_ID {
        return Err(CryptoKeyError::InvalidGroupKeyId);
    }
    let hardware_index = AP_GROUP_HARDWARE_INDEX_BASE + key_id;
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

    match hardware.install_ap_ccmp_entry(hardware_index, words) {
        MacKeyInstallOutcome::Installed => {}
        MacKeyInstallOutcome::Occupied => return Err(CryptoKeyError::Occupied),
        MacKeyInstallOutcome::Rejected => return Err(CryptoKeyError::HardwareRejected),
    }
    Ok(ApGroupCcmpSlot {
        key_id,
        hardware_index,
    })
}

/// Replace the association-owned STA group key in its single hardware slot.
///
/// The existing token remains the unique authority for slot 1. Clearing is
/// performed before publication because the recovered hardware primitive
/// rejects an occupied entry. If publication fails, the token is marked
/// uninstalled so later teardown cannot claim or clear a key which hardware
/// never accepted; the association must then be terminated.
pub fn replace_sta_group_ccmp<H: CcmpKeyHardware>(
    hardware: &mut H,
    slot: &mut StaGroupCcmpSlot,
    key_id: u8,
    temporal_key: &[u8; CCMP_KEY_BYTES],
) -> Result<(), CryptoKeyError> {
    if key_id > MAX_WPA2_GTK_ID {
        return Err(CryptoKeyError::InvalidGroupKeyId);
    }
    if slot.installed {
        hardware.clear_ccmp_entry(STA_GROUP_HARDWARE_INDEX);
        slot.installed = false;
    }
    match install_sta_group_ccmp(hardware, key_id, temporal_key) {
        Ok(replacement) => {
            *slot = replacement;
            Ok(())
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Hardware {
        occupied: bool,
        reject_next: bool,
        installs: u8,
        clears: u8,
        last_index: Option<u8>,
    }

    impl CcmpKeyHardware for Hardware {
        fn install_sta_ccmp_entry(
            &mut self,
            index: u8,
            _words: [u32; CCMP_ENTRY_WORDS],
        ) -> MacKeyInstallOutcome {
            if self.reject_next {
                self.reject_next = false;
                return MacKeyInstallOutcome::Rejected;
            }
            if self.occupied {
                return MacKeyInstallOutcome::Occupied;
            }
            self.occupied = true;
            self.installs += 1;
            self.last_index = Some(index);
            MacKeyInstallOutcome::Installed
        }

        fn clear_ccmp_entry(&mut self, _index: u8) {
            self.occupied = false;
            self.clears += 1;
        }
    }

    #[test]
    fn first_ap_peer_and_gtk_own_the_evidenced_disjoint_slots() {
        let mut hardware = Hardware::default();
        let pairwise =
            install_ap_pairwise_ccmp(&mut hardware, [1, 2, 3, 4, 5, 6], &[7; 16]).unwrap();
        assert_eq!(pairwise.hardware_index(), 8);
        assert_eq!(hardware.last_index, Some(8));
        pairwise.clear(&mut hardware);

        let group = install_ap_group_ccmp(&mut hardware, 1, &[9; 16]).unwrap();
        assert_eq!(group.hardware_index(), 2);
        assert_eq!(hardware.last_index, Some(2));
        group.clear(&mut hardware);
        assert_eq!(hardware.clears, 2);
    }

    #[test]
    fn group_rekey_reuses_one_authority_and_clears_the_replacement_once() {
        let mut hardware = Hardware::default();
        let mut slot = install_sta_group_ccmp(&mut hardware, 1, &[1; 16]).unwrap();

        replace_sta_group_ccmp(&mut hardware, &mut slot, 2, &[2; 16]).unwrap();
        assert_eq!(slot.key_id(), 2);
        assert_eq!(hardware.installs, 2);
        assert_eq!(hardware.clears, 1);

        slot.clear(&mut hardware);
        assert_eq!(hardware.clears, 2);
        assert!(!hardware.occupied);
    }

    #[test]
    fn rejected_rekey_invalidates_the_token_and_teardown_does_not_double_clear() {
        let mut hardware = Hardware::default();
        let mut slot = install_sta_group_ccmp(&mut hardware, 1, &[1; 16]).unwrap();
        hardware.reject_next = true;

        assert_eq!(
            replace_sta_group_ccmp(&mut hardware, &mut slot, 2, &[2; 16]),
            Err(CryptoKeyError::HardwareRejected)
        );
        assert_eq!(hardware.clears, 1);
        slot.clear(&mut hardware);
        assert_eq!(hardware.clears, 1);
    }
}
