//! Explicit single-owner state for ESP32-S31 PHY cold initialization.
//!
//! `libphy.a[phy_init.o]` defines a 508-byte mutable `phy_param` object and
//! passes its address through the rev0 ROM ABI.  The Rust cold path must not
//! reproduce that hidden ownership model.  This module owns the complete
//! parameter image as an ordinary Rust value and supplies only typed snapshots
//! to the event-driven `phy_rf_init` transition.
//!
//! The initial image below is the complete `.data.phy_param` section from the
//! pinned `libphy.a` (SHA-256
//! `51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223`).
//! The extracted 508-byte section has SHA-256
//! `d8b4dbeeedcfb2cbaa6a00d2a7c84bc8c9ad5bbf54a2ff6bc30dee7f3b46ed83`.
//! Keeping the sparse nonzero bytes here avoids retaining the vendor object
//! merely to obtain its initial data.

use crate::{
    phy_dc_iq::{
        PhyDcIqAccumulatorSnapshot, PhyDcIqAction, PhyDcIqCompletion, PhyDcIqEstimateRequest,
        PhyDcIqReadinessSnapshot,
    },
    phy_frequency::{
        PhyChannelFrequencyInitAction, PhyChannelFrequencyInitCompletion,
        PhyChannelFrequencyInitControl, PhyFrequencyI2cAction, PhyFrequencyI2cCompletion,
        PhyFrequencyTableAction, PhyFrequencyTableCompletion,
    },
    phy_i2c::{
        AdcRateAction, AdcRateCompletion, BiasRegAction, BiasRegCompletion, FilterDcapAction,
        FilterDcapCompletion, FilterDcapParameters, I2cBbpllAction, I2cBbpllCompletion,
        I2cInit1Action, I2cInit1Completion, MaskedI2cWriteAction, MaskedI2cWriteCompletion,
        OpenI2cXpdAction, OpenI2cXpdCompletion, PhyI2cAddress, PhyI2cError, PhyRfInitPrefixAction,
        PhyRfInitPrefixCompletion, PhyRfInitPrefixOutcome, PhyRfInitPrefixTransition,
        PhyRfInitPrefixTransitionError, RcCalibrationAction, RcCalibrationCompletion,
        RcCalibrationSetAction, RcCalibrationSetCompletion, RfpllChargePumpAction,
        RfpllChargePumpCompletion, Sar2InitAction, Sar2InitCompletion,
    },
    phy_param::{
        apply_init_data, apply_rc_calibration_result, calibration_record_check_or_write,
        xtal_parameter_code, PHY_CALIBRATION_PAYLOAD_OFFSET, PHY_CALIBRATION_PREFIX_LEN,
        PHY_INIT_DATA_LEN, PHY_PARAM_LEN,
    },
    phy_pbus::{PhyPbusClearAction, PhyPbusClearCompletion, PhyPbusForceTest},
    phy_rfpll::{RfpllFrequencyAction, RfpllFrequencyCompletion},
    phy_rx_dco::{PhyRxDcoAction, PhyRxDcoCompletion},
    phy_signal_power::{
        PhySignalPowerAccumulatorSnapshot, PhySignalPowerAction, PhySignalPowerCompletion,
        PhySignalPowerRequest,
    },
    phy_xtal_duty::{
        XtalDutyCalibrationAction, XtalDutyCalibrationCompletion, XtalDutyCalibrationParameters,
        XtalDutyPassAction, XtalDutyPassCompletion, XtalDutyPrepareAction,
        XtalDutyPrepareCompletion, XtalDutyRestoreAction, XtalDutyRestoreCompletion,
        XtalDutySearchAction, XtalDutySearchCompletion,
    },
};
#[cfg(target_arch = "riscv32")]
use open_esp_radio_hal_esp32s31::RadioRegisters;

pub const PHY_COLD_PARAMETER_LEN: usize = PHY_PARAM_LEN;
pub const PHY_COLD_INIT_PROFILE_LEN: usize = PHY_INIT_DATA_LEN;
pub const PHY_COLD_CALIBRATION_RECORD_LEN: usize = PHY_CALIBRATION_PREFIX_LEN;

const fn initial_parameter_image() -> [u8; PHY_PARAM_LEN] {
    let mut parameter = [0; PHY_PARAM_LEN];

    parameter[0x002] = 0xbf;
    parameter[0x003] = 0x20;
    parameter[0x006] = 0x54;
    parameter[0x00b] = 0x01;
    parameter[0x00e] = 0x60;
    parameter[0x00f] = 0x01;
    parameter[0x012] = 0x1f;
    parameter[0x013] = 0x16;
    parameter[0x014] = 0x01;
    parameter[0x015] = 0x40;
    parameter[0x016] = 0x02;
    parameter[0x018] = 0x50;
    parameter[0x024] = 0x30;
    // The pinned ESP32-S31 `esp-phy` oracle does not issue
    // `phy_init_param_set(1)`: its live cold image keeps byte 0x196 at zero.
    // That matters because a nonzero byte makes `phy_bb_init` turn Wi-Fi RX
    // back off immediately after the initial channel selection.
    parameter[0x1ab] = 0x01;
    parameter[0x1af] = 0x01;

    parameter
}

/// Complete fixed-size calibration record used by `register_chipv7_phy`.
///
/// Bytes 0..12 are the version and eFuse identity, bytes 12..520 are the
/// parameter payload, and bytes 520..524 contain the one's-complement
/// checksum.  It is separate from [`PhyColdState`] because callers may keep a
/// retained calibration record while constructing a fresh radio owner.
#[repr(C, align(4))]
pub struct PhyCalibrationRecord {
    bytes: [u8; PHY_CALIBRATION_PREFIX_LEN],
}

impl PhyCalibrationRecord {
    pub const fn new() -> Self {
        Self {
            bytes: [0; PHY_CALIBRATION_PREFIX_LEN],
        }
    }

    pub const fn from_bytes(bytes: [u8; PHY_CALIBRATION_PREFIX_LEN]) -> Self {
        Self { bytes }
    }

    pub const fn bytes(&self) -> &[u8; PHY_CALIBRATION_PREFIX_LEN] {
        &self.bytes
    }

    pub fn refresh_header_and_checksum(&mut self, version: u32, mac_sys0: u32, mac_sys1: u32) {
        let result =
            calibration_record_check_or_write(&mut self.bytes, false, version, mac_sys0, mac_sys1);
        debug_assert_eq!(result, 0);
    }

    /// Refresh the identity fields and compare the stored checksum.
    ///
    /// The identity refresh before comparison matches the pinned vendor body;
    /// this method performs no MMIO itself, so the eFuse words are explicit
    /// inputs owned by the outer cold-init executor.
    pub fn checksum_matches(&mut self, version: u32, mac_sys0: u32, mac_sys1: u32) -> bool {
        calibration_record_check_or_write(&mut self.bytes, true, version, mac_sys0, mac_sys1) == 0
    }
}

impl Default for PhyCalibrationRecord {
    fn default() -> Self {
        Self::new()
    }
}

/// Unique Rust owner of all parameter state used by PHY cold initialization.
///
/// The type deliberately is neither `Copy` nor `Clone`.  Moving it transfers
/// ownership; duplicating the live radio state is not a supported operation.
/// Alignment matches `.data.phy_param` in the pinned archive and permits a
/// later direct ABI publication without changing the representation.
#[repr(C, align(4))]
pub struct PhyColdState {
    parameter: [u8; PHY_PARAM_LEN],
}

impl PhyColdState {
    pub const fn new() -> Self {
        Self {
            parameter: initial_parameter_image(),
        }
    }

    pub const fn from_parameter_image(parameter: [u8; PHY_PARAM_LEN]) -> Self {
        Self { parameter }
    }

    pub const fn parameter_image(&self) -> &[u8; PHY_PARAM_LEN] {
        &self.parameter
    }

    /// Whether the guarded calibration prefix of `phy_bb_init` has already
    /// completed.
    ///
    /// The pinned parent tests bit three of the word at `phy_param+0xa4`.
    /// That bit is now private state owned by this value rather than an
    /// implicit branch through the vendor global.
    pub const fn baseband_calibration_complete(&self) -> bool {
        self.parameter[0x0a4] & 0x08 != 0
    }

    /// Commit completion of the guarded `phy_bb_init` calibration prefix.
    pub fn mark_baseband_calibration_complete(&mut self) {
        self.parameter[0x0a4] |= 0x08;
    }

    /// Preserve the one conditional Wi-Fi-disable branch in `phy_bb_init`.
    pub const fn disable_wifi_after_baseband_init(&self) -> bool {
        self.parameter[0x196] != 0
    }

    /// Capture the two explicit bytes consumed by the finite `phy_reg_init`
    /// register composition.
    pub const fn register_init_parameters(&self) -> crate::phy_bb::PhyRegisterInitParameters {
        crate::phy_bb::PhyRegisterInitParameters {
            parameter_121: self.parameter[0x121],
            parameter_120: self.parameter[0x120],
        }
    }

    /// Capture the explicit inputs and apply the sole software-state mutation
    /// performed by pinned `phy_rx_table_init`.
    pub fn prepare_rx_table_init(&mut self) -> crate::phy_bb::PhyRxTableInitParameters {
        let parameters = crate::phy_bb::PhyRxTableInitParameters {
            parameter_002: self.parameter[0x002],
            parameter_121: self.parameter[0x121],
        };
        self.parameter[0x120] = crate::phy_bb::PHY_RX_TABLE_ENTRY_COUNT;
        parameters
    }

    /// Copy every parameter consumed by the generated RX-gain memory
    /// publisher. The returned value has no pointer back into this owner.
    pub fn rx_gain_memory_parameters(&self) -> crate::phy_bb::PhyRxGainMemoryParameters {
        fn pair(parameter: &[u8; PHY_PARAM_LEN], offset: usize) -> [u16; 2] {
            [
                u16::from_le_bytes([parameter[offset], parameter[offset + 1]]),
                u16::from_le_bytes([parameter[offset + 2], parameter[offset + 3]]),
            ]
        }

        let mut wifi_index_dc = [[0_u16; 2]; 8];
        let mut index = 0;
        while index != wifi_index_dc.len() {
            wifi_index_dc[index] = pair(&self.parameter, 0x14e + index * 4);
            index += 1;
        }
        let mut shared_index_dc = [[0_u16; 2]; 11];
        index = 0;
        while index != shared_index_dc.len() {
            shared_index_dc[index] = pair(&self.parameter, 0x1b4 + index * 4);
            index += 1;
        }
        let mut rxbb_dc_adjustments = [[0_u16; 2]; 6];
        index = 0;
        while index != rxbb_dc_adjustments.len() {
            rxbb_dc_adjustments[index] = pair(&self.parameter, 0x1e0 + index * 4);
            index += 1;
        }
        crate::phy_bb::PhyRxGainMemoryParameters {
            parameter_002: self.parameter[0x002],
            wifi_index_dc,
            wifi_dc_base: pair(&self.parameter, 0x16e),
            shared_index_dc,
            rxbb_dc_adjustments,
            wifi_auxiliary: u16::from_le_bytes([self.parameter[0x0d4], self.parameter[0x0d5]]),
        }
    }

    /// Copy every former `phy_param` input used by
    /// `phy_set_rx_gain_cal_dc(1, ...)` and
    /// `phy_set_rx_gain_cal_dc(0, ...)`.
    pub const fn rx_gain_dc_parameters(&self) -> crate::phy_rx_gain_cal::PhyRxGainDcParameters {
        crate::phy_rx_gain_cal::PhyRxGainDcParameters {
            crystal_selector: self.parameter[0x04f],
            pbus_rx_path_value: self.parameter[0x002],
            rx_saturation_detected: self.parameter[0x1ae] == 1,
        }
    }

    /// Commit the exact three non-overlapping output regions formerly
    /// addressed through raw pointers into `phy_param`.
    pub fn apply_rx_gain_dc_outcome(
        &mut self,
        outcome: crate::phy_rx_gain_cal::PhyRxGainDcOutcome,
    ) {
        fn put_pair(parameter: &mut [u8; PHY_PARAM_LEN], offset: usize, pair: [u16; 2]) {
            let i = pair[0].to_le_bytes();
            let q = pair[1].to_le_bytes();
            parameter[offset..offset + 2].copy_from_slice(&i);
            parameter[offset + 2..offset + 4].copy_from_slice(&q);
        }

        let mut index = 0;
        while index != outcome.wifi_index_dc.len() {
            put_pair(
                &mut self.parameter,
                0x14e + index * 4,
                outcome.wifi_index_dc[index],
            );
            index += 1;
        }
        put_pair(&mut self.parameter, 0x16e, outcome.wifi_dc_base);
        index = 0;
        while index != outcome.shared_index_dc.len() {
            put_pair(
                &mut self.parameter,
                0x1b4 + index * 4,
                outcome.shared_index_dc[index],
            );
            index += 1;
        }
        index = 0;
        while index != outcome.rxbb_dc_adjustments.len() {
            put_pair(
                &mut self.parameter,
                0x1e0 + index * 4,
                outcome.rxbb_dc_adjustments[index],
            );
            index += 1;
        }
    }

    /// Capture every former `phy_param` input of the complete RX-gain root.
    pub fn rx_gain_init_parameters(&self) -> crate::phy_rx_gain::PhyRxGainInitParameters {
        let flags = u32::from_le_bytes([
            self.parameter[0x0b4],
            self.parameter[0x0b5],
            self.parameter[0x0b6],
            self.parameter[0x0b7],
        ]);
        crate::phy_rx_gain::PhyRxGainInitParameters {
            dc_calibrated: flags & 0x80 != 0,
            tables_initialized: flags & 0x200 != 0,
            dc: self.rx_gain_dc_parameters(),
            memory: self.rx_gain_memory_parameters(),
        }
    }

    /// Atomically publish the software-state effects of successful
    /// `phy_set_rx_gain_table`. Failed transitions never receive this method.
    pub fn apply_rx_gain_init_outcome(
        &mut self,
        outcome: crate::phy_rx_gain::PhyRxGainInitOutcome,
    ) {
        if let Some(dc) = outcome.dc {
            self.apply_rx_gain_dc_outcome(dc);
            self.parameter[0x0b4] |= 0x80;
        }
        if outcome.generated_tables {
            self.parameter[0x121] = outcome.wifi_last_index.min(0x4f);
            self.parameter[0x120] = outcome.shared_last_index.min(0x4f);
            self.parameter[0x0b5] |= 0x02;
            self.parameter[0x190] = self.parameter[0];
            self.parameter[0x191] = self.parameter[1];
        }
    }

    /// Commit the pure table generator's three former global-state effects.
    pub fn apply_generated_rx_gain_tables(
        &mut self,
        wifi: crate::phy_bb::PhyGeneratedRxGainTable,
        shared: crate::phy_bb::PhyGeneratedRxGainTable,
    ) {
        self.parameter[0x121] = wifi.last_index.min(crate::phy_bb::PHY_RX_TABLE_ENTRY_COUNT);
        self.parameter[0x120] = shared
            .last_index
            .min(crate::phy_bb::PHY_RX_TABLE_ENTRY_COUNT);
        self.parameter[0xa5] |= 0x02;
        self.parameter[0x190] = self.parameter[0];
        self.parameter[0x191] = self.parameter[1];
    }

    /// Capture the sole owned parameter input consumed by `phy_check_rx_sat`.
    pub const fn rx_saturation_parameter_002(&self) -> u8 {
        self.parameter[0x002]
    }

    /// Capture the only two parameter bytes used to construct the twelve
    /// PBus-memory tables.
    pub const fn pbus_memory_parameters(&self) -> crate::phy_pbus_memory::PhyPbusMemoryParameters {
        crate::phy_pbus_memory::PhyPbusMemoryParameters {
            parameter_002: self.parameter[0x002],
            parameter_014: self.parameter[0x014],
        }
    }

    /// Commit the six words formerly written by ROM `phy_save_pbus_reg`
    /// through the global `phy_param` pointer.
    pub fn apply_pbus_memory_outcome(
        &mut self,
        outcome: crate::phy_pbus_memory::PhyPbusMemoryOutcome,
    ) {
        let mut word = 0;
        while word != outcome.saved_registers.len() {
            let bytes = outcome.saved_registers[word].to_le_bytes();
            let offset = 0x30 + word * 4;
            self.parameter[offset] = bytes[0];
            self.parameter[offset + 1] = bytes[1];
            self.parameter[offset + 2] = bytes[2];
            self.parameter[offset + 3] = bytes[3];
            word += 1;
        }
    }

    /// Commit the two persistent effects of a completed temperature sample:
    /// the signed result at bytes `0x000..=0x001` and the sensor-range index
    /// at byte `0x016`.
    pub fn apply_temperature_outcome(
        &mut self,
        outcome: crate::phy_temperature::PhyTemperatureOutcome,
    ) {
        let bytes = outcome.temperature.to_le_bytes();
        self.parameter[0] = bytes[0];
        self.parameter[1] = bytes[1];
        self.parameter[0x16] = outcome.sensor_index;
    }

    pub const fn dcode_parameters(&self) -> crate::phy_dcode::PhyDcodeParameters {
        crate::phy_dcode::PhyDcodeParameters {
            crystal_selector: self.parameter[0x4f],
        }
    }

    /// Commit the eight D-code samples formerly written to
    /// `phy_param[0x1a1..=0x1a8]`.
    pub fn apply_dcode_outcome(&mut self, outcome: crate::phy_dcode::PhyDcodeOutcome) {
        self.parameter[0x1a1..=0x1a8].copy_from_slice(&outcome.codes);
    }

    /// Capture every field consumed by ROM `phy_pwdet_code_cal` and its
    /// complete child graph.
    ///
    /// Bit 24 of the little-endian word at offset `0x0a4` is byte `0x0a7`
    /// bit zero. The two DCO paths and two signed reference values are copied
    /// into typed values, so the transition never receives a raw parameter
    /// pointer or aliases this owner while calibration is active.
    pub fn pwdet_parameters(&self) -> crate::phy_pwdet::PhyPwdetParameters {
        crate::phy_pwdet::PhyPwdetParameters {
            already_calibrated: self.parameter[0x0a7] & 1 != 0,
            pbus_tx_path_value: self.parameter[0x012],
            pbus_rx_path_value: self.parameter[0x002],
            dco: [
                u16::from_le_bytes([self.parameter[0x0a8], self.parameter[0x0a9]]),
                u16::from_le_bytes([self.parameter[0x0aa], self.parameter[0x0ab]]),
                u16::from_le_bytes([self.parameter[0x0ac], self.parameter[0x0ad]]),
                u16::from_le_bytes([self.parameter[0x0ae], self.parameter[0x0af]]),
            ],
            clear_tone_after_ready: self.parameter[0x1aa] != 0,
            reference_codes: [
                i16::from_le_bytes([self.parameter[0x01a], self.parameter[0x01b]]),
                i16::from_le_bytes([self.parameter[0x01c], self.parameter[0x01d]]),
            ],
        }
    }

    /// Commit the only persistent effects of successful PWDET calibration.
    ///
    /// Failed transitions do not produce an outcome and therefore cannot
    /// publish partial reference codes or mark the owner calibrated.
    pub fn apply_pwdet_outcome(&mut self, outcome: crate::phy_pwdet::PhyPwdetOutcome) {
        let reference_0 = outcome.reference_codes[0].to_le_bytes();
        let reference_1 = outcome.reference_codes[1].to_le_bytes();
        self.parameter[0x01a] = reference_0[0];
        self.parameter[0x01b] = reference_0[1];
        self.parameter[0x01c] = reference_1[0];
        self.parameter[0x01d] = reference_1[1];
        if outcome.calibrated {
            self.parameter[0x0a7] |= 1;
        }
    }

