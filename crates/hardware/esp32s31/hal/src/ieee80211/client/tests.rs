use super::WifiCold;
use crate::root::RadioHardware;

/// The partition round-trips through the cold owner without MMIO.
#[test]
fn a_cold_client_returns_its_partition() {
    let (_radio, partitions) = RadioHardware::for_validation().into_concurrent(());
    let cold = WifiCold::from_partition(partitions.wifi);
    let _partition = cold.into_partition();
}
