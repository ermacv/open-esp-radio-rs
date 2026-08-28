//! Target-owned wire representation of retained PHY calibration.
//!
//! The driver exposes semantic values only. HIL chooses postcard as its
//! persistence format and the host deliberately treats these bytes as opaque.

use open_esp_radio_esp32s31_phy::{
    PhyBluetoothCalibration, PhyCalibrationCache, PhyCalibrationIdentity, PhyCalibrationSnapshot,
    PhyCommonCalibration, PhyWifiCalibration,
};
use serde::{Deserialize, Serialize};

pub const MAX_ENCODED_LEN: usize = 512;
const MAGIC: [u8; 8] = *b"ORCAL004";

#[derive(Serialize, Deserialize)]
struct Artifact {
    magic: [u8; 8],
    #[serde(with = "Snapshot")]
    calibration: PhyCalibrationSnapshot,
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "PhyCalibrationIdentity")]
struct Identity {
    rf_cal_version: u32,
    base_mac_address: [u8; 6],
    mac_extension: u16,
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "PhyCommonCalibration")]
struct Common {
    temperature: i16,
    sensor_index: u8,
    crystal_selector: u8,
    rc_result: u8,
    filter_dcap: [u8; 5],
    rc_calibrated: bool,
    dcode: [u8; 8],
    i2c_frequency_parameter: u8,
    xtal_duty: [u8; 3],
    clear_tone_after_ready: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "PhyWifiCalibration")]
struct Wifi {
    baseband_calibrated: bool,
    pwdet_calibrated: bool,
    tx_power_calibrated: bool,
    tx_iq_calibrated: bool,
    rx_gain_dc_calibrated: bool,
    rx_gain_tables_initialized: bool,
    rx_saturation_detected: bool,
    tx_dco: [[u16; 4]; 5],
    tx_reference_codes: [i16; 2],
    tx_capacitance: [u8; 6],
    tx_power_curve: [i8; 3],
    tx_power_corrections: [i8; 3],
    tx_power_adjustment: i8,
    calibrated_attenuation: u8,
    tx_iq_config: u16,
    tx_iq_coefficient: u16,
    rx_iq_coefficients: [u16; 4],
    external_dcode: [u8; 2],
    calibration_temperature: i16,
    calibration_channel: u16,
    wifi_rx_table_last_index: u8,
    shared_rx_table_last_index: u8,
    wifi_index_dc: [[u16; 2]; 8],
    wifi_dc_base: [u16; 2],
    shared_index_dc: [[u16; 2]; 11],
    rxbb_dc_adjustments: [[u16; 2]; 6],
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "PhyBluetoothCalibration")]
struct Bluetooth {
    tx_dc_calibrated: bool,
    tx_power_calibrated: bool,
    tx_dco: [[u16; 4]; 3],
    tx_power_curve: [i8; 3],
    tx_power_corrections: [i8; 3],
    tx_power_adjustment: i8,
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "PhyCalibrationSnapshot")]
struct Snapshot {
    schema: u16,
    #[serde(with = "Identity")]
    identity: PhyCalibrationIdentity,
    #[serde(with = "Common")]
    common: PhyCommonCalibration,
    #[serde(with = "Wifi")]
    wifi: PhyWifiCalibration,
    #[serde(with = "Bluetooth")]
    bluetooth: PhyBluetoothCalibration,
}

pub fn decode(bytes: &[u8]) -> Option<PhyCalibrationCache> {
    let artifact: Artifact = postcard::from_bytes(bytes).ok()?;
    if artifact.magic != MAGIC {
        return None;
    }
    PhyCalibrationCache::from_snapshot(artifact.calibration)
}

pub fn encode<'a>(
    cache: &PhyCalibrationCache,
    storage: &'a mut [u8; MAX_ENCODED_LEN],
) -> Option<&'a [u8]> {
    postcard::to_slice(
        &Artifact {
            magic: MAGIC,
            calibration: *cache.snapshot(),
        },
        storage,
    )
    .ok()
    .map(|encoded| &*encoded)
}