    /// Capture the only global parameter byte consumed by the mandatory
    /// `phy_txdc_cal_init(&phy_param[0xa8], 15, 0, 0)` call.
    pub const fn tx_dc_parameters(&self) -> crate::phy_txdc::PhyTxDcParameters {
        crate::phy_txdc::PhyTxDcParameters {
            pbus_rx_path_value: self.parameter[0x002],
        }
    }

    /// Commit the five four-halfword TX-DC rows formerly written through the
    /// raw `&phy_param[0xa8]` pointer.
    pub fn apply_tx_dc_outcome(&mut self, outcome: crate::phy_txdc::PhyTxDcOutcome) {
        let mut row = 0;
        while row != outcome.dco.len() {
            let mut column = 0;
            while column != outcome.dco[row].len() {
                let bytes = outcome.dco[row][column].to_le_bytes();
                let offset = 0x0a8 + (row * 4 + column) * 2;
                self.parameter[offset] = bytes[0];
                self.parameter[offset + 1] = bytes[1];
                column += 1;
            }
            row += 1;
        }
    }

    /// Capture all global inputs of `phy_tx_cap_init` and its complete child
    /// graph as a value detached from this unique owner.
    pub fn tx_cap_parameters(&self) -> crate::phy_tx_cal::PhyTxCapParameters {
        let dco = [
            u16::from_le_bytes([self.parameter[0x0a8], self.parameter[0x0a9]]),
            u16::from_le_bytes([self.parameter[0x0aa], self.parameter[0x0ab]]),
            u16::from_le_bytes([self.parameter[0x0ac], self.parameter[0x0ad]]),
            u16::from_le_bytes([self.parameter[0x0ae], self.parameter[0x0af]]),
        ];
        crate::phy_tx_cal::PhyTxCapParameters {
            crystal_selector: self.parameter[0x04f],
            environment: crate::phy_tx_cal::PhyTxCalibrationParameters {
                pbus_tx_path_value: self.parameter[0x012],
                pbus_rx_path_value: self.parameter[0x002],
                dco,
            },
            clear_tone_after_ready: self.parameter[0x1aa] != 0,
            reference_codes: [
                i16::from_le_bytes([self.parameter[0x01a], self.parameter[0x01b]]),
                i16::from_le_bytes([self.parameter[0x01c], self.parameter[0x01d]]),
            ],
            power_offset: i16::from_le_bytes([self.parameter[0x00e], self.parameter[0x00f]]),
            initial_attenuation: self.parameter[0x018],
        }
    }

    /// Commit the six calibrated TX-capacitance bytes and the selected
    /// attenuation only after the complete transition has restored work mode.
    pub fn apply_tx_cap_outcome(&mut self, outcome: crate::phy_tx_cal::PhyTxCapOutcome) {
        self.parameter[0x0dc..0x0e2].copy_from_slice(&outcome.capacitance);
        self.parameter[0x018] = outcome.attenuation;
    }

    /// Capture every former global input of `phy_tx_pwctrl_init`.
    pub fn tx_power_parameters(&self) -> crate::phy_tx_power::PhyTxPowerParameters {
        let flags = u32::from_le_bytes([
            self.parameter[0x0a4],
            self.parameter[0x0a5],
            self.parameter[0x0a6],
            self.parameter[0x0a7],
        ]);
        let cap = [
            self.parameter[0x0dc],
            self.parameter[0x0dd],
            self.parameter[0x0de],
            self.parameter[0x0df],
            self.parameter[0x0e0],
            self.parameter[0x0e1],
        ];
        let dco = [
            u16::from_le_bytes([self.parameter[0x0a8], self.parameter[0x0a9]]),
            u16::from_le_bytes([self.parameter[0x0aa], self.parameter[0x0ab]]),
            u16::from_le_bytes([self.parameter[0x0ac], self.parameter[0x0ad]]),
            u16::from_le_bytes([self.parameter[0x0ae], self.parameter[0x0af]]),
        ];
        crate::phy_tx_power::PhyTxPowerParameters {
            already_calibrated: flags & 0x0010_0000 != 0,
            crystal_selector: self.parameter[0x04f],
            environment: crate::phy_tx_cal::PhyTxCalibrationParameters {
                pbus_tx_path_value: self.parameter[0x012],
                pbus_rx_path_value: self.parameter[0x002],
                dco,
            },
            capacitance: cap,
            target_adjustment: self.parameter[0x02b],
            power_offset: i16::from_le_bytes([self.parameter[0x00e], self.parameter[0x00f]]),
            initial_attenuation: self.parameter[0x018],
            clear_tone_after_ready: self.parameter[0x1aa] != 0,
        }
    }

    /// Publish TX power-control calibration only after work-mode cleanup.
    pub fn apply_tx_power_outcome(&mut self, outcome: crate::phy_tx_power::PhyTxPowerOutcome) {
        if !outcome.calibration_performed {
            return;
        }
        for (index, value) in outcome.reference_codes.into_iter().enumerate() {
            let bytes = value.to_le_bytes();
            let offset = 0x01a + index * 2;
            self.parameter[offset] = bytes[0];
            self.parameter[offset + 1] = bytes[1];
        }
        for index in 0..3 {
            self.parameter[0x0f1 + index] = outcome.power_curve[index] as u8;
            self.parameter[0x0f4 + index] = outcome.point_corrections[index] as u8;
        }
        self.parameter[0x0f7] = outcome.power_adjustment as u8;
        self.parameter[0x018] = outcome.final_attenuation;
        self.parameter[0x1aa] = 0;
        self.parameter[0x0a6] |= 0x10;
        let channel = outcome.current_channel.to_le_bytes();
        self.parameter[0x11c] = channel[0];
        self.parameter[0x11d] = channel[1];
    }

    /// Capture the three Wi-Fi DCO rows used by
    /// `phy_txdc_cal_pwdet_init(1, 0, 0)`.
    pub fn tx_dc_pwdet_parameters(&self) -> crate::phy_txdc_pwdet::PhyTxDcPwdetParameters {
        let mut dco = [[0_u16; 4]; 3];
        let mut row = 0;
        while row != dco.len() {
            let mut column = 0;
            while column != dco[row].len() {
                let offset = 0x0a8 + (row * 4 + column) * 2;
                dco[row][column] =
                    u16::from_le_bytes([self.parameter[offset], self.parameter[offset + 1]]);
                column += 1;
            }
            row += 1;
        }
        crate::phy_txdc_pwdet::PhyTxDcPwdetParameters {
            dco,
            clear_tone_after_ready: self.parameter[0x1aa] != 0,
        }
    }

    /// Commit the calibrated Wi-Fi DCO rows only after unconditional radio
    /// cleanup has completed.
    pub fn apply_tx_dc_pwdet_outcome(
        &mut self,
        outcome: crate::phy_txdc_pwdet::PhyTxDcPwdetOutcome,
    ) {
        let mut row = 0;
        while row != outcome.dco.len() {
            let mut column = 0;
            while column != outcome.dco[row].len() {
                let offset = 0x0a8 + (row * 4 + column) * 2;
                let bytes = outcome.dco[row][column].to_le_bytes();
                self.parameter[offset] = bytes[0];
                self.parameter[offset + 1] = bytes[1];
                column += 1;
            }
            row += 1;
        }
    }

    /// Capture every former global input of archive `phy_txiq_cal_init`.
    ///
    /// Channel-six D-code bytes are selected from the Rust-owned eight-byte
    /// calibration result. No pointer into this owner survives the call.
    pub fn tx_iq_parameters(&self) -> crate::phy_txiq::PhyTxIqInitParameters {
        let flags = u32::from_le_bytes([
            self.parameter[0x0a4],
            self.parameter[0x0a5],
            self.parameter[0x0a6],
            self.parameter[0x0a7],
        ]);
        let dco = [
            u16::from_le_bytes([self.parameter[0x0a8], self.parameter[0x0a9]]),
            u16::from_le_bytes([self.parameter[0x0aa], self.parameter[0x0ab]]),
            u16::from_le_bytes([self.parameter[0x0ac], self.parameter[0x0ad]]),
            u16::from_le_bytes([self.parameter[0x0ae], self.parameter[0x0af]]),
        ];
        crate::phy_txiq::PhyTxIqInitParameters {
            already_calibrated: flags & 0x0000_4000 != 0,
            crystal_selector: self.parameter[0x04f],
            environment: crate::phy_tx_cal::PhyTxCalibrationParameters {
                pbus_tx_path_value: self.parameter[0x012],
                pbus_rx_path_value: self.parameter[0x002],
                dco,
            },
            capacitance: [
                self.parameter[0x0dc],
                self.parameter[0x0dd],
                self.parameter[0x0de],
                self.parameter[0x0df],
                self.parameter[0x0e0],
                self.parameter[0x0e1],
            ],
            channel_6_dcode: [self.parameter[0x1a3], self.parameter[0x1a4]],
            initial_attenuation: self.parameter[0x018] as i8,
            power_offset: i16::from_le_bytes([self.parameter[0x00e], self.parameter[0x00f]]),
            reference_codes: [
                i16::from_le_bytes([self.parameter[0x01a], self.parameter[0x01b]]),
                i16::from_le_bytes([self.parameter[0x01c], self.parameter[0x01d]]),
            ],
            clear_tone_after_ready: self.parameter[0x1aa] != 0,
        }
    }

    /// Publish TX-IQ coefficients and temperature only after both calibration
    /// variants have completed their unconditional radio cleanup.
    pub fn apply_tx_iq_outcome(&mut self, outcome: crate::phy_txiq::PhyTxIqInitOutcome) {
        if !outcome.calibration_performed {
            return;
        }
        self.parameter[0x198..0x19a].copy_from_slice(&outcome.external_dcode);
        let first = outcome.coefficient[0].to_le_bytes();
        self.parameter[0x0d0] = first[0];
        self.parameter[0x0d1] = first[1];
        let second = outcome.coefficient[1].to_le_bytes();
        self.parameter[0x0e6] = second[0];
        self.parameter[0x0e7] = second[1];
        if let Some(temperature) = outcome.temperature {
            let bytes = temperature.temperature.to_le_bytes();
            self.parameter[0x19a] = bytes[0];
            self.parameter[0x19b] = bytes[1];
            self.apply_temperature_outcome(temperature);
        }
        self.parameter[0x0a5] |= 0x40;
    }

    /// Capture every former global input of archive `phy_rxiq_cal_init`.
    ///
    /// The four coefficient halfwords and all temperature/D-code adjustment
    /// inputs are copied, so the transition cannot alias this unique owner.
    pub fn rx_iq_parameters(&self) -> crate::phy_rxiq::PhyRxIqInitParameters {
        let mut coefficients = [0_u16; 4];
        let mut index = 0;
        while index != coefficients.len() {
            let offset = 0x0d4 + index * 2;
            coefficients[index] =
                u16::from_le_bytes([self.parameter[offset], self.parameter[offset + 1]]);
            index += 1;
        }
        crate::phy_rxiq::PhyRxIqInitParameters {
            crystal_selector: self.parameter[0x04f],
            pbus_rx_path_value: self.parameter[0x002],
            capacitance: [
                self.parameter[0x0dc],
                self.parameter[0x0dd],
                self.parameter[0x0de],
                self.parameter[0x0df],
                self.parameter[0x0e0],
                self.parameter[0x0e1],
            ],
            channel_6_dcode: [self.parameter[0x1a3], self.parameter[0x1a4]],
            adjusted_tx: crate::phy_rxiq::PhyRxIqAdjustedTxParameters {
                coefficient: u16::from_le_bytes([self.parameter[0x0e6], self.parameter[0x0e7]]),
                current_channel: u16::from_le_bytes([self.parameter[0x11c], self.parameter[0x11d]]),
                current_temperature: u16::from_le_bytes([
                    self.parameter[0x000],
                    self.parameter[0x001],
                ]),
                calibration_temperature: u16::from_le_bytes([
                    self.parameter[0x19a],
                    self.parameter[0x19b],
                ]),
                calibration_dcode: [self.parameter[0x198], self.parameter[0x199]],
            },
            coefficients,
        }
    }

    /// Publish the exact five software-state writes of completed RXIQ init.
    ///
    /// Failed transitions cannot call this method and therefore cannot expose
    /// a partially converted coefficient table.
    pub fn apply_rx_iq_outcome(&mut self, outcome: crate::phy_rxiq::PhyRxIqInitOutcome) {
        let mut index = 0;
        while index != outcome.coefficients.len() {
            let offset = 0x0d4 + index * 2;
            let bytes = outcome.coefficients[index].to_le_bytes();
            self.parameter[offset] = bytes[0];
            self.parameter[offset + 1] = bytes[1];
            index += 1;
        }
        let channel = outcome.current_channel.to_le_bytes();
        self.parameter[0x11c] = channel[0];
        self.parameter[0x11d] = channel[1];
    }

    /// Capture every former global input of `phy_chip_set_chan`.
    ///
    /// The transition owns this value and cannot alias the 508-byte cold
    /// state. The optional channel-14 path is retained as an explicit
    /// fail-closed profile bit rather than an indirect vendor branch.
    pub fn channel_parameters(&self) -> crate::phy_channel::PhyChipChannelParameters {
        let mut seed = [0_u32; 6];
        let mut index = 0;
        while index != seed.len() {
            let offset = 0x0a8 + index * core::mem::size_of::<u32>();
            seed[index] = u32::from_le_bytes([
                self.parameter[offset],
                self.parameter[offset + 1],
                self.parameter[offset + 2],
                self.parameter[offset + 3],
            ]);
            index += 1;
        }
        let mut curve = [0_u8; 6];
        curve.copy_from_slice(&self.parameter[0x0f1..0x0f7]);
        let mut capacitance = [0_u8; 6];
        capacitance.copy_from_slice(&self.parameter[0x0dc..0x0e2]);

        crate::phy_channel::PhyChipChannelParameters {
            frequency_offset: i16::from_le_bytes([self.parameter[0x020], self.parameter[0x021]]),
            crystal_selector: self.parameter[0x04f],
            channel_14_mic_enabled: self.parameter[0x026] != 0,
            dot11p_enabled: self.parameter[0x028] != 0,
            dot11p_config: self.parameter[0x029],
            tx_gain_skip_publication: self.parameter[0x007] != 0,
            tx_gain_seed: seed,
            tx_gain_config: u16::from_le_bytes([self.parameter[0x0d0], self.parameter[0x0d1]]),
            tx_gain_curve: curve,
            tx_gain_correction: self.parameter[0x0f7] as i8,
            tx_gain_base: self.parameter[0x123],
            tx_gain_delta: self.parameter[0x1b2],
            tx_capacitance: capacitance,
        }
    }

    /// Commit the three persistent software-state effects of a successful
    /// channel transition only after its unconditional AGC/BBPLL/DC cleanup.
    pub fn apply_channel_outcome(&mut self, outcome: crate::phy_channel::PhyChipChannelOutcome) {
        let channel = outcome.channel.to_le_bytes();
        self.parameter[0x11c] = channel[0];
        self.parameter[0x11d] = channel[1];
        self.parameter[0x11e] = u8::from(outcome.init_complete);
        self.parameter[0x11f] = outcome.cbw;
        self.apply_temperature_outcome(outcome.temperature);
    }

    /// Commit the sole software-state effect of a completed
    /// `phy_check_rx_sat` measurement.
    ///
    /// The pinned body only sets `phy_param[0x1ae]` when at least one of the
    /// 100 samples reports activity. A zero result does not clear an existing
    /// value. Failed PBus or capture operations must be handled by the parent
    /// and never receive permission to mutate the owned parameter image.
    pub fn apply_rx_saturation_outcome(
        &mut self,
        outcome: crate::phy_rx_saturation::PhyRxSaturationOutcome,
    ) -> Result<(), crate::phy_rx_saturation::PhyRxSaturationOutcome> {
        match outcome {
            crate::phy_rx_saturation::PhyRxSaturationOutcome::Measured {
                saturated_samples,
                ..
            } => {
                if saturated_samples != 0 {
                    self.parameter[0x1ae] = 1;
                }
                Ok(())
            }
            failure => Err(failure),
        }
    }

    /// Apply the exact 71-byte mapping from the 128-byte S31 init profile.
    pub fn apply_init_profile(&mut self, init: &[u8; PHY_INIT_DATA_LEN]) {
        apply_init_data(&mut self.parameter, init);
    }

    /// Select the primary heap-free profile: always perform a complete radio
    /// calibration and never restore a vendor calibration record.
    ///
    /// This replaces the persistence/check/recovery branches in
    /// `register_chipv7_phy`. The 40 MHz S31 crystal code is zero and the
    /// complete four-byte calibration flag word is cleared exactly as in the
    /// vendor full-calibration branch.
    pub fn begin_full_wifi_calibration(&mut self, init: &[u8; PHY_INIT_DATA_LEN]) {
        self.apply_init_profile(init);
        self.set_xtal_frequency_mhz(40);
        self.parameter[0x0a4..0x0a8].fill(0);
    }

    /// Commit the complete `phy_get_temp_init(1, 1)` state transform used by
    /// the full-calibration profile after RF and baseband initialization.
    pub fn apply_full_calibration_temperature(
        &mut self,
        outcome: crate::phy_temperature::PhyTemperatureOutcome,
    ) {
        self.apply_temperature_outcome(outcome);
        let temperature = outcome.temperature.to_le_bytes();
        for offset in [0x048, 0x12e, 0x1f8, 0x1fa] {
            self.parameter[offset] = temperature[0];
            self.parameter[offset + 1] = temperature[1];
        }
        self.parameter[0x004] = self.parameter[0x12e];
        self.parameter[0x005] = self.parameter[0x12f];
        self.parameter[0x130] = temperature[0];
        self.parameter[0x131] = temperature[1];
    }

    /// Mark the completed `register_chipv7_phy` cold image.
    pub fn mark_phy_registered(&mut self) {
        self.parameter[0x025] = 1;
    }

    pub const fn phy_registered(&self) -> bool {
        self.parameter[0x025] != 0
    }

    pub fn backup_into(&self, calibration: &mut PhyCalibrationRecord) {
        let mut index = 0;
        while index != PHY_PARAM_LEN {
            calibration.bytes[PHY_CALIBRATION_PAYLOAD_OFFSET + index] = self.parameter[index];
            index += 1;
        }
    }

    pub fn recover_from(&mut self, calibration: &PhyCalibrationRecord) {
        let mut index = 0;
        while index != PHY_PARAM_LEN {
            self.parameter[index] = calibration.bytes[PHY_CALIBRATION_PAYLOAD_OFFSET + index];
            index += 1;
        }
    }

    pub const fn rc_calibration_complete(&self) -> bool {
        self.parameter[0xa6] & 0x80 != 0
    }

    pub fn apply_rc_calibration(&mut self, result: u8) {
        apply_rc_calibration_result(&mut self.parameter, result);
    }

    pub const fn filter_dcap_parameters(&self) -> FilterDcapParameters {
        FilterDcapParameters::new(
            self.parameter[0xe9],
            self.parameter[0xea],
            self.parameter[0xed],
            self.parameter[0xee],
            self.parameter[0xf0],
        )
    }

