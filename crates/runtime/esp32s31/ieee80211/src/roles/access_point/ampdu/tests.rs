use super::*;

#[test]
fn aggregate_role_handle_remains_a_small_borrowed_owner() {
    // Each arena's retry state carries its aggregate's MSDU lifetime deadline
    // and the Block Ack agreement the aggregate was built under.
    assert!(core::mem::size_of::<AccessPointAmpdu<'static, (), 32, 0>>() <= 296);
}
