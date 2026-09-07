//! Ownership-bound access to recovered ESP32-S31 PHY/baseband registers.
//!
//! Register layout and legal field images come from
//! `registers/esp32s31/published/radio.svd`. Complete ROM/blob bodies cited there define the
//! finite operation order.

#![deny(unsafe_code)]

use crate::{RadioPhyRegisters, generated::PhyTxPowerTrackingState};

fn vendor_register_argument(input: u32) -> crate::generated::PhyVendorRegisterArgument {
    crate::generated::PhyVendorRegisterArgument::new(input)
        .expect("every u32 fits the complete generated vendor-argument domain")
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct TxDcPwdetRestoreFields {
    table_low: u8,
    calibration: u8,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct TxIqToneControlFields {
    selector_high: u8,
    low_reserved_clear_unknown: u8,
    negated_step_or_attenuation: u8,
    tone_enable_or_arm: bool,
    txiq_mismatch_mode_unknown: u8,
    middle_reserved_clear_unknown: u8,
    txiq_polarity_image: u8,
    high_nibble_unknown: u8,
}

/// Preparing TX-DC PWDET was rejected before any register access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxDcPwdetPrepareError {
    /// Another calibration still owns the one pending restore operation.
    RestorePending,
}

/// Restoring TX-DC PWDET was rejected before any register access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxDcPwdetRestoreError {
    /// No successful prepare operation owns saved fields.
    RestoreNotPending,
}

/// A lifecycle operation would overwrite TX-DC fields awaiting restore.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxDcPwdetLifecycleError {
    RestorePending,
}

/// Preparing a TX-IQ tone-control restore was rejected before register access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxIqToneControlPrepareError {
    /// Another calibration still owns the pending restore operation.
    RestorePending,
}

/// Restoring TX-IQ tone control was rejected before register access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxIqToneControlRestoreError {
    /// No successful prepare operation owns saved field state.
    RestoreNotPending,
}

/// Preparing an RX-DCO control restore was rejected before register access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxDcoControlPrepareError {
    /// A different calibration owns the shared restore slot.
    RestorePending,
    /// Both reviewed RX-DCO nesting levels already own saved fields.
    RestoreStackFull,
}

/// Restoring RX-DCO control was rejected before register access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxDcoControlRestoreError {
    /// No RX-DCO control field is awaiting restoration.
    RestoreNotPending,
}

/// Preparing the Bluetooth TX-power analog-control restore was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothTxPowerControlPrepareError {
    /// Another calibration still owns the shared restore slot.
    RestorePending,
}

/// Using the Bluetooth TX-power analog-control restore was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothTxPowerControlRestoreError {
    /// No prepared Bluetooth TX-power analog-control restore is pending.
    RestoreNotPending,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum RadioPhyRestoreKind {
    Empty,
    TxDcPwdet,
    TxIqToneControl,
    RxDcoControlOne,
    RxDcoControlTwo,
    BluetoothTxPowerControl,
}

/// One private restore authority shared by mutually exclusive PHY calibrations.
///
/// Byte storage keeps only named accessor values and does not retain a raw
/// register image. The owner crosses async suspension points, so the private
/// slot remains byte-aligned.
pub(crate) struct RadioPhyRestoreSlot {
    kind: RadioPhyRestoreKind,
    payload: [u8; 8],
}

impl RadioPhyRestoreSlot {
    pub(crate) const fn new() -> Self {
        Self {
            kind: RadioPhyRestoreKind::Empty,
            payload: [0; 8],
        }
    }

    pub(crate) const fn txdc_pending(&self) -> bool {
        matches!(self.kind, RadioPhyRestoreKind::TxDcPwdet)
    }

    pub(crate) const fn txiq_pending(&self) -> bool {
        matches!(self.kind, RadioPhyRestoreKind::TxIqToneControl)
    }

    pub(crate) const fn rx_dco_pending(&self) -> bool {
        matches!(
            self.kind,
            RadioPhyRestoreKind::RxDcoControlOne | RadioPhyRestoreKind::RxDcoControlTwo
        )
    }

    pub(crate) const fn bluetooth_tx_power_control_pending(&self) -> bool {
        matches!(self.kind, RadioPhyRestoreKind::BluetoothTxPowerControl)
    }

    fn prepare_txdc_with<Captured, Capture, Prepare>(
        &mut self,
        capture: Capture,
        prepare: Prepare,
    ) -> Result<(), TxDcPwdetPrepareError>
    where
        Capture: FnOnce() -> (TxDcPwdetRestoreFields, Captured),
        Prepare: FnOnce(Captured),
    {
        if !matches!(self.kind, RadioPhyRestoreKind::Empty) {
            return Err(TxDcPwdetPrepareError::RestorePending);
        }
        let (fields, captured) = capture();
        self.payload = [fields.table_low, fields.calibration, 0, 0, 0, 0, 0, 0];
        self.kind = RadioPhyRestoreKind::TxDcPwdet;
        prepare(captured);
        Ok(())
    }

    fn restore_txdc_with<Restore>(&mut self, restore: Restore) -> Result<(), TxDcPwdetRestoreError>
    where
        Restore: FnOnce(TxDcPwdetRestoreFields),
    {
        if !matches!(self.kind, RadioPhyRestoreKind::TxDcPwdet) {
            return Err(TxDcPwdetRestoreError::RestoreNotPending);
        }
        let fields = TxDcPwdetRestoreFields {
            table_low: self.payload[0],
            calibration: self.payload[1],
        };
        restore(fields);
        self.kind = RadioPhyRestoreKind::Empty;
        self.payload = [0; 8];
        Ok(())
    }