    pub const fn xtal_duty_parameters(&self) -> XtalDutyCalibrationParameters {
        XtalDutyCalibrationParameters {
            rf_frequency_offset_base: self.parameter[0x4f],
            pbus_rx_path_value: self.parameter[0x002],
        }
    }

    pub const fn channel_frequency_control(&self) -> PhyChannelFrequencyInitControl {
        PhyChannelFrequencyInitControl {
            frequency_register_parameter_override: self.parameter[0x193] != 0,
            frequency_table_initialized: self.parameter[0xa4] & 0x20 != 0,
            front_end_parameter_bit: self.parameter[0x1af] != 0,
        }
    }

    pub fn set_xtal_frequency_mhz(&mut self, frequency_mhz: u32) {
        self.parameter[0x4f] = xtal_parameter_code(frequency_mhz);
    }

    fn synchronize_success(&mut self, outcome: PhyRfInitPrefixOutcome) {
        let PhyRfInitPrefixOutcome::ChannelFrequencyInitialized {
            bbpll_register_snapshot,
            parameter,
            xtal_duty,
            channel_frequency,
            ..
        } = outcome
        else {
            return;
        };

        self.parameter[0x4a] = bbpll_register_snapshot;
        self.parameter[0x18e] = parameter.parameter_18e();
        self.parameter[0x19e] = xtal_duty.initial_duty;
        self.parameter[0x19f] = xtal_duty.low_frequency.best_candidate;
        self.parameter[0x1a0] = xtal_duty.high_frequency.best_candidate;
        if channel_frequency.table_is_initialized {
            self.parameter[0xa4] |= 0x20;
        } else {
            self.parameter[0xa4] &= !0x20;
        }
    }
}

impl Default for PhyColdState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyColdI2cRequest {
    ReadByte {
        address: PhyI2cAddress,
    },
    ReadMasked {
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
    },
    WriteByte {
        address: PhyI2cAddress,
        value: u8,
    },
    WriteMasked {
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
        value: u8,
    },
}

impl PhyColdI2cRequest {
    pub const fn read_byte(address: PhyI2cAddress) -> Self {
        Self::ReadByte { address }
    }

    pub const fn read_masked(address: PhyI2cAddress, high_bit: u8, low_bit: u8) -> Option<Self> {
        if high_bit < 8 && low_bit <= high_bit {
            Some(Self::ReadMasked {
                address,
                high_bit,
                low_bit,
            })
        } else {
            None
        }
    }

    pub const fn write_byte(address: PhyI2cAddress, value: u8) -> Self {
        Self::WriteByte { address, value }
    }

    pub const fn write_masked(
        address: PhyI2cAddress,
        high_bit: u8,
        low_bit: u8,
        value: u8,
    ) -> Option<Self> {
        if high_bit < 8 && low_bit <= high_bit {
            Some(Self::WriteMasked {
                address,
                high_bit,
                low_bit,
                value,
            })
        } else {
            None
        }
    }

    const fn address(self) -> PhyI2cAddress {
        match self {
            Self::ReadByte { address }
            | Self::ReadMasked { address, .. }
            | Self::WriteByte { address, .. }
            | Self::WriteMasked { address, .. } => address,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyColdI2cAction {
    StartRead { address: PhyI2cAddress },
    AwaitReadCompletionEdge { address: PhyI2cAddress },
    StartWrite { address: PhyI2cAddress, value: u8 },
    AwaitWriteCompletionEdge { address: PhyI2cAddress },
    Complete(PhyColdI2cOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyColdI2cOutcome {
    Read { address: PhyI2cAddress, value: u8 },
    Written { address: PhyI2cAddress },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyColdI2cObservation {
    /// The externally delivered edge arrived before the peripheral completed.
    ///
    /// The transaction remains unchanged and does not arrange another wake.
    /// Only a new hardware edge or an outer deadline may call the observation
    /// method again.
    StillPending,
    EdgeConsumed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyColdI2cError {
    BusyAtStart,
    WrongEdge,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyColdI2cPhase {
    StartRead,
    AwaitRead,
    StartWrite(u8),
    AwaitWrite,
    Complete(PhyColdI2cOutcome),
}

/// One nonblocking PHY-I2C transaction, including masked read/modify/write.
///
/// Start and completion are different states.  Observing `Busy` after an
/// externally delivered edge leaves the state at `Await*` and returns
/// [`PhyColdI2cObservation::StillPending`]; it does not spin, retry, register a
/// waker, or request an executor poll.  A separate owner must provide either a
/// later hardware edge or a deadline.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyColdI2cTransaction {
    request: PhyColdI2cRequest,
    phase: PhyColdI2cPhase,
}

impl PhyColdI2cTransaction {
    pub const fn new(request: PhyColdI2cRequest) -> Self {
        let phase = match request {
            PhyColdI2cRequest::ReadByte { .. }
            | PhyColdI2cRequest::ReadMasked { .. }
            | PhyColdI2cRequest::WriteMasked { .. } => PhyColdI2cPhase::StartRead,
            PhyColdI2cRequest::WriteByte { value, .. } => PhyColdI2cPhase::StartWrite(value),
        };
        Self { request, phase }
    }

    pub const fn action(&self) -> PhyColdI2cAction {
        let address = self.request.address();
        match self.phase {
            PhyColdI2cPhase::StartRead => PhyColdI2cAction::StartRead { address },
            PhyColdI2cPhase::AwaitRead => PhyColdI2cAction::AwaitReadCompletionEdge { address },
            PhyColdI2cPhase::StartWrite(value) => PhyColdI2cAction::StartWrite { address, value },
            PhyColdI2cPhase::AwaitWrite => PhyColdI2cAction::AwaitWriteCompletionEdge { address },
            PhyColdI2cPhase::Complete(outcome) => PhyColdI2cAction::Complete(outcome),
        }
    }

    pub fn read_started(&mut self) -> Result<(), PhyColdI2cError> {
        if self.phase != PhyColdI2cPhase::StartRead {
            return Err(self.phase_error());
        }
        self.phase = PhyColdI2cPhase::AwaitRead;
        Ok(())
    }

    pub fn write_started(&mut self) -> Result<(), PhyColdI2cError> {
        if !matches!(self.phase, PhyColdI2cPhase::StartWrite(_)) {
            return Err(self.phase_error());
        }
        self.phase = PhyColdI2cPhase::AwaitWrite;
        Ok(())
    }

    pub fn observe_read_result(
        &mut self,
        result: Result<u8, PhyI2cError>,
    ) -> Result<PhyColdI2cObservation, PhyColdI2cError> {
        if self.phase != PhyColdI2cPhase::AwaitRead {
            return Err(self.phase_error());
        }
        let value = match result {
            Ok(value) => value,
            Err(PhyI2cError::Busy) => return Ok(PhyColdI2cObservation::StillPending),
        };

        let address = self.request.address();
        self.phase = match self.request {
            PhyColdI2cRequest::ReadByte { .. } => {
                PhyColdI2cPhase::Complete(PhyColdI2cOutcome::Read { address, value })
            }
            PhyColdI2cRequest::ReadMasked {
                high_bit, low_bit, ..
            } => PhyColdI2cPhase::Complete(PhyColdI2cOutcome::Read {
                address,
                value: extract_field(value, high_bit, low_bit),
            }),
            PhyColdI2cRequest::WriteMasked {
                high_bit,
                low_bit,
                value: field_value,
                ..
            } => PhyColdI2cPhase::StartWrite(replace_field(value, high_bit, low_bit, field_value)),
            PhyColdI2cRequest::WriteByte { .. } => return Err(PhyColdI2cError::WrongEdge),
        };
        Ok(PhyColdI2cObservation::EdgeConsumed)
    }

    pub fn observe_write_result(
        &mut self,
        result: Result<(), PhyI2cError>,
    ) -> Result<PhyColdI2cObservation, PhyColdI2cError> {
        if self.phase != PhyColdI2cPhase::AwaitWrite {
            return Err(self.phase_error());
        }
        match result {
            Ok(()) => {
                self.phase = PhyColdI2cPhase::Complete(PhyColdI2cOutcome::Written {
                    address: self.request.address(),
                });
                Ok(PhyColdI2cObservation::EdgeConsumed)
            }
            Err(PhyI2cError::Busy) => Ok(PhyColdI2cObservation::StillPending),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn start_target(&mut self, registers: &mut RadioRegisters) -> Result<(), PhyColdI2cError> {
        match self.action() {
            PhyColdI2cAction::StartRead { address } => {
                crate::phy_i2c::try_start_read(registers, address)
                    .map_err(|PhyI2cError::Busy| PhyColdI2cError::BusyAtStart)?;
                self.read_started()
            }
            PhyColdI2cAction::StartWrite { address, value } => {
                crate::phy_i2c::try_start_write(registers, address, value)
                    .map_err(|PhyI2cError::Busy| PhyColdI2cError::BusyAtStart)?;
                self.write_started()
            }
            PhyColdI2cAction::Complete(_) => Err(PhyColdI2cError::AlreadyComplete),
            _ => Err(PhyColdI2cError::WrongEdge),
        }
    }

    /// Consume exactly one independently delivered target completion edge.
    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge(
        &mut self,
        registers: &RadioRegisters,
    ) -> Result<PhyColdI2cObservation, PhyColdI2cError> {
        match self.action() {
            PhyColdI2cAction::AwaitReadCompletionEdge { address } => {
                self.observe_read_result(crate::phy_i2c::try_finish_read(registers, address))
            }
            PhyColdI2cAction::AwaitWriteCompletionEdge { address } => {
                self.observe_write_result(crate::phy_i2c::try_finish_write(registers, address))
            }
            PhyColdI2cAction::Complete(_) => Err(PhyColdI2cError::AlreadyComplete),
            _ => Err(PhyColdI2cError::WrongEdge),
        }
    }

    const fn phase_error(&self) -> PhyColdI2cError {
        if matches!(self.phase, PhyColdI2cPhase::Complete(_)) {
            PhyColdI2cError::AlreadyComplete
        } else {
            PhyColdI2cError::WrongEdge
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyColdLoweringError {
    UnsupportedAction,
    IncompleteTransaction,
    UnexpectedOutcome,
}

/// Identity-bound lowering of one RF-init action to one PHY-I2C transaction.
///
/// The original action remains part of the binding until the transaction is
/// complete. This prevents a completion from being reused for a later action
/// which happens to address the same PHY-I2C register.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyColdI2cBinding {
    outer_action: PhyRfInitPrefixAction,
    transaction: PhyColdI2cTransaction,
}

impl PhyColdI2cBinding {
    pub fn new(outer_action: PhyRfInitPrefixAction) -> Result<Self, PhyColdLoweringError> {
        let request = lower_prefix_i2c_request(outer_action)
            .ok_or(PhyColdLoweringError::UnsupportedAction)?;
        Ok(Self {
            outer_action,
            transaction: PhyColdI2cTransaction::new(request),
        })
    }

    pub const fn outer_action(&self) -> PhyRfInitPrefixAction {
        self.outer_action
    }

    pub const fn action(&self) -> PhyColdI2cAction {
        self.transaction.action()
    }

    pub fn read_started(&mut self) -> Result<(), PhyColdI2cError> {
        self.transaction.read_started()
    }

    pub fn write_started(&mut self) -> Result<(), PhyColdI2cError> {
        self.transaction.write_started()
    }

    pub fn observe_read_result(
        &mut self,
        result: Result<u8, PhyI2cError>,
    ) -> Result<PhyColdI2cObservation, PhyColdI2cError> {
        self.transaction.observe_read_result(result)
    }

    pub fn observe_write_result(
        &mut self,
        result: Result<(), PhyI2cError>,
    ) -> Result<PhyColdI2cObservation, PhyColdI2cError> {
        self.transaction.observe_write_result(result)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn start_target(&mut self, registers: &mut RadioRegisters) -> Result<(), PhyColdI2cError> {
        self.transaction.start_target(registers)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge(
        &mut self,
        registers: &RadioRegisters,
    ) -> Result<PhyColdI2cObservation, PhyColdI2cError> {
        self.transaction.observe_target_edge(registers)
    }

    pub fn into_completion(self) -> Result<PhyRfInitPrefixCompletion, PhyColdLoweringError> {
        let PhyColdI2cAction::Complete(outcome) = self.transaction.action() else {
            return Err(PhyColdLoweringError::IncompleteTransaction);
        };
        lower_prefix_i2c_completion(self.outer_action, outcome)
            .ok_or(PhyColdLoweringError::UnexpectedOutcome)
    }
}

fn checked_masked_read(
    address: PhyI2cAddress,
    high_bit: u8,
    low_bit: u8,
) -> Option<PhyColdI2cRequest> {
    PhyColdI2cRequest::read_masked(address, high_bit, low_bit)
}

fn checked_masked_write(
    address: PhyI2cAddress,
    high_bit: u8,
    low_bit: u8,
    value: u8,
) -> Option<PhyColdI2cRequest> {
    PhyColdI2cRequest::write_masked(address, high_bit, low_bit, value)
}

fn lower_prefix_i2c_request(action: PhyRfInitPrefixAction) -> Option<PhyColdI2cRequest> {
    match action {
        PhyRfInitPrefixAction::Bias(BiasRegAction::Write { address, value })
        | PhyRfInitPrefixAction::FilterDcap(FilterDcapAction::Write { address, value })
        | PhyRfInitPrefixAction::I2cInit1(I2cInit1Action::Write { address, value })
        | PhyRfInitPrefixAction::Sar2Init(Sar2InitAction::WriteByte { address, value })
        | PhyRfInitPrefixAction::I2cBbpll(I2cBbpllAction::WriteByte { address, value })
        | PhyRfInitPrefixAction::AdcRate(AdcRateAction::WriteI2c { address, value })
        | PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::WriteByte {
            address,
            value,
        })
        | PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::Rfpll {
            action: RfpllFrequencyAction::WriteByte { address, value },
            ..
        })
        | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::WriteByte { address, value },
        ))
        | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::Rfpll(
                RfpllFrequencyAction::WriteByte { address, value },
            )),
        )) => Some(PhyColdI2cRequest::write_byte(address, value)),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Search(XtalDutySearchAction::WriteCandidate { address, candidate }),
        )) => Some(PhyColdI2cRequest::write_byte(address, candidate)),
        PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::ReadSdmSample { address })
        | PhyRfInitPrefixAction::ReadParameter18e { address }
        | PhyRfInitPrefixAction::I2cBbpll(I2cBbpllAction::ReadMaskedByte { address })
        | PhyRfInitPrefixAction::I2cBbpll(I2cBbpllAction::ReadSnapshot { address })
        | PhyRfInitPrefixAction::AdcRate(AdcRateAction::ReadI2c { address })
        | PhyRfInitPrefixAction::RfpllChargePump(RfpllChargePumpAction::ReadByte { address })
        | PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::ReadByte {
            address,
        })
        | PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::Rfpll {
            action: RfpllFrequencyAction::ReadByte { address },
            ..
        })
        | PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::I2c(
            PhyFrequencyI2cAction::ReadByte { address },
        ))
        | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::Rfpll(
                RfpllFrequencyAction::ReadByte { address },
            )),
        )) => Some(PhyColdI2cRequest::read_byte(address)),
        PhyRfInitPrefixAction::RcCalibrationSet(RcCalibrationSetAction::MaskedWrite(
            MaskedI2cWriteAction::ReadByte { address },
        )) => Some(PhyColdI2cRequest::read_byte(address)),
        PhyRfInitPrefixAction::RcCalibrationSet(RcCalibrationSetAction::MaskedWrite(
            MaskedI2cWriteAction::WriteByte { address, value },
        )) => Some(PhyColdI2cRequest::write_byte(address, value)),
        PhyRfInitPrefixAction::RcCalibration(RcCalibrationAction::WriteMasked {
            address,
            high_bit,
            low_bit,
            value,
        })
        | PhyRfInitPrefixAction::RfpllChargePump(RfpllChargePumpAction::WriteMasked {
            address,
            high_bit,
            low_bit,
            value,
        })
        | PhyRfInitPrefixAction::Sar2Init(Sar2InitAction::WriteMasked {
            address,
            high_bit,
            low_bit,
            value,
        })
        | PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::WriteMasked {
            address,
            high_bit,
            low_bit,
            value,
        })
        | PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::Rfpll {
            action:
                RfpllFrequencyAction::WriteMasked {
                    address,
                    high_bit,
                    low_bit,
                    value,
                },
            ..
        })
        | PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::I2c(
            PhyFrequencyI2cAction::WriteMasked {
                address,
                high_bit,
                low_bit,
                value,
            },
        ))
        | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::DisableCalibrationPath {
            address,
            high_bit,
            low_bit,
            value,
        })
        | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::WriteMasked {
                address,
                high_bit,
                low_bit,
                value,
            },
        ))
        | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::Rfpll(
                RfpllFrequencyAction::WriteMasked {
                    address,
                    high_bit,
                    low_bit,
                    value,
                },
            )),
        )) => checked_masked_write(address, high_bit, low_bit, value),
        PhyRfInitPrefixAction::RcCalibration(RcCalibrationAction::ReadMasked {
            address,
            high_bit,
            low_bit,
        })
        | PhyRfInitPrefixAction::RfpllChargePump(RfpllChargePumpAction::ReadMasked {
            address,
            high_bit,
            low_bit,
        })
        | PhyRfInitPrefixAction::ReadMasked69 {
            address,
            high_bit,
            low_bit,
        }
        | PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::Rfpll {
            action:
                RfpllFrequencyAction::ReadMasked {
                    address,
                    high_bit,
                    low_bit,
                },
            ..
        })
        | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::ReadInitialDuty {
            address,
            high_bit,
            low_bit,
        })
        | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::Rfpll(
                RfpllFrequencyAction::ReadMasked {
                    address,
                    high_bit,
                    low_bit,
                },
            )),
        )) => checked_masked_read(address, high_bit, low_bit),
        _ => None,
    }
}

