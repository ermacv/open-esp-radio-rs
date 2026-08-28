//! Typed ESP32-S31 hardware-key transactions.
//!
//! This is the allocation-free subset recovered from
//! `hal_crypto_set_key_entry` and `hal_crypto_enable`. It owns only MMIO
//! publication; WPA2 derivation and long-lived secret ownership remain in the
//! protocol crate.

use open_esp_radio_esp32s31_hal::types::{MacCcmpKeyIdentity, MacKeyInstallOutcome};
use open_esp_radio_esp32s31_hal::{RadioRuntimeOwner, wifi_mac::WifiMacHal};
use open_esp_radio_ieee80211::ccmp::ccmp_header;
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

const STA_PAIRWISE_HARDWARE_INDEX: u8 = 4;
const STA_GROUP_HARDWARE_INDEX: u8 = 1;
// The vendor AP pairwise pool begins at slot 8. Public AP AIDs 1..=15 map to
// slots 8..=22; the remaining physical entries are not claimed by this API.
// AP GTK key id one maps to `1 + key_id`, therefore slot 2.
pub const AP_PAIRWISE_HARDWARE_INDEX_BASE: u8 = 8;
pub const AP_PAIRWISE_SLOT_COUNT: u8 = 15;
const AP_GROUP_HARDWARE_INDEX_BASE: u8 = 1;
const MAX_WPA2_GTK_ID: u8 = 3;
const CCMP_KEY_BYTES: usize = 16;

const fn advance_esp32s31_tx_pn(low: u32, high: u16) -> Option<(u32, u16)> {
    let next_low = low.wrapping_add(3);
    if next_low < low {
        if high == u16::MAX {
            None
        } else {
            Some((next_low, high + 1))
        }
    } else {
        Some((next_low, high))
    }
}

/// Exhaustion of one key's finite 48-bit CCMP transmit packet-number space.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CcmpTxPacketNumberError {
    Exhausted,
}

/// Unique software owner of one installed key's transmit packet number.
struct CcmpTxPacketNumber {
    low: u32,
    high: u16,
}

impl CcmpTxPacketNumber {
    const fn new() -> Self {
        Self { low: 0, high: 0 }
    }

    fn next_header(&mut self, key_id_bits: u8) -> Result<[u8; 8], CcmpTxPacketNumberError> {
        let Some((low, high)) = advance_esp32s31_tx_pn(self.low, self.high) else {
            return Err(CcmpTxPacketNumberError::Exhausted);
        };
        self.low = low;
        self.high = high;
        Ok(ccmp_header(low, u32::from(high), key_id_bits))
    }
}

/// Finite hardware authority for the two open STA CCMP slots.
pub trait CcmpKeyHardware {
    fn install_sta_ccmp_entry(
        &mut self,
        index: u8,
        identity: MacCcmpKeyIdentity,
        temporal_key: &[u8; CCMP_KEY_BYTES],
    ) -> MacKeyInstallOutcome;
    fn install_ap_ccmp_entry(
        &mut self,
        index: u8,
        identity: MacCcmpKeyIdentity,
        temporal_key: &[u8; CCMP_KEY_BYTES],
    ) -> MacKeyInstallOutcome {
        self.install_sta_ccmp_entry(index, identity, temporal_key)
    }
    fn clear_ccmp_entry(&mut self, index: u8);

    /// Query one hardware entry when the implementation exposes validity.
    fn ccmp_entry_is_valid(&self, _index: u8) -> Option<bool> {
        None
    }
}