    fn prepare_txiq_with<Capture>(
        &mut self,
        capture: Capture,
    ) -> Result<(), TxIqToneControlPrepareError>
    where
        Capture: FnOnce() -> TxIqToneControlFields,
    {
        if !matches!(self.kind, RadioPhyRestoreKind::Empty) {
            return Err(TxIqToneControlPrepareError::RestorePending);
        }
        let fields = capture();
        self.payload = [
            fields.selector_high,
            fields.low_reserved_clear_unknown,
            fields.negated_step_or_attenuation,
            u8::from(fields.tone_enable_or_arm),
            fields.txiq_mismatch_mode_unknown,
            fields.middle_reserved_clear_unknown,
            fields.txiq_polarity_image,
            fields.high_nibble_unknown,
        ];
        self.kind = RadioPhyRestoreKind::TxIqToneControl;
        Ok(())
    }

    fn restore_txiq_with<Restore>(
        &mut self,
        restore: Restore,
    ) -> Result<(), TxIqToneControlRestoreError>
    where
        Restore: FnOnce(TxIqToneControlFields),
    {
        if !matches!(self.kind, RadioPhyRestoreKind::TxIqToneControl) {
            return Err(TxIqToneControlRestoreError::RestoreNotPending);
        }
        restore(TxIqToneControlFields {
            selector_high: self.payload[0],
            low_reserved_clear_unknown: self.payload[1],
            negated_step_or_attenuation: self.payload[2],
            tone_enable_or_arm: self.payload[3] != 0,
            txiq_mismatch_mode_unknown: self.payload[4],
            middle_reserved_clear_unknown: self.payload[5],
            txiq_polarity_image: self.payload[6],
            high_nibble_unknown: self.payload[7],
        });
        self.kind = RadioPhyRestoreKind::Empty;
        self.payload = [0; 8];
        Ok(())
    }

    pub(crate) fn prepare_rx_dco_with<Capture>(
        &mut self,
        capture: Capture,
    ) -> Result<(), RxDcoControlPrepareError>
    where
        Capture: FnOnce() -> u8,
    {
        let (index, next_kind) = match self.kind {
            RadioPhyRestoreKind::Empty => (0, RadioPhyRestoreKind::RxDcoControlOne),
            RadioPhyRestoreKind::RxDcoControlOne => (1, RadioPhyRestoreKind::RxDcoControlTwo),
            RadioPhyRestoreKind::RxDcoControlTwo => {
                return Err(RxDcoControlPrepareError::RestoreStackFull);
            }
            _ => return Err(RxDcoControlPrepareError::RestorePending),
        };
        self.payload[index] = capture();
        self.kind = next_kind;
        Ok(())
    }

    pub(crate) fn restore_rx_dco_with<Restore>(
        &mut self,
        restore: Restore,
    ) -> Result<(), RxDcoControlRestoreError>
    where
        Restore: FnOnce(u8),
    {
        let (index, next_kind) = match self.kind {
            RadioPhyRestoreKind::RxDcoControlOne => (0, RadioPhyRestoreKind::Empty),
            RadioPhyRestoreKind::RxDcoControlTwo => (1, RadioPhyRestoreKind::RxDcoControlOne),
            _ => return Err(RxDcoControlRestoreError::RestoreNotPending),
        };
        restore(self.payload[index]);
        self.payload[index] = 0;
        self.kind = next_kind;
        Ok(())
    }

    pub(crate) fn prepare_bluetooth_tx_power_control(
        &mut self,
    ) -> Result<(), BluetoothTxPowerControlPrepareError> {
        if !matches!(self.kind, RadioPhyRestoreKind::Empty) {
            return Err(BluetoothTxPowerControlPrepareError::RestorePending);
        }
        self.payload = [0; 8];
        self.kind = RadioPhyRestoreKind::BluetoothTxPowerControl;
        Ok(())
    }

    pub(crate) fn capture_bluetooth_tx_power_control_low(
        &mut self,
        value: u8,
    ) -> Result<(), BluetoothTxPowerControlRestoreError> {
        if !self.bluetooth_tx_power_control_pending() {
            return Err(BluetoothTxPowerControlRestoreError::RestoreNotPending);
        }
        self.payload[0] = value;
        Ok(())
    }

    pub(crate) fn capture_bluetooth_tx_power_control_high(
        &mut self,
        value: u8,
    ) -> Result<(), BluetoothTxPowerControlRestoreError> {
        if !self.bluetooth_tx_power_control_pending() {
            return Err(BluetoothTxPowerControlRestoreError::RestoreNotPending);
        }
        self.payload[1] = value;
        Ok(())
    }

    pub(crate) fn bluetooth_tx_power_control_values(
        &self,
    ) -> Result<(u8, u8), BluetoothTxPowerControlRestoreError> {
        if !self.bluetooth_tx_power_control_pending() {
            return Err(BluetoothTxPowerControlRestoreError::RestoreNotPending);
        }
        Ok((self.payload[0], self.payload[1]))
    }

