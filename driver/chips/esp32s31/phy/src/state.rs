//! Typed software state retained by the ESP32-S31 PHY.
//!
//! The vendor implementation stores unrelated configuration, calibration and
//! runtime values in one 508-byte `phy_param` byte image.  The Rust driver
//! keeps those values in semantic fields instead.  Vendor offsets belong to
//! qualification code, never to the live radio owner.

use crate::{
    analog::crystal_duty::XtalDutyCalibrationParameters,
    analog::dcode::{PhyDcodeOutcome, PhyDcodeParameters},
    analog::frequency::PhyChannelFrequencyInitControl,
    analog::i2c::{FilterDcapParameters, PhyRfInitPrefixOutcome},
    analog::pbus::memory::PhyPbusMemoryParameters,
    analog::rfpll::{RfpllCapTrackingOutcome, RfpllCapTrackingParameters},
    analog::temperature::PhyTemperatureOutcome,
    calibration::baseband::{
        PHY_RX_TABLE_ENTRY_COUNT, PhyGeneratedRxGainTable, PhyRegisterInitParameters,
        PhyRxGainMemoryParameters, PhyRxTableInitParameters,
    },
    calibration::bluetooth::{
        PhyBluetoothTxDcPwdetTransition, PhyBluetoothTxDcTransition, PhyBluetoothTxGainImage,
        PhyBluetoothTxGainInitOutcome, PhyBluetoothTxGainInitParameters,
        PhyBluetoothTxGainInitTransition, PhyBluetoothTxGainParameters, PhyBluetoothTxPowerOutcome,
        PhyBluetoothTxPowerParameters, PhyBluetoothTxPowerTransition, calculate_bluetooth_tx_gain,
    },
    channel::{
        PhyChipChannelOutcome, PhyChipChannelParameters, PhyWifiTxGainImage, PhyWifiTxGainRequest,
        calculate_wifi_tx_gain,
    },
    rx::gain::{PhyRxGainInitOutcome, PhyRxGainInitParameters},
    rx::gain_calibration::{PhyRxGainDcOutcome, PhyRxGainDcParameters},
    rx::iq::{PhyRxIqAdjustedTxParameters, PhyRxIqInitOutcome, PhyRxIqInitParameters},
    rx::saturation::PhyRxSaturationOutcome,
    tracking::calibration::{PhyCalibrationTrackingOutcome, PhyCalibrationTrackingParameters},
    tracking::i2c::{
        PhyWifiI2cTrackingBand, PhyWifiI2cTrackingOutcome, PhyWifiI2cTrackingParameters,
    },
    tracking::power::{PhyTxPowerTrackingOutcome, PhyTxPowerTrackingParameters},
    tx::calibration::{PhyTxCalibrationParameters, PhyTxCapOutcome, PhyTxCapParameters},
    tx::dc_offset::{PhyTxDcOutcome, PhyTxDcParameters},
    tx::dc_power_detector::{PhyTxDcPwdetOutcome, PhyTxDcPwdetParameters},
    tx::iq::{PhyTxIqInitOutcome, PhyTxIqInitParameters},
    tx::power::{
        PHY_TX_TARGET_POWER_COUNT, PhyTxPowerOutcome, PhyTxPowerParameters, PhyTxTargetPowerProfile,
    },
    tx::power_detector::{PhyPwdetOutcome, PhyPwdetParameters},
};

const fn saturate_phy_value(value: i32, upper: u8, lower: u8) -> u8 {
    if value < lower as i32 {
        lower
    } else if value > upper as i32 {
        upper
    } else {
        value as u8
    }
}

/// Immutable parameters selected for one ESP32-S31 radio instance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyConfig {
    pbus_rx_path: u8,
    tone_selector: u8,
    tx_power_path: u8,
    bluetooth_power_path: u8,
    bluetooth_tx_path: u8,
    power_offset: i16,
    initial_attenuation: u8,
    tx_gain_attenuation: u8,
    tx_gain_base: u8,
    bluetooth_tx_gain_base: u8,
    target_power_maximum: i8,
    target_power: [i8; PHY_TX_TARGET_POWER_COUNT],
    regulatory_override: bool,
    target_adjustment: u8,
    channel_14_mic_enabled: bool,
    tx_gain_skip_publication: bool,
}

impl PhyConfig {
    /// Initial source-owned S31 configuration before board policy clamps TX.
    pub const fn esp32s31_default() -> Self {
        Self {
            pbus_rx_path: 0xbf,
            tone_selector: 0x20,
            tx_power_path: 0x1f,
            bluetooth_power_path: 0x16,
            bluetooth_tx_path: 1,
            power_offset: 0x160,
            initial_attenuation: 0x50,
            tx_gain_attenuation: 0,
            tx_gain_base: 0,
            bluetooth_tx_gain_base: 0,
            target_power_maximum: 0x54,
            target_power: [0; PHY_TX_TARGET_POWER_COUNT],
            regulatory_override: false,
            target_adjustment: 0,
            channel_14_mic_enabled: false,
            tx_gain_skip_publication: false,
        }
    }

    /// Qualified production defaults formerly selected from the vendor init
    /// profile. Unused bytes from that profile are deliberately absent.
    pub const fn production() -> Self {
        Self::esp32s31_default().with_target_power(
            0x54,
            [
                0x50, 0x50, 0x50, 0x50, 0x4c, 0x48, 0x50, 0x50, 0x4c, 0x48, 0x40, 0x3c, 0x3c, 0x3c,
                0x4c, 0x4c, 0x48, 0x44,
            ],
            false,
        )
    }

    pub const fn with_target_power(
        mut self,
        maximum: i8,
        target: [i8; PHY_TX_TARGET_POWER_COUNT],
        regulatory_override: bool,
    ) -> Self {
        self.target_power_maximum = maximum;
        self.target_power = target;
        self.regulatory_override = regulatory_override;
        self
    }
}