impl CcmpKeyHardware for WifiMacHal<'_> {
    fn install_sta_ccmp_entry(
        &mut self,
        index: u8,
        identity: MacCcmpKeyIdentity,
        temporal_key: &[u8; CCMP_KEY_BYTES],
    ) -> MacKeyInstallOutcome {
        self.install_station_ccmp_entry(index, identity, temporal_key)
    }

    fn install_ap_ccmp_entry(
        &mut self,
        index: u8,
        identity: MacCcmpKeyIdentity,
        temporal_key: &[u8; CCMP_KEY_BYTES],
    ) -> MacKeyInstallOutcome {
        self.install_access_point_ccmp_entry(index, identity, temporal_key)
    }

    fn clear_ccmp_entry(&mut self, index: u8) {
        WifiMacHal::clear_ccmp_entry(self, index);
    }

    fn ccmp_entry_is_valid(&self, index: u8) -> Option<bool> {
        WifiMacHal::ccmp_entry_is_valid(self, index)
    }
}

impl CcmpKeyHardware for RadioRuntimeOwner {
    fn install_sta_ccmp_entry(
        &mut self,
        index: u8,
        identity: MacCcmpKeyIdentity,
        temporal_key: &[u8; CCMP_KEY_BYTES],
    ) -> MacKeyInstallOutcome {
        CcmpKeyHardware::install_sta_ccmp_entry(
            &mut self.wifi_mac_hal(),
            index,
            identity,
            temporal_key,
        )
    }

    fn install_ap_ccmp_entry(
        &mut self,
        index: u8,
        identity: MacCcmpKeyIdentity,
        temporal_key: &[u8; CCMP_KEY_BYTES],
    ) -> MacKeyInstallOutcome {
        CcmpKeyHardware::install_ap_ccmp_entry(
            &mut self.wifi_mac_hal(),
            index,
            identity,
            temporal_key,
        )
    }

    fn clear_ccmp_entry(&mut self, index: u8) {
        CcmpKeyHardware::clear_ccmp_entry(&mut self.wifi_mac_hal(), index);
    }

    fn ccmp_entry_is_valid(&self, _index: u8) -> Option<bool> {
        None
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CryptoKeyError {
    InvalidGroupKeyId,
    InvalidAccessPointAssociationId,
    Occupied,
    HardwareRejected,
}

/// Fail-closed result of replacing the one occupied STA group slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaGroupCcmpReplaceError {
    /// Validation failed before the old entry was touched.
    InvalidReplacement(CryptoKeyError),
    /// The new publication failed and the exact old key was restored.
    ReplacementRolledBack(CryptoKeyError),
    /// Neither complete epoch is installed. The slot token is invalidated and
    /// group RX must remain quarantined until disconnect teardown.
    RollbackFailed {
        replacement: CryptoKeyError,
        rollback: CryptoKeyError,
    },
    /// The supplied old material does not authorize the occupied slot.
    CurrentMaterialMismatch,
}

/// Authority for the one STA pairwise CCMP slot installed by this module.
///
/// The token deliberately contains no key bytes. Consuming it through
/// [`StaPairwiseCcmpSlot::clear`] clears the hardware entry.
#[must_use = "the installed hardware key must remain owned until it is explicitly cleared"]
pub struct StaPairwiseCcmpSlot {
    peer: [u8; 6],
    tx_packet_number: CcmpTxPacketNumber,
}

/// Authority for the one active STA group CCMP slot recovered from the
/// promoted production backend.
#[must_use = "the installed hardware key must remain owned until it is explicitly cleared"]
pub struct StaGroupCcmpSlot {
    key_id: u8,
    installed: bool,
}

/// Zeroizing software rollback authority for the one installed STA GTK.
///
/// The hardware slot token intentionally contains no secret. Connected group
/// rekey must therefore retain this separate owner; without it a failed
/// replacement cannot truthfully restore the old hardware epoch.
#[must_use = "GTK material must remain owned until replacement or teardown"]
pub struct StaGroupCcmpKeyMaterial {
    key_id: u8,
    temporal_key: [u8; CCMP_KEY_BYTES],
}

impl StaGroupCcmpKeyMaterial {
    pub fn new(key_id: u8, temporal_key: [u8; CCMP_KEY_BYTES]) -> Result<Self, CryptoKeyError> {
        if key_id > MAX_WPA2_GTK_ID {
            return Err(CryptoKeyError::InvalidGroupKeyId);
        }
        Ok(Self {
            key_id,
            temporal_key,
        })
    }

