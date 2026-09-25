use crate::root::RadioHardware;

use super::Ieee802154Owned;

#[derive(Debug)]
struct FakePlatform;

#[test]
fn untouched_owner_releases_the_complete_neutral_root() {
    let owned = Ieee802154Owned::from_hardware(FakePlatform, RadioHardware::for_validation());
    let (_platform, hardware) = owned
        .release()
        .expect("an untouched IEEE 802.15.4 route can be released");

    let (_platform, _hardware) = Ieee802154Owned::from_hardware(FakePlatform, hardware)
        .release()
        .expect("an untouched IEEE 802.15.4 route can be released");
}
