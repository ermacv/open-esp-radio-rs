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