fn lower_prefix_i2c_completion(
    action: PhyRfInitPrefixAction,
    outcome: PhyColdI2cOutcome,
) -> Option<PhyRfInitPrefixCompletion> {
    match (action, outcome) {
        (
            PhyRfInitPrefixAction::Bias(BiasRegAction::Write { address, .. }),
            PhyColdI2cOutcome::Written { address: completed },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::Bias(
            BiasRegCompletion::WriteCompleted { address },
        )),
        (
            PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::ReadSdmSample { address }),
            PhyColdI2cOutcome::Read {
                address: completed,
                value,
            },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::OpenI2cXpd(
            OpenI2cXpdCompletion::SdmSample(value),
        )),
        (
            PhyRfInitPrefixAction::I2cBbpll(
                I2cBbpllAction::ReadMaskedByte { address }
                | I2cBbpllAction::ReadSnapshot { address },
            ),
            PhyColdI2cOutcome::Read {
                address: completed,
                value,
            },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::I2cBbpll(
            I2cBbpllCompletion::I2cReadCompleted { address, value },
        )),
        (
            PhyRfInitPrefixAction::I2cBbpll(I2cBbpllAction::WriteByte { address, .. }),
            PhyColdI2cOutcome::Written { address: completed },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::I2cBbpll(
            I2cBbpllCompletion::I2cWriteCompleted { address },
        )),
        (
            PhyRfInitPrefixAction::AdcRate(AdcRateAction::ReadI2c { address }),
            PhyColdI2cOutcome::Read {
                address: completed,
                value,
            },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::AdcRate(
            AdcRateCompletion::I2cReadCompleted { address, value },
        )),
        (
            PhyRfInitPrefixAction::AdcRate(AdcRateAction::WriteI2c { address, .. }),
            PhyColdI2cOutcome::Written { address: completed },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::AdcRate(
            AdcRateCompletion::I2cWriteCompleted { address },
        )),
        (
            PhyRfInitPrefixAction::RcCalibrationSet(RcCalibrationSetAction::MaskedWrite(
                MaskedI2cWriteAction::ReadByte { address },
            )),
            PhyColdI2cOutcome::Read {
                address: completed,
                value,
            },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::RcCalibrationSet(
            RcCalibrationSetCompletion::MaskedWrite(MaskedI2cWriteCompletion::I2cReadCompleted {
                address,
                value,
            }),
        )),
        (
            PhyRfInitPrefixAction::RcCalibrationSet(RcCalibrationSetAction::MaskedWrite(
                MaskedI2cWriteAction::WriteByte { address, .. },
            )),
            PhyColdI2cOutcome::Written { address: completed },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::RcCalibrationSet(
            RcCalibrationSetCompletion::MaskedWrite(MaskedI2cWriteCompletion::I2cWriteCompleted {
                address,
            }),
        )),
        (
            PhyRfInitPrefixAction::RcCalibration(RcCalibrationAction::WriteMasked { .. }),
            PhyColdI2cOutcome::Written { .. },
        ) => Some(PhyRfInitPrefixCompletion::RcCalibration(
            RcCalibrationCompletion::Write,
        )),
        (
            PhyRfInitPrefixAction::RcCalibration(RcCalibrationAction::ReadMasked { .. }),
            PhyColdI2cOutcome::Read { value, .. },
        ) => Some(PhyRfInitPrefixCompletion::RcCalibration(
            RcCalibrationCompletion::Read(value),
        )),
        (
            PhyRfInitPrefixAction::FilterDcap(FilterDcapAction::Write { address, .. }),
            PhyColdI2cOutcome::Written { address: completed },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::FilterDcap(
            FilterDcapCompletion::WriteCompleted { address },
        )),
        (
            PhyRfInitPrefixAction::ReadParameter18e { address },
            PhyColdI2cOutcome::Read {
                address: completed,
                value,
            },
        ) if address == completed => {
            Some(PhyRfInitPrefixCompletion::Parameter18eRead { address, value })
        }
        (
            PhyRfInitPrefixAction::I2cInit1(I2cInit1Action::Write { address, .. }),
            PhyColdI2cOutcome::Written { address: completed },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::I2cInit1(
            I2cInit1Completion::WriteCompleted { address },
        )),
        (
            PhyRfInitPrefixAction::RfpllChargePump(RfpllChargePumpAction::WriteMasked { .. }),
            PhyColdI2cOutcome::Written { .. },
        ) => Some(PhyRfInitPrefixCompletion::RfpllChargePump(
            RfpllChargePumpCompletion::Write,
        )),
        (
            PhyRfInitPrefixAction::RfpllChargePump(RfpllChargePumpAction::ReadMasked { .. }),
            PhyColdI2cOutcome::Read { value, .. },
        ) => Some(PhyRfInitPrefixCompletion::RfpllChargePump(
            RfpllChargePumpCompletion::ReadMasked(value),
        )),
        (
            PhyRfInitPrefixAction::RfpllChargePump(RfpllChargePumpAction::ReadByte { address }),
            PhyColdI2cOutcome::Read {
                address: completed,
                value,
            },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::RfpllChargePump(
            RfpllChargePumpCompletion::ReadByte { address, value },
        )),
        (PhyRfInitPrefixAction::ReadMasked69 { .. }, PhyColdI2cOutcome::Read { value, .. }) => {
            Some(PhyRfInitPrefixCompletion::Masked69Read(value))
        }
        (
            PhyRfInitPrefixAction::Sar2Init(Sar2InitAction::WriteMasked { .. }),
            PhyColdI2cOutcome::Written { .. },
        ) => Some(PhyRfInitPrefixCompletion::Sar2Init(
            Sar2InitCompletion::MaskedWrite,
        )),
        (
            PhyRfInitPrefixAction::Sar2Init(Sar2InitAction::WriteByte { address, .. }),
            PhyColdI2cOutcome::Written { address: completed },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::Sar2Init(
            Sar2InitCompletion::ByteWrite { address },
        )),
        (
            PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::WriteMasked {
                address,
                high_bit,
                low_bit,
                ..
            }),
            PhyColdI2cOutcome::Written { address: completed },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::ChannelFrequency(
            PhyChannelFrequencyInitCompletion::MaskedWrite {
                address,
                high_bit,
                low_bit,
            },
        )),
        (
            PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::WriteByte {
                address,
                ..
            }),
            PhyColdI2cOutcome::Written { address: completed },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::ChannelFrequency(
            PhyChannelFrequencyInitCompletion::ByteWrite { address },
        )),
        (
            PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::ReadByte {
                address,
            }),
            PhyColdI2cOutcome::Read {
                address: completed,
                value,
            },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::ChannelFrequency(
            PhyChannelFrequencyInitCompletion::ByteRead { address, value },
        )),
        (
            PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::Rfpll {
                action:
                    RfpllFrequencyAction::WriteMasked {
                        address,
                        high_bit,
                        low_bit,
                        ..
                    },
                ..
            }),
            PhyColdI2cOutcome::Written { address: completed },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::ChannelFrequency(
            PhyChannelFrequencyInitCompletion::Rfpll(RfpllFrequencyCompletion::MaskedWrite {
                address,
                high_bit,
                low_bit,
            }),
        )),
        (
            PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::Rfpll {
                action: RfpllFrequencyAction::WriteByte { address, .. },
                ..
            }),
            PhyColdI2cOutcome::Written { address: completed },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::ChannelFrequency(
            PhyChannelFrequencyInitCompletion::Rfpll(RfpllFrequencyCompletion::ByteWrite {
                address,
            }),
        )),
        (
            PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::Rfpll {
                action:
                    RfpllFrequencyAction::ReadMasked {
                        address,
                        high_bit,
                        low_bit,
                    },
                ..
            }),
            PhyColdI2cOutcome::Read {
                address: completed,
                value,
            },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::ChannelFrequency(
            PhyChannelFrequencyInitCompletion::Rfpll(RfpllFrequencyCompletion::MaskedRead {
                address,
                high_bit,
                low_bit,
                value,
            }),
        )),
        (
            PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::Rfpll {
                action: RfpllFrequencyAction::ReadByte { address },
                ..
            }),
            PhyColdI2cOutcome::Read {
                address: completed,
                value,
            },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::ChannelFrequency(
            PhyChannelFrequencyInitCompletion::Rfpll(RfpllFrequencyCompletion::ByteRead {
                address,
                value,
            }),
        )),
        (
            PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::I2c(
                PhyFrequencyI2cAction::WriteMasked {
                    address,
                    high_bit,
                    low_bit,
                    ..
                },
            )),
            PhyColdI2cOutcome::Written { address: completed },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::ChannelFrequency(
            PhyChannelFrequencyInitCompletion::I2c(PhyFrequencyI2cCompletion::MaskedWrite {
                address,
                high_bit,
                low_bit,
            }),
        )),
        (
            PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::I2c(
                PhyFrequencyI2cAction::ReadByte { address },
            )),
            PhyColdI2cOutcome::Read {
                address: completed,
                value,
            },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::ChannelFrequency(
            PhyChannelFrequencyInitCompletion::I2c(PhyFrequencyI2cCompletion::ByteRead {
                address,
                value,
            }),
        )),
        (
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::ReadInitialDuty {
                address,
                ..
            }),
            PhyColdI2cOutcome::Read {
                address: completed,
                value,
            },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::InitialDutyRead { address, value },
        )),
        (
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::DisableCalibrationPath {
                address,
                ..
            }),
            PhyColdI2cOutcome::Written { address: completed },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::CalibrationPathDisabled { address },
        )),
        (
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::WriteMasked { address, .. },
            )),
            PhyColdI2cOutcome::Written { address: completed },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::MaskedWrite { address }),
        )),
        (
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::WriteByte { address, .. },
            )),
            PhyColdI2cOutcome::Written { address: completed },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::ByteWrite { address }),
        )),
        (
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::Rfpll(
                    RfpllFrequencyAction::WriteMasked {
                        address,
                        high_bit,
                        low_bit,
                        ..
                    },
                )),
            )),
            PhyColdI2cOutcome::Written { address: completed },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::Rfpll(RfpllFrequencyCompletion::MaskedWrite {
                    address,
                    high_bit,
                    low_bit,
                }),
            )),
        )),
        (
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::Rfpll(
                    RfpllFrequencyAction::WriteByte { address, .. },
                )),
            )),
            PhyColdI2cOutcome::Written { address: completed },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::Rfpll(RfpllFrequencyCompletion::ByteWrite { address }),
            )),
        )),
        (
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::Rfpll(
                    RfpllFrequencyAction::ReadMasked {
                        address,
                        high_bit,
                        low_bit,
                    },
                )),
            )),
            PhyColdI2cOutcome::Read {
                address: completed,
                value,
            },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::Rfpll(RfpllFrequencyCompletion::MaskedRead {
                    address,
                    high_bit,
                    low_bit,
                    value,
                }),
            )),
        )),
        (
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::Rfpll(
                    RfpllFrequencyAction::ReadByte { address },
                )),
            )),
            PhyColdI2cOutcome::Read {
                address: completed,
                value,
            },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::Rfpll(RfpllFrequencyCompletion::ByteRead {
                    address,
                    value,
                }),
            )),
        )),
        (
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Search(XtalDutySearchAction::WriteCandidate {
                    address,
                    candidate,
                }),
            )),
            PhyColdI2cOutcome::Written { address: completed },
        ) if address == completed => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                XtalDutySearchCompletion::CandidateWritten { address, candidate },
            )),
        )),
        _ => None,
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyColdMmioBinding {
    outer_action: PhyRfInitPrefixAction,
}

impl PhyColdMmioBinding {
    pub fn new(outer_action: PhyRfInitPrefixAction) -> Result<Self, PhyColdLoweringError> {
        if lower_prefix_mmio_completion(outer_action).is_none() {
            return Err(PhyColdLoweringError::UnsupportedAction);
        }
        Ok(Self { outer_action })
    }

    pub const fn outer_action(&self) -> PhyRfInitPrefixAction {
        self.outer_action
    }

    pub fn into_completion(self) -> Result<PhyRfInitPrefixCompletion, PhyColdLoweringError> {
        lower_prefix_mmio_completion(self.outer_action)
            .ok_or(PhyColdLoweringError::UnsupportedAction)
    }

    /// Execute exactly one finite target MMIO transaction and consume its
    /// identity token.
    #[cfg(target_arch = "riscv32")]
    pub unsafe fn execute_target(
        self,
        registers: &mut RadioRegisters,
    ) -> Result<PhyRfInitPrefixCompletion, PhyColdLoweringError> {
        match self.outer_action {
            PhyRfInitPrefixAction::ConfigureFeBbClock => {
                crate::radio_hal::wifi_strict_phy_open_fe_bb_clk()
            }
            PhyRfInitPrefixAction::ConfigureBbpllCalibration { enabled } => {
                open_esp_radio_hal_esp32s31::phy_i2c::configure_bbpll_calibration(
                    registers, enabled,
                )
            }
            PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::ConfigurePreDelay) => {
                crate::phy_i2c::configure_open_i2c_pre_delay(registers)
            }
            PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ConfigureDebugMode) => {
                open_esp_radio_hal_esp32s31::pbus::configure_debug_mode(registers)
            }
            PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ConfigureWorkModePulse) => {
                open_esp_radio_hal_esp32s31::phy_agc::configure_pbus_work_mode_pulse(registers)
            }
            PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ClearWorkModePulse) => {
                open_esp_radio_hal_esp32s31::phy_agc::clear_pbus_work_mode_pulse(registers)
            }
            PhyRfInitPrefixAction::ConfigureI2cClockSelection { selection } => {
                open_esp_radio_hal_esp32s31::phy_i2c::configure_clock_selection(
                    registers, selection,
                )
            }
            PhyRfInitPrefixAction::AdcRate(AdcRateAction::ConfigureMmio { rate }) => {
                crate::radio_hal::configure_phy_adc_rate(rate)
            }
            PhyRfInitPrefixAction::ConfigureI2cMasterRegisters => {
                open_esp_radio_hal_esp32s31::phy_i2c::configure_master_registers(registers)
            }
            PhyRfInitPrefixAction::ConfigurePowerDetectorRegisters => {
                crate::radio_hal::configure_phy_power_detector_registers()
            }
            PhyRfInitPrefixAction::ConfigureFrontEndRegisters => {
                crate::radio_hal::configure_phy_front_end_registers(registers)
            }
            PhyRfInitPrefixAction::ConfigureTemperatureSensorRead => {
                crate::radio_hal::configure_phy_temperature_sensor_read()
            }
            PhyRfInitPrefixAction::ConfigureTxPowerControlBackground => {
                crate::radio_hal::configure_phy_tx_power_control_background()
            }
            PhyRfInitPrefixAction::ConfigureI2cMasterCommandMemory { parameter } => {
                crate::phy_i2c::configure_i2c_master_command_memory(registers, parameter)
            }
            PhyRfInitPrefixAction::ConfigureFrontEndRegisterUpdate => {
                crate::radio_hal::configure_phy_front_end_update()
            }
            PhyRfInitPrefixAction::ChannelFrequency(
                PhyChannelFrequencyInitAction::ConfigureFrequencyRegisters { parameter_override },
            ) => crate::radio_hal::configure_phy_frequency_registers(parameter_override),
            PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::Table(
                PhyFrequencyTableAction::WriteMemory {
                    address,
                    value,
                    mode,
                    ..
                },
            ))
            | PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::I2c(
                PhyFrequencyI2cAction::WriteMemory {
                    address,
                    value,
                    mode,
                    ..
                },
            )) => crate::radio_hal::write_phy_frequency_memory(address, value, mode),
            PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::I2c(
                PhyFrequencyI2cAction::ConfigureNumberAddresses(image),
            )) => crate::radio_hal::configure_phy_frequency_i2c_number_addresses(
                image.control_field,
                image.words,
            ),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::ConfigureCalibrationTone {
                    enabled,
                    selector,
                    step,
                }),
            )) => crate::radio_hal::configure_phy_calibration_tone(enabled, selector, step),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::ConfigureRxClock { enabled }),
            )) => open_esp_radio_hal_esp32s31::pbus::configure_rx_clock(registers, enabled),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::ConfigureTxClock { enabled }),
            )) => open_esp_radio_hal_esp32s31::pbus::configure_tx_clock(registers, enabled),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::ConfigurePbusDebugMode),
            )) => open_esp_radio_hal_esp32s31::pbus::configure_debug_mode(registers),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RestoreRxDcoControl {
                    saved_field,
                    ..
                }),
            ))
            | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                    PhyRxDcoAction::RestoreRxDcoControl { saved_field, .. },
                )),
            )) => crate::radio_hal::restore_phy_rx_dco_control_field(saved_field),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
                    PhyDcIqAction::Configure(request),
                ))),
            )) => crate::radio_hal::configure_phy_dc_iq_estimator(request.control),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
                    PhyDcIqAction::SetEnable { phase, enabled, .. },
                ))),
            ))
            | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                    PhySignalPowerAction::SetEstimatorEnable { phase, enabled, .. },
                )),
            )) => crate::radio_hal::set_phy_dc_iq_estimator_enable(phase, enabled),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                    PhySignalPowerAction::ConfigureClock { clock, enabled, .. },
                )),
            )) => match clock {
                crate::phy_signal_power::PhySignalPowerClock::Tx => {
                    open_esp_radio_hal_esp32s31::pbus::configure_tx_clock(registers, enabled)
                }
                crate::phy_signal_power::PhySignalPowerClock::Rx => {
                    open_esp_radio_hal_esp32s31::pbus::configure_rx_clock(registers, enabled)
                }
            },
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                    PhySignalPowerAction::ConfigureEstimator { control, .. },
                )),
            )) => crate::radio_hal::configure_phy_dc_iq_estimator(control),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Restore(XtalDutyRestoreAction::ConfigureCalibrationTone {
                    enabled,
                    selector,
                    step,
                }),
            )) => crate::radio_hal::configure_phy_calibration_tone(enabled, selector, step),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Restore(XtalDutyRestoreAction::ConfigureRxClock { enabled }),
            )) => open_esp_radio_hal_esp32s31::pbus::configure_rx_clock(registers, enabled),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Restore(XtalDutyRestoreAction::ConfigureTxClock { enabled }),
            )) => open_esp_radio_hal_esp32s31::pbus::configure_tx_clock(registers, enabled),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Restore(XtalDutyRestoreAction::ConfigurePbusWorkModePulse),
            )) => open_esp_radio_hal_esp32s31::phy_agc::configure_pbus_work_mode_pulse(registers),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Restore(XtalDutyRestoreAction::ClearPbusWorkModePulse),
            )) => open_esp_radio_hal_esp32s31::phy_agc::clear_pbus_work_mode_pulse(registers),
            _ => return Err(PhyColdLoweringError::UnsupportedAction),
        }
        self.into_completion()
    }
}

