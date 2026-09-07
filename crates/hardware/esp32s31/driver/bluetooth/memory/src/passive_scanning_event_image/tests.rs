use crate::{
    le_phy_packet::{LeAccessAddress, LeCrcInit},
    sram_link::ControllerSramLinkAddress,
};

use oer_esp32s31_hal::bluetooth::BluetoothControllerLatchedTime;

use super::{
    PassiveScanDefaultTxPowerDbm, PassiveScanLinkStateImage, PassiveScanResetConfig,
    PassiveScanRxHeadProjection,
};

#[test]
fn restricted_profile_retains_only_semantic_dynamic_inputs() {
    let head = PassiveScanRxHeadProjection::from_bound(
        ControllerSramLinkAddress::new(0x2f00_0100)
            .expect("the model header is a nonzero controller link"),
    );
    let config = PassiveScanResetConfig::le_1m_public_accept_all(
        PassiveScanDefaultTxPowerDbm::new(0),
        BluetoothControllerLatchedTime::from_bits(0x1234_5678),
    );

    let image = PassiveScanLinkStateImage::restricted_passive_le_1m(head, config);

    assert!(image.retains_rx_head(head));
    assert_eq!(image.crc_init(), LeCrcInit::LE_PRESET);
    assert_eq!(image.access_address(), LeAccessAddress::PRIMARY_ADVERTISING);
    assert_eq!(image.controller_time(), config.controller_time().bits());
}
