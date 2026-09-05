//! Exact bounded Bluetooth baseband initialization transaction.
//!
//! This module is deliberately crate-private. The transaction is the observed
//! MMIO portion of `bt_bb_v2_init_cmplx(1)`, but it is not a controller-enable
//! API: the vendor lifecycle first completes common PHY initialization. The
//! standalone Bluetooth edge closes that body with a device fence. The IEEE
//! 802.15.4 edge deliberately leaves the body unfenced so its two source-owned
//! timing overrides and the sole final fence remain one inseparable transition.

#![deny(unsafe_code)]

use crate::{BluetoothTaskRegisters, Ieee802154TaskRegisters, device_fence, svd};

impl BluetoothTaskRegisters {
    /// Execute only the exact MMIO path of vendor `bt_bb_v2_init_cmplx(1)`.
    ///
    /// The caller-provided byte is the positional value read by the vendor
    /// body from the real linked `phy_param` object at offset `0x120`. Its
    /// higher-level meaning remains unassigned. The vendor diagnostic print
    /// selected by argument one is intentionally outside this hardware
    /// transaction and is omitted from the comparison effect contract. After
    /// the body, this standalone Bluetooth edge adds exactly one device fence
    /// before returning to its lifecycle owner.
    ///
    /// Common-PHY and clock ordering do not belong to the PAC. The controller
    /// lifecycle above HAL is the only safe production caller and retains
    /// those owners while this finite register transaction executes.
    #[doc(hidden)]
    pub fn initialize_baseband_v2_arg_one(&mut self, gain_parameter: u8) {
        let mut port = BluetoothBasebandV2Transaction {
            bluetooth: &self.bluetooth,
            radio_phy: &self.radio_phy.peripherals,
            shared_radio: &self.shared_radio,
        };
        execute_standalone_bluetooth_transition(&mut port, gain_parameter);
    }
}

impl Ieee802154TaskRegisters {
    /// Execute the shared BTBB transaction required after common PHY setup.
    ///
    /// This crate-private edge is the same recovered
    /// `bt_bb_v2_init_cmplx(1)` MMIO body used by the standalone Bluetooth
    /// owner, without the standalone lifecycle fence. The public IEEE 802.15.4
    /// transition is deliberately defined in `ieee802154_timing`: it appends
    /// both protocol-specific timing overrides and the sole final fence before
    /// returning, so downstream code cannot stop at this internal boundary.
    ///
    /// # Safety
    ///
    /// The caller must prove that controller clocks/resets are active, common
    /// PHY initialization completed for this same hardware owner, and
    /// `gain_parameter` came from that terminal PHY state. Every physical
    /// owner must remain retained until a verified last-owner PHY teardown;
    /// the task partition must not be reunited into cold ownership after this
    /// transaction without that teardown.
    #[allow(
        unsafe_code,
        reason = "the unsafe signature encodes the cross-crate common-PHY hardware prerequisite"
    )]
    pub(crate) unsafe fn initialize_baseband_v2_arg_one_body_without_fence(
        &mut self,
        gain_parameter: u8,
    ) {
        let mut port = BluetoothBasebandV2Transaction {
            bluetooth: &self.peripherals.btbb.bluetooth,
            radio_phy: &self.peripherals.radio_phy.peripherals,
            shared_radio: &self.peripherals.btbb.shared_radio,
        };
        execute_ieee802154_baseband_body(&mut port, gain_parameter);
    }
}

trait BluetoothBasebandV2TransitionPort {
    fn execute_body(&mut self, gain_parameter: u8);
    fn order_device_accesses(&mut self);
}

fn execute_standalone_bluetooth_transition<Port>(port: &mut Port, gain_parameter: u8)
where
    Port: BluetoothBasebandV2TransitionPort,
{
    port.execute_body(gain_parameter);
    port.order_device_accesses();
}

fn execute_ieee802154_baseband_body<Port>(port: &mut Port, gain_parameter: u8)
where
    Port: BluetoothBasebandV2TransitionPort,
{
    port.execute_body(gain_parameter);
}