    pub const fn key_id(&self) -> u8 {
        self.key_id
    }

    /// Compare key bytes without making a same-KeyID admission decision leak
    /// the first differing secret octet.
    pub fn same_temporal_key(&self, other: &Self) -> bool {
        bool::from(self.temporal_key.ct_eq(&other.temporal_key))
    }
}

impl Drop for StaGroupCcmpKeyMaterial {
    fn drop(&mut self) {
        self.temporal_key.zeroize();
    }
}

/// Authority for one AP peer pairwise CCMP entry.
#[must_use = "the installed hardware key must remain owned until it is explicitly cleared"]
pub struct ApPairwiseCcmpSlot {
    peer: [u8; 6],
    association_id: u16,
    hardware_index: u8,
    tx_packet_number: CcmpTxPacketNumber,
}

/// Authority for the one group key exposed by an AP service epoch.
#[must_use = "the installed hardware key must remain owned until it is explicitly cleared"]
pub struct ApGroupCcmpSlot {
    key_id: u8,
    hardware_index: u8,
    tx_packet_number: CcmpTxPacketNumber,
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
    /// zero, so only the ExtIV bit is present in byte three. Once PN 2^48 - 1
    /// has been emitted, later calls fail without wrapping or mutating state.
    pub fn next_tx_ccmp_header(&mut self) -> Result<[u8; 8], CcmpTxPacketNumberError> {
        self.tx_packet_number.next_header(0)
    }

    pub fn clear<H: CcmpKeyHardware>(self, hardware: &mut H) {
        hardware.clear_ccmp_entry(STA_PAIRWISE_HARDWARE_INDEX);
    }
}

impl ApPairwiseCcmpSlot {
    pub const fn hardware_index(&self) -> u8 {
        self.hardware_index
    }

    pub const fn peer(&self) -> &[u8; 6] {
        &self.peer
    }

    pub const fn association_id(&self) -> u16 {
        self.association_id
    }

    pub fn next_tx_ccmp_header(&mut self) -> Result<[u8; 8], CcmpTxPacketNumberError> {
        self.tx_packet_number.next_header(0)
    }

    pub fn clear<H: CcmpKeyHardware>(self, hardware: &mut H) {
        hardware.clear_ccmp_entry(self.hardware_index);
    }
}

impl ApGroupCcmpSlot {
    pub const fn hardware_index(&self) -> u8 {
        self.hardware_index
    }

    pub const fn key_id(&self) -> u8 {
        self.key_id
    }

