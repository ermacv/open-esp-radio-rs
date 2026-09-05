use super::*;

#[test]
fn aggregate_role_handle_remains_a_small_borrowed_owner() {
    assert!(core::mem::size_of::<Esp32s31AccessPointAmpdu<'static, (), 32, 0>>() <= 256);
}
