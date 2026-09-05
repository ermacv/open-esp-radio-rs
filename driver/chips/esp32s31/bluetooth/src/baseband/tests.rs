use open_esp_radio_esp32s31_phy::PhyState;

use super::apply_baseband_input;

#[test]
fn baseband_transition_forwards_terminal_phy_input_once() {
    let mut phy = PhyState::default();
    let _parameters = phy.prepare_rx_table_init();
    let expected_gain = phy.register_init_parameters().parameter_120;
    let mut observed = None;

    let report = apply_baseband_input(&phy, |gain_parameter| {
        assert!(observed.replace(gain_parameter).is_none());
    });

    assert_eq!(observed, Some(expected_gain));
    assert_eq!(report.gain_parameter, expected_gain);
}