impl Default for PhyConfig {
    fn default() -> Self {
        Self::esp32s31_default()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CommonPhyState {
    temperature: i16,
    rfpll_tracking_temperature: i16,
    calibration_tracking_temperature: i16,
    tracking_temperature: i16,
    tracking_gain_base: i8,
    sensor_index: u8,
    crystal_selector: u8,
    rc_result: u8,
    filter_dcap: [u8; 5],
    rc_calibrated: bool,
    dcode: [u8; 8],
    i2c_frequency_parameter: u8,
    xtal_duty: [u8; 3],
    frequency_table_initialized: bool,
    front_end_parameter: bool,
    clear_tone_after_ready: bool,
    initialization_parameter: bool,
    registered: bool,
    temperature_debug: [u8; 2],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct WifiPhyState {
    dot11p_enabled: u8,
    dot11p_configuration: u8,
    current_level: u8,
    tx_power_tracking_slow: u8,
    tx_i2c_tracking_band: PhyWifiI2cTrackingBand,
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
    tx_iq_config: u16,
    tx_iq_coefficient: u16,
    rx_iq_coefficients: [u16; 4],
    external_dcode: [u8; 2],
    calibration_temperature: i16,
    txdc_tracking_temperature: i16,
    current_channel: u16,
    channel_initialized: bool,
    channel_bandwidth: u8,
    wifi_rx_table_last_index: u8,
    shared_rx_table_last_index: u8,
    wifi_index_dc: [[u16; 2]; 8],
    wifi_dc_base: [u16; 2],
    shared_index_dc: [[u16; 2]; 11],
    rxbb_dc_adjustments: [[u16; 2]; 6],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BluetoothPhyState {
    power_tracking: u8,
    tx_dc_calibrated: bool,
    tx_power_calibrated: bool,
    tx_dco: [[u16; 4]; 3],
    tx_power_curve: [i8; 3],
    tx_power_corrections: [i8; 3],
    tx_power_adjustment: i8,
    txdc_tracking_temperature: i16,
    channel_base: u8,
}

/// Unique owner of all software state required after PHY registration.
///
/// The type is intentionally neither `Copy` nor `Clone`: moving it transfers
/// the authority to update the calibrated radio state.
pub struct PhyState {
    config: PhyConfig,
    common: CommonPhyState,
    wifi: WifiPhyState,
    bluetooth: BluetoothPhyState,
}

/// Typed retained calibration for one physical ESP32-S31 radio.
///
/// This is deliberately not `Copy` or `Clone`: persistence code must move the
/// one cache value instead of duplicating a former vendor memory image.
pub struct PhyCalibrationCache {
    snapshot: PhyCalibrationSnapshot,
}

/// Stable, value-only boundary used by caller-selected persistence backends.
///
/// This is not a memory image of vendor state. Each field is retained because
/// a named calibration consumer reads it after a cold hardware reset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyCalibrationSnapshot {
    pub schema: u16,
    pub identity: crate::calibration::registration::PhyCalibrationIdentity,
    pub common: PhyCommonCalibration,
    pub wifi: PhyWifiCalibration,
    pub bluetooth: PhyBluetoothCalibration,
}

pub const PHY_CALIBRATION_SNAPSHOT_SCHEMA: u16 = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyCommonCalibration {
    pub temperature: i16,
    pub sensor_index: u8,
    pub crystal_selector: u8,
    pub rc_result: u8,
    pub filter_dcap: [u8; 5],
    pub rc_calibrated: bool,
    pub dcode: [u8; 8],
    pub i2c_frequency_parameter: u8,
    pub xtal_duty: [u8; 3],
    pub clear_tone_after_ready: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyWifiCalibration {
    pub baseband_calibrated: bool,
    pub pwdet_calibrated: bool,
    pub tx_power_calibrated: bool,
    pub tx_iq_calibrated: bool,
    pub rx_gain_dc_calibrated: bool,
    pub rx_gain_tables_initialized: bool,
    pub rx_saturation_detected: bool,
    pub tx_dco: [[u16; 4]; 5],
    pub tx_reference_codes: [i16; 2],
    pub tx_capacitance: [u8; 6],
    pub tx_power_curve: [i8; 3],
    pub tx_power_corrections: [i8; 3],
    pub tx_power_adjustment: i8,
    pub calibrated_attenuation: u8,
    pub tx_iq_config: u16,
    pub tx_iq_coefficient: u16,
    pub rx_iq_coefficients: [u16; 4],
    pub external_dcode: [u8; 2],
    pub calibration_temperature: i16,
    pub calibration_channel: u16,
    pub wifi_rx_table_last_index: u8,
    pub shared_rx_table_last_index: u8,
    pub wifi_index_dc: [[u16; 2]; 8],
    pub wifi_dc_base: [u16; 2],
    pub shared_index_dc: [[u16; 2]; 11],
    pub rxbb_dc_adjustments: [[u16; 2]; 6],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyBluetoothCalibration {
    pub tx_dc_calibrated: bool,
    pub tx_power_calibrated: bool,
    pub tx_dco: [[u16; 4]; 3],
    pub tx_power_curve: [i8; 3],
    pub tx_power_corrections: [i8; 3],
    pub tx_power_adjustment: i8,
}

impl PhyCalibrationCache {
    pub(crate) const fn capture(
        identity: crate::calibration::registration::PhyCalibrationIdentity,
        state: &PhyState,
    ) -> Self {
        Self {
            snapshot: state.calibration_snapshot(identity),
        }
    }

    pub const fn from_snapshot(snapshot: PhyCalibrationSnapshot) -> Option<Self> {
        if snapshot.schema == PHY_CALIBRATION_SNAPSHOT_SCHEMA {
            Some(Self { snapshot })
        } else {
            None
        }
    }

    pub const fn snapshot(&self) -> &PhyCalibrationSnapshot {
        &self.snapshot
    }

    pub const fn into_snapshot(self) -> PhyCalibrationSnapshot {
        self.snapshot
    }

    pub const fn identity(&self) -> crate::calibration::registration::PhyCalibrationIdentity {
        self.snapshot.identity
    }

    pub fn matches(
        &self,
        identity: crate::calibration::registration::PhyCalibrationIdentity,
    ) -> bool {
        self.snapshot.schema == PHY_CALIBRATION_SNAPSHOT_SCHEMA
            && self.snapshot.identity == identity
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyDot11pConfiguration {
    pub enabled: u8,
    pub configuration: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTemperatureTrackingDebug {
    pub first: u8,
    pub second: u8,
}

impl PhyTemperatureTrackingDebug {
    /// Optional RFPLL-cap threshold selected by the debug policy's first flag.
    pub const fn rfpll_threshold_override(self) -> Option<u8> {
        if self.first & 1 != 0 {
            Some(self.second)
        } else {
            None
        }
    }

    /// Optional calibration threshold selected independently by the second
    /// debug-policy flag. Both children intentionally share the same value.
    pub const fn calibration_threshold_override(self) -> Option<u8> {
        if self.first & 2 != 0 {
            Some(self.second)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRegisterTemperatureControl {
    update_registration_reference: bool,
    update_calibration_reference: bool,
}

impl PhyRegisterTemperatureControl {
    pub const FULL: Self = Self {
        update_registration_reference: true,
        update_calibration_reference: true,
    };

    pub const fn updates_offset_130(self) -> bool {
        self.update_registration_reference
    }

    pub const fn updates_reference_copies(self) -> bool {
        self.update_calibration_reference
    }
}

impl PhyState {
    pub const fn new(config: PhyConfig) -> Self {
        Self {
            config,
            common: CommonPhyState {
                temperature: 0,
                rfpll_tracking_temperature: 0,
                calibration_tracking_temperature: 0,
                tracking_temperature: 0,
                tracking_gain_base: 0,
                sensor_index: 2,
                crystal_selector: 0,
                rc_result: 0,
                filter_dcap: [0; 5],
                rc_calibrated: false,
                dcode: [0; 8],
                i2c_frequency_parameter: 0,
                xtal_duty: [0; 3],
                frequency_table_initialized: false,
                front_end_parameter: true,
                clear_tone_after_ready: false,
                initialization_parameter: false,
                registered: false,
                temperature_debug: [0; 2],
            },
            wifi: WifiPhyState {
                dot11p_enabled: 0,
                dot11p_configuration: 0,
                current_level: 0,
                tx_power_tracking_slow: 1,
                tx_i2c_tracking_band: PhyWifiI2cTrackingBand::Nominal,
                baseband_calibrated: false,
                pwdet_calibrated: false,
                tx_power_calibrated: false,
                tx_iq_calibrated: false,
                rx_gain_dc_calibrated: false,
                rx_gain_tables_initialized: false,
                rx_saturation_detected: false,
                tx_dco: [[0; 4]; 5],
                tx_reference_codes: [0; 2],
                tx_capacitance: [0; 6],
                tx_power_curve: [0; 3],
                tx_power_corrections: [0; 3],
                tx_power_adjustment: 0,
                tx_iq_config: 0,
                tx_iq_coefficient: 0,
                rx_iq_coefficients: [0; 4],
                external_dcode: [0; 2],
                calibration_temperature: 0,
                txdc_tracking_temperature: 0,
                current_channel: 0,
                channel_initialized: false,
                channel_bandwidth: 0,
                wifi_rx_table_last_index: 0,
                shared_rx_table_last_index: 0,
                wifi_index_dc: [[0; 2]; 8],
                wifi_dc_base: [0; 2],
                shared_index_dc: [[0; 2]; 11],
                rxbb_dc_adjustments: [[0; 2]; 6],
            },
            bluetooth: BluetoothPhyState {
                power_tracking: 1,
                tx_dc_calibrated: false,
                tx_power_calibrated: false,
                tx_dco: [[0; 4]; 3],
                tx_power_curve: [0; 3],
                tx_power_corrections: [0; 3],
                tx_power_adjustment: 0,
                txdc_tracking_temperature: 0,
                channel_base: 0,
            },
        }
    }

    pub const fn config(&self) -> &PhyConfig {
        &self.config
    }

    pub fn set_tx_power_tracking_slow(&mut self, value: u8) {
        self.wifi.tx_power_tracking_slow = value;
    }

    pub const fn tx_power_tracking_slow(&self) -> u8 {
        self.wifi.tx_power_tracking_slow
    }

    pub const fn wifi_i2c_tracking_parameters(&self) -> PhyWifiI2cTrackingParameters {
        PhyWifiI2cTrackingParameters {
            current_temperature: self.common.temperature,
            previous_band: self.wifi.tx_i2c_tracking_band,
        }
    }

    pub fn apply_wifi_i2c_tracking_outcome(&mut self, outcome: PhyWifiI2cTrackingOutcome) {
        self.wifi.tx_i2c_tracking_band = outcome.band;
    }

    pub fn set_dot11p_configuration(&mut self, enabled: u8, configuration: u8) {
        self.wifi.dot11p_enabled = enabled;
        self.wifi.dot11p_configuration = configuration;
    }

    pub const fn dot11p_configuration(&self) -> PhyDot11pConfiguration {
        PhyDot11pConfiguration {
            enabled: self.wifi.dot11p_enabled,
            configuration: self.wifi.dot11p_configuration,
        }
    }

    pub fn set_current_level(&mut self, value: u8) {
        self.wifi.current_level = value;
    }

    pub const fn current_level(&self) -> u8 {
        self.wifi.current_level
    }

    pub fn set_bt_power_tracking(&mut self, value: u8) {
        self.bluetooth.power_tracking = value;
    }

    pub const fn bt_power_tracking(&self) -> u8 {
        self.bluetooth.power_tracking
    }

    pub const fn bluetooth_tx_dc_transition(&self) -> PhyBluetoothTxDcTransition {
        PhyBluetoothTxDcTransition::new(
            PhyTxDcParameters {
                pbus_rx_path_value: self.config.pbus_rx_path,
            },
            self.config.bluetooth_tx_path,
        )
    }

    pub fn apply_bluetooth_tx_dc_outcome(&mut self, outcome: PhyTxDcOutcome) {
        let mut row = 0;
        while row != self.bluetooth.tx_dco.len() {
            self.bluetooth.tx_dco[row] = outcome.dco[row];
            row += 1;
        }
        self.bluetooth.tx_dc_calibrated = true;
    }

    pub const fn bluetooth_tx_dc_calibrated(&self) -> bool {
        self.bluetooth.tx_dc_calibrated
    }

    #[cfg(test)]
    pub(crate) const fn bluetooth_tx_dco(&self) -> [[u16; 4]; 3] {
        self.bluetooth.tx_dco
    }

    pub const fn bluetooth_tx_dc_pwdet_transition(&self) -> PhyBluetoothTxDcPwdetTransition {
        PhyBluetoothTxDcPwdetTransition::new(
            PhyTxDcPwdetParameters {
                dco: self.bluetooth.tx_dco,
                clear_tone_after_ready: self.common.clear_tone_after_ready,
            },
            self.config.bluetooth_tx_path,
        )
    }

    pub fn apply_bluetooth_tx_dc_pwdet_outcome(&mut self, outcome: PhyTxDcPwdetOutcome) {
        self.bluetooth.tx_dco = outcome.dco;
    }

    pub fn bluetooth_tx_power_parameters(&self) -> PhyBluetoothTxPowerParameters {
        let mut calibration = self.tx_power_parameters();
        calibration.already_calibrated = self.bluetooth.tx_power_calibrated;
        PhyBluetoothTxPowerParameters {
            calibration,
            pbus_power_path_value: self.config.bluetooth_power_path,
            pbus_tx_path_value: self.config.bluetooth_tx_path,
            dco: self.bluetooth.tx_dco[0],
            tone_selector: self.config.tone_selector as u16,
        }
    }

    pub fn bluetooth_tx_power_transition(&self) -> PhyBluetoothTxPowerTransition {
        PhyBluetoothTxPowerTransition::new(self.bluetooth_tx_power_parameters())
    }

    pub fn apply_bluetooth_tx_power_outcome(&mut self, outcome: PhyBluetoothTxPowerOutcome) {
        let calibration = outcome.calibration;
        if !calibration.calibration_performed {
            return;
        }
        self.bluetooth.tx_power_corrections = calibration.point_corrections;
        self.bluetooth.tx_power_curve = calibration.power_curve;
        self.bluetooth.tx_power_adjustment = calibration.power_adjustment;
        self.config.initial_attenuation = calibration.final_attenuation;
        self.common.clear_tone_after_ready = false;
        self.bluetooth.tx_power_calibrated = true;
    }

    pub const fn bluetooth_tx_power_calibrated(&self) -> bool {
        self.bluetooth.tx_power_calibrated
    }

    #[cfg(test)]
    pub(crate) const fn bluetooth_tx_power_result(&self) -> ([i8; 3], [i8; 3], i8) {
        (
            self.bluetooth.tx_power_curve,
            self.bluetooth.tx_power_corrections,
            self.bluetooth.tx_power_adjustment,
        )
    }

    const fn packed_seed(rows: &[[u16; 4]; 3]) -> [u32; 6] {
        let mut seed = [0; 6];
        let mut index = 0;
        while index != seed.len() {
            let first = rows[index / 2][(index % 2) * 2];
            let second = rows[index / 2][(index % 2) * 2 + 1];
            seed[index] = first as u32 | ((second as u32) << 16);
            index += 1;
        }
        seed
    }

    pub const fn bluetooth_tx_gain_parameters(&self) -> PhyBluetoothTxGainParameters {
        PhyBluetoothTxGainParameters {
            seed: Self::packed_seed(&self.bluetooth.tx_dco),
            config: self.wifi.tx_iq_config,
            calibration_curve: [
                self.bluetooth.tx_power_curve[0] as u8,
                self.bluetooth.tx_power_curve[1] as u8,
                self.bluetooth.tx_power_curve[2] as u8,
            ],
            correction: self.bluetooth.tx_power_adjustment,
            base: self.config.bluetooth_tx_gain_base,
            attenuation: self.config.tx_gain_attenuation,
        }
    }

    pub const fn bluetooth_tx_gain_image(&self) -> PhyBluetoothTxGainImage {
        calculate_bluetooth_tx_gain(self.bluetooth_tx_gain_parameters())
    }

    /// Regenerate the shared Bluetooth/IEEE 802.15.4 gain bank from the live
    /// calibration state and one temperature-tracked base value.
    pub const fn bluetooth_ieee802154_tracking_gain_image(
        &self,
        gain_base: i8,
    ) -> PhyBluetoothTxGainImage {
        let mut parameters = self.bluetooth_tx_gain_parameters();
        parameters.base = gain_base as u8;
        calculate_bluetooth_tx_gain(parameters)
    }

    /// Regenerate the shared calibration gain bank from a just-completed
    /// TXDC/PWDET result that has not yet reached the outer state commit.
    pub const fn bluetooth_ieee802154_calibration_gain_image(
        &self,
        tx_dc_pwdet: PhyTxDcPwdetOutcome,
    ) -> PhyBluetoothTxGainImage {
        let mut parameters = self.bluetooth_tx_gain_parameters();
        parameters.seed = Self::packed_seed(&tx_dc_pwdet.dco);
        calculate_bluetooth_tx_gain(parameters)
    }

    pub fn bluetooth_tx_gain_init_transition(&self) -> PhyBluetoothTxGainInitTransition {
        PhyBluetoothTxGainInitTransition::new(PhyBluetoothTxGainInitParameters {
            crystal_selector: self.common.crystal_selector,
            capacitance: self.wifi.tx_capacitance,
            tx_dc_calibrated: self.bluetooth.tx_dc_calibrated,
            tx_dc: PhyTxDcParameters {
                pbus_rx_path_value: self.config.pbus_rx_path,
            },
            tx_path_value: self.config.bluetooth_tx_path,
            tx_power: self.bluetooth_tx_power_parameters(),
            tx_dc_pwdet: PhyTxDcPwdetParameters {
                dco: self.bluetooth.tx_dco,
                clear_tone_after_ready: self.common.clear_tone_after_ready,
            },
            gain: self.bluetooth_tx_gain_parameters(),
        })
    }

    pub fn apply_bluetooth_tx_gain_init_outcome(&mut self, outcome: PhyBluetoothTxGainInitOutcome) {
        self.bluetooth.tx_dc_calibrated = outcome.tx_dc_calibrated;
        self.bluetooth.tx_dco = outcome.dco;
        self.apply_bluetooth_tx_power_outcome(outcome.tx_power);
    }

    pub fn set_ble_channel_base(&mut self, value: u8) {
        self.bluetooth.channel_base = value;
    }

    pub const fn ble_channel_base(&self) -> u8 {
        self.bluetooth.channel_base
    }

    pub fn set_initialization_parameter(&mut self, value: u32) {
        self.common.initialization_parameter = value & 1 != 0;
    }

    pub const fn initialization_parameter(&self) -> bool {
        self.common.initialization_parameter
    }

    pub fn set_temperature_tracking_debug(&mut self, first: u8, second: u8) {
        self.common.temperature_debug = [first, second];
    }

    pub const fn temperature_tracking_debug(&self) -> PhyTemperatureTrackingDebug {
        PhyTemperatureTrackingDebug {
            first: self.common.temperature_debug[0],
            second: self.common.temperature_debug[1],
        }
    }

    pub const fn tx_target_power_profile(&self) -> PhyTxTargetPowerProfile {
        PhyTxTargetPowerProfile::new(
            self.config.target_power_maximum,
            self.config.target_power,
            self.config.regulatory_override,
        )
    }

    pub const fn baseband_calibration_complete(&self) -> bool {
        self.wifi.baseband_calibrated
    }

    pub fn mark_baseband_calibration_complete(&mut self) {
        self.wifi.baseband_calibrated = true;
    }

    pub const fn disable_wifi_after_baseband_init(&self) -> bool {
        self.common.initialization_parameter
    }

    pub const fn register_init_parameters(&self) -> PhyRegisterInitParameters {
        PhyRegisterInitParameters {
            parameter_121: self.wifi.wifi_rx_table_last_index,
            parameter_120: self.wifi.shared_rx_table_last_index,
        }
    }

    pub fn prepare_rx_table_init(&mut self) -> PhyRxTableInitParameters {
        self.wifi.wifi_rx_table_last_index = PHY_RX_TABLE_ENTRY_COUNT;
        self.wifi.shared_rx_table_last_index = PHY_RX_TABLE_ENTRY_COUNT;
        PhyRxTableInitParameters {
            parameter_002: self.config.pbus_rx_path,
            parameter_121: PHY_RX_TABLE_ENTRY_COUNT,
        }
    }

    pub const fn rx_gain_memory_parameters(&self) -> PhyRxGainMemoryParameters {
        PhyRxGainMemoryParameters {
            parameter_002: self.config.pbus_rx_path,
            wifi_index_dc: self.wifi.wifi_index_dc,
            wifi_dc_base: self.wifi.wifi_dc_base,
            shared_index_dc: self.wifi.shared_index_dc,
            rxbb_dc_adjustments: self.wifi.rxbb_dc_adjustments,
            wifi_auxiliary: self.wifi.rx_iq_coefficients[0],
        }
    }

    pub const fn rx_gain_dc_parameters(&self) -> PhyRxGainDcParameters {
        PhyRxGainDcParameters {
            crystal_selector: self.common.crystal_selector,
            pbus_rx_path_value: self.config.pbus_rx_path,
            rx_saturation_detected: self.wifi.rx_saturation_detected,
        }
    }

    pub fn apply_rx_gain_dc_outcome(&mut self, outcome: PhyRxGainDcOutcome) {
        self.wifi.wifi_index_dc = outcome.wifi_index_dc;
        self.wifi.wifi_dc_base = outcome.wifi_dc_base;
        self.wifi.shared_index_dc = outcome.shared_index_dc;
        self.wifi.rxbb_dc_adjustments = outcome.rxbb_dc_adjustments;
    }

    pub const fn rx_gain_init_parameters(&self) -> PhyRxGainInitParameters {
        PhyRxGainInitParameters {
            dc_calibrated: self.wifi.rx_gain_dc_calibrated,
            tables_initialized: self.wifi.rx_gain_tables_initialized,
            dc: self.rx_gain_dc_parameters(),
            memory: self.rx_gain_memory_parameters(),
        }
    }

    pub fn apply_rx_gain_init_outcome(&mut self, outcome: PhyRxGainInitOutcome) {
        if let Some(dc) = outcome.dc {
            self.apply_rx_gain_dc_outcome(dc);
            self.wifi.rx_gain_dc_calibrated = true;
        }
        if outcome.generated_tables {
            self.wifi.wifi_rx_table_last_index = outcome.wifi_last_index.min(0x4f);
            self.wifi.shared_rx_table_last_index = outcome.shared_last_index.min(0x4f);
            self.wifi.rx_gain_tables_initialized = true;
        }
    }

    pub fn apply_generated_rx_gain_tables(
        &mut self,
        wifi: PhyGeneratedRxGainTable,
        shared: PhyGeneratedRxGainTable,
    ) {
        self.wifi.wifi_rx_table_last_index = wifi.last_index.min(PHY_RX_TABLE_ENTRY_COUNT);
        self.wifi.shared_rx_table_last_index = shared.last_index.min(PHY_RX_TABLE_ENTRY_COUNT);
        self.wifi.rx_gain_tables_initialized = true;
    }

    pub const fn rx_saturation_parameter_002(&self) -> u8 {
        self.config.pbus_rx_path
    }

    pub const fn pbus_memory_parameters(&self) -> PhyPbusMemoryParameters {
        PhyPbusMemoryParameters {
            parameter_002: self.config.pbus_rx_path,
            parameter_014: self.config.bluetooth_tx_path,
        }
    }

    pub fn apply_temperature_outcome(&mut self, outcome: PhyTemperatureOutcome) {
        self.common.temperature = outcome.temperature;
        self.common.sensor_index = outcome.sensor_index;
    }

    /// Project the semantic state consumed by periodic RFPLL-cap tracking.
    pub const fn rfpll_cap_tracking_parameters(
        &self,
        threshold_override: Option<u8>,
    ) -> RfpllCapTrackingParameters {
        RfpllCapTrackingParameters {
            current_temperature: self.common.temperature,
            reference_temperature: self.common.rfpll_tracking_temperature,
            threshold_override,
            current_channel: self.wifi.current_channel,
        }
    }

    /// Commit only the reference-temperature effect of a terminal RFPLL
    /// tracking transaction.
    pub fn apply_rfpll_cap_tracking_outcome(&mut self, outcome: RfpllCapTrackingOutcome) {
        if outcome.updated {
            self.common.rfpll_tracking_temperature = outcome.reference_temperature;
        }
    }

    /// Project the three independent semantic references consumed by
    /// `phy_cal_param_track`.
    pub const fn calibration_tracking_parameters(
        &self,
        threshold_override: Option<u8>,
    ) -> PhyCalibrationTrackingParameters {
        PhyCalibrationTrackingParameters {
            current_temperature: self.common.temperature,
            common_reference_temperature: self.common.calibration_tracking_temperature,
            wifi_reference_temperature: self.wifi.txdc_tracking_temperature,
            bluetooth_ieee802154_reference_temperature: self.bluetooth.txdc_tracking_temperature,
            threshold_override,
            current_channel: self.wifi.current_channel,
            channel_bandwidth: self.wifi.channel_bandwidth,
            crystal_selector: self.common.crystal_selector,
        }
    }

    /// Commit only references whose complete calibration branch reached its
    /// terminal restore edge.
    pub fn apply_calibration_tracking_outcome(&mut self, outcome: PhyCalibrationTrackingOutcome) {
        let common = match (
            outcome.common_updated,
            outcome.dcode,
            outcome.rx_gain,
            outcome.channel,
        ) {
            (true, Some(dcode), Some(rx_gain), Some(channel)) => {
                // The periodic parent forcibly clears both vendor completion
                // guards. Its successful child must therefore contain fresh
                // RX-DC data and freshly generated tables. The channel child
                // also refreshes temperature before the vendor stores the
                // common tracking reference.
                if rx_gain.dc.is_none()
                    || !rx_gain.generated_tables
                    || channel.temperature.temperature != outcome.common_reference_temperature
                {
                    return;
                }
                Some((dcode, rx_gain, channel))
            }
            (false, None, None, None) => None,
            // A terminal common branch publishes all three values atomically.
            // Reject structurally inconsistent caller-created outcomes.
            _ => return,
        };
        let class_tx_dc_pwdet = match (outcome.class_updated, outcome.tx_dc_pwdet) {
            (true, Some(tx_dc_pwdet)) => Some(tx_dc_pwdet),
            (false, None) => None,
            _ => return,
        };

        if let Some((dcode, rx_gain, channel)) = common {
            self.apply_dcode_outcome(dcode);
            self.apply_rx_gain_init_outcome(rx_gain);
            self.apply_channel_outcome(channel);
            self.common.calibration_tracking_temperature = outcome.common_reference_temperature;
        }
        if let Some(tx_dc_pwdet) = class_tx_dc_pwdet {
            match outcome.class {
                crate::tracking::parameters::PhyCalibrationTrackClass::Wifi => {
                    self.apply_tx_dc_pwdet_outcome(tx_dc_pwdet);
                    self.wifi.txdc_tracking_temperature = outcome.wifi_reference_temperature;
                }
                crate::tracking::parameters::PhyCalibrationTrackClass::BluetoothIeee802154 => {
                    self.apply_bluetooth_tx_dc_pwdet_outcome(tx_dc_pwdet);
                    self.bluetooth.txdc_tracking_temperature =
                        outcome.bluetooth_ieee802154_reference_temperature;
                }
            }
        }
    }

    /// Project the semantic state consumed by periodic TX-power tracking.
    ///
    /// `relaxed_threshold` is supplied by the reviewed outer tracking policy;
    /// it is not persisted as an unexplained vendor parameter byte.
    pub const fn tx_power_tracking_parameters(
        &self,
        relaxed_threshold: bool,
    ) -> PhyTxPowerTrackingParameters {
        PhyTxPowerTrackingParameters {
            current_temperature: self.common.temperature,
            reference_temperature: self.wifi.calibration_temperature,
            previous_tracking_temperature: self.common.tracking_temperature,
            previous_tracking_gain_base: self.common.tracking_gain_base,
            wifi_gain_base: self.config.tx_gain_base as i8,
            bluetooth_ieee802154_gain_base: self.config.bluetooth_tx_gain_base as i8,
            relaxed_threshold,
        }
    }

    /// Commit a completed tracking transaction to the live radio owner.
    pub fn apply_tx_power_tracking_outcome(&mut self, outcome: PhyTxPowerTrackingOutcome) {
        if !outcome.gain_updated {
            return;
        }
        self.common.tracking_temperature = outcome.tracking_temperature;
        self.common.tracking_gain_base = outcome.tracking_gain_base;
        match outcome.class {
            crate::tracking::parameters::PhyCalibrationTrackClass::Wifi => {
                self.config.tx_gain_base = outcome.wifi_gain_base as u8;
            }
            crate::tracking::parameters::PhyCalibrationTrackClass::BluetoothIeee802154 => {
                self.config.bluetooth_tx_gain_base = outcome.bluetooth_ieee802154_gain_base as u8;
            }
        }
    }

    pub const fn dcode_parameters(&self) -> PhyDcodeParameters {
        PhyDcodeParameters {
            crystal_selector: self.common.crystal_selector,
        }
    }

    pub fn apply_dcode_outcome(&mut self, outcome: PhyDcodeOutcome) {
        self.common.dcode = outcome.codes;
    }

    pub const fn pwdet_parameters(&self) -> PhyPwdetParameters {
        PhyPwdetParameters {
            already_calibrated: self.wifi.pwdet_calibrated,
            pbus_tx_path_value: self.config.tx_power_path,
            pbus_rx_path_value: self.config.pbus_rx_path,
            dco: self.wifi.tx_dco[0],
            clear_tone_after_ready: self.common.clear_tone_after_ready,
            reference_codes: self.wifi.tx_reference_codes,
        }
    }

    pub fn apply_pwdet_outcome(&mut self, outcome: PhyPwdetOutcome) {
        self.wifi.tx_reference_codes = outcome.reference_codes;
        self.wifi.pwdet_calibrated |= outcome.calibrated;
    }

    pub const fn tx_dc_parameters(&self) -> PhyTxDcParameters {
        PhyTxDcParameters {
            pbus_rx_path_value: self.config.pbus_rx_path,
        }
    }

    pub fn apply_tx_dc_outcome(&mut self, outcome: PhyTxDcOutcome) {
        self.wifi.tx_dco = outcome.dco;
    }

    pub const fn tx_cap_parameters(&self) -> PhyTxCapParameters {
        PhyTxCapParameters {
            crystal_selector: self.common.crystal_selector,
            environment: PhyTxCalibrationParameters {
                pbus_tx_path_value: self.config.tx_power_path,
                pbus_rx_path_value: self.config.pbus_rx_path,
                dco: self.wifi.tx_dco[0],
            },
            clear_tone_after_ready: self.common.clear_tone_after_ready,
            reference_codes: self.wifi.tx_reference_codes,
            power_offset: self.config.power_offset,
            initial_attenuation: self.config.initial_attenuation,
        }
    }

    pub fn apply_tx_cap_outcome(&mut self, outcome: PhyTxCapOutcome) {
        self.wifi.tx_capacitance = outcome.capacitance;
        self.config.initial_attenuation = outcome.attenuation;
    }

    pub const fn tx_power_parameters(&self) -> PhyTxPowerParameters {
        PhyTxPowerParameters {
            already_calibrated: self.wifi.tx_power_calibrated,
            crystal_selector: self.common.crystal_selector,
            environment: PhyTxCalibrationParameters {
                pbus_tx_path_value: self.config.tx_power_path,
                pbus_rx_path_value: self.config.pbus_rx_path,
                dco: self.wifi.tx_dco[0],
            },
            capacitance: self.wifi.tx_capacitance,
            target_adjustment: self.config.target_adjustment,
            power_offset: self.config.power_offset,
            initial_attenuation: self.config.initial_attenuation,
            clear_tone_after_ready: self.common.clear_tone_after_ready,
            reference_codes: self.wifi.tx_reference_codes,
        }
    }

    pub fn apply_tx_power_outcome(&mut self, outcome: PhyTxPowerOutcome) {
        if !outcome.calibration_performed {
            return;
        }
        self.wifi.tx_reference_codes = outcome.reference_codes;
        self.wifi.tx_power_curve = outcome.power_curve;
        self.wifi.tx_power_corrections = outcome.point_corrections;
        self.wifi.tx_power_adjustment = outcome.power_adjustment;
        self.config.initial_attenuation = outcome.final_attenuation;
        self.common.clear_tone_after_ready = false;
        self.wifi.tx_power_calibrated = true;
        self.wifi.current_channel = outcome.current_channel;
    }

    pub const fn tx_dc_pwdet_parameters(&self) -> PhyTxDcPwdetParameters {
        PhyTxDcPwdetParameters {
            dco: [
                self.wifi.tx_dco[0],
                self.wifi.tx_dco[1],
                self.wifi.tx_dco[2],
            ],
            clear_tone_after_ready: self.common.clear_tone_after_ready,
        }
    }

    pub fn apply_tx_dc_pwdet_outcome(&mut self, outcome: PhyTxDcPwdetOutcome) {
        self.wifi.tx_dco[0] = outcome.dco[0];
        self.wifi.tx_dco[1] = outcome.dco[1];
        self.wifi.tx_dco[2] = outcome.dco[2];
    }

    pub const fn tx_iq_parameters(&self) -> PhyTxIqInitParameters {
        PhyTxIqInitParameters {
            already_calibrated: self.wifi.tx_iq_calibrated,
            crystal_selector: self.common.crystal_selector,
            environment: PhyTxCalibrationParameters {
                pbus_tx_path_value: self.config.tx_power_path,
                pbus_rx_path_value: self.config.pbus_rx_path,
                dco: self.wifi.tx_dco[0],
            },
            capacitance: self.wifi.tx_capacitance,
            channel_6_dcode: [self.common.dcode[2], self.common.dcode[3]],
            initial_attenuation: self.config.initial_attenuation as i8,
            power_offset: self.config.power_offset,
            reference_codes: self.wifi.tx_reference_codes,
            clear_tone_after_ready: self.common.clear_tone_after_ready,
        }
    }

    pub fn apply_tx_iq_outcome(&mut self, outcome: PhyTxIqInitOutcome) {
        if !outcome.calibration_performed {
            return;
        }
        self.wifi.external_dcode = outcome.external_dcode;
        self.wifi.tx_iq_config = outcome.coefficient[0];
        self.wifi.tx_iq_coefficient = outcome.coefficient[1];
        if let Some(temperature) = outcome.temperature {
            self.wifi.calibration_temperature = temperature.temperature;
            self.apply_temperature_outcome(temperature);
        }
        self.wifi.tx_iq_calibrated = true;
    }

    pub const fn rx_iq_parameters(&self) -> PhyRxIqInitParameters {
        PhyRxIqInitParameters {
            crystal_selector: self.common.crystal_selector,
            pbus_rx_path_value: self.config.pbus_rx_path,
            capacitance: self.wifi.tx_capacitance,
            channel_6_dcode: [self.common.dcode[2], self.common.dcode[3]],
            adjusted_tx: PhyRxIqAdjustedTxParameters {
                coefficient: self.wifi.tx_iq_coefficient,
                current_channel: self.wifi.current_channel,
                current_temperature: self.common.temperature as u16,
                calibration_temperature: self.wifi.calibration_temperature as u16,
                calibration_dcode: self.wifi.external_dcode,
            },
            coefficients: self.wifi.rx_iq_coefficients,
        }
    }

    pub fn apply_rx_iq_outcome(&mut self, outcome: PhyRxIqInitOutcome) {
        self.wifi.rx_iq_coefficients = outcome.coefficients;
        self.wifi.current_channel = outcome.current_channel;
    }

    const fn wifi_seed(&self) -> [u32; 6] {
        let rows = [
            self.wifi.tx_dco[0],
            self.wifi.tx_dco[1],
            self.wifi.tx_dco[2],
        ];
        Self::packed_seed(&rows)
    }

    pub const fn channel_parameters(&self) -> PhyChipChannelParameters {
        PhyChipChannelParameters {
            frequency_offset: 0,
            crystal_selector: self.common.crystal_selector,
            channel_14_mic_enabled: self.config.channel_14_mic_enabled,
            dot11p_enabled: self.wifi.dot11p_enabled != 0,
            dot11p_config: self.wifi.dot11p_configuration,
            tx_gain_skip_publication: self.config.tx_gain_skip_publication,
            tx_gain_seed: self.wifi_seed(),
            tx_gain_config: self.wifi.tx_iq_config,
            tx_gain_curve: [
                self.wifi.tx_power_curve[0] as u8,
                self.wifi.tx_power_curve[1] as u8,
                self.wifi.tx_power_curve[2] as u8,
                self.wifi.tx_power_corrections[0] as u8,
                self.wifi.tx_power_corrections[1] as u8,
                self.wifi.tx_power_corrections[2] as u8,
            ],
            tx_gain_correction: self.wifi.tx_power_adjustment,
            tx_gain_base: self.config.tx_gain_base,
            tx_gain_attenuation: self.config.tx_gain_attenuation,
            tx_capacitance: self.wifi.tx_capacitance,
        }
    }

    /// Channel selected by the completed typed channel transition.
    pub const fn current_wifi_channel(&self) -> u16 {
        self.wifi.current_channel
    }

    /// Regenerate the Wi-Fi gain bank from live calibration state.
    ///
    /// `None` preserves the vendor's explicit skip-publication configuration;
    /// the surrounding tracking child still completes normally in that case.
    pub const fn wifi_tracking_gain_image(
        &self,
        channel: u16,
        gain_base: i8,
    ) -> Option<PhyWifiTxGainImage> {
        let parameters = self.channel_parameters();
        if parameters.tx_gain_skip_publication {
            return None;
        }
        let mut image = calculate_wifi_tx_gain(PhyWifiTxGainRequest {
            channel,
            calibration_curve: parameters.tx_gain_curve,
            correction: parameters.tx_gain_correction,
            base_and_delta: (gain_base as u8).wrapping_sub(parameters.tx_gain_attenuation) as i8,
        });
        image.seed = parameters.tx_gain_seed;
        image.config = parameters.tx_gain_config;
        Some(image)
    }

    /// Regenerate the Wi-Fi calibration gain bank from a just-completed
    /// TXDC/PWDET result that has not yet reached the outer state commit.
    ///
    /// The periodic parent invokes the gain publisher immediately after TXDC
    /// and commits both effects only after restoring the RF guards. Using the
    /// retained state seed here would therefore republish the previous DCO.
    pub const fn wifi_calibration_gain_image(
        &self,
        channel: u16,
        tx_dc_pwdet: PhyTxDcPwdetOutcome,
    ) -> Option<PhyWifiTxGainImage> {
        let mut image = match self.wifi_tracking_gain_image(channel, self.config.tx_gain_base as i8)
        {
            Some(image) => image,
            None => return None,
        };
        image.seed = Self::packed_seed(&tx_dc_pwdet.dco);
        Some(image)
    }

    pub fn apply_channel_outcome(&mut self, outcome: PhyChipChannelOutcome) {
        self.wifi.current_channel = outcome.channel;
        self.wifi.channel_initialized = outcome.init_complete;
        self.wifi.channel_bandwidth = outcome.cbw;
        self.apply_temperature_outcome(outcome.temperature);
    }

    pub fn apply_rx_saturation_outcome(
        &mut self,
        outcome: PhyRxSaturationOutcome,
    ) -> Result<(), PhyRxSaturationOutcome> {
        match outcome {
            PhyRxSaturationOutcome::Measured {
                saturated_samples, ..
            } => {
                self.wifi.rx_saturation_detected |= saturated_samples != 0;
                Ok(())
            }
            failure => Err(failure),
        }
    }

    pub fn begin_full_calibration(&mut self, config: PhyConfig) {
        self.config = config;
        self.common.crystal_selector = 0;
        self.clear_calibration_status();
    }

    const fn calibration_snapshot(
        &self,
        identity: crate::calibration::registration::PhyCalibrationIdentity,
    ) -> PhyCalibrationSnapshot {
        PhyCalibrationSnapshot {
            schema: PHY_CALIBRATION_SNAPSHOT_SCHEMA,
            identity,
            common: PhyCommonCalibration {
                temperature: self.common.temperature,
                sensor_index: self.common.sensor_index,
                crystal_selector: self.common.crystal_selector,
                rc_result: self.common.rc_result,
                filter_dcap: self.common.filter_dcap,
                rc_calibrated: self.common.rc_calibrated,
                dcode: self.common.dcode,
                i2c_frequency_parameter: self.common.i2c_frequency_parameter,
                xtal_duty: self.common.xtal_duty,
                clear_tone_after_ready: self.common.clear_tone_after_ready,
            },
            wifi: PhyWifiCalibration {
                baseband_calibrated: self.wifi.baseband_calibrated,
                pwdet_calibrated: self.wifi.pwdet_calibrated,
                tx_power_calibrated: self.wifi.tx_power_calibrated,
                tx_iq_calibrated: self.wifi.tx_iq_calibrated,
                rx_gain_dc_calibrated: self.wifi.rx_gain_dc_calibrated,
                rx_gain_tables_initialized: self.wifi.rx_gain_tables_initialized,
                rx_saturation_detected: self.wifi.rx_saturation_detected,
                tx_dco: self.wifi.tx_dco,
                tx_reference_codes: self.wifi.tx_reference_codes,
                tx_capacitance: self.wifi.tx_capacitance,
                tx_power_curve: self.wifi.tx_power_curve,
                tx_power_corrections: self.wifi.tx_power_corrections,
                tx_power_adjustment: self.wifi.tx_power_adjustment,
                calibrated_attenuation: self.config.initial_attenuation,
                tx_iq_config: self.wifi.tx_iq_config,
                tx_iq_coefficient: self.wifi.tx_iq_coefficient,
                rx_iq_coefficients: self.wifi.rx_iq_coefficients,
                external_dcode: self.wifi.external_dcode,
                calibration_temperature: self.wifi.calibration_temperature,
                calibration_channel: self.wifi.current_channel,
                wifi_rx_table_last_index: self.wifi.wifi_rx_table_last_index,
                shared_rx_table_last_index: self.wifi.shared_rx_table_last_index,
                wifi_index_dc: self.wifi.wifi_index_dc,
                wifi_dc_base: self.wifi.wifi_dc_base,
                shared_index_dc: self.wifi.shared_index_dc,
                rxbb_dc_adjustments: self.wifi.rxbb_dc_adjustments,
            },
            bluetooth: PhyBluetoothCalibration {
                tx_dc_calibrated: self.bluetooth.tx_dc_calibrated,
                tx_power_calibrated: self.bluetooth.tx_power_calibrated,
                tx_dco: self.bluetooth.tx_dco,
                tx_power_curve: self.bluetooth.tx_power_curve,
                tx_power_corrections: self.bluetooth.tx_power_corrections,
                tx_power_adjustment: self.bluetooth.tx_power_adjustment,
            },
        }
    }

    pub(crate) const fn calibration_cache(
        &self,
        identity: crate::calibration::registration::PhyCalibrationIdentity,
    ) -> PhyCalibrationCache {
        PhyCalibrationCache::capture(identity, self)
    }

    pub const fn register_temperature_control(&self) -> PhyRegisterTemperatureControl {
        PhyRegisterTemperatureControl {
            update_registration_reference: !self.common.frequency_table_initialized,
            update_calibration_reference: !self.wifi.tx_power_calibrated,
        }
    }

    pub fn apply_register_temperature_outcome(
        &mut self,
        control: PhyRegisterTemperatureControl,
        outcome: PhyTemperatureOutcome,
    ) {
        self.apply_temperature_outcome(outcome);
        if control.updates_offset_130() {
            self.common.rfpll_tracking_temperature = outcome.temperature;
        }
        if control.updates_reference_copies() {
            self.common.calibration_tracking_temperature = outcome.temperature;
            self.wifi.txdc_tracking_temperature = outcome.temperature;
            self.bluetooth.txdc_tracking_temperature = outcome.temperature;
        }
    }

    pub fn apply_full_calibration_temperature(&mut self, outcome: PhyTemperatureOutcome) {
        self.apply_register_temperature_outcome(PhyRegisterTemperatureControl::FULL, outcome);
    }

    fn clear_calibration_status(&mut self) {
        self.common.rc_calibrated = false;
        self.common.frequency_table_initialized = false;
        self.wifi.baseband_calibrated = false;
        self.wifi.pwdet_calibrated = false;
        self.wifi.tx_power_calibrated = false;
        self.wifi.tx_iq_calibrated = false;
        self.bluetooth.tx_dc_calibrated = false;
        self.bluetooth.tx_power_calibrated = false;
    }

    pub(crate) fn mark_phy_registered(&mut self) {
        self.common.registered = true;
    }

    pub const fn phy_registered(&self) -> bool {
        self.common.registered
    }

    pub const fn rc_calibration_complete(&self) -> bool {
        self.common.rc_calibrated
    }

    pub fn apply_rc_calibration(&mut self, result: u8) {
        const UPPER: [u8; 4] = [0x28, 0x14, 0x1e, 0x14];
        const PRIMARY: [u8; 2] = [0x14, 0x28];
        const AUX: [u8; 4] = [0x24, 0x28, 0x16, 0x20];
        let bounded = if result > 45 { 50 } else { result };
        let base = bounded as i32 + 56;
        self.common.rc_result = result;
        let mut index = 0;
        while index != PRIMARY.len() {
            let value = base * 82 / (PRIMARY[index] as i32 * 10) - 8;
            self.common.filter_dcap[index] = saturate_phy_value(value, UPPER[index], 2);
            index += 1;
        }
        index = 0;
        while index != AUX.len() {
            let value = base * 0x334 / (AUX[index] as i32 * 104) - 8;
            let slot = match index {
                0 => 2,
                1 => 3,
                3 => 4,
                _ => {
                    index += 1;
                    continue;
                }
            };
            self.common.filter_dcap[slot] = saturate_phy_value(value, UPPER[2 + (index & 1)], 0);
            index += 1;
        }
        self.common.rc_calibrated = true;
    }

    pub const fn filter_dcap_parameters(&self) -> FilterDcapParameters {
        FilterDcapParameters::new(
            self.common.filter_dcap[0],
            self.common.filter_dcap[1],
            self.common.filter_dcap[2],
            self.common.filter_dcap[3],
            self.common.filter_dcap[4],
        )
    }

    pub const fn xtal_duty_parameters(&self) -> XtalDutyCalibrationParameters {
        XtalDutyCalibrationParameters {
            rf_frequency_offset_base: self.common.crystal_selector,
            pbus_rx_path_value: self.config.pbus_rx_path,
        }
    }

    pub const fn channel_frequency_control(&self) -> PhyChannelFrequencyInitControl {
        PhyChannelFrequencyInitControl {
            frequency_register_parameter_override: self.bluetooth.channel_base != 0,
            frequency_table_initialized: self.common.frequency_table_initialized,
            front_end_parameter_bit: self.common.front_end_parameter,
        }
    }

    pub fn set_xtal_frequency_mhz(&mut self, frequency_mhz: u32) {
        self.common.crystal_selector = match frequency_mhz {
            26 => 1,
            32 => 2,
            _ => 0,
        };
    }

    pub(crate) fn synchronize_success(&mut self, outcome: PhyRfInitPrefixOutcome) {
        let PhyRfInitPrefixOutcome::ChannelFrequencyInitialized {
            parameter,
            xtal_duty,
            channel_frequency,
            ..
        } = outcome
        else {
            return;
        };
        self.common.i2c_frequency_parameter = parameter.parameter_18e();
        self.common.xtal_duty = [
            xtal_duty.initial_duty,
            xtal_duty.low_frequency.best_candidate,
            xtal_duty.high_frequency.best_candidate,
        ];
        self.common.frequency_table_initialized = channel_frequency.table_is_initialized;
    }
}

impl Default for PhyState {
    fn default() -> Self {
        Self::new(PhyConfig::esp32s31_default())
    }
}

#[cfg(test)]
mod tests;

pub mod client;
