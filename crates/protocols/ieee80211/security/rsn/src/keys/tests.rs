use super::*;

fn key(peer: [u8; 6], value: u8) -> RsnKeyInstall {
    RsnKeyInstall {
        interface: RsnInterface::Station,
        peer,
        kind: RsnKeyKind::Pairwise,
        receive_sequence: [0; 8],
        key: CcmpKey::new([value; RSN_TK_LEN]),
    }
}

#[test]
fn ccmp_key_is_word_aligned() {
    let key = CcmpKey::new([7; RSN_TK_LEN]);
    assert!(key.is_word_aligned());
    assert_eq!(core::mem::align_of::<CcmpKey>(), 4);
}

#[test]
fn static_key_table_replaces_matching_slot_and_fails_when_full() {
    let mut keys = StaticRsnKeys::<1>::new();
    assert_eq!(keys.insert(key([1; 6], 2)), Ok(0));
    assert_eq!(keys.insert(key([1; 6], 3)), Ok(0));
    assert_eq!(keys.get(0).unwrap().key().as_bytes(), &[3; RSN_TK_LEN]);
    assert_eq!(keys.insert(key([2; 6], 4)), Err(StaticKeyTableError::Full));
}

#[test]
fn group_transmit_flag_change_reuses_key_id_slot() {
    let mut keys = StaticRsnKeys::<1>::new();
    let first = RsnGtk::new(2, false, [6; RSN_TK_LEN]).unwrap();
    let second = RsnGtk::new(2, true, [7; RSN_TK_LEN]).unwrap();
    assert_eq!(
        keys.insert(RsnKeyInstall::group(
            RsnInterface::AccessPoint,
            &first,
            [0; 8]
        )),
        Ok(0)
    );
    assert_eq!(
        keys.insert(RsnKeyInstall::group(
            RsnInterface::AccessPoint,
            &second,
            [1; 8]
        )),
        Ok(0)
    );
    assert_eq!(
        keys.get(0).unwrap().kind(),
        RsnKeyKind::Group {
            key_id: 2,
            transmit: true
        }
    );
}
