//! Conversion of shared PHY observations, with no protocol-specific owner.
pub(super) fn rx_quality(
    quality: oer_esp32s31_phy::rx::gain_calibration::PhyRxGainDcQuality,
) -> open_esp_radio_hil_protocol::PhyRxGainQualityEvidence {
    open_esp_radio_hil_protocol::PhyRxGainQualityEvidence {
        shared_baseband: quality.shared_baseband(),
        wifi_baseband: quality.wifi_baseband(),
        wifi_fine: quality.wifi_fine(),
        wifi_radio: quality.wifi_radio(),
    }
}