/// One borrow-scoped view of the exact generated owners touched by BTBB v2.
struct BluetoothBasebandV2Transaction<'a> {
    bluetooth: &'a svd::peripheral_ownership::BluetoothControllerPeripherals,
    radio_phy: &'a svd::peripheral_ownership::RadioPhyPeripherals,
    shared_radio: &'a svd::peripheral_ownership::SharedRadioPeripherals,
}

impl BluetoothBasebandV2TransitionPort for BluetoothBasebandV2Transaction<'_> {
    fn execute_body(&mut self, gain_parameter: u8) {
        self.initialize_baseband_v2_tx();
        self.initialize_baseband_v2_rx(gain_parameter);
        self.initialize_gaussian_1m_coefficients();
        self.initialize_gaussian_2m_coefficients();
        self.initialize_baseband_tx_timing();
        self.initialize_baseband_coexistence_defaults();
        self.initialize_baseband_cca_defaults();
        self.initialize_shared_receive_image();
    }

    fn order_device_accesses(&mut self) {
        device_fence();
    }
}

impl BluetoothBasebandV2Transaction<'_> {
    fn initialize_baseband_v2_tx(&self) {
        let baseband = &self.bluetooth.bt_v3_2_baseband;
        crate::generated::initialize_bluetooth_baseband_tx_argument(baseband);
        crate::generated::initialize_bluetooth_baseband_tx_setup_image(baseband);
        crate::generated::initialize_bluetooth_baseband_tx_low_byte(baseband);
    }

    fn initialize_baseband_v2_rx(&self, gain_parameter: u8) {
        self.initialize_baseband_rx_setup();
        self.initialize_receive_compensation();
        self.initialize_receive_gain_offsets();
        self.initialize_receive_gain(gain_parameter);
        self.initialize_receive_rssi_thresholds();
        self.initialize_receive_targets();
        self.initialize_receive_restart();
        self.initialize_receive_recorrection();
        self.initialize_receive_detection();

        let baseband = &self.bluetooth.bt_v3_2_baseband;
        crate::generated::initialize_bluetooth_receive_correlator(baseband);
        crate::generated::initialize_bluetooth_receive_dpo_bit_19(baseband);
        crate::generated::initialize_bluetooth_receive_dpo_value(baseband);
        crate::generated::initialize_bluetooth_receive_filter_1(baseband);
        crate::generated::initialize_bluetooth_receive_filter_5(baseband);
        crate::generated::initialize_bluetooth_receive_filter_4(baseband);
        crate::generated::initialize_bluetooth_receive_filter_3(baseband);
        crate::generated::initialize_bluetooth_receive_filter_2(baseband);
        crate::generated::initialize_bluetooth_receive_filter_0(baseband);

        self.initialize_shared_receive_prefix();
        crate::generated::initialize_bluetooth_receive_correlator_final(baseband);
    }

    fn initialize_baseband_rx_setup(&self) {
        let baseband = &self.bluetooth.bt_v3_2_baseband;
        crate::generated::initialize_bluetooth_receive_setup_argument(baseband);
        crate::generated::initialize_bluetooth_receive_setup_image(baseband);
        crate::generated::initialize_bluetooth_receive_setup_shared_control(
            &self.shared_radio.shared_radio_init_control,
        );
        crate::generated::set_bluetooth_receive_setup_control_1_bit_2(baseband);

        let btagc = &self.radio_phy.phy_btagc_recovered;
        crate::generated::initialize_bluetooth_receive_setup_agc_00d0_high(btagc);
        crate::generated::initialize_bluetooth_receive_setup_agc_00d0_low(btagc);
        crate::generated::initialize_bluetooth_receive_setup_agc_00d4(btagc);
        crate::generated::initialize_bluetooth_receive_setup_008c_bit_29_initial(btagc);
        crate::generated::initialize_bluetooth_receive_setup_0088_bit_29(btagc);
        crate::generated::initialize_bluetooth_receive_setup_0088_bit_18(btagc);
        crate::generated::initialize_bluetooth_receive_setup_cca_1(baseband);
        crate::generated::initialize_bluetooth_receive_setup_cca_2_disable(baseband);
        crate::generated::initialize_bluetooth_receive_setup_cte_value(btagc);
        crate::generated::initialize_bluetooth_receive_setup_cte_max(btagc);
        crate::generated::initialize_bluetooth_receive_setup_cca_2_enable(baseband);
        crate::generated::initialize_bluetooth_receive_setup_004c_bit_26(btagc);
        crate::generated::initialize_bluetooth_receive_setup_008c_bit_29_final(btagc);
        crate::generated::set_bluetooth_receive_setup_control_0_bit_0(baseband);

        let shared = &self.shared_radio.zbbb_radio_control;
        crate::generated::initialize_bluetooth_receive_setup_zbbb_bit_8(shared);
        crate::generated::initialize_bluetooth_receive_setup_zbbb_image(shared);
        crate::generated::initialize_bluetooth_receive_setup_zbbb_low(shared);
    }

    fn initialize_receive_compensation(&self) {
        let btagc = &self.radio_phy.phy_btagc_recovered;
        crate::generated::initialize_bluetooth_receive_compensation_7_13(btagc);
        crate::generated::initialize_bluetooth_receive_compensation_0_6(btagc);
        crate::generated::initialize_bluetooth_receive_compensation_24_28(btagc);
        crate::generated::initialize_bluetooth_receive_compensation_19_23(btagc);
        crate::generated::initialize_bluetooth_receive_compensation_14_18(btagc);
    }

    fn initialize_receive_gain_offsets(&self) {
        let btagc = &self.radio_phy.phy_btagc_recovered;
        crate::generated::initialize_bluetooth_receive_gain_offset_0_bits_5_9(btagc);
        crate::generated::initialize_bluetooth_receive_gain_offset_0_bits_0_4(btagc);
        crate::generated::initialize_bluetooth_receive_gain_offset_0_bits_16_23(btagc);
        crate::generated::initialize_bluetooth_receive_gain_offset_0_bits_24_31(btagc);
        crate::generated::initialize_bluetooth_receive_gain_offset_1_bits_24_31(btagc);
        crate::generated::initialize_bluetooth_receive_gain_offset_1_bits_16_23(btagc);
    }

    fn initialize_receive_gain(&self, parameter: u8) {
        crate::generated::initialize_bluetooth_receive_gain_baseband(
            &self.radio_phy.phy_baseband_config_oracle,
        );
        let parameter =
            crate::generated::BluetoothBasebandGainParameterByte::new(u32::from(parameter))
                .expect("every byte belongs to the complete reviewed domain");
        crate::generated::initialize_bluetooth_receive_gain_force(
            &self.radio_phy.phy_btagc_recovered,
            parameter,
        );
        crate::generated::initialize_bluetooth_receive_gain_image(
            &self.radio_phy.phy_btagc_recovered,
            parameter,
        );
    }

    fn initialize_receive_rssi_thresholds(&self) {
        let btagc = &self.radio_phy.phy_btagc_recovered;
        crate::generated::initialize_bluetooth_receive_rssi_threshold_00a0(btagc);
        crate::generated::initialize_bluetooth_receive_rssi_threshold_00a8_high(btagc);
        crate::generated::initialize_bluetooth_receive_rssi_threshold_00a8_low(btagc);
        crate::generated::initialize_bluetooth_receive_rssi_threshold_00b8(btagc);
    }

    fn initialize_receive_targets(&self) {
        let btagc = &self.radio_phy.phy_btagc_recovered;
        crate::generated::initialize_bluetooth_receive_target_cte(btagc);
        crate::generated::initialize_bluetooth_receive_target_00b0(btagc);
        crate::generated::initialize_bluetooth_receive_target_gain_force(btagc);
        crate::generated::initialize_bluetooth_receive_target_008c_low(btagc);
        crate::generated::initialize_bluetooth_receive_target_008c_high(btagc);
        crate::generated::initialize_bluetooth_receive_target_cca(btagc);
        crate::generated::initialize_bluetooth_receive_target_00b4(btagc);
    }

    fn initialize_receive_restart(&self) {
        let btagc = &self.radio_phy.phy_btagc_recovered;
        crate::generated::initialize_bluetooth_receive_restart_config_0088(btagc);
        crate::generated::initialize_bluetooth_receive_restart_control_00bc(btagc);
        crate::generated::initialize_bluetooth_receive_restart_config_0084(btagc);

        crate::generated::initialize_bluetooth_receive_restart_e0_24_31(btagc);
        crate::generated::initialize_bluetooth_receive_restart_e0_16_23(btagc);
        crate::generated::initialize_bluetooth_receive_restart_e0_8_15(btagc);
        crate::generated::initialize_bluetooth_receive_restart_e4_16_23(btagc);
        crate::generated::initialize_bluetooth_receive_restart_e4_8_15(btagc);
        crate::generated::initialize_bluetooth_receive_restart_e4_0_7(btagc);
        crate::generated::initialize_bluetooth_receive_restart_e0_0_7(btagc);
        crate::generated::initialize_bluetooth_receive_restart_e8_24_31(btagc);
        crate::generated::initialize_bluetooth_receive_restart_ec_24_31(btagc);
        crate::generated::initialize_bluetooth_receive_restart_ec_8_15(btagc);
        crate::generated::initialize_bluetooth_receive_restart_f0_24_31(btagc);
        crate::generated::initialize_bluetooth_receive_restart_e4_24_31(btagc);
        crate::generated::initialize_bluetooth_receive_restart_e8_16_23(btagc);
        crate::generated::initialize_bluetooth_receive_restart_ec_16_23(btagc);
        crate::generated::initialize_bluetooth_receive_restart_ec_0_7(btagc);
        crate::generated::initialize_bluetooth_receive_restart_f0_16_23(btagc);

        crate::generated::initialize_bluetooth_receive_restart_100_20_23(btagc);
        crate::generated::initialize_bluetooth_receive_restart_100_24_27(btagc);
        crate::generated::initialize_bluetooth_receive_restart_100_16_19(btagc);
        crate::generated::initialize_bluetooth_receive_restart_100_12_15(btagc);
        crate::generated::initialize_bluetooth_receive_restart_100_28_31(btagc);
        crate::generated::initialize_bluetooth_receive_restart_e8_10_15(btagc);

        let baseband = &self.bluetooth.bt_v3_2_baseband;
        crate::generated::initialize_bluetooth_receive_restart_correlator(baseband);
        crate::generated::initialize_bluetooth_receive_restart_baseband_21(baseband);
        crate::generated::initialize_bluetooth_receive_restart_baseband_20(baseband);
        crate::generated::initialize_bluetooth_receive_restart_baseband_19(baseband);
        crate::generated::initialize_bluetooth_receive_restart_baseband_10(baseband);
        crate::generated::initialize_bluetooth_receive_restart_baseband_9(baseband);
        crate::generated::initialize_bluetooth_receive_restart_baseband_8(baseband);
        crate::generated::initialize_bluetooth_receive_restart_baseband_7(baseband);
        crate::generated::initialize_bluetooth_receive_restart_baseband_6(baseband);

        crate::generated::initialize_bluetooth_receive_restart_btagc_31(btagc);
        crate::generated::initialize_bluetooth_receive_restart_btagc_30(btagc);
        crate::generated::initialize_bluetooth_receive_restart_btagc_29(btagc);
        crate::generated::initialize_bluetooth_receive_restart_btagc_27(btagc);
        crate::generated::initialize_bluetooth_receive_restart_btagc_28(btagc);

        crate::generated::initialize_bluetooth_receive_restart_f8_2_7(btagc);
        crate::generated::initialize_bluetooth_receive_restart_f8_8_13(btagc);
        crate::generated::initialize_bluetooth_receive_restart_f8_14_19(btagc);
        crate::generated::initialize_bluetooth_receive_restart_f8_26_31(btagc);
        crate::generated::initialize_bluetooth_receive_restart_f8_20_25(btagc);
    }

    fn initialize_receive_recorrection(&self) {
        let btagc = &self.radio_phy.phy_btagc_recovered;
        crate::generated::initialize_bluetooth_receive_recorrection_00b4_bit_8(btagc);
        crate::generated::initialize_bluetooth_receive_recorrection_00b4_bits_9_13(btagc);
        crate::generated::initialize_bluetooth_receive_recorrection_00bc_set_pair(btagc);
        crate::generated::initialize_bluetooth_receive_recorrection_gain_bit_13(btagc);
        crate::generated::initialize_bluetooth_receive_recorrection_00bc_clear_pair(btagc);
        crate::generated::initialize_bluetooth_receive_recorrection_gain_bit_5(btagc);
        crate::generated::initialize_bluetooth_receive_recorrection_gain_bits_11_12(btagc);
        crate::generated::initialize_bluetooth_receive_recorrection_gain_bits_6_10(btagc);
        crate::generated::initialize_bluetooth_receive_recorrection_00b0_bits_18_22(btagc);
        crate::generated::initialize_bluetooth_receive_recorrection_00b0_bits_13_17(btagc);
        crate::generated::initialize_bluetooth_receive_recorrection_gain_bits_0_4(btagc);
        crate::generated::initialize_bluetooth_receive_recorrection_control_006c(btagc);
    }

    fn initialize_receive_detection(&self) {
        let btagc = &self.radio_phy.phy_btagc_recovered;
        crate::generated::initialize_bluetooth_receive_detection_00a0(btagc);
        crate::generated::initialize_bluetooth_receive_detection_00c0_bits_5_9(btagc);
        crate::generated::initialize_bluetooth_receive_detection_00a8_middle(btagc);
        crate::generated::initialize_bluetooth_receive_detection_00a8_low(btagc);
        crate::generated::initialize_bluetooth_receive_detection_00c0_bits_10_14(btagc);
        crate::generated::initialize_bluetooth_receive_detection_00b8(btagc);
        crate::generated::initialize_bluetooth_receive_detection_00c4_bits_10_14(btagc);
        crate::generated::initialize_bluetooth_receive_detection_00c0_bits_24_31(btagc);
        crate::generated::initialize_bluetooth_receive_detection_00c0_bits_20_23(btagc);
        crate::generated::initialize_bluetooth_receive_detection_00c0_bits_15_19(btagc);
        crate::generated::initialize_bluetooth_receive_detection_00c4_bits_24_31(btagc);
        crate::generated::initialize_bluetooth_receive_detection_00c4_bits_20_23(btagc);
        crate::generated::initialize_bluetooth_receive_detection_00c4_bits_15_19(btagc);
    }

    fn initialize_gaussian_1m_coefficients(&self) {
        let baseband = &self.bluetooth.bt_v3_2_baseband;
        crate::generated::initialize_bluetooth_gaussian_1m_0_bits_28_31(baseband);
        crate::generated::initialize_bluetooth_gaussian_1m_0_bits_23_27(baseband);
        crate::generated::initialize_bluetooth_gaussian_1m_0_bits_17_22(baseband);
        crate::generated::initialize_bluetooth_gaussian_1m_0_bits_10_16(baseband);
        crate::generated::initialize_bluetooth_gaussian_1m_0_bits_2_9(baseband);
        crate::generated::initialize_bluetooth_gaussian_1m_1_bits_23_31(baseband);
        crate::generated::initialize_bluetooth_gaussian_1m_1_bits_13_22(baseband);
        crate::generated::initialize_bluetooth_gaussian_1m_1_bits_2_12(baseband);
        crate::generated::initialize_bluetooth_gaussian_1m_2_bits_21_31(baseband);
        crate::generated::initialize_bluetooth_gaussian_1m_2_bits_10_20(baseband);
        crate::generated::initialize_bluetooth_gaussian_1m_3_bits_21_31(baseband);
        crate::generated::initialize_bluetooth_gaussian_1m_3_bits_10_20(baseband);
    }

    fn initialize_gaussian_2m_coefficients(&self) {
        let baseband = &self.bluetooth.bt_v3_2_baseband;
        crate::generated::initialize_bluetooth_gaussian_2m_0_bits_25_31(baseband);
        crate::generated::initialize_bluetooth_gaussian_2m_0_bits_17_24(baseband);
        crate::generated::initialize_bluetooth_gaussian_2m_0_bits_8_16(baseband);
        crate::generated::initialize_bluetooth_gaussian_2m_1_bits_22_31(baseband);
        crate::generated::initialize_bluetooth_gaussian_2m_1_bits_11_21(baseband);
        crate::generated::initialize_bluetooth_gaussian_2m_1_bits_0_10(baseband);
    }

    fn initialize_baseband_tx_timing(&self) {
        crate::generated::initialize_bluetooth_tx_pa_delay(
            &self.radio_phy.phy_baseband_config_oracle,
        );
        let baseband = &self.bluetooth.bt_v3_2_baseband;
        crate::generated::initialize_bluetooth_le_tx_delay(baseband);
        crate::generated::initialize_bluetooth_tx_cca_period_difference(baseband);
        crate::generated::initialize_bluetooth_tx_cca_period_argument(baseband);
        crate::generated::initialize_bluetooth_shared_tx_delay(
            &self.shared_radio.shared_baseband_tx_timing,
        );
    }

    fn initialize_baseband_coexistence_defaults(&self) {
        crate::generated::initialize_bluetooth_baseband_coexistence_18(
            &self.bluetooth.bt_v3_2_baseband,
        );
        crate::generated::initialize_bluetooth_baseband_coexistence_20(
            &self.bluetooth.bt_v3_2_baseband,
        );
    }

    fn initialize_baseband_cca_defaults(&self) {
        crate::generated::initialize_bluetooth_baseband_cca_value_0(
            &self.radio_phy.phy_btagc_recovered,
        );
        crate::generated::initialize_bluetooth_baseband_cca_value_1(
            &self.radio_phy.phy_btagc_recovered,
        );
        crate::generated::initialize_bluetooth_baseband_cca_bit_23(
            &self.radio_phy.phy_btagc_recovered,
        );
        crate::generated::initialize_bluetooth_baseband_cca_default(
            &self.bluetooth.bt_v3_2_baseband,
        );
    }

    fn initialize_shared_receive_prefix(&self) {
        let shared = &self.shared_radio.zbbb_radio_control;
        crate::generated::initialize_shared_receive_image_0_high(shared);
        crate::generated::initialize_shared_receive_control_image(shared);
        crate::generated::initialize_shared_receive_image_1_low(shared);
        crate::generated::initialize_shared_receive_image_0_middle(shared);
        crate::generated::initialize_shared_receive_image_4_high(shared);
        crate::generated::initialize_shared_receive_image_4_middle(shared);
        crate::generated::initialize_shared_receive_image_2_high(shared);
        crate::generated::initialize_shared_receive_image_3_high(shared);
        crate::generated::initialize_shared_receive_control_0(shared);
        crate::generated::initialize_shared_receive_control_1(shared);
        crate::generated::initialize_shared_receive_image_2_low(shared);
        crate::generated::initialize_shared_receive_image_4_low(shared);
    }

    fn initialize_shared_receive_image(&self) {
        self.initialize_shared_receive_prefix();
        crate::generated::initialize_bluetooth_shared_receive_final_image(
            &self.shared_radio.zbbb_radio_control,
        );
    }
}

#[cfg(test)]
mod tests;
