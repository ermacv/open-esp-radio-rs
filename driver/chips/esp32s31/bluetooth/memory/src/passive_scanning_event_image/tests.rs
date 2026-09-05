use open_esp_radio_esp32s31_hal::BluetoothControllerLatchedTime;

use crate::{
    le_phy_packet::{BluetoothLeAccessAddress, BluetoothLeCrcInit},
    sram_link::BluetoothControllerSramLinkAddress,
};

use super::{
    BluetoothPassiveScanDefaultTxPowerDbm, BluetoothPassiveScanLinkStateImage,
    BluetoothPassiveScanResetConfig, BluetoothPassiveScanRxHeadProjection,
};

#[test]
fn restricted_profile_retains_only_semantic_dynamic_inputs() {
    let head = BluetoothPassiveScanRxHeadProjection::from_bound(
        BluetoothControllerSramLinkAddress::new(0x2f00_0100)
            .expect("the model header is a nonzero controller link"),
    );
    let config = BluetoothPassiveScanResetConfig::le_1m_public_accept_all(
        BluetoothPassiveScanDefaultTxPowerDbm::new(0),
        BluetoothControllerLatchedTime::from_bits(0x1234_5678),
    );

    let image = BluetoothPassiveScanLinkStateImage::restricted_passive_le_1m(head, config);

    assert!(image.retains_rx_head(head));
    assert_eq!(image.crc_init(), BluetoothLeCrcInit::LE_PRESET);
    assert_eq!(
        image.access_address(),
        BluetoothLeAccessAddress::PRIMARY_ADVERTISING
    );
    assert_eq!(image.controller_time(), config.controller_time().bits());
}