    pub fn next_tx_ccmp_header(&mut self) -> Result<[u8; 8], CcmpTxPacketNumberError> {
        self.tx_packet_number.next_header(self.key_id << 6)
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
    match hardware.install_sta_ccmp_entry(
        STA_PAIRWISE_HARDWARE_INDEX,
        MacCcmpKeyIdentity::Pairwise { peer },
        temporal_key,
    ) {
        MacKeyInstallOutcome::Installed => {}
        MacKeyInstallOutcome::Occupied => return Err(CryptoKeyError::Occupied),
        MacKeyInstallOutcome::Rejected => return Err(CryptoKeyError::HardwareRejected),
    }
    Ok(StaPairwiseCcmpSlot {
        peer,
        tx_packet_number: CcmpTxPacketNumber::new(),
    })
}

/// Installs the recovered STA GTK into hardware slot 1.
///
/// ESP32-S31 exposes one active station group slot regardless of the logical
/// RSN key ID. The logical ID is still encoded in the hardware control word,
/// exactly as in the reviewed promoted CCMP installation path.
pub fn install_sta_group_ccmp<H: CcmpKeyHardware>(
    hardware: &mut H,
    key_id: u8,
    temporal_key: &[u8; CCMP_KEY_BYTES],
) -> Result<StaGroupCcmpSlot, CryptoKeyError> {
    if key_id > MAX_WPA2_GTK_ID {
        return Err(CryptoKeyError::InvalidGroupKeyId);
    }
    match hardware.install_sta_ccmp_entry(
        STA_GROUP_HARDWARE_INDEX,
        MacCcmpKeyIdentity::Group { key_id },
        temporal_key,
    ) {
        MacKeyInstallOutcome::Installed => {}
        MacKeyInstallOutcome::Occupied => return Err(CryptoKeyError::Occupied),
        MacKeyInstallOutcome::Rejected => return Err(CryptoKeyError::HardwareRejected),
    }
    Ok(StaGroupCcmpSlot {
        key_id,
        installed: true,
    })
}

/// Install one AP client's pairwise CCMP key into the AID-derived slot.
///
/// The slot and direction encoding are evidenced by the retained vendor AP
/// qualification: AP peer identity one selected descriptor key byte `0x48`,
/// and `hal_crypto_set_key_entry` uses pairwise direction three for entries
/// above the four group-key slots. AID one owns slot 8 and AID fifteen owns
/// slot 22. The complete vendor path
/// `esp_wifi_set_ap_key_internal -> ic_set_key -> wDev_Insert_KeyEntry ->
/// hal_crypto_set_key_entry` passes connection context one and encodes it in
/// bits 8..=9 of the key-entry control halfword. The station path passes zero.
pub fn install_ap_pairwise_ccmp<H: CcmpKeyHardware>(
    hardware: &mut H,
    peer: [u8; 6],
    association_id: u16,
    temporal_key: &[u8; CCMP_KEY_BYTES],
) -> Result<ApPairwiseCcmpSlot, CryptoKeyError> {
    if association_id == 0 || association_id > u16::from(AP_PAIRWISE_SLOT_COUNT) {
        return Err(CryptoKeyError::InvalidAccessPointAssociationId);
    }
    let hardware_index = AP_PAIRWISE_HARDWARE_INDEX_BASE
        + u8::try_from(association_id - 1).expect("validated AP association id fits u8");
    match hardware.install_ap_ccmp_entry(
        hardware_index,
        MacCcmpKeyIdentity::Pairwise { peer },
        temporal_key,
    ) {
        MacKeyInstallOutcome::Installed => {}
        MacKeyInstallOutcome::Occupied => return Err(CryptoKeyError::Occupied),
        MacKeyInstallOutcome::Rejected => return Err(CryptoKeyError::HardwareRejected),
    }
    Ok(ApPairwiseCcmpSlot {
        peer,
        association_id,
        hardware_index,
        tx_packet_number: CcmpTxPacketNumber::new(),
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
    match hardware.install_ap_ccmp_entry(
        hardware_index,
        MacCcmpKeyIdentity::Group { key_id },
        temporal_key,
    ) {
        MacKeyInstallOutcome::Installed => {}
        MacKeyInstallOutcome::Occupied => return Err(CryptoKeyError::Occupied),
        MacKeyInstallOutcome::Rejected => return Err(CryptoKeyError::HardwareRejected),
    }
    Ok(ApGroupCcmpSlot {
        key_id,
        hardware_index,
        tx_packet_number: CcmpTxPacketNumber::new(),
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

/// Replace the occupied STA GTK and restore the exact old entry on failure.
///
/// The caller must keep group replay publication gated for the whole call.
/// A successful return changes only hardware; software replay/key-id rotation
/// is a separate affine commit. `ReplacementRolledBack` proves that the old
/// key is installed again and the old replay epoch may be un-gated.
pub fn replace_sta_group_ccmp_with_rollback<H: CcmpKeyHardware>(
    hardware: &mut H,
    slot: &mut StaGroupCcmpSlot,
    current: &StaGroupCcmpKeyMaterial,
    replacement: &StaGroupCcmpKeyMaterial,
) -> Result<(), StaGroupCcmpReplaceError> {
    if replacement.key_id > MAX_WPA2_GTK_ID {
        return Err(StaGroupCcmpReplaceError::InvalidReplacement(
            CryptoKeyError::InvalidGroupKeyId,
        ));
    }
    if !slot.installed || slot.key_id != current.key_id {
        return Err(StaGroupCcmpReplaceError::CurrentMaterialMismatch);
    }

    hardware.clear_ccmp_entry(STA_GROUP_HARDWARE_INDEX);
    slot.installed = false;
    match install_sta_group_ccmp(hardware, replacement.key_id, &replacement.temporal_key) {
        Ok(installed) => {
            *slot = installed;
            Ok(())
        }
        Err(replacement_error) => {
            match install_sta_group_ccmp(hardware, current.key_id, &current.temporal_key) {
                Ok(restored) => {
                    *slot = restored;
                    Err(StaGroupCcmpReplaceError::ReplacementRolledBack(
                        replacement_error,
                    ))
                }
                Err(rollback) => Err(StaGroupCcmpReplaceError::RollbackFailed {
                    replacement: replacement_error,
                    rollback,
                }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Hardware {
        occupied: bool,
        reject_installs: u8,
        installs: u8,
        clears: u8,
        last_index: Option<u8>,
        last_install: Option<(MacCcmpKeyIdentity, [u8; CCMP_KEY_BYTES])>,
    }

    impl CcmpKeyHardware for Hardware {
        fn install_sta_ccmp_entry(
            &mut self,
            index: u8,
            identity: MacCcmpKeyIdentity,
            temporal_key: &[u8; CCMP_KEY_BYTES],
        ) -> MacKeyInstallOutcome {
            if self.reject_installs != 0 {
                self.reject_installs -= 1;
                return MacKeyInstallOutcome::Rejected;
            }
            if self.occupied {
                return MacKeyInstallOutcome::Occupied;
            }
            self.occupied = true;
            self.installs += 1;
            self.last_index = Some(index);
            self.last_install = Some((identity, *temporal_key));
            MacKeyInstallOutcome::Installed
        }

        fn clear_ccmp_entry(&mut self, _index: u8) {
            self.occupied = false;
            self.clears += 1;
        }
    }

    #[test]
    fn tx_packet_number_emits_the_48_bit_maximum_once_then_fails_closed() {
        let mut packet_number = CcmpTxPacketNumber {
            low: u32::MAX - 3,
            high: u16::MAX,
        };
        assert_eq!(
            packet_number.next_header(0x40),
            Ok([0xff, 0xff, 0, 0x60, 0xff, 0xff, 0xff, 0xff])
        );
        assert_eq!(
            packet_number.next_header(0x40),
            Err(CcmpTxPacketNumberError::Exhausted)
        );
        assert_eq!(
            packet_number.next_header(0x40),
            Err(CcmpTxPacketNumberError::Exhausted)
        );
    }

    #[test]
    fn exhausted_pairwise_slot_retains_hardware_clear_authority() {
        let mut hardware = Hardware {
            occupied: true,
            ..Hardware::default()
        };
        let mut slot = StaPairwiseCcmpSlot {
            peer: [1, 2, 3, 4, 5, 6],
            tx_packet_number: CcmpTxPacketNumber {
                low: u32::MAX,
                high: u16::MAX,
            },
        };
        assert_eq!(
            slot.next_tx_ccmp_header(),
            Err(CcmpTxPacketNumberError::Exhausted)
        );
        slot.clear(&mut hardware);
        assert_eq!(hardware.clears, 1);
        assert!(!hardware.occupied);
    }

    #[test]
    fn ap_pairwise_and_group_slots_share_the_fail_closed_pn_boundary() {
        let exhausted = || CcmpTxPacketNumber {
            low: u32::MAX,
            high: u16::MAX,
        };
        let mut pairwise = ApPairwiseCcmpSlot {
            peer: [1, 2, 3, 4, 5, 6],
            association_id: 1,
            hardware_index: AP_PAIRWISE_HARDWARE_INDEX_BASE,
            tx_packet_number: exhausted(),
        };
        let mut group = ApGroupCcmpSlot {
            key_id: 1,
            hardware_index: AP_GROUP_HARDWARE_INDEX_BASE + 1,
            tx_packet_number: exhausted(),
        };
        assert_eq!(
            pairwise.next_tx_ccmp_header(),
            Err(CcmpTxPacketNumberError::Exhausted)
        );
        assert_eq!(
            group.next_tx_ccmp_header(),
            Err(CcmpTxPacketNumberError::Exhausted)
        );
    }

    #[test]
    fn first_ap_peer_and_gtk_own_the_evidenced_disjoint_slots() {
        let mut hardware = Hardware::default();
        let pairwise =
            install_ap_pairwise_ccmp(&mut hardware, [1, 2, 3, 4, 5, 6], 1, &[7; 16]).unwrap();
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
        hardware.reject_installs = 1;

        assert_eq!(
            replace_sta_group_ccmp(&mut hardware, &mut slot, 2, &[2; 16]),
            Err(CryptoKeyError::HardwareRejected)
        );
        assert_eq!(hardware.clears, 1);
        slot.clear(&mut hardware);
        assert_eq!(hardware.clears, 1);
    }

    #[test]
    fn group_rekey_failure_restores_exact_old_key_and_slot_authority() {
        let mut hardware = Hardware::default();
        let mut slot = install_sta_group_ccmp(&mut hardware, 1, &[1; 16]).unwrap();
        let current = StaGroupCcmpKeyMaterial::new(1, [1; 16]).unwrap();
        let replacement = StaGroupCcmpKeyMaterial::new(2, [2; 16]).unwrap();
        let old_install = hardware.last_install;
        hardware.reject_installs = 1;

        assert_eq!(
            replace_sta_group_ccmp_with_rollback(&mut hardware, &mut slot, &current, &replacement,),
            Err(StaGroupCcmpReplaceError::ReplacementRolledBack(
                CryptoKeyError::HardwareRejected,
            ))
        );
        assert_eq!(slot.key_id(), 1);
        assert!(hardware.occupied);
        assert_eq!(hardware.last_install, old_install);
        slot.clear(&mut hardware);
        assert_eq!(hardware.clears, 2);
    }

    #[test]
    fn group_rekey_rollback_failure_invalidates_slot_and_requires_quarantine() {
        let mut hardware = Hardware::default();
        let mut slot = install_sta_group_ccmp(&mut hardware, 1, &[1; 16]).unwrap();
        let current = StaGroupCcmpKeyMaterial::new(1, [1; 16]).unwrap();
        let replacement = StaGroupCcmpKeyMaterial::new(2, [2; 16]).unwrap();
        hardware.reject_installs = 2;

        assert_eq!(
            replace_sta_group_ccmp_with_rollback(&mut hardware, &mut slot, &current, &replacement,),
            Err(StaGroupCcmpReplaceError::RollbackFailed {
                replacement: CryptoKeyError::HardwareRejected,
                rollback: CryptoKeyError::HardwareRejected,
            })
        );
        assert!(!hardware.occupied);
        slot.clear(&mut hardware);
        assert_eq!(hardware.clears, 1);
    }

    #[test]
    fn group_material_comparison_covers_same_and_different_key_id_cases() {
        let current = StaGroupCcmpKeyMaterial::new(1, [0x11; 16]).unwrap();
        let same_key_other_id = StaGroupCcmpKeyMaterial::new(2, [0x11; 16]).unwrap();
        let changed_same_id = StaGroupCcmpKeyMaterial::new(1, [0x22; 16]).unwrap();
        assert!(current.same_temporal_key(&same_key_other_id));
        assert!(!current.same_temporal_key(&changed_same_id));
    }
}
