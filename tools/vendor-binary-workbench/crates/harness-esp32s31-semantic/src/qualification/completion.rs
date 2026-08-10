//! Deterministic host-side completions shared by hierarchical PHY contracts.
//!
//! This is deliberately not a device model and never contributes proof by
//! itself. It supplies one reviewed, finite environment to Rust transitions;
//! the surrounding contract still compares the resulting semantic projection
//! with a concretely executed vendor path.

use super::*;

#[derive(Default)]
pub(super) struct DeterministicPhyCompletion {
    tx_power_events: Vec<BluetoothTxPowerEvent>,
    txdc_events: Vec<BluetoothTxDcEvent>,
    txdc_pwdet_events: Vec<BluetoothTxDcPwdetEvent>,
    rx_iq_cap_status_reads: u8,
    rx_iq_configured_phase: Option<i8>,
}

mod common;
mod parent;
mod rx_gain;
mod rx_iq;
mod tx;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_driver_preserves_action_identity() {
        let mut driver = DeterministicPhyCompletion::default();
        let publication =
            open_esp_radio_esp32s31_phy::phy_bluetooth::PhyBluetoothTxGainPublication::new(
                open_esp_radio_esp32s31_phy::phy_bluetooth::PhyBluetoothTxGainImage {
                    seed: [0; 6],
                    output_32: [0; 16],
                    output_64: [0; 16],
                    output_72: [0; 16],
                    config: 0,
                },
            );
        assert_eq!(
            driver
                .bluetooth_tx_gain(PhyBluetoothTxGainInitAction::Publish(publication))
                .unwrap(),
            PhyBluetoothTxGainInitCompletion::Published(publication)
        );
    }
}