    pub(crate) fn finish_bluetooth_tx_power_control_restore(
        &mut self,
    ) -> Result<(), BluetoothTxPowerControlRestoreError> {
        if !self.bluetooth_tx_power_control_pending() {
            return Err(BluetoothTxPowerControlRestoreError::RestoreNotPending);
        }
        self.kind = RadioPhyRestoreKind::Empty;
        self.payload = [0; 8];
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn occupy_txdc_for_test(&mut self) {
        self.kind = RadioPhyRestoreKind::TxDcPwdet;
        self.payload = [0; 8];
    }

    #[cfg(test)]
    pub(crate) fn occupy_txiq_for_test(&mut self) {
        self.kind = RadioPhyRestoreKind::TxIqToneControl;
        self.payload = [0; 8];
    }

    #[cfg(test)]
    pub(crate) fn occupy_rx_dco_for_test(&mut self) {
        self.kind = RadioPhyRestoreKind::RxDcoControlOne;
        self.payload = [0; 8];
    }

    #[cfg(test)]
    pub(crate) fn occupy_bluetooth_tx_power_control_for_test(&mut self) {
        self.kind = RadioPhyRestoreKind::BluetoothTxPowerControl;
        self.payload = [0; 8];
    }
}

const fn decode_noise_floor_quarter_db(raw_low_twelve: u16) -> i32 {
    // Complete ROM `phy_read_hw_noisefloor` subtracts 0x1000 from the
    // generated twelve-bit field unconditionally and shifts by two.
    let signed_sixteenth_db = raw_low_twelve as i32 - 0x1000;
    signed_sixteenth_db >> 2
}

const fn quarter_db_to_dbm(quarter_db: i32) -> i8 {
    // Complete blob `wDev_GetNoiseFloor` consumes the quarter-dB ROM result,
    // adds two, shifts by two and stores a byte.
    ((quarter_db + 2) >> 2) as i8
}

const fn saturate_tx_iq_gain(coefficient: i8) -> i8 {
    // Complete ROM `phy_txiq_set_reg` deliberately excludes the most
    // negative two's-complement endpoint before publishing the six-bit
    // field. This differs from merely retaining the low bits of an `i8`.
    if coefficient < -31 {
        -31
    } else if coefficient > 31 {
        31
    } else {
        coefficient
    }
}

const fn saturate_tx_iq_phase(coefficient: i8) -> i8 {
    // The seven-bit phase field has the analogous symmetric ROM range.
    if coefficient < -63 {
        -63
    } else if coefficient > 63 {
        63
    } else {
        coefficient
    }
}

fn iq_coefficient_image(coefficient: i8) -> crate::generated::PhyIqCoefficientImageByte {
    crate::generated::PhyIqCoefficientImageByte::new(u32::from(coefficient as u8))
        .expect("one coefficient byte fits the complete generated IQ domain")
}

fn tone_byte(value: u8) -> crate::generated::PhyToneByteImage {
    crate::generated::PhyToneByteImage::new(u32::from(value))
        .expect("one byte fits the complete generated tone-byte domain")
}

fn tone_two_bit(value: u32) -> crate::generated::PhyToneTwoBitImage {
    crate::generated::PhyToneTwoBitImage::new(value)
        .expect("reviewed tone transaction supplies a complete two-bit image")
}

fn tone_three_bit(value: u32) -> crate::generated::PhyToneThreeBitImage {
    crate::generated::PhyToneThreeBitImage::new(value)
        .expect("reviewed tone transaction supplies a complete three-bit image")
}

fn tone_four_bit(value: u32) -> crate::generated::PhyToneFourBitImage {
    crate::generated::PhyToneFourBitImage::new(value)
        .expect("reviewed tone transaction supplies a complete four-bit image")
}

fn tone_selector_high(selector: u16) -> crate::generated::PhyToneByteImage {
    let selector = crate::generated::PhyToneSelector::new(u32::from(selector))
        .expect("tone selector must fit the generated ten-bit domain");
    crate::generated::PhyToneByteImage::new(selector.get() / 4)
        .expect("upper eight selector bits fit the generated tone-byte domain")
}

impl RadioPhyRegisters {
    pub(crate) const fn txdc_pwdet_restore_pending(&self) -> bool {
        self.restore_slot.txdc_pending()
    }

    #[cfg(test)]
    pub(crate) fn occupy_txdc_pwdet_restore_for_test(&mut self) {
        self.restore_slot.occupy_txdc_for_test();
    }

    pub(crate) const fn txiq_tone_control_restore_pending(&self) -> bool {
        self.restore_slot.txiq_pending()
    }

    #[cfg(test)]
    pub(crate) fn occupy_txiq_tone_control_restore_for_test(&mut self) {
        self.restore_slot.occupy_txiq_for_test();
    }

    pub(crate) const fn rx_dco_control_restore_pending(&self) -> bool {
        self.restore_slot.rx_dco_pending()
    }

    #[cfg(test)]
    pub(crate) fn occupy_rx_dco_control_restore_for_test(&mut self) {
        self.restore_slot.occupy_rx_dco_for_test();
    }

    pub(crate) const fn bluetooth_tx_power_control_restore_pending(&self) -> bool {
        self.restore_slot.bluetooth_tx_power_control_pending()
    }

    #[cfg(test)]
    pub(crate) fn occupy_bluetooth_tx_power_control_restore_for_test(&mut self) {
        self.restore_slot
            .occupy_bluetooth_tx_power_control_for_test();
    }

    /// Enable both RX- and TX-IQ correction modes through two fresh RMWs.
    ///
    /// Complete rev0 ROM `phy_iq_corr_enable` at `0x2f82_7d8c` sets both
    /// recovered mode bits in each word while preserving all coefficients.
    pub fn enable_iq_correction_modes(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::enable_rx_iq_correction_modes(bb);
        crate::generated::enable_tx_iq_correction_modes(bb);
    }

    /// Publish both RXIQ root status bits through independent fresh RMWs.
    pub fn configure_rxiq_root_status(&mut self) {
        let pbus = &self.peripherals.phy_pbus;
        crate::generated::set_pbus_rxiq_status_first(pbus);
        crate::generated::set_pbus_rxiq_status_second(pbus);
    }

    /// Apply the complete four-edge RXIQ correction prefix or suffix.
    pub fn configure_rxiq_root_correction(&mut self, begin: bool) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        if begin {
            crate::generated::set_rxiq_root_rx_correction_mode_low(bb);
            crate::generated::set_rxiq_root_tx_correction_mode_low(bb);
            crate::generated::clear_rxiq_root_rx_correction_mode_high(bb);
            crate::generated::clear_rxiq_root_tx_correction_mode_high(bb);
        } else {
            crate::generated::set_rxiq_root_rx_correction_mode_high(bb);
            crate::generated::set_rxiq_root_tx_correction_mode_high(bb);
            crate::generated::clear_rxiq_root_rx_correction_mode_low(bb);
            crate::generated::clear_pbus_rxiq_status_second(&self.peripherals.phy_pbus);
        }
    }