fn lower_prefix_mmio_completion(
    action: PhyRfInitPrefixAction,
) -> Option<PhyRfInitPrefixCompletion> {
    match action {
        PhyRfInitPrefixAction::ConfigureFeBbClock => {
            Some(PhyRfInitPrefixCompletion::FeBbClockConfigured)
        }
        PhyRfInitPrefixAction::ConfigureBbpllCalibration { .. } => {
            Some(PhyRfInitPrefixCompletion::BbpllCalibrationConfigured)
        }
        PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::ConfigurePreDelay) => Some(
            PhyRfInitPrefixCompletion::OpenI2cXpd(OpenI2cXpdCompletion::PreDelayConfigured),
        ),
        PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ConfigureDebugMode) => Some(
            PhyRfInitPrefixCompletion::PbusClear(PhyPbusClearCompletion::DebugModeConfigured),
        ),
        PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ConfigureWorkModePulse) => Some(
            PhyRfInitPrefixCompletion::PbusClear(PhyPbusClearCompletion::WorkModePulseConfigured),
        ),
        PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ClearWorkModePulse) => Some(
            PhyRfInitPrefixCompletion::PbusClear(PhyPbusClearCompletion::WorkModePulseCleared),
        ),
        PhyRfInitPrefixAction::ConfigureI2cClockSelection { .. } => {
            Some(PhyRfInitPrefixCompletion::I2cClockSelectionConfigured)
        }
        PhyRfInitPrefixAction::AdcRate(AdcRateAction::ConfigureMmio { .. }) => Some(
            PhyRfInitPrefixCompletion::AdcRate(AdcRateCompletion::MmioConfigured),
        ),
        PhyRfInitPrefixAction::ConfigureI2cMasterRegisters => {
            Some(PhyRfInitPrefixCompletion::I2cMasterRegistersConfigured)
        }
        PhyRfInitPrefixAction::ConfigurePowerDetectorRegisters => {
            Some(PhyRfInitPrefixCompletion::PowerDetectorRegistersConfigured)
        }
        PhyRfInitPrefixAction::ConfigureFrontEndRegisters => {
            Some(PhyRfInitPrefixCompletion::FrontEndRegistersConfigured)
        }
        PhyRfInitPrefixAction::ConfigureTemperatureSensorRead => {
            Some(PhyRfInitPrefixCompletion::TemperatureSensorReadConfigured)
        }
        PhyRfInitPrefixAction::ConfigureTxPowerControlBackground => {
            Some(PhyRfInitPrefixCompletion::TxPowerControlBackgroundConfigured)
        }
        PhyRfInitPrefixAction::ConfigureI2cMasterCommandMemory { .. } => {
            Some(PhyRfInitPrefixCompletion::I2cMasterCommandMemoryConfigured)
        }
        PhyRfInitPrefixAction::ConfigureFrontEndRegisterUpdate => {
            Some(PhyRfInitPrefixCompletion::FrontEndRegisterUpdateConfigured)
        }
        PhyRfInitPrefixAction::ChannelFrequency(
            PhyChannelFrequencyInitAction::ConfigureFrequencyRegisters { parameter_override },
        ) => Some(PhyRfInitPrefixCompletion::ChannelFrequency(
            PhyChannelFrequencyInitCompletion::FrequencyRegistersConfigured { parameter_override },
        )),
        PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::Table(
            PhyFrequencyTableAction::WriteMemory {
                entry_index,
                word_index,
                address,
                ..
            },
        )) => Some(PhyRfInitPrefixCompletion::ChannelFrequency(
            PhyChannelFrequencyInitCompletion::Table(PhyFrequencyTableCompletion {
                entry_index,
                word_index,
                address,
            }),
        )),
        PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::I2c(
            PhyFrequencyI2cAction::WriteMemory {
                descriptor_index,
                copy_index,
                address,
                ..
            },
        )) => Some(PhyRfInitPrefixCompletion::ChannelFrequency(
            PhyChannelFrequencyInitCompletion::I2c(PhyFrequencyI2cCompletion::MemoryWrite {
                descriptor_index,
                copy_index,
                address,
            }),
        )),
        PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::I2c(
            PhyFrequencyI2cAction::ConfigureNumberAddresses(image),
        )) => Some(PhyRfInitPrefixCompletion::ChannelFrequency(
            PhyChannelFrequencyInitCompletion::I2c(
                PhyFrequencyI2cCompletion::NumberAddressesConfigured(image),
            ),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::ConfigureCalibrationTone {
                enabled,
                selector,
                step,
            }),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::CalibrationToneConfigured {
                    enabled,
                    selector,
                    step,
                },
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::ConfigureRxClock { enabled }),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::RxClockConfigured { enabled },
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::ConfigureTxClock { enabled }),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::TxClockConfigured { enabled },
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::ConfigurePbusDebugMode),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::PbusDebugModeConfigured,
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RestoreRxDcoControl {
                address,
                saved_field,
                ..
            }),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::RxDcoControlRestored {
                    address,
                    saved_field,
                },
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                PhyRxDcoAction::RestoreRxDcoControl {
                    address,
                    saved_field,
                    ..
                },
            )),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::RxDcoControlRestored {
                    address,
                    saved_field,
                }),
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
                PhyDcIqAction::Configure(request),
            ))),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DcIq(
                    PhyDcIqCompletion::Configured(request),
                )),
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
                PhyDcIqAction::SetEnable {
                    request,
                    phase,
                    enabled,
                },
            ))),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DcIq(
                    PhyDcIqCompletion::EnableSet {
                        request,
                        phase,
                        enabled,
                    },
                )),
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                PhySignalPowerAction::ConfigureClock {
                    request,
                    clock,
                    enabled,
                },
            )),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                XtalDutySearchCompletion::SignalPower(PhySignalPowerCompletion::ClockConfigured {
                    request,
                    clock,
                    enabled,
                }),
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                PhySignalPowerAction::SetEstimatorEnable {
                    request,
                    phase,
                    enabled,
                },
            )),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                XtalDutySearchCompletion::SignalPower(
                    PhySignalPowerCompletion::EstimatorEnableSet {
                        request,
                        phase,
                        enabled,
                    },
                ),
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                PhySignalPowerAction::ConfigureEstimator { request, control },
            )),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                XtalDutySearchCompletion::SignalPower(
                    PhySignalPowerCompletion::EstimatorConfigured { request, control },
                ),
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Restore(XtalDutyRestoreAction::ConfigureCalibrationTone {
                enabled,
                selector,
                step,
            }),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                XtalDutyRestoreCompletion::CalibrationToneConfigured {
                    enabled,
                    selector,
                    step,
                },
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Restore(XtalDutyRestoreAction::ConfigureRxClock { enabled }),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                XtalDutyRestoreCompletion::RxClockConfigured { enabled },
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Restore(XtalDutyRestoreAction::ConfigureTxClock { enabled }),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                XtalDutyRestoreCompletion::TxClockConfigured { enabled },
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Restore(XtalDutyRestoreAction::ConfigurePbusWorkModePulse),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                XtalDutyRestoreCompletion::PbusWorkModePulseConfigured,
            )),
        )),
        PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Restore(XtalDutyRestoreAction::ClearPbusWorkModePulse),
        )) => Some(PhyRfInitPrefixCompletion::XtalDuty(
            XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                XtalDutyRestoreCompletion::PbusWorkModePulseCleared,
            )),
        )),
        _ => None,
    }
}

/// One timer edge belonging to one exact RF-init action.
///
/// The value owns no timer implementation and cannot wake itself. The outer
/// Rust executor arms its timer from [`micros`](Self::micros), then consumes
/// this binding only when that timer reports expiry.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyColdTimerBinding {
    outer_action: PhyRfInitPrefixAction,
    micros: u32,
}

impl PhyColdTimerBinding {
    pub fn new(outer_action: PhyRfInitPrefixAction) -> Result<Self, PhyColdLoweringError> {
        let micros = match outer_action {
            PhyRfInitPrefixAction::DelayMicros(micros)
            | PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::DelayMicros(micros))
            | PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::DelayMicros(micros))
            | PhyRfInitPrefixAction::RcCalibration(RcCalibrationAction::DelayMicros(micros))
            | PhyRfInitPrefixAction::RfpllChargePump(RfpllChargePumpAction::DelayMicros(micros))
            | PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::Rfpll {
                action: RfpllFrequencyAction::DelayMicros(micros),
                ..
            }) => micros,
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::Rfpll(
                    RfpllFrequencyAction::DelayMicros(micros),
                )),
            ))
            | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                    PhyRxDcoAction::DelayMicros { micros, .. },
                )),
            ))
            | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
                    PhyDcIqAction::DelayMicros { micros, .. },
                ))),
            ))
            | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Search(XtalDutySearchAction::DelayMicros { micros, .. }),
            ))
            | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                    PhySignalPowerAction::DelayMicros { micros, .. },
                )),
            ))
            | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Restore(XtalDutyRestoreAction::DelayMicros(micros)),
            )) => micros,
            _ => return Err(PhyColdLoweringError::UnsupportedAction),
        };
        Ok(Self {
            outer_action,
            micros,
        })
    }

    pub const fn outer_action(&self) -> PhyRfInitPrefixAction {
        self.outer_action
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub fn into_elapsed_completion(
        self,
    ) -> Result<PhyRfInitPrefixCompletion, PhyColdLoweringError> {
        match self.outer_action {
            PhyRfInitPrefixAction::DelayMicros(_) => Ok(PhyRfInitPrefixCompletion::DelayElapsed),
            PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::DelayMicros(_)) => Ok(
                PhyRfInitPrefixCompletion::OpenI2cXpd(OpenI2cXpdCompletion::DelayElapsed),
            ),
            PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::DelayMicros(_)) => Ok(
                PhyRfInitPrefixCompletion::PbusClear(PhyPbusClearCompletion::DelayElapsed),
            ),
            PhyRfInitPrefixAction::RcCalibration(RcCalibrationAction::DelayMicros(_)) => Ok(
                PhyRfInitPrefixCompletion::RcCalibration(RcCalibrationCompletion::Delay),
            ),
            PhyRfInitPrefixAction::RfpllChargePump(RfpllChargePumpAction::DelayMicros(_)) => Ok(
                PhyRfInitPrefixCompletion::RfpllChargePump(RfpllChargePumpCompletion::Delay),
            ),
            PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::Rfpll {
                action: RfpllFrequencyAction::DelayMicros(micros),
                ..
            }) => Ok(PhyRfInitPrefixCompletion::ChannelFrequency(
                PhyChannelFrequencyInitCompletion::Rfpll(RfpllFrequencyCompletion::DelayElapsed(
                    micros,
                )),
            )),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::Rfpll(
                    RfpllFrequencyAction::DelayMicros(micros),
                )),
            )) => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::Rfpll(RfpllFrequencyCompletion::DelayElapsed(
                        micros,
                    )),
                )),
            )),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                    PhyRxDcoAction::DelayMicros { iteration, micros },
                )),
            )) => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DelayElapsed {
                        iteration,
                        micros,
                    }),
                )),
            )),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
                    PhyDcIqAction::DelayMicros {
                        request,
                        phase,
                        micros,
                    },
                ))),
            )) => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DcIq(
                        PhyDcIqCompletion::DelayElapsed {
                            request,
                            phase,
                            micros,
                        },
                    )),
                )),
            )),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Search(XtalDutySearchAction::DelayMicros { candidate, .. }),
            )) => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                    XtalDutySearchCompletion::DelayElapsed { candidate },
                )),
            )),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                    PhySignalPowerAction::DelayMicros {
                        request,
                        phase,
                        micros,
                    },
                )),
            )) => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                    XtalDutySearchCompletion::SignalPower(PhySignalPowerCompletion::DelayElapsed {
                        request,
                        phase,
                        micros,
                    }),
                )),
            )),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Restore(XtalDutyRestoreAction::DelayMicros(micros)),
            )) => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                    XtalDutyRestoreCompletion::DelayElapsed { micros },
                )),
            )),
            _ => Err(PhyColdLoweringError::UnsupportedAction),
        }
    }
}

/// Exactly one lowered external operation owned by the cold-init executor.
///
/// Unsupported nested actions are rejected during construction; there is no
/// generic vendor callback or synchronous fallback variant.
#[derive(Debug, Eq, PartialEq)]
pub enum PhyColdExternalBinding {
    I2c(PhyColdI2cBinding),
    Mmio(PhyColdMmioBinding),
    Observation(PhyColdObservationBinding),
    Pbus(PhyColdPbusBinding),
    Timer(PhyColdTimerBinding),
}

impl PhyColdExternalBinding {
    pub fn lower(action: PhyRfInitPrefixAction) -> Result<Self, PhyColdLoweringError> {
        if let Ok(binding) = PhyColdI2cBinding::new(action) {
            return Ok(Self::I2c(binding));
        }
        if let Ok(binding) = PhyColdMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        if let Ok(binding) = PhyColdPbusBinding::new(action) {
            return Ok(Self::Pbus(binding));
        }
        if let Ok(binding) = PhyColdObservationBinding::new(action) {
            return Ok(Self::Observation(binding));
        }
        if let Ok(binding) = PhyColdTimerBinding::new(action) {
            return Ok(Self::Timer(binding));
        }
        Err(PhyColdLoweringError::UnsupportedAction)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyColdObservationRequest {
    ConfigureOpenI2cPowerAndPulse,
    CheckOpenI2cSdmDeadline {
        started_at_cycle: u32,
        maximum_cycles: u32,
    },
    ConfigurePbusWorkMode,
    MaskRxDcoControl {
        address: usize,
        clear_mask: u32,
    },
    ReadRxDcoPbus {
        selector: u8,
        path: u8,
    },
    ObserveDcIqReadiness {
        request: PhyDcIqEstimateRequest,
        readiness_activity_edges: u16,
    },
    ReadDcIqAccumulators(PhyDcIqEstimateRequest),
    ObserveSignalPowerReadiness {
        request: PhySignalPowerRequest,
        readiness_activity_edges: u16,
    },
    ReadSignalPowerAccumulators(PhySignalPowerRequest),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyColdObservationResult {
    OpenI2cPowerAndPulse {
        started_at_cycle: u32,
    },
    OpenI2cSdmDeadline {
        expired: bool,
    },
    PbusWorkMode {
        settle_required: bool,
    },
    RxDcoControlMasked {
        address: usize,
        saved_field: u32,
    },
    RxDcoPbusRead {
        selector: u8,
        path: u8,
        value: u32,
    },
    DcIqReadiness {
        request: PhyDcIqEstimateRequest,
        snapshot: PhyDcIqReadinessSnapshot,
    },
    DcIqAccumulators {
        request: PhyDcIqEstimateRequest,
        snapshot: PhyDcIqAccumulatorSnapshot,
    },
    SignalPowerReadiness {
        request: PhySignalPowerRequest,
        snapshot: PhyDcIqReadinessSnapshot,
    },
    SignalPowerAccumulators {
        request: PhySignalPowerRequest,
        snapshot: PhySignalPowerAccumulatorSnapshot,
    },
}

/// One finite MMIO operation whose sampled value is part of the completion.
///
/// This is separate from [`PhyColdMmioBinding`] so a dynamic register sample
/// cannot be fabricated by constructing a fixed completion. Consuming the
/// binding returns the observation to exactly the parent action that requested
/// it.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyColdObservationBinding {
    outer_action: PhyRfInitPrefixAction,
    request: PhyColdObservationRequest,
}

impl PhyColdObservationBinding {
    pub fn new(outer_action: PhyRfInitPrefixAction) -> Result<Self, PhyColdLoweringError> {
        let request = match outer_action {
            PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::ConfigurePowerAndPulse) => {
                PhyColdObservationRequest::ConfigureOpenI2cPowerAndPulse
            }
            PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::CheckSdmDeadline {
                started_at_cycle,
                maximum_cycles,
            }) => PhyColdObservationRequest::CheckOpenI2cSdmDeadline {
                started_at_cycle,
                maximum_cycles,
            },
            PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ConfigureWorkMode)
            | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Restore(XtalDutyRestoreAction::ConfigurePbusWorkMode),
            )) => PhyColdObservationRequest::ConfigurePbusWorkMode,
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::MaskRxDcoControl {
                    address,
                    clear_mask,
                }),
            ))
            | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                    PhyRxDcoAction::MaskRxDcoControl {
                        address,
                        clear_mask,
                    },
                )),
            )) if address == crate::phy_rx_dco::RX_DCO_CONTROL_ADDRESS
                && clear_mask == crate::phy_rx_dco::RX_DCO_CONTROL_FIELD_MASK =>
            {
                PhyColdObservationRequest::MaskRxDcoControl {
                    address,
                    clear_mask,
                }
            }
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                    PhyRxDcoAction::ReadPbus { selector, path },
                )),
            )) if selector == 1 && path == 2 => {
                PhyColdObservationRequest::ReadRxDcoPbus { selector, path }
            }
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
                    PhyDcIqAction::AwaitReadinessEdge {
                        request,
                        readiness_activity_edges,
                    },
                ))),
            )) => PhyColdObservationRequest::ObserveDcIqReadiness {
                request,
                readiness_activity_edges,
            },
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
                    PhyDcIqAction::ReadAccumulators(request),
                ))),
            )) => PhyColdObservationRequest::ReadDcIqAccumulators(request),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                    PhySignalPowerAction::AwaitReadinessEdge {
                        request,
                        readiness_activity_edges,
                    },
                )),
            )) => PhyColdObservationRequest::ObserveSignalPowerReadiness {
                request,
                readiness_activity_edges,
            },
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                    PhySignalPowerAction::ReadAccumulators(request),
                )),
            )) => PhyColdObservationRequest::ReadSignalPowerAccumulators(request),
            _ => return Err(PhyColdLoweringError::UnsupportedAction),
        };
        Ok(Self {
            outer_action,
            request,
        })
    }

    pub const fn outer_action(&self) -> PhyRfInitPrefixAction {
        self.outer_action
    }

    pub const fn request(&self) -> PhyColdObservationRequest {
        self.request
    }

    pub fn into_completion(
        self,
        result: PhyColdObservationResult,
    ) -> Result<PhyRfInitPrefixCompletion, PhyColdLoweringError> {
        match (self.outer_action, result) {
            (
                PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::ConfigurePowerAndPulse),
                PhyColdObservationResult::OpenI2cPowerAndPulse { started_at_cycle },
            ) => Ok(PhyRfInitPrefixCompletion::OpenI2cXpd(
                OpenI2cXpdCompletion::PowerAndPulseConfigured { started_at_cycle },
            )),
            (
                PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::CheckSdmDeadline { .. }),
                PhyColdObservationResult::OpenI2cSdmDeadline { expired },
            ) => Ok(PhyRfInitPrefixCompletion::OpenI2cXpd(
                OpenI2cXpdCompletion::DeadlineObserved { expired },
            )),
            (
                PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ConfigureWorkMode),
                PhyColdObservationResult::PbusWorkMode { settle_required },
            ) => Ok(PhyRfInitPrefixCompletion::PbusClear(
                PhyPbusClearCompletion::WorkModeConfigured { settle_required },
            )),
            (
                PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                    XtalDutyPassAction::Restore(XtalDutyRestoreAction::ConfigurePbusWorkMode),
                )),
                PhyColdObservationResult::PbusWorkMode { settle_required },
            ) => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                    XtalDutyRestoreCompletion::PbusWorkModeConfigured { settle_required },
                )),
            )),
            (
                PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                    XtalDutyPassAction::Prepare(XtalDutyPrepareAction::MaskRxDcoControl {
                        address,
                        ..
                    }),
                )),
                PhyColdObservationResult::RxDcoControlMasked {
                    address: completed,
                    saved_field,
                },
            ) if address == completed => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDcoControlMasked {
                        address,
                        saved_field,
                    },
                )),
            )),
            (
                PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                    XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                        PhyRxDcoAction::MaskRxDcoControl { address, .. },
                    )),
                )),
                PhyColdObservationResult::RxDcoControlMasked {
                    address: completed,
                    saved_field,
                },
            ) if address == completed => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::RxDcoControlMasked {
                        address,
                        saved_field,
                    }),
                )),
            )),
            (
                PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                    XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                        PhyRxDcoAction::ReadPbus { selector, path },
                    )),
                )),
                PhyColdObservationResult::RxDcoPbusRead {
                    selector: completed_selector,
                    path: completed_path,
                    value,
                },
            ) if selector == completed_selector && path == completed_path => {
                Ok(PhyRfInitPrefixCompletion::XtalDuty(
                    XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                        XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::PbusRead {
                            selector,
                            path,
                            value,
                        }),
                    )),
                ))
            }
            (
                PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                    XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                        PhyRxDcoAction::DcIq(PhyDcIqAction::AwaitReadinessEdge { request, .. }),
                    )),
                )),
                PhyColdObservationResult::DcIqReadiness {
                    request: completed,
                    snapshot,
                },
            ) if request == completed => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DcIq(
                        PhyDcIqCompletion::ReadinessObserved { request, snapshot },
                    )),
                )),
            )),
            (
                PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                    XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                        PhyRxDcoAction::DcIq(PhyDcIqAction::ReadAccumulators(request)),
                    )),
                )),
                PhyColdObservationResult::DcIqAccumulators {
                    request: completed,
                    snapshot,
                },
            ) if request == completed => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DcIq(
                        PhyDcIqCompletion::AccumulatorsRead { request, snapshot },
                    )),
                )),
            )),
            (
                PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                    XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                        PhySignalPowerAction::AwaitReadinessEdge { request, .. },
                    )),
                )),
                PhyColdObservationResult::SignalPowerReadiness {
                    request: completed,
                    snapshot,
                },
            ) if request == completed => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                    XtalDutySearchCompletion::SignalPower(
                        PhySignalPowerCompletion::ReadinessObserved { request, snapshot },
                    ),
                )),
            )),
            (
                PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                    XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                        PhySignalPowerAction::ReadAccumulators(request),
                    )),
                )),
                PhyColdObservationResult::SignalPowerAccumulators {
                    request: completed,
                    snapshot,
                },
            ) if request == completed => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                    XtalDutySearchCompletion::SignalPower(
                        PhySignalPowerCompletion::AccumulatorsRead { request, snapshot },
                    ),
                )),
            )),
            _ => Err(PhyColdLoweringError::UnexpectedOutcome),
        }
    }

    /// Consume an independently owned Rust deadline for a readiness action.
    ///
    /// Ordinary sampled observations cannot fabricate a timeout. Conversely,
    /// only the two readiness actions accept this completion; fixed MMIO
    /// samples and the open-I2C deadline fail closed.
    pub fn into_timeout_completion(
        self,
    ) -> Result<PhyRfInitPrefixCompletion, PhyColdLoweringError> {
        match self.outer_action {
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
                    PhyDcIqAction::AwaitReadinessEdge { request, .. },
                ))),
            )) => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DcIq(
                        PhyDcIqCompletion::ReadinessTimedOut(request),
                    )),
                )),
            )),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                    PhySignalPowerAction::AwaitReadinessEdge { request, .. },
                )),
            )) => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                    XtalDutySearchCompletion::SignalPower(
                        PhySignalPowerCompletion::ReadinessTimedOut(request),
                    ),
                )),
            )),
            _ => Err(PhyColdLoweringError::UnsupportedAction),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub unsafe fn execute_target(
        self,
        registers: &mut RadioRegisters,
    ) -> Result<PhyRfInitPrefixCompletion, PhyColdLoweringError> {
        match self.request {
            PhyColdObservationRequest::ConfigureOpenI2cPowerAndPulse => {
                crate::phy_i2c::configure_open_i2c_power_and_pulse(registers);
                let started_at_cycle = crate::radio_hal::read_phy_sdm_cycle_counter();
                self.into_completion(PhyColdObservationResult::OpenI2cPowerAndPulse {
                    started_at_cycle,
                })
            }
            PhyColdObservationRequest::CheckOpenI2cSdmDeadline {
                started_at_cycle,
                maximum_cycles,
            } => {
                let current_cycle = crate::radio_hal::read_phy_sdm_cycle_counter();
                self.into_completion(PhyColdObservationResult::OpenI2cSdmDeadline {
                    expired: phy_sdm_deadline_expired(
                        started_at_cycle,
                        current_cycle,
                        maximum_cycles,
                    ),
                })
            }
            PhyColdObservationRequest::ConfigurePbusWorkMode => {
                let settle_required =
                    open_esp_radio_hal_esp32s31::pbus::configure_work_mode(registers);
                self.into_completion(PhyColdObservationResult::PbusWorkMode { settle_required })
            }
            PhyColdObservationRequest::MaskRxDcoControl { address, .. } => {
                let saved_field = crate::radio_hal::mask_phy_rx_dco_control_field();
                self.into_completion(PhyColdObservationResult::RxDcoControlMasked {
                    address,
                    saved_field,
                })
            }
            PhyColdObservationRequest::ReadRxDcoPbus { selector, path } => {
                let value = u32::from(
                    open_esp_radio_hal_esp32s31::pbus::read_result(registers, selector, path)
                        .ok_or(PhyColdLoweringError::UnexpectedOutcome)?,
                );
                self.into_completion(PhyColdObservationResult::RxDcoPbusRead {
                    selector,
                    path,
                    value,
                })
            }
            PhyColdObservationRequest::ObserveDcIqReadiness { request, .. } => {
                let snapshot = crate::radio_hal::sample_phy_dc_iq_readiness();
                self.into_completion(PhyColdObservationResult::DcIqReadiness { request, snapshot })
            }
            PhyColdObservationRequest::ReadDcIqAccumulators(request) => {
                let snapshot = crate::radio_hal::read_phy_dc_iq_accumulators();
                self.into_completion(PhyColdObservationResult::DcIqAccumulators {
                    request,
                    snapshot,
                })
            }
            PhyColdObservationRequest::ObserveSignalPowerReadiness { request, .. } => {
                let snapshot = crate::radio_hal::sample_phy_dc_iq_readiness();
                self.into_completion(PhyColdObservationResult::SignalPowerReadiness {
                    request,
                    snapshot,
                })
            }
            PhyColdObservationRequest::ReadSignalPowerAccumulators(request) => {
                let snapshot = crate::radio_hal::read_phy_signal_power_accumulators();
                self.into_completion(PhyColdObservationResult::SignalPowerAccumulators {
                    request,
                    snapshot,
                })
            }
        }
    }
}

