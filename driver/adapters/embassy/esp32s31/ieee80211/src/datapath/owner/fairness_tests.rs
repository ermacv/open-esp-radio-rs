use super::{charge_pair_tx_frames, select_pair_tx_slot};

#[test]
fn unequal_aggregate_sizes_receive_equal_frame_service() {
    let mut served = [0_u64; 2];
    assert_eq!(select_pair_tx_slot([true; 2], served), Some(0));
    charge_pair_tx_frames(&mut served, 0, 32);
    assert_eq!(select_pair_tx_slot([true; 2], served), Some(1));
    charge_pair_tx_frames(&mut served, 1, 16);
    assert_eq!(select_pair_tx_slot([true; 2], served), Some(1));
    charge_pair_tx_frames(&mut served, 1, 16);
    assert_eq!(served, [0, 0]);
    assert_eq!(select_pair_tx_slot([true; 2], served), Some(0));
}