    /// Configure all fourteen ordered TX-power tracking RMW edges.
    pub fn configure_tx_power_tracking(&mut self, enabled: bool) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        let state = if enabled {
            PhyTxPowerTrackingState::Enabled
        } else {
            PhyTxPowerTrackingState::Disabled
        };
        crate::generated::configure_tx_power_tracking_state(bb, state);
        crate::generated::clear_tx_power_tracking_initial_field(bb);
        crate::generated::configure_tx_power_tracking_initial_field(bb);

        // The complete body clears the adjacent bits through separate reads.
        crate::generated::clear_tx_power_tracking_control_low(bb);
        crate::generated::clear_tx_power_tracking_control_high(bb);

        crate::generated::configure_tx_power_tracking_value_5(bb);
        crate::generated::configure_tx_power_tracking_value_4(bb);
        crate::generated::configure_tx_power_tracking_value_3(bb);
        crate::generated::configure_tx_power_tracking_value_2(bb);
        crate::generated::configure_tx_power_tracking_value_1(bb);
        crate::generated::configure_tx_power_tracking_value_0(bb);
        crate::generated::configure_tx_power_tracking_value_8(bb);
        crate::generated::configure_tx_power_tracking_value_7(bb);
        crate::generated::configure_tx_power_tracking_value_6(bb);
    }

    /// Apply complete rev0 ROM `phy_btbb_wifi_bb_cfg2`.
    pub fn configure_bt_wifi_baseband(&mut self) {
        crate::generated::configure_bt_wifi_baseband_fields(
            &self.peripherals.phy_baseband_config_oracle,
        );
    }

    /// Apply complete rev0 ROM `phy_chan_dump_cfg`.
    pub fn configure_channel_dump(&mut self, value: u32, enabled: u32, mode: u32) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::configure_phy_channel_dump_value(bb, vendor_register_argument(value));
        crate::generated::configure_phy_channel_dump_mode(bb, vendor_register_argument(mode));
        crate::generated::configure_phy_channel_dump_enabled(bb, vendor_register_argument(enabled));
    }

    /// Apply complete rev0 ROM `phy_dac_rate_set`.
    pub fn configure_dac_rate(&mut self, rate: crate::PhyAdcRate) {
        self.configure_adc_rate(rate);
    }

    /// Configure both I²C TX-rate fields and the four gain-compensation bytes.
    pub fn configure_i2c_tx_rate(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::configure_phy_i2c_tx_rate_high(bb);
        crate::generated::configure_phy_i2c_tx_rate_low(bb);
        self.restore_tx_gain_compensation();
    }

    /// Configure the complete baseband watchdog leaf.
    pub fn configure_baseband_watchdog(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::configure_phy_baseband_watchdog_control(bb);
        crate::generated::enable_phy_baseband_watchdog(bb);
    }

    /// Replace the standalone PHY VHT-support bit through one fresh RMW.
    pub fn set_vht_support(&mut self, input: u32) {
        crate::generated::configure_phy_vht_support(
            &self.peripherals.phy_frequency_channel_oracle,
            vendor_register_argument(input),
        );
    }

    /// Replace the PHY CSI-dump force-LLTF bit through one fresh RMW.
    pub fn set_csi_dump_force_lltf(&mut self, input: u32) {
        crate::generated::configure_phy_csi_dump_force_lltf(
            &self.peripherals.phy_agc_oracle,
            vendor_register_argument(input),
        );
    }

    /// Apply complete ROM `phy_hemu_ru26_good_res`.
    pub fn configure_he_ru26_good_response(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::enable_phy_he_ru26_good_response(bb);
        crate::generated::clear_phy_he_ru26_good_response_disable(bb);
    }

    /// Apply complete ROM `phy_freq_band_reg_set` and its VHT tail.
    pub fn set_frequency_band(&mut self, input: u32) {
        crate::generated::configure_phy_frequency_band_inverse(
            &self.peripherals.phy_agc_oracle,
            vendor_register_argument(input),
        );
        self.set_vht_support(input);
    }

    /// Apply the three fresh RMWs of complete ROM `phy_bbtx_outfilter`.
    pub fn configure_tx_output_filter(&mut self, input_0: u32, input_1: u32, input_2: u32) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::configure_phy_tx_output_filter_0(bb, vendor_register_argument(input_0));
        crate::generated::configure_phy_tx_output_filter_1(bb, vendor_register_argument(input_1));
        crate::generated::configure_phy_tx_output_filter_2(bb, vendor_register_argument(input_2));
    }

    /// Replace the baseband watchdog reset-enable bit.
    pub fn set_baseband_watchdog_reset_enabled(&mut self, input: u32) {
        crate::generated::configure_phy_baseband_watchdog_reset(
            &self.peripherals.phy_baseband_config_oracle,
            vendor_register_argument(input),
        );
    }

    /// Replace the baseband watchdog interrupt-enable bit.
    pub fn set_baseband_watchdog_interrupt_enabled(&mut self, input: u32) {
        crate::generated::configure_phy_baseband_watchdog_interrupt(
            &self.peripherals.phy_baseband_config_oracle,
            vendor_register_argument(input),
        );
    }

    /// Set the baseband watchdog timeout-clear bit through one fresh RMW.
    pub fn clear_baseband_watchdog_timeout(&mut self) {
        crate::generated::clear_phy_baseband_watchdog_timeout(
            &self.peripherals.phy_baseband_config_oracle,
        );
    }

    /// Return the complete standalone baseband watchdog status word.
    pub fn baseband_watchdog_status(&mut self) -> u32 {
        crate::svd::field_read::observe_phy_baseband_watchdog_status(
            &self.peripherals.phy_baseband_config_oracle,
        )
    }

    /// Apply both fresh RMWs of complete ROM `phy_lltf_mask_en`.
    pub fn configure_lltf_mask(&mut self, input_0: u32, input_1: u32) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::configure_phy_lltf_mask_0(bb, vendor_register_argument(input_0));
        crate::generated::configure_phy_lltf_mask_1(bb, vendor_register_argument(input_1));
    }

    /// Enable all four recovered automatic noise-floor controls.
    pub fn configure_noise_floor_auto(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::enable_noise_floor_auto_control_low(bb);
        crate::generated::enable_noise_floor_auto_control_high(bb);
        crate::generated::enable_noise_floor_auto_path_0(bb);
        crate::generated::enable_noise_floor_auto_path_1(bb);
    }

    /// Read the current hardware noise floor as the signed byte used by MAC
    /// rate control.
    ///
    /// SOURCE: complete rev0 ROM `phy_read_hw_noisefloor` at
    /// `0x2f82_7d72`, size `0x1a`, reads `0x2010_708c[11:0]` and performs the
    /// first arithmetic divide by four. Complete
    /// `libpp.a[wdev.o]::wDev_GetNoiseFloor`, size `0x36`, applies
    /// `(quarter_db + 2) >> 2` and retains the result as a signed byte.
    pub fn read_noise_floor_dbm(&self) -> i8 {
        quarter_db_to_dbm(self.read_noise_floor_quarter_db())
    }

    /// Read the exact signed quarter-dB result returned by complete rev0 ROM
    /// `phy_read_hw_noisefloor`.
    pub fn read_noise_floor_quarter_db(&self) -> i32 {
        let raw = crate::svd::field_read::observe_phy_noise_floor_sixteenth_db_code(
            &self.peripherals.phy_baseband_config_oracle,
        );
        decode_noise_floor_quarter_db(raw)
    }

    /// Apply all six ordered PA-on configuration operations.
    pub fn configure_tx_pa_on(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::configure_phy_tx_pa_on_field(bb);
        crate::generated::configure_phy_tx_pa_on_high_0(bb);
        crate::svd::fixed_register_image::initialize_tx_pa_table(bb);
        crate::generated::configure_phy_tx_pa_on_timing(bb);
        crate::generated::configure_phy_tx_pa_on_high_1(bb);
        crate::generated::configure_phy_tx_pa_on_bt_delay(bb);
    }

    /// Apply the local prefix of complete rev0 ROM `phy_bb_reg_init`.
    pub fn initialize_baseband_prefix(&mut self) {
        crate::generated::initialize_phy_baseband_prefix(
            &self.peripherals.phy_baseband_config_oracle,
        );
    }

    /// Apply the twelve local middle edges of complete `phy_bb_reg_init`.
    pub fn initialize_baseband_middle(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::initialize_phy_baseband_7808(bb);
        crate::generated::initialize_phy_baseband_78dc(bb);
        crate::generated::clear_phy_baseband_78e4(bb);
        crate::generated::clear_phy_baseband_tx_pa_timing_init(bb);
        crate::generated::clear_phy_baseband_790c_init(bb);
        crate::generated::enable_phy_baseband_7ca8_init(bb);
        crate::generated::clear_phy_baseband_7980_init(bb);

        // Complete ROM updates the adjacent mode bits through separate reads.
        crate::generated::clear_phy_he_ru26_good_response_disable(bb);
        crate::generated::enable_phy_he_ru26_good_response(bb);
        crate::generated::clear_phy_baseband_7a28_init(bb);
        crate::generated::initialize_phy_baseband_mode_fields(bb);
        crate::generated::enable_phy_baseband_tx_pa_init(bb);
    }

    /// Apply the five local tail edges of complete `phy_bb_reg_init`.
    pub fn initialize_baseband_tail(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        // Complete ROM clears bits 7:6 and bit 8 through separate reads.
        crate::generated::clear_phy_baseband_743c_low(bb);
        crate::generated::clear_phy_baseband_743c_high(bb);
        crate::generated::enable_phy_baseband_7428_init(bb);
        crate::generated::initialize_phy_baseband_7428_value(bb);
        crate::generated::initialize_phy_baseband_mode_fields(bb);
    }

    /// Apply the five internal-MMIO stores of complete ROM `phy_pwdet_reg_init`.
    pub fn initialize_power_detector_registers(&mut self) -> Result<(), TxDcPwdetLifecycleError> {
        if self.restore_slot.txdc_pending() {
            return Err(TxDcPwdetLifecycleError::RestorePending);
        }
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::svd::fixed_register_image::initialize_power_detector_table_0(bb);
        crate::svd::fixed_register_image::initialize_power_detector_table_1(bb);
        crate::generated::initialize_phy_power_detector_calibration(bb);
        crate::svd::zero_based_field_write::power_detector_reference(bb, 0xaaaa);
        crate::generated::initialize_phy_power_detector_mode(bb);
        Ok(())
    }

    /// Apply the internal-MMIO portion of complete ROM `phy_en_pwdet`.
    pub fn configure_power_detector_enabled(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::clear_phy_power_detector_enable_middle(bb);
        crate::generated::clear_phy_power_detector_enable_low(bb);
        crate::generated::clear_phy_power_detector_enable_high(bb);
        crate::generated::enable_phy_power_detector_sar_mode(bb);
        crate::generated::clear_phy_power_detector_sar_config(bb);
        crate::svd::zero_based_field_write::power_detector_reference(bb, 0x016a);
    }

    /// Set the final background-control bit after PWDET enable.
    pub fn enable_power_detector_background_control(&mut self) {
        crate::generated::enable_phy_power_detector_background_control(
            &self.peripherals.phy_baseband_config_oracle,
        );
    }

    /// Save and replace the two fields owned by TX-DC PWDET calibration.
    ///
    /// The private restore slot is filled before either temporary field is
    /// published. A second caller is rejected without touching MMIO, so it
    /// cannot steal the first caller's restore authority.
    pub fn prepare_txdc_power_detector(&mut self) -> Result<(), TxDcPwdetPrepareError> {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        self.restore_slot.prepare_txdc_with(
            || {
                (
                    TxDcPwdetRestoreFields {
                        table_low:
                            crate::svd::field_read::capture_phy_txdc_power_detector_table_low(bb),
                        calibration:
                            crate::svd::field_read::capture_phy_txdc_power_detector_calibration(bb),
                    },
                    (),
                )
            },
            |()| {
                crate::generated::prepare_phy_txdc_power_detector_table_low(bb);
                crate::generated::prepare_phy_txdc_power_detector_calibration(bb);
            },
        )
    }

    /// Select TX-DC SAR mode one after the initial PBus setup.
    pub fn configure_txdc_power_detector_sar(&mut self) {
        crate::generated::select_phy_txdc_power_detector_sar_mode(
            &self.peripherals.phy_baseband_config_oracle,
        );
    }

    /// Restore the privately saved TX-DC fields and select final SAR mode.
    ///
    /// A caller without a successful prepare operation is rejected without
    /// touching MMIO. The slot is cleared only after the complete restore
    /// sequence has run.
    pub fn restore_txdc_power_detector(&mut self) -> Result<(), TxDcPwdetRestoreError> {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        self.restore_slot.restore_txdc_with(|fields| {
            let table_low =
                crate::generated::PhyPowerDetectorRestoreByte::new(u32::from(fields.table_low))
                    .expect("captured power-detector byte fits its generated restore domain");
            let calibration =
                crate::generated::PhyPowerDetectorRestoreByte::new(u32::from(fields.calibration))
                    .expect("captured power-detector byte fits its generated restore domain");
            crate::generated::restore_phy_txdc_power_detector_table_low(bb, table_low);
            crate::generated::restore_phy_txdc_power_detector_calibration(bb, calibration);
            crate::generated::enable_phy_power_detector_sar_mode(bb);
        })
    }

    /// Publish one zero-extended power-detector reference word.
    pub fn write_power_detector_reference(&mut self, value: u16) {
        crate::svd::zero_based_field_write::power_detector_reference(
            &self.peripherals.phy_baseband_config_oracle,
            value,
        );
    }

    /// Pulse the power-detector SAR trigger through two fresh RMW edges.
    pub fn trigger_power_detector_sar(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::lower_phy_power_detector_sar_trigger(bb);
        crate::generated::raise_phy_power_detector_sar_trigger(bb);
    }

    /// Read the SVD-described power-detector readiness field.
    pub fn power_detector_ready(&mut self) -> bool {
        crate::svd::field_read::observe_phy_power_detector_ready(
            &self.peripherals.phy_baseband_config_oracle,
        ) == 0b111
    }

    /// Read the SVD-described power-detector SAR sample field.
    pub fn power_detector_sar_sample(&mut self) -> u16 {
        crate::svd::field_read::observe_phy_power_detector_sar_sample(
            &self.peripherals.phy_baseband_config_oracle,
        )
    }

    fn clear_tx_gain_compensation(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::svd::zero_register_write::clear_tx_gain_compensation(bb);
        crate::svd::zero_register_write::clear_tx_gain_compensation_aux(bb);
    }

    /// Apply complete pinned `phy_txgain_comp_pacfg_new(1)` as four ordered
    /// fresh-read byte updates.
    pub fn restore_tx_gain_compensation(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::restore_phy_tx_gain_compensation_byte_0(bb);
        crate::generated::restore_phy_tx_gain_compensation_byte_1(bb);
        crate::generated::restore_phy_tx_gain_compensation_byte_2(bb);
        crate::generated::restore_phy_tx_gain_compensation_byte_3(bb);
    }

    fn configure_tone_selectors(&mut self, path_0: u16, path_1: u16) {
        let path_0 = crate::generated::PhyToneSelector::new(u32::from(path_0))
            .expect("tone selector must fit the generated ten-bit domain");
        let path_1 = crate::generated::PhyToneSelector::new(u32::from(path_1))
            .expect("tone selector must fit the generated ten-bit domain");
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::configure_phy_tone_path_0_selector_low(bb, path_0);
        crate::generated::configure_phy_tone_path_1_selector_low(bb, path_1);
    }

    fn configure_tone_paths(&mut self, enabled: bool, path_0_selector: u16, path_0_step: u8) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::configure_phy_tone_path_0(
            bb,
            tone_selector_high(path_0_selector),
            tone_two_bit(0),
            tone_byte(path_0_step.wrapping_neg()),
            enabled,
            tone_three_bit(0),
            tone_two_bit(0),
            tone_four_bit(0),
        );
        crate::generated::clear_phy_tone_path_1_low_image(bb);
    }

    /// Program the complete archive calibration-tone leaf and restore TX gain.
    ///
    /// This preserves every fresh-read/write edge in
    /// `libphy.a[phy_reg.o]::phy_start_tx_tone_step_new` and its
    /// `phy_txgain_comp_pacfg_new` child.
    pub fn configure_calibration_tone(&mut self, enabled: bool, selector: u16, step: u8) {
        self.clear_tx_gain_compensation();
        self.configure_tone_selectors(selector, 0);
        self.configure_tone_paths(enabled, selector, step);
        self.restore_tx_gain_compensation();
    }

    /// Program the ROM power-control tone with DAC scale and TX gain disabled.
    pub fn configure_power_control_tone(&mut self, selector: u16, step: u8) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::clear_phy_power_control_tone_stop(bb);
        crate::generated::clear_phy_dac_scale_high(bb);
        crate::generated::clear_phy_dac_scale_low(bb);
        self.clear_tx_gain_compensation();
        self.configure_tone_selectors(selector, 0);
        self.configure_tone_paths(true, selector, step);
    }

    /// Capture the first-path word into the private TX-IQ restore slot.
    ///
    /// A second caller is rejected before reading the register and therefore
    /// cannot replace another calibration's restore authority.
    pub fn prepare_txiq_tone_control_restore(&mut self) -> Result<(), TxIqToneControlPrepareError> {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        self.restore_slot.prepare_txiq_with(|| {
            let (
                selector_high,
                low_reserved_clear_unknown,
                negated_step_or_attenuation,
                tone_enable_or_arm,
                txiq_mismatch_mode_unknown,
                middle_reserved_clear_unknown,
                txiq_polarity_image,
                high_nibble_unknown,
            ) = crate::svd::field_snapshot_read::capture_phy_txiq_tone_control(bb);
            TxIqToneControlFields {
                selector_high,
                low_reserved_clear_unknown,
                negated_step_or_attenuation,
                tone_enable_or_arm,
                txiq_mismatch_mode_unknown,
                middle_reserved_clear_unknown,
                txiq_polarity_image,
                high_nibble_unknown,
            }
        })
    }

    /// Restore and consume the private TX-IQ tone-control field state.
    ///
    /// A caller without a successful prepare operation is rejected before
    /// MMIO. The slot is cleared only after the complete accessor write.
    pub fn restore_txiq_tone_control(&mut self) -> Result<(), TxIqToneControlRestoreError> {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        self.restore_slot.restore_txiq_with(|fields| {
            crate::svd::zero_based_field_write::restore_phy_txiq_tone_control(
                bb,
                fields.selector_high,
                fields.low_reserved_clear_unknown,
                fields.negated_step_or_attenuation,
                fields.tone_enable_or_arm,
                fields.txiq_mismatch_mode_unknown,
                fields.middle_reserved_clear_unknown,
                fields.txiq_polarity_image,
                fields.high_nibble_unknown,
            );
        })
    }

    /// Configure one of the two complete TX-IQ mismatch-power polarity edges.
    pub fn configure_txiq_mismatch_power(
        &mut self,
        first: bool,
        polarity: bool,
        attenuation: u8,
        selector: u16,
    ) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        if first {
            crate::generated::configure_phy_tone_path_0(
                bb,
                tone_selector_high(selector),
                tone_two_bit(0),
                tone_byte(attenuation.wrapping_neg()),
                true,
                tone_three_bit(5),
                tone_two_bit(0),
                tone_four_bit(if polarity { 4 } else { 0 }),
            );
            let selector = crate::generated::PhyToneSelector::new(u32::from(selector))
                .expect("tone selector must fit the generated ten-bit domain");
            crate::generated::configure_phy_tone_path_0_selector_low(bb, selector);
        } else {
            crate::generated::configure_phy_txiq_second_polarity(
                bb,
                tone_four_bit(if polarity { 8 } else { 1 }),
            );
        }
    }

    /// Set or clear the shared first-path arm bit for one PWDET sample.
    pub fn set_power_detector_tone_armed(&mut self, armed: bool) {
        let armed = if armed {
            crate::generated::PhyPowerDetectorToneArmState::Armed
        } else {
            crate::generated::PhyPowerDetectorToneArmState::Disarmed
        };
        crate::generated::set_phy_power_detector_tone_armed(
            &self.peripherals.phy_baseband_config_oracle,
            armed,
        );
    }

    /// Stop both tone paths and restore the two DAC-scale fields.
    pub fn stop_power_detector_tone(&mut self) {
        self.stop_calibration_tone_paths();
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::restore_phy_dac_scale_high(bb);
        crate::generated::restore_phy_dac_scale_low(bb);
    }

    /// Stop both tone paths without changing their DAC-scale fields.
    ///
    /// This is the complete pinned `libphy.a` `phy_stop_tx_tone_new` leaf.
    /// The longer ROM `phy_stop_tx_tone(1)` composes this exact prefix with
    /// two additional DAC-scale restores in [`Self::stop_power_detector_tone`].
    pub fn stop_calibration_tone_paths(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::disable_phy_tone_path_0(bb);
        crate::generated::disable_phy_tone_path_1(bb);
        crate::generated::stop_phy_tone_paths(bb);
    }

    /// Enter or complete the TX-IQ correction phase with one fresh RMW.
    ///
    /// Complete ROM `phy_rfcal_txiq` clears the high mode bit while setting
    /// the low bit on entry. Its completion edge sets only the high bit.
    pub fn configure_tx_iq_correction(&mut self, begin: bool) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        if begin {
            crate::generated::begin_phy_tx_iq_correction(bb);
        } else {
            crate::generated::complete_phy_tx_iq_correction(bb);
        }
    }

    /// Select the RX-IQ calibration mode with one fresh RMW.
    pub fn configure_rx_iq_calibration_mode(&mut self) {
        crate::generated::configure_phy_rx_iq_calibration_mode(
            &self.peripherals.phy_baseband_config_oracle,
        );
    }

    /// Publish one signed TX-IQ gain coefficient using the ROM saturation.
    pub fn set_tx_iq_gain_coefficient(&mut self, coefficient: i8) {
        crate::generated::publish_phy_tx_iq_gain_coefficient(
            &self.peripherals.phy_baseband_config_oracle,
            iq_coefficient_image(saturate_tx_iq_gain(coefficient)),
        );
    }

    /// Publish one signed TX-IQ phase coefficient using the ROM saturation.
    pub fn set_tx_iq_phase_coefficient(&mut self, coefficient: i8) {
        crate::generated::publish_phy_tx_iq_phase_coefficient(
            &self.peripherals.phy_baseband_config_oracle,
            iq_coefficient_image(saturate_tx_iq_phase(coefficient)),
        );
    }

    /// Publish one signed RX-IQ gain coefficient using the ROM truncation.
    pub fn set_rx_iq_gain_coefficient(&mut self, coefficient: i8) {
        crate::generated::publish_phy_rx_iq_gain_coefficient(
            &self.peripherals.phy_baseband_config_oracle,
            iq_coefficient_image(coefficient),
        );
    }

    /// Publish one signed RX-IQ phase coefficient using the ROM truncation.
    pub fn set_rx_iq_phase_coefficient(&mut self, coefficient: i8) {
        crate::generated::publish_phy_rx_iq_phase_coefficient(
            &self.peripherals.phy_baseband_config_oracle,
            iq_coefficient_image(coefficient),
        );
    }

    /// Trigger one TX-DC comparator measurement using three fresh RMW edges.
    pub fn trigger_tx_dc_measurement(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::enable_phy_tx_dc_measurement(bb);
        crate::generated::clear_phy_tx_dc_measurement_start(bb);
        crate::generated::start_phy_tx_dc_measurement(bb);
    }

    /// Sample the TX-DC ready bit exactly once.
    pub fn tx_dc_measurement_is_ready(&mut self) -> bool {
        crate::svd::field_read::observe_phy_tx_dc_measurement_ready(
            &self.peripherals.phy_baseband_config_oracle,
        )
    }

    /// Preserve the complete ROM's independent I and Q comparator reads.
    pub fn sample_tx_dc_comparators(&mut self) -> [bool; 2] {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        [
            crate::svd::field_read::observe_phy_tx_dc_i_comparator_high(bb),
            crate::svd::field_read::observe_phy_tx_dc_q_comparator_high(bb),
        ]
    }

    /// Clear TX-DC enable and start through two fresh RMW edges.
    pub fn clear_tx_dc_measurement(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::disable_phy_tx_dc_measurement(bb);
        crate::generated::clear_phy_tx_dc_measurement_start(bb);
    }

    /// Publish the two-register suffix of complete ROM `phy_adc_rate_set`.
    ///
    /// The ROM body at `0x2f82_a6d2`, size `0x4a`, uses two fresh reads to
    /// publish the selected semantic rate into the two recovered fields.
    pub fn configure_adc_rate(&mut self, rate: crate::PhyAdcRate) {
        let rate = match rate {
            crate::PhyAdcRate::Low => crate::generated::PhyAdcRateSelection::Low,
            crate::PhyAdcRate::High => crate::generated::PhyAdcRateSelection::High,
        };
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::configure_phy_adc_rate_high(bb, rate);
        crate::generated::configure_phy_adc_rate_low(bb, rate);
    }

    /// Apply the four front-end initialization edges before table-memory setup.
    ///
    /// This is the exact prefix of complete rev0 ROM `phy_fe_reg_init` at
    /// `0x2f82_7740`, size `0xf6`. The table-memory edge remains between this
    /// method and [`Self::initialize_front_end_suffix`].
    pub fn initialize_front_end_prefix(&mut self) {
        crate::generated::initialize_phy_front_end_pbus(&self.peripherals.phy_pbus);
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::initialize_phy_front_end_first(bb);
        crate::generated::initialize_phy_front_end_second(bb);
        crate::generated::clear_phy_front_end_first(bb);
    }

    /// Apply the twelve front-end initialization edges after table-memory setup.
    ///
    /// Complete rev0 ROM `phy_fe_reg_init` performs every update below using
    /// a fresh read. Repeated sets are retained because intermediate device
    /// states are observable hardware behavior.
    pub fn initialize_front_end_suffix(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::enable_phy_front_end_init(bb);
        crate::generated::enable_rx_iq_correction_modes(bb);
        crate::generated::enable_tx_iq_correction_modes(bb);
        crate::generated::clear_phy_front_end_second(bb);
        crate::generated::enable_phy_front_end_adc_rate_high(bb);
        crate::generated::enable_phy_front_end_adc_rate_low(bb);
        crate::generated::configure_phy_front_end_low(bb);
        crate::generated::enable_phy_front_end_adc_rate_low(bb);
        crate::generated::enable_phy_front_end_adc_rate_high(bb);
        crate::generated::enable_phy_rx_iq_front_end_high(bb);
        crate::generated::enable_phy_tx_iq_front_end_high(bb);
        crate::generated::initialize_phy_front_end_low(bb);
    }

    /// Apply complete pinned `libphy.a[phy_reg.o]::phy_fe_reg_update`.
    ///
    /// The `0x32`-byte body performs exactly three fresh-read RMW edges and
    /// has no ROM-only DAC-scale tail.
    pub fn update_front_end(&mut self) {
        let bb = &self.peripherals.phy_baseband_config_oracle;
        crate::generated::initialize_phy_front_end_first(bb);
        crate::generated::initialize_phy_front_end_second(bb);
        crate::generated::enable_phy_front_end_adc_rates(bb);
    }

    /// Select the direct-register prefix or cleanup state of RX-gain DC calibration.
    ///
    /// Complete rev0 ROM `phy_set_rx_gain_cal_dc` at `0x2f82_9858`, size
    /// `0x206`, sets bits 6:5 to `0b11` before entering the bounded
    /// calibration graph and clears them to `0b00` in its common cleanup.
    /// The field's narrower electrical meaning is not independently proved.
    pub fn set_rx_gain_dc_calibration(&mut self, enabled: bool) {
        let state = if enabled {
            crate::generated::PhyRxGainDcCalibrationState::Enabled
        } else {
            crate::generated::PhyRxGainDcCalibrationState::Disabled
        };
        crate::generated::configure_phy_rx_gain_dc_calibration(
            &self.peripherals.phy_baseband_config_oracle,
            state,
        );
    }
}

#[cfg(test)]
mod tests;