const fn phy_sdm_deadline_expired(
    started_at_cycle: u32,
    current_cycle: u32,
    maximum_cycles: u32,
) -> bool {
    current_cycle.wrapping_sub(started_at_cycle) > maximum_cycles
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyColdPbusAction {
    Start(PhyPbusForceTest),
    AwaitCompletionEdge(PhyPbusForceTest),
    Complete(PhyPbusForceTest),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyColdPbusObservation {
    StillPending,
    EdgeConsumed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyColdPbusHardwareResult {
    Busy,
    Completed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyColdPbusError {
    BusyAtStart,
    WrongEdge,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyColdPbusPhase {
    Start,
    AwaitCompletionEdge,
    Complete,
}

/// One uniquely owned PBus command and its independently delivered edge.
///
/// `Busy` after an observation preserves `AwaitCompletionEdge`; the binding
/// does not retry, poll, or arrange another wake. An outer deadline may
/// instead consume the binding through [`into_timeout_completion`].
#[derive(Debug, Eq, PartialEq)]
pub struct PhyColdPbusBinding {
    outer_action: PhyRfInitPrefixAction,
    transaction: PhyPbusForceTest,
    phase: PhyColdPbusPhase,
}

impl PhyColdPbusBinding {
    pub fn new(outer_action: PhyRfInitPrefixAction) -> Result<Self, PhyColdLoweringError> {
        let transaction = match outer_action {
            PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ForceTest(transaction)) => {
                transaction
            }
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::ForcePbus(transaction)),
            ))
            | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                    PhyRxDcoAction::ForcePbus(transaction),
                )),
            ))
            | PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Restore(XtalDutyRestoreAction::ForcePbus(transaction)),
            )) => transaction,
            _ => return Err(PhyColdLoweringError::UnsupportedAction),
        };
        Ok(Self {
            outer_action,
            transaction,
            phase: PhyColdPbusPhase::Start,
        })
    }

    pub const fn outer_action(&self) -> PhyRfInitPrefixAction {
        self.outer_action
    }

    pub const fn action(&self) -> PhyColdPbusAction {
        match self.phase {
            PhyColdPbusPhase::Start => PhyColdPbusAction::Start(self.transaction),
            PhyColdPbusPhase::AwaitCompletionEdge => {
                PhyColdPbusAction::AwaitCompletionEdge(self.transaction)
            }
            PhyColdPbusPhase::Complete => PhyColdPbusAction::Complete(self.transaction),
        }
    }

    pub fn started(&mut self) -> Result<(), PhyColdPbusError> {
        match self.phase {
            PhyColdPbusPhase::Start => {
                self.phase = PhyColdPbusPhase::AwaitCompletionEdge;
                Ok(())
            }
            PhyColdPbusPhase::AwaitCompletionEdge => Err(PhyColdPbusError::WrongEdge),
            PhyColdPbusPhase::Complete => Err(PhyColdPbusError::AlreadyComplete),
        }
    }

    pub fn observe_result(
        &mut self,
        result: PhyColdPbusHardwareResult,
    ) -> Result<PhyColdPbusObservation, PhyColdPbusError> {
        match self.phase {
            PhyColdPbusPhase::AwaitCompletionEdge
                if result == PhyColdPbusHardwareResult::Completed =>
            {
                self.phase = PhyColdPbusPhase::Complete;
                Ok(PhyColdPbusObservation::EdgeConsumed)
            }
            PhyColdPbusPhase::AwaitCompletionEdge => Ok(PhyColdPbusObservation::StillPending),
            PhyColdPbusPhase::Start => Err(PhyColdPbusError::WrongEdge),
            PhyColdPbusPhase::Complete => Err(PhyColdPbusError::AlreadyComplete),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn start_target(&mut self, registers: &mut RadioRegisters) -> Result<(), PhyColdPbusError> {
        if self.phase != PhyColdPbusPhase::Start {
            return Err(if self.phase == PhyColdPbusPhase::Complete {
                PhyColdPbusError::AlreadyComplete
            } else {
                PhyColdPbusError::WrongEdge
            });
        }
        open_esp_radio_hal_esp32s31::pbus::try_start_force_test(
            registers,
            self.transaction.selector(),
            self.transaction.path(),
            self.transaction.value(),
        )
        .map_err(|error| match error {
            open_esp_radio_hal_esp32s31::pbus::PbusError::Busy => PhyColdPbusError::BusyAtStart,
            _ => PhyColdPbusError::WrongEdge,
        })?;
        self.started()
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge(
        &mut self,
        registers: &mut RadioRegisters,
    ) -> Result<PhyColdPbusObservation, PhyColdPbusError> {
        if self.phase != PhyColdPbusPhase::AwaitCompletionEdge {
            return Err(if self.phase == PhyColdPbusPhase::Complete {
                PhyColdPbusError::AlreadyComplete
            } else {
                PhyColdPbusError::WrongEdge
            });
        }
        match open_esp_radio_hal_esp32s31::pbus::try_finish_force_test(registers) {
            Ok(()) => self.observe_result(PhyColdPbusHardwareResult::Completed),
            Err(open_esp_radio_hal_esp32s31::pbus::PbusError::Busy) => {
                self.observe_result(PhyColdPbusHardwareResult::Busy)
            }
            Err(_) => Err(PhyColdPbusError::WrongEdge),
        }
    }

    pub fn into_completion(self) -> Result<PhyRfInitPrefixCompletion, PhyColdLoweringError> {
        if self.phase != PhyColdPbusPhase::Complete {
            return Err(PhyColdLoweringError::IncompleteTransaction);
        }
        match self.outer_action {
            PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ForceTest(transaction))
                if transaction == self.transaction =>
            {
                Ok(PhyRfInitPrefixCompletion::PbusClear(
                    PhyPbusClearCompletion::ForceTestCompleted(transaction),
                ))
            }
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::ForcePbus(transaction)),
            )) if transaction == self.transaction => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::PbusForceCompleted(transaction),
                )),
            )),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                    PhyRxDcoAction::ForcePbus(transaction),
                )),
            )) if transaction == self.transaction => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::PbusForceCompleted(
                        transaction,
                    )),
                )),
            )),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Restore(XtalDutyRestoreAction::ForcePbus(transaction)),
            )) if transaction == self.transaction => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                    XtalDutyRestoreCompletion::PbusForceCompleted(transaction),
                )),
            )),
            _ => Err(PhyColdLoweringError::UnexpectedOutcome),
        }
    }

    pub fn into_timeout_completion(
        self,
    ) -> Result<PhyRfInitPrefixCompletion, PhyColdLoweringError> {
        if self.phase != PhyColdPbusPhase::AwaitCompletionEdge {
            return Err(PhyColdLoweringError::IncompleteTransaction);
        }
        match self.outer_action {
            PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ForceTest(transaction))
                if transaction == self.transaction =>
            {
                Ok(PhyRfInitPrefixCompletion::PbusClear(
                    PhyPbusClearCompletion::ForceTestTimedOut(transaction),
                ))
            }
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::ForcePbus(transaction)),
            )) if transaction == self.transaction => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::PbusForceTimedOut(transaction),
                )),
            )),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                    PhyRxDcoAction::ForcePbus(transaction),
                )),
            )) if transaction == self.transaction => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::PbusForceTimedOut(
                        transaction,
                    )),
                )),
            )),
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
                XtalDutyPassAction::Restore(XtalDutyRestoreAction::ForcePbus(transaction)),
            )) if transaction == self.transaction => Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                    XtalDutyRestoreCompletion::PbusForceTimedOut(transaction),
                )),
            )),
            _ => Err(PhyColdLoweringError::UnexpectedOutcome),
        }
    }
}

const fn field_mask(high_bit: u8, low_bit: u8) -> u8 {
    let width = high_bit - low_bit + 1;
    ((((1_u16 << width) - 1) << low_bit) & 0xff) as u8
}

const fn extract_field(value: u8, high_bit: u8, low_bit: u8) -> u8 {
    (value & field_mask(high_bit, low_bit)) >> low_bit
}

const fn replace_field(value: u8, high_bit: u8, low_bit: u8, field_value: u8) -> u8 {
    let mask = field_mask(high_bit, low_bit);
    (value & !mask) | ((field_value << low_bit) & mask)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyColdLocalStep {
    /// One finite state-only action was applied.  The caller may consume
    /// another bounded action in the same executor dispatch or yield.
    StateAdvanced,
    /// Hardware, timer, or observation work must be completed externally.
    External(PhyRfInitPrefixAction),
    Complete(PhyRfInitPrefixOutcome),
}

/// Error from the single-owner composition around `phy_rf_init`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyColdTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

impl From<PhyRfInitPrefixTransitionError> for PhyColdTransitionError {
    fn from(error: PhyRfInitPrefixTransitionError) -> Self {
        match error {
            PhyRfInitPrefixTransitionError::WrongCompletion => Self::WrongCompletion,
            PhyRfInitPrefixTransitionError::AlreadyComplete => Self::AlreadyComplete,
        }
    }
}

/// `phy_rf_init` transition and its only mutable software-state owner.
///
/// [`step_local`](Self::step_local) performs at most one finite state action.
/// It never loops, polls hardware, creates a waker, or retries a busy
/// transaction.  All non-local actions are returned verbatim to the target
/// executor and require one identity-bound external completion.
pub struct PhyRfColdInit {
    state: PhyColdState,
    transition: PhyRfInitPrefixTransition,
}

impl PhyRfColdInit {
    pub const fn new(state: PhyColdState) -> Self {
        Self {
            state,
            transition: PhyRfInitPrefixTransition::new(),
        }
    }

    pub const fn state(&self) -> &PhyColdState {
        &self.state
    }

    pub const fn action(&self) -> PhyRfInitPrefixAction {
        self.transition.action()
    }

    pub fn step_local(&mut self) -> Result<PhyColdLocalStep, PhyColdTransitionError> {
        let action = self.transition.action();
        let completion = match action {
            PhyRfInitPrefixAction::InspectRcCalibrationState => {
                PhyRfInitPrefixCompletion::RcCalibrationStateInspected {
                    already_complete: self.state.rc_calibration_complete(),
                }
            }
            PhyRfInitPrefixAction::RcCalibration(RcCalibrationAction::ApplyResult(result)) => {
                self.state.apply_rc_calibration(result);
                PhyRfInitPrefixCompletion::RcCalibration(RcCalibrationCompletion::Applied)
            }
            PhyRfInitPrefixAction::CaptureFilterDcapParameters => {
                PhyRfInitPrefixCompletion::FilterDcapParametersCaptured(
                    self.state.filter_dcap_parameters(),
                )
            }
            PhyRfInitPrefixAction::CaptureXtalDutyParameters => {
                PhyRfInitPrefixCompletion::XtalDutyParametersCaptured(
                    self.state.xtal_duty_parameters(),
                )
            }
            PhyRfInitPrefixAction::CaptureChannelFrequencyControl => {
                PhyRfInitPrefixCompletion::ChannelFrequencyControlCaptured(
                    self.state.channel_frequency_control(),
                )
            }
            PhyRfInitPrefixAction::Complete(outcome) => {
                self.state.synchronize_success(outcome);
                return Ok(PhyColdLocalStep::Complete(outcome));
            }
            external => return Ok(PhyColdLocalStep::External(external)),
        };

        self.transition.advance(completion)?;
        Ok(PhyColdLocalStep::StateAdvanced)
    }

    pub fn advance_external(
        &mut self,
        completion: PhyRfInitPrefixCompletion,
    ) -> Result<(), PhyColdTransitionError> {
        self.transition.advance(completion)?;
        if let PhyRfInitPrefixAction::Complete(outcome) = self.transition.action() {
            self.state.synchronize_success(outcome);
        }
        Ok(())
    }

    pub fn into_state(self) -> PhyColdState {
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::{
        initial_parameter_image, phy_sdm_deadline_expired, PhyCalibrationRecord,
        PhyColdExternalBinding, PhyColdI2cAction, PhyColdI2cBinding, PhyColdI2cObservation,
        PhyColdI2cOutcome, PhyColdI2cRequest, PhyColdI2cTransaction, PhyColdLoweringError,
        PhyColdMmioBinding, PhyColdObservationBinding, PhyColdObservationRequest,
        PhyColdObservationResult, PhyColdPbusAction, PhyColdPbusBinding, PhyColdPbusHardwareResult,
        PhyColdPbusObservation, PhyColdState, PhyColdTimerBinding, PHY_COLD_PARAMETER_LEN,
    };
    use crate::phy_dc_iq::{
        PhyDcIqAccumulatorSnapshot, PhyDcIqAction, PhyDcIqCompletion, PhyDcIqDelayPhase,
        PhyDcIqEnablePhase, PhyDcIqEstimateRequest, PhyDcIqReadinessSnapshot,
    };
    use crate::phy_frequency::{PhyChannelFrequencyInitAction, PhyChannelFrequencyInitCompletion};
    use crate::phy_i2c::{
        BiasRegAction, BiasRegCompletion, OpenI2cXpdAction, OpenI2cXpdCompletion, PhyI2cAddress,
        PhyI2cError, PhyRfInitPrefixAction, PhyRfInitPrefixCompletion, RcCalibrationAction,
        RcCalibrationCompletion,
    };
    use crate::phy_pbus::{PhyPbusClearAction, PhyPbusClearCompletion, PhyPbusForceTest};
    use crate::phy_rfpll::{RfpllFrequencyAction, RfpllFrequencyCompletion};
    use crate::phy_rx_dco::{PhyRxDcoAction, PhyRxDcoCompletion};
    use crate::phy_signal_power::{
        PhySignalPowerAccumulatorSnapshot, PhySignalPowerAction, PhySignalPowerClock,
        PhySignalPowerCompletion, PhySignalPowerRequest,
    };
    use crate::phy_xtal_duty::{
        XtalDutyCalibrationAction, XtalDutyCalibrationCompletion, XtalDutyPassAction,
        XtalDutyPassCompletion, XtalDutyPrepareAction, XtalDutyPrepareCompletion,
        XtalDutyRestoreAction, XtalDutyRestoreCompletion, XtalDutySearchAction,
        XtalDutySearchCompletion,
    };

    #[test]
    fn baseline_matches_the_complete_sparse_vendor_data_image() {
        let image = initial_parameter_image();
        assert_eq!(image.len(), 508);
        let nonzero: std::vec::Vec<_> = image
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, value)| *value != 0)
            .collect();
        assert_eq!(
            nonzero,
            [
                (0x002, 0xbf),
                (0x003, 0x20),
                (0x006, 0x54),
                (0x00b, 0x01),
                (0x00e, 0x60),
                (0x00f, 0x01),
                (0x012, 0x1f),
                (0x013, 0x16),
                (0x014, 0x01),
                (0x015, 0x40),
                (0x016, 0x02),
                (0x018, 0x50),
                (0x024, 0x30),
                (0x1ab, 0x01),
                (0x1af, 0x01),
            ]
        );
    }

    #[test]
    fn state_is_aligned_unique_storage_with_the_exact_abi_size() {
        assert_eq!(PHY_COLD_PARAMETER_LEN, 0x1fc);
        assert_eq!(core::mem::size_of::<PhyColdState>(), 0x1fc);
        assert_eq!(core::mem::align_of::<PhyColdState>(), 4);
        assert!(!core::mem::needs_drop::<PhyColdState>());
    }

    #[test]
    fn rx_table_preparation_mutates_only_the_explicit_owned_parameter() {
        let mut image = initial_parameter_image();
        image[0x121] = 0x4e;
        image[0x120] = 0xa5;
        let mut state = PhyColdState::from_parameter_image(image);

        assert_eq!(
            state.prepare_rx_table_init(),
            crate::phy_bb::PhyRxTableInitParameters {
                parameter_002: 0xbf,
                parameter_121: 0x4e,
            }
        );
        assert_eq!(state.parameter_image()[0x120], 0x4f);
        assert_eq!(state.parameter_image()[0x121], 0x4e);
    }

    #[test]
    fn rx_saturation_commit_is_owned_and_preserves_the_one_way_flag() {
        use crate::phy_rx_saturation::PhyRxSaturationOutcome;

        let mut state = PhyColdState::new();
        assert_eq!(state.rx_saturation_parameter_002(), 0xbf);
        assert_eq!(state.parameter_image()[0x1ae], 0);

        state
            .apply_rx_saturation_outcome(PhyRxSaturationOutcome::Measured {
                saturated_samples: 1,
                samples: 100,
            })
            .unwrap();
        assert_eq!(state.parameter_image()[0x1ae], 1);

        state
            .apply_rx_saturation_outcome(PhyRxSaturationOutcome::Measured {
                saturated_samples: 0,
                samples: 100,
            })
            .unwrap();
        assert_eq!(state.parameter_image()[0x1ae], 1);
    }

    #[test]
    fn pbus_memory_inputs_and_saved_registers_are_explicit_owned_state() {
        use crate::phy_pbus_memory::{PhyPbusMemoryOutcome, PhyPbusMemoryParameters};

        let mut state = PhyColdState::new();
        assert_eq!(
            state.pbus_memory_parameters(),
            PhyPbusMemoryParameters {
                parameter_002: 0xbf,
                parameter_014: 1,
            }
        );

        state.apply_pbus_memory_outcome(PhyPbusMemoryOutcome {
            saved_registers: [
                0x0302_0100,
                0x0706_0504,
                0x0b0a_0908,
                0x0f0e_0d0c,
                0x1312_1110,
                0x1716_1514,
            ],
        });
        assert_eq!(
            &state.parameter_image()[0x30..0x48],
            &[
                0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
                0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17,
            ]
        );
    }

    #[test]
    fn temperature_result_mutates_only_the_explicit_owned_bytes() {
        let mut state = PhyColdState::new();
        let before = *state.parameter_image();
        state.apply_temperature_outcome(crate::phy_temperature::PhyTemperatureOutcome {
            temperature: -37,
            sensor_index: 3,
            next_dac: 11,
        });
        assert_eq!(&state.parameter_image()[0..2], &(-37_i16).to_le_bytes());
        assert_eq!(state.parameter_image()[0x16], 3);
        for index in 0..PHY_COLD_PARAMETER_LEN {
            if index != 0 && index != 1 && index != 0x16 {
                assert_eq!(state.parameter_image()[index], before[index]);
            }
        }
    }

    #[test]
    fn dcode_parameters_and_results_have_one_explicit_owner() {
        let mut state = PhyColdState::new();
        assert_eq!(
            state.dcode_parameters(),
            crate::phy_dcode::PhyDcodeParameters {
                crystal_selector: state.parameter_image()[0x4f],
            }
        );
        state.apply_dcode_outcome(crate::phy_dcode::PhyDcodeOutcome {
            codes: [1, 2, 3, 4, 5, 6, 7, 8],
        });
        assert_eq!(
            &state.parameter_image()[0x1a1..=0x1a8],
            &[1, 2, 3, 4, 5, 6, 7, 8]
        );
    }

    #[test]
    fn pwdet_parameters_and_commit_replace_the_global_parameter_pointer() {
        let mut image = initial_parameter_image();
        image[0x0a7] = 1;
        image[0x0a8..0x0aa].copy_from_slice(&0x0102_u16.to_le_bytes());
        image[0x0aa..0x0ac].copy_from_slice(&0x0304_u16.to_le_bytes());
        image[0x0ac..0x0ae].copy_from_slice(&0x0506_u16.to_le_bytes());
        image[0x0ae..0x0b0].copy_from_slice(&0x0708_u16.to_le_bytes());
        image[0x01a..0x01c].copy_from_slice(&(-19_i16).to_le_bytes());
        image[0x01c..0x01e].copy_from_slice(&(37_i16).to_le_bytes());
        image[0x1aa] = 1;
        let mut state = PhyColdState::from_parameter_image(image);

        assert_eq!(
            state.pwdet_parameters(),
            crate::phy_pwdet::PhyPwdetParameters {
                already_calibrated: true,
                pbus_tx_path_value: 0x1f,
                pbus_rx_path_value: 0xbf,
                dco: [0x0102, 0x0304, 0x0506, 0x0708],
                clear_tone_after_ready: true,
                reference_codes: [-19, 37],
            }
        );

        state.apply_pwdet_outcome(crate::phy_pwdet::PhyPwdetOutcome {
            reference_codes: [-101, 202],
            calibrated: true,
            measurement_performed: true,
        });
        assert_eq!(
            &state.parameter_image()[0x01a..0x01e],
            &[0x9b, 0xff, 0xca, 0x00]
        );
        assert_eq!(state.parameter_image()[0x0a7] & 1, 1);
    }

    #[test]
    fn tx_dc_result_mutates_only_its_owned_five_rows() {
        let mut state = PhyColdState::new();
        let before = *state.parameter_image();
        assert_eq!(
            state.tx_dc_parameters(),
            crate::phy_txdc::PhyTxDcParameters {
                pbus_rx_path_value: before[0x002],
            }
        );
        let outcome = crate::phy_txdc::PhyTxDcOutcome {
            dco: [
                [1, 2, 3, 4],
                [5, 6, 7, 8],
                [9, 10, 11, 12],
                [13, 14, 15, 16],
                [17, 18, 19, 20],
            ],
        };
        state.apply_tx_dc_outcome(outcome);
        for (index, expected) in (1_u16..=20).enumerate() {
            let offset = 0x0a8 + index * 2;
            assert_eq!(
                u16::from_le_bytes([
                    state.parameter_image()[offset],
                    state.parameter_image()[offset + 1],
                ]),
                expected
            );
        }
        for index in 0..PHY_COLD_PARAMETER_LEN {
            if !(0x0a8..0x0d0).contains(&index) {
                assert_eq!(state.parameter_image()[index], before[index]);
            }
        }
    }

    #[test]
    fn failed_rx_saturation_capture_cannot_mutate_owned_state() {
        use crate::phy_rx_saturation::PhyRxSaturationOutcome;

        let mut state = PhyColdState::new();
        let before = *state.parameter_image();
        assert_eq!(
            state.apply_rx_saturation_outcome(PhyRxSaturationOutcome::CaptureTimedOut),
            Err(PhyRxSaturationOutcome::CaptureTimedOut)
        );
        assert_eq!(state.parameter_image(), &before);
    }

    #[test]
    fn typed_views_replace_every_parameter_read_in_the_rf_prefix() {
        let state = PhyColdState::new();
        assert!(!state.rc_calibration_complete());
        assert_eq!(
            state.xtal_duty_parameters(),
            crate::phy_xtal_duty::XtalDutyCalibrationParameters {
                rf_frequency_offset_base: 0,
                pbus_rx_path_value: 0xbf,
            }
        );
        assert_eq!(
            state.channel_frequency_control(),
            crate::phy_frequency::PhyChannelFrequencyInitControl {
                frequency_register_parameter_override: false,
                frequency_table_initialized: false,
                front_end_parameter_bit: true,
            }
        );
    }

    #[test]
    fn init_calibration_and_backup_are_owned_by_one_state_value() {
        let mut state = PhyColdState::new();
        let mut init = [0; super::PHY_COLD_INIT_PROFILE_LEN];
        let mut index = 0;
        while index != init.len() {
            init[index] = index as u8;
            index += 1;
        }
        state.apply_init_profile(&init);
        state.apply_rc_calibration(45);

        let expected = *state.parameter_image();
        let mut record = PhyCalibrationRecord::new();
        state.backup_into(&mut record);

        let mut restored = PhyColdState::new();
        restored.recover_from(&record);
        assert_eq!(restored.parameter_image(), &expected);
        assert!(restored.rc_calibration_complete());
    }

    #[test]
    fn calibration_record_checksum_has_fixed_owned_storage() {
        let state = PhyColdState::new();
        let mut record = PhyCalibrationRecord::new();
        state.backup_into(&mut record);
        record.refresh_header_and_checksum(0x1234_5678, 0xa1b2_c3d4, 0xe5f6_0718);
        assert!(record.checksum_matches(0x1234_5678, 0xa1b2_c3d4, 0xe5f6_0718));

        let mut bytes = *record.bytes();
        bytes[0x40] ^= 0x80;
        let mut corrupted = PhyCalibrationRecord::from_bytes(bytes);
        assert!(!corrupted.checksum_matches(0x1234_5678, 0xa1b2_c3d4, 0xe5f6_0718));

        // Keep the state live until after the record checks so the test also
        // proves no shared global backing was used.
        assert_eq!(state.parameter_image()[0x002], 0xbf);
    }

    #[test]
    fn busy_observation_preserves_await_state_without_self_progress() {
        let address = PhyI2cAddress::new(0x66, 4).unwrap();
        let mut transaction = PhyColdI2cTransaction::new(PhyColdI2cRequest::read_byte(address));
        assert_eq!(
            transaction.action(),
            PhyColdI2cAction::StartRead { address }
        );
        transaction.read_started().unwrap();
        let awaiting = PhyColdI2cAction::AwaitReadCompletionEdge { address };
        assert_eq!(transaction.action(), awaiting);

        assert_eq!(
            transaction.observe_read_result(Err(PhyI2cError::Busy)),
            Ok(PhyColdI2cObservation::StillPending)
        );
        assert_eq!(transaction.action(), awaiting);

        assert_eq!(
            transaction.observe_read_result(Ok(0xa5)),
            Ok(PhyColdI2cObservation::EdgeConsumed)
        );
        assert_eq!(
            transaction.action(),
            PhyColdI2cAction::Complete(PhyColdI2cOutcome::Read {
                address,
                value: 0xa5,
            })
        );
    }

    #[test]
    fn masked_write_needs_two_distinct_external_edges() {
        let address = PhyI2cAddress::new(0x6b, 0x13).unwrap();
        let request = PhyColdI2cRequest::write_masked(address, 5, 2, 9).unwrap();
        let mut transaction = PhyColdI2cTransaction::new(request);

        transaction.read_started().unwrap();
        transaction.observe_read_result(Ok(0xc3)).unwrap();
        assert_eq!(
            transaction.action(),
            PhyColdI2cAction::StartWrite {
                address,
                value: 0xe7,
            }
        );

        transaction.write_started().unwrap();
        assert_eq!(
            transaction.observe_write_result(Err(PhyI2cError::Busy)),
            Ok(PhyColdI2cObservation::StillPending)
        );
        assert_eq!(
            transaction.action(),
            PhyColdI2cAction::AwaitWriteCompletionEdge { address }
        );
        transaction.observe_write_result(Ok(())).unwrap();
        assert_eq!(
            transaction.action(),
            PhyColdI2cAction::Complete(PhyColdI2cOutcome::Written { address })
        );
    }

    #[test]
    fn masked_read_returns_only_the_requested_field() {
        let address = PhyI2cAddress::new(0x62, 0x0e).unwrap();
        let request = PhyColdI2cRequest::read_masked(address, 4, 1).unwrap();
        let mut transaction = PhyColdI2cTransaction::new(request);
        transaction.read_started().unwrap();
        transaction.observe_read_result(Ok(0xb6)).unwrap();
        assert_eq!(
            transaction.action(),
            PhyColdI2cAction::Complete(PhyColdI2cOutcome::Read {
                address,
                value: 0x0b,
            })
        );
    }

    #[test]
    fn binding_retains_the_exact_outer_action_until_completion() {
        let address = PhyI2cAddress::new(0x6a, 0).unwrap();
        let outer_action = PhyRfInitPrefixAction::Bias(BiasRegAction::Write {
            address,
            value: 0xaf,
        });
        let mut binding = PhyColdI2cBinding::new(outer_action).unwrap();
        assert_eq!(binding.outer_action(), outer_action);
        assert_eq!(
            binding.action(),
            PhyColdI2cAction::StartWrite {
                address,
                value: 0xaf,
            }
        );

        binding.write_started().unwrap();
        assert_eq!(
            binding.observe_write_result(Ok(())),
            Ok(PhyColdI2cObservation::EdgeConsumed)
        );
        assert_eq!(
            binding.into_completion(),
            Ok(PhyRfInitPrefixCompletion::Bias(
                BiasRegCompletion::WriteCompleted { address }
            ))
        );
    }

    #[test]
    fn masked_outer_write_is_two_edges_but_one_identity_bound_completion() {
        let address = PhyI2cAddress::new(0x67, 3).unwrap();
        let outer_action = PhyRfInitPrefixAction::RcCalibration(RcCalibrationAction::WriteMasked {
            address,
            high_bit: 6,
            low_bit: 4,
            value: 5,
        });
        let mut binding = PhyColdI2cBinding::new(outer_action).unwrap();

        binding.read_started().unwrap();
        binding.observe_read_result(Ok(0x83)).unwrap();
        assert_eq!(
            binding.action(),
            PhyColdI2cAction::StartWrite {
                address,
                value: 0xd3,
            }
        );
        binding.write_started().unwrap();
        assert_eq!(
            binding.observe_write_result(Err(PhyI2cError::Busy)),
            Ok(PhyColdI2cObservation::StillPending)
        );
        assert_eq!(
            binding.action(),
            PhyColdI2cAction::AwaitWriteCompletionEdge { address }
        );

        binding.observe_write_result(Ok(())).unwrap();
        assert_eq!(
            binding.into_completion(),
            Ok(PhyRfInitPrefixCompletion::RcCalibration(
                RcCalibrationCompletion::Write
            ))
        );
    }

    #[test]
    fn non_i2c_outer_action_is_rejected_instead_of_becoming_a_fallback() {
        assert_eq!(
            PhyColdI2cBinding::new(PhyRfInitPrefixAction::ConfigureFeBbClock),
            Err(PhyColdLoweringError::UnsupportedAction)
        );
    }

    #[test]
    fn finite_mmio_binding_preserves_dynamic_frequency_identity() {
        let outer_action = PhyRfInitPrefixAction::ChannelFrequency(
            PhyChannelFrequencyInitAction::ConfigureFrequencyRegisters {
                parameter_override: true,
            },
        );
        let binding = PhyColdMmioBinding::new(outer_action).unwrap();
        assert_eq!(binding.outer_action(), outer_action);
        assert_eq!(
            binding.into_completion(),
            Ok(PhyRfInitPrefixCompletion::ChannelFrequency(
                PhyChannelFrequencyInitCompletion::FrequencyRegistersConfigured {
                    parameter_override: true,
                }
            ))
        );

        assert_eq!(
            PhyColdMmioBinding::new(PhyRfInitPrefixAction::DelayMicros(10)),
            Err(PhyColdLoweringError::UnsupportedAction)
        );
    }

    #[test]
    fn nested_calibration_mmio_keeps_every_parent_identity_field() {
        let tone_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::ConfigureCalibrationTone {
                enabled: true,
                selector: 0x80,
                step: 0,
            }),
        ));
        assert_eq!(
            PhyColdMmioBinding::new(tone_action)
                .unwrap()
                .into_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::CalibrationToneConfigured {
                        enabled: true,
                        selector: 0x80,
                        step: 0,
                    }
                ))
            ))
        );

        let dc_iq_request = PhyDcIqEstimateRequest {
            iteration: 4,
            chain: 1,
            control: 0x1234,
            mode: 2,
        };
        let dc_iq_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
                PhyDcIqAction::SetEnable {
                    request: dc_iq_request,
                    phase: PhyDcIqEnablePhase::Measurement,
                    enabled: true,
                },
            ))),
        ));
        assert_eq!(
            PhyColdMmioBinding::new(dc_iq_action)
                .unwrap()
                .into_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DcIq(
                        PhyDcIqCompletion::EnableSet {
                            request: dc_iq_request,
                            phase: PhyDcIqEnablePhase::Measurement,
                            enabled: true,
                        }
                    ))
                ))
            ))
        );

        let signal_request = PhySignalPowerRequest {
            measurement: 0x3a7,
            shift: 12,
        };
        let signal_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                PhySignalPowerAction::ConfigureClock {
                    request: signal_request,
                    clock: PhySignalPowerClock::Rx,
                    enabled: false,
                },
            )),
        ));
        assert_eq!(
            PhyColdMmioBinding::new(signal_action)
                .unwrap()
                .into_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                    XtalDutySearchCompletion::SignalPower(
                        PhySignalPowerCompletion::ClockConfigured {
                            request: signal_request,
                            clock: PhySignalPowerClock::Rx,
                            enabled: false,
                        }
                    )
                ))
            ))
        );

        let restore_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Restore(XtalDutyRestoreAction::ConfigurePbusWorkModePulse),
        ));
        assert_eq!(
            PhyColdMmioBinding::new(restore_action)
                .unwrap()
                .into_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                    XtalDutyRestoreCompletion::PbusWorkModePulseConfigured
                ))
            ))
        );
    }

    #[test]
    fn timer_binding_consumes_one_exact_delay_edge() {
        let outer_action =
            PhyRfInitPrefixAction::RcCalibration(RcCalibrationAction::DelayMicros(100));
        let binding = PhyColdTimerBinding::new(outer_action).unwrap();
        assert_eq!(binding.outer_action(), outer_action);
        assert_eq!(binding.micros(), 100);
        assert_eq!(
            binding.into_elapsed_completion(),
            Ok(PhyRfInitPrefixCompletion::RcCalibration(
                RcCalibrationCompletion::Delay
            ))
        );

        assert_eq!(
            PhyColdTimerBinding::new(PhyRfInitPrefixAction::ConfigureFeBbClock),
            Err(PhyColdLoweringError::UnsupportedAction)
        );
    }

    #[test]
    fn nested_calibration_timers_preserve_every_parent_identity_field() {
        let rfpll_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::Rfpll(
                RfpllFrequencyAction::DelayMicros(20),
            )),
        ));
        let rfpll = PhyColdTimerBinding::new(rfpll_action).unwrap();
        assert_eq!(rfpll.micros(), 20);
        assert_eq!(
            rfpll.into_elapsed_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::Rfpll(RfpllFrequencyCompletion::DelayElapsed(20))
                ))
            ))
        );

        let rx_dco_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(
                PhyRxDcoAction::DelayMicros {
                    iteration: 7,
                    micros: 10,
                },
            )),
        ));
        assert_eq!(
            PhyColdTimerBinding::new(rx_dco_action)
                .unwrap()
                .into_elapsed_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DelayElapsed {
                        iteration: 7,
                        micros: 10,
                    })
                ))
            ))
        );

        let dc_iq_request = PhyDcIqEstimateRequest {
            iteration: 7,
            chain: 1,
            control: 0x1234,
            mode: 2,
        };
        let dc_iq_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
                PhyDcIqAction::DelayMicros {
                    request: dc_iq_request,
                    phase: PhyDcIqDelayPhase::Stop,
                    micros: 1,
                },
            ))),
        ));
        assert_eq!(
            PhyColdTimerBinding::new(dc_iq_action)
                .unwrap()
                .into_elapsed_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DcIq(
                        PhyDcIqCompletion::DelayElapsed {
                            request: dc_iq_request,
                            phase: PhyDcIqDelayPhase::Stop,
                            micros: 1,
                        }
                    ))
                ))
            ))
        );

        let search_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Search(XtalDutySearchAction::DelayMicros {
                candidate: 0x3a,
                micros: 20,
            }),
        ));
        assert_eq!(
            PhyColdTimerBinding::new(search_action)
                .unwrap()
                .into_elapsed_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                    XtalDutySearchCompletion::DelayElapsed { candidate: 0x3a }
                ))
            ))
        );

        let signal_request = PhySignalPowerRequest {
            measurement: 0x3a7,
            shift: 12,
        };
        let signal_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                PhySignalPowerAction::DelayMicros {
                    request: signal_request,
                    phase: PhyDcIqDelayPhase::Start,
                    micros: 1,
                },
            )),
        ));
        assert_eq!(
            PhyColdTimerBinding::new(signal_action)
                .unwrap()
                .into_elapsed_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                    XtalDutySearchCompletion::SignalPower(PhySignalPowerCompletion::DelayElapsed {
                        request: signal_request,
                        phase: PhyDcIqDelayPhase::Start,
                        micros: 1,
                    })
                ))
            ))
        );

        let restore_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Restore(XtalDutyRestoreAction::DelayMicros(2)),
        ));
        assert_eq!(
            PhyColdTimerBinding::new(restore_action)
                .unwrap()
                .into_elapsed_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                    XtalDutyRestoreCompletion::DelayElapsed { micros: 2 }
                ))
            ))
        );
    }

    #[test]
    fn pbus_busy_result_preserves_one_owned_awaiting_edge() {
        let transaction = PhyPbusForceTest::new(4, 1, 0);
        let outer_action =
            PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ForceTest(transaction));
        let mut binding = PhyColdPbusBinding::new(outer_action).unwrap();
        assert_eq!(binding.action(), PhyColdPbusAction::Start(transaction));

        binding.started().unwrap();
        let awaiting = PhyColdPbusAction::AwaitCompletionEdge(transaction);
        assert_eq!(binding.action(), awaiting);
        assert_eq!(
            binding.observe_result(PhyColdPbusHardwareResult::Busy),
            Ok(PhyColdPbusObservation::StillPending)
        );
        assert_eq!(binding.action(), awaiting);

        assert_eq!(
            binding.observe_result(PhyColdPbusHardwareResult::Completed),
            Ok(PhyColdPbusObservation::EdgeConsumed)
        );
        assert_eq!(binding.action(), PhyColdPbusAction::Complete(transaction));
        assert_eq!(
            binding.into_completion(),
            Ok(PhyRfInitPrefixCompletion::PbusClear(
                PhyPbusClearCompletion::ForceTestCompleted(transaction)
            ))
        );
    }

    #[test]
    fn pbus_timeout_consumes_the_exact_awaiting_transaction() {
        let transaction = PhyPbusForceTest::new(3, 2, 0x100);
        let outer_action =
            PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ForceTest(transaction));
        let mut binding = PhyColdPbusBinding::new(outer_action).unwrap();
        binding.started().unwrap();
        assert_eq!(
            binding.into_timeout_completion(),
            Ok(PhyRfInitPrefixCompletion::PbusClear(
                PhyPbusClearCompletion::ForceTestTimedOut(transaction)
            ))
        );
    }

    #[test]
    fn nested_xtal_pbus_edges_return_to_the_exact_parent_transition() {
        let prepare_transaction = PhyPbusForceTest::new(0, 2, 0x42);
        let prepare_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::ForcePbus(prepare_transaction)),
        ));
        let mut prepare = PhyColdPbusBinding::new(prepare_action).unwrap();
        prepare.started().unwrap();
        prepare
            .observe_result(PhyColdPbusHardwareResult::Completed)
            .unwrap();
        assert_eq!(
            prepare.into_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::PbusForceCompleted(prepare_transaction)
                ))
            ))
        );

        let rx_dco_transaction = PhyPbusForceTest::new(3, 1, 0x1ff);
        let rx_dco_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::ForcePbus(
                rx_dco_transaction,
            ))),
        ));
        let mut rx_dco = PhyColdPbusBinding::new(rx_dco_action).unwrap();
        rx_dco.started().unwrap();
        assert_eq!(
            rx_dco.into_timeout_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::PbusForceTimedOut(
                        rx_dco_transaction
                    ))
                ))
            ))
        );

        let restore_transaction = PhyPbusForceTest::new(1, 2, 0);
        let restore_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Restore(XtalDutyRestoreAction::ForcePbus(restore_transaction)),
        ));
        let mut restore = PhyColdPbusBinding::new(restore_action).unwrap();
        restore.started().unwrap();
        restore
            .observe_result(PhyColdPbusHardwareResult::Completed)
            .unwrap();
        assert_eq!(
            restore.into_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                    XtalDutyRestoreCompletion::PbusForceCompleted(restore_transaction)
                ))
            ))
        );
    }

    #[test]
    fn sampled_pbus_work_mode_is_bound_to_its_exact_parent() {
        let clear_action = PhyRfInitPrefixAction::PbusClear(PhyPbusClearAction::ConfigureWorkMode);
        let clear = PhyColdObservationBinding::new(clear_action).unwrap();
        assert_eq!(clear.outer_action(), clear_action);
        assert_eq!(
            clear.request(),
            PhyColdObservationRequest::ConfigurePbusWorkMode
        );
        assert_eq!(
            clear.into_completion(PhyColdObservationResult::PbusWorkMode {
                settle_required: true,
            }),
            Ok(PhyRfInitPrefixCompletion::PbusClear(
                PhyPbusClearCompletion::WorkModeConfigured {
                    settle_required: true
                }
            ))
        );

        let restore_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Restore(XtalDutyRestoreAction::ConfigurePbusWorkMode),
        ));
        let restore = PhyColdObservationBinding::new(restore_action).unwrap();
        assert_eq!(
            restore.into_completion(PhyColdObservationResult::PbusWorkMode {
                settle_required: false,
            }),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Restore(
                    XtalDutyRestoreCompletion::PbusWorkModeConfigured {
                        settle_required: false
                    }
                ))
            ))
        );
    }

    #[test]
    fn open_i2c_deadline_keeps_one_epoch_and_the_inclusive_rom_bound() {
        assert!(!phy_sdm_deadline_expired(100, 10_099, 9_999));
        assert!(phy_sdm_deadline_expired(100, 10_100, 9_999));
        assert!(!phy_sdm_deadline_expired(0xffff_ff00, 0x0000_260f, 9_999));
        assert!(phy_sdm_deadline_expired(0xffff_ff00, 0x0000_2610, 9_999));

        let configure_action =
            PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::ConfigurePowerAndPulse);
        let configure = PhyColdObservationBinding::new(configure_action).unwrap();
        assert_eq!(
            configure.request(),
            PhyColdObservationRequest::ConfigureOpenI2cPowerAndPulse
        );
        assert_eq!(
            configure.into_completion(PhyColdObservationResult::OpenI2cPowerAndPulse {
                started_at_cycle: 0xffff_ff00,
            }),
            Ok(PhyRfInitPrefixCompletion::OpenI2cXpd(
                OpenI2cXpdCompletion::PowerAndPulseConfigured {
                    started_at_cycle: 0xffff_ff00
                }
            ))
        );

        let deadline_action =
            PhyRfInitPrefixAction::OpenI2cXpd(OpenI2cXpdAction::CheckSdmDeadline {
                started_at_cycle: 0xffff_ff00,
                maximum_cycles: 9_999,
            });
        let deadline = PhyColdObservationBinding::new(deadline_action).unwrap();
        assert_eq!(
            deadline.request(),
            PhyColdObservationRequest::CheckOpenI2cSdmDeadline {
                started_at_cycle: 0xffff_ff00,
                maximum_cycles: 9_999,
            }
        );
        assert_eq!(
            deadline
                .into_completion(PhyColdObservationResult::OpenI2cSdmDeadline { expired: false }),
            Ok(PhyRfInitPrefixCompletion::OpenI2cXpd(
                OpenI2cXpdCompletion::DeadlineObserved { expired: false }
            ))
        );
    }

    #[test]
    fn nested_sampled_edges_are_one_shot_identity_bound_observations() {
        let mask_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::MaskRxDcoControl {
                address: crate::phy_rx_dco::RX_DCO_CONTROL_ADDRESS,
                clear_mask: crate::phy_rx_dco::RX_DCO_CONTROL_FIELD_MASK,
            }),
        ));
        let mask = PhyColdObservationBinding::new(mask_action).unwrap();
        assert_eq!(
            mask.request(),
            PhyColdObservationRequest::MaskRxDcoControl {
                address: crate::phy_rx_dco::RX_DCO_CONTROL_ADDRESS,
                clear_mask: crate::phy_rx_dco::RX_DCO_CONTROL_FIELD_MASK,
            }
        );
        assert_eq!(
            mask.into_completion(PhyColdObservationResult::RxDcoControlMasked {
                address: crate::phy_rx_dco::RX_DCO_CONTROL_ADDRESS,
                saved_field: 0x0080_0000,
            }),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDcoControlMasked {
                        address: crate::phy_rx_dco::RX_DCO_CONTROL_ADDRESS,
                        saved_field: 0x0080_0000,
                    }
                ))
            ))
        );
        assert_eq!(
            PhyColdObservationBinding::new(PhyRfInitPrefixAction::XtalDuty(
                XtalDutyCalibrationAction::Pass(XtalDutyPassAction::Prepare(
                    XtalDutyPrepareAction::MaskRxDcoControl {
                        address: crate::phy_rx_dco::RX_DCO_CONTROL_ADDRESS + 4,
                        clear_mask: crate::phy_rx_dco::RX_DCO_CONTROL_FIELD_MASK,
                    }
                ))
            )),
            Err(PhyColdLoweringError::UnsupportedAction)
        );

        let pbus_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::ReadPbus {
                selector: 1,
                path: 2,
            })),
        ));
        assert_eq!(
            PhyColdObservationBinding::new(pbus_action)
                .unwrap()
                .into_completion(PhyColdObservationResult::RxDcoPbusRead {
                    selector: 1,
                    path: 2,
                    value: 0x1a5,
                }),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::PbusRead {
                        selector: 1,
                        path: 2,
                        value: 0x1a5,
                    })
                ))
            ))
        );

        let dc_iq_request = PhyDcIqEstimateRequest {
            iteration: 6,
            chain: 1,
            control: 0x0fa0,
            mode: 0,
        };
        let dc_iq_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
                PhyDcIqAction::AwaitReadinessEdge {
                    request: dc_iq_request,
                    readiness_activity_edges: 3,
                },
            ))),
        ));
        assert_eq!(
            PhyColdObservationBinding::new(dc_iq_action)
                .unwrap()
                .into_completion(PhyColdObservationResult::DcIqReadiness {
                    request: dc_iq_request,
                    snapshot: PhyDcIqReadinessSnapshot {
                        ready: false,
                        activity: true,
                    },
                }),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DcIq(
                        PhyDcIqCompletion::ReadinessObserved {
                            request: dc_iq_request,
                            snapshot: PhyDcIqReadinessSnapshot {
                                ready: false,
                                activity: true,
                            },
                        }
                    ))
                ))
            ))
        );
        assert_eq!(
            PhyColdObservationBinding::new(dc_iq_action)
                .unwrap()
                .into_timeout_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DcIq(
                        PhyDcIqCompletion::ReadinessTimedOut(dc_iq_request)
                    ))
                ))
            ))
        );

        let dc_iq_accumulators = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::RxDco(PhyRxDcoAction::DcIq(
                PhyDcIqAction::ReadAccumulators(dc_iq_request),
            ))),
        ));
        let dc_iq_snapshot = PhyDcIqAccumulatorSnapshot {
            i: -3,
            q: 7,
            power: 0x1234,
        };
        assert_eq!(
            PhyColdObservationBinding::new(dc_iq_accumulators)
                .unwrap()
                .into_completion(PhyColdObservationResult::DcIqAccumulators {
                    request: dc_iq_request,
                    snapshot: dc_iq_snapshot,
                }),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::RxDco(PhyRxDcoCompletion::DcIq(
                        PhyDcIqCompletion::AccumulatorsRead {
                            request: dc_iq_request,
                            snapshot: dc_iq_snapshot,
                        }
                    ))
                ))
            ))
        );

        let signal_request = PhySignalPowerRequest {
            measurement: 0x25,
            shift: 12,
        };
        let signal_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Search(XtalDutySearchAction::SignalPower(
                PhySignalPowerAction::ReadAccumulators(signal_request),
            )),
        ));
        let signal_snapshot = PhySignalPowerAccumulatorSnapshot {
            sum_i: 10,
            difference_i: -20,
            difference_q: 30,
            sum_q: -40,
        };
        assert_eq!(
            PhyColdObservationBinding::new(signal_action)
                .unwrap()
                .into_completion(PhyColdObservationResult::SignalPowerAccumulators {
                    request: signal_request,
                    snapshot: signal_snapshot,
                }),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                    XtalDutySearchCompletion::SignalPower(
                        PhySignalPowerCompletion::AccumulatorsRead {
                            request: signal_request,
                            snapshot: signal_snapshot,
                        }
                    )
                ))
            ))
        );
    }

    #[test]
    fn external_lowering_has_no_vendor_or_synchronous_fallback_variant() {
        assert!(matches!(
            PhyColdExternalBinding::lower(PhyRfInitPrefixAction::DelayMicros(10)),
            Ok(PhyColdExternalBinding::Timer(_))
        ));
        assert!(matches!(
            PhyColdExternalBinding::lower(PhyRfInitPrefixAction::ConfigureFrontEndRegisters),
            Ok(PhyColdExternalBinding::Mmio(_))
        ));

        let address = PhyI2cAddress::new(0x62, 1).unwrap();
        assert!(matches!(
            PhyColdExternalBinding::lower(PhyRfInitPrefixAction::ReadParameter18e { address }),
            Ok(PhyColdExternalBinding::I2c(_))
        ));
        let transaction = PhyPbusForceTest::new(4, 1, 0);
        assert!(matches!(
            PhyColdExternalBinding::lower(PhyRfInitPrefixAction::PbusClear(
                PhyPbusClearAction::ForceTest(transaction)
            )),
            Ok(PhyColdExternalBinding::Pbus(_))
        ));
        assert!(matches!(
            PhyColdExternalBinding::lower(PhyRfInitPrefixAction::PbusClear(
                PhyPbusClearAction::ConfigureWorkMode
            )),
            Ok(PhyColdExternalBinding::Observation(_))
        ));
        assert_eq!(
            PhyColdExternalBinding::lower(PhyRfInitPrefixAction::CaptureFilterDcapParameters),
            Err(PhyColdLoweringError::UnsupportedAction)
        );
    }

    #[test]
    fn channel_frequency_i2c_completion_keeps_its_field_identity() {
        let address = PhyI2cAddress::new(0x63, 6).unwrap();
        let outer_action =
            PhyRfInitPrefixAction::ChannelFrequency(PhyChannelFrequencyInitAction::WriteMasked {
                address,
                high_bit: 7,
                low_bit: 3,
                value: 0x12,
            });
        let mut binding = PhyColdI2cBinding::new(outer_action).unwrap();
        binding.read_started().unwrap();
        binding.observe_read_result(Ok(0x05)).unwrap();
        binding.write_started().unwrap();
        binding.observe_write_result(Ok(())).unwrap();
        assert_eq!(
            binding.into_completion(),
            Ok(PhyRfInitPrefixCompletion::ChannelFrequency(
                PhyChannelFrequencyInitCompletion::MaskedWrite {
                    address,
                    high_bit: 7,
                    low_bit: 3,
                }
            ))
        );
    }

    #[test]
    fn xtal_and_rfpll_i2c_edges_keep_nested_identity() {
        let initial_address = PhyI2cAddress::new(0x61, 9).unwrap();
        let initial_action =
            PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::ReadInitialDuty {
                address: initial_address,
                high_bit: 5,
                low_bit: 0,
            });
        let mut initial = PhyColdI2cBinding::new(initial_action).unwrap();
        initial.read_started().unwrap();
        initial.observe_read_result(Ok(0xeb)).unwrap();
        assert_eq!(
            initial.into_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::InitialDutyRead {
                    address: initial_address,
                    value: 0x2b,
                }
            ))
        );

        let rfpll_address = PhyI2cAddress::new(0x63, 6).unwrap();
        let rfpll_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Prepare(XtalDutyPrepareAction::Rfpll(
                RfpllFrequencyAction::WriteMasked {
                    address: rfpll_address,
                    high_bit: 7,
                    low_bit: 3,
                    value: 0x12,
                },
            )),
        ));
        let mut rfpll = PhyColdI2cBinding::new(rfpll_action).unwrap();
        rfpll.read_started().unwrap();
        rfpll.observe_read_result(Ok(0x05)).unwrap();
        rfpll.write_started().unwrap();
        rfpll.observe_write_result(Ok(())).unwrap();
        assert_eq!(
            rfpll.into_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Prepare(
                    XtalDutyPrepareCompletion::Rfpll(RfpllFrequencyCompletion::MaskedWrite {
                        address: rfpll_address,
                        high_bit: 7,
                        low_bit: 3,
                    })
                ))
            ))
        );

        let candidate_address = PhyI2cAddress::new(0x61, 0x0a).unwrap();
        let candidate_action = PhyRfInitPrefixAction::XtalDuty(XtalDutyCalibrationAction::Pass(
            XtalDutyPassAction::Search(XtalDutySearchAction::WriteCandidate {
                address: candidate_address,
                candidate: 0x3a,
            }),
        ));
        let mut candidate = PhyColdI2cBinding::new(candidate_action).unwrap();
        candidate.write_started().unwrap();
        candidate.observe_write_result(Ok(())).unwrap();
        assert_eq!(
            candidate.into_completion(),
            Ok(PhyRfInitPrefixCompletion::XtalDuty(
                XtalDutyCalibrationCompletion::Pass(XtalDutyPassCompletion::Search(
                    XtalDutySearchCompletion::CandidateWritten {
                        address: candidate_address,
                        candidate: 0x3a,
                    }
                ))
            ))
        );
    }
}
