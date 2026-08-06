//! HIL-only observations emitted from typed PHY and MAC callbacks.

#![forbid(unsafe_code)]

use core::sync::atomic::Ordering;

use open_esp_radio::esp32s31::phy::{PhyRfBoundary, PhyTargetObserver};

use super::{DIAGNOSTIC_ACTION_ORDINAL, set_diagnostic_stage};
use crate::console::emergency_log;

#[derive(Clone, Copy)]
pub(super) struct HilPhyObserver;

impl PhyTargetObserver for HilPhyObserver {
    fn operation_started(&mut self) {
        DIAGNOSTIC_ACTION_ORDINAL.fetch_add(1, Ordering::AcqRel);
        set_diagnostic_stage(110);
        set_diagnostic_stage(120);
    }

    fn operation_completed(&mut self) {
        set_diagnostic_stage(130);
    }

    fn channel_frequency_ready_timed_out(&mut self, samples: u32) {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL channel=frequency-ready-timeout samples={samples}"
        ));
    }

    fn channel_completed(
        &mut self,
        outcome: open_esp_radio::esp32s31::phy::phy_channel::PhyChipChannelOutcome,
        operations: u32,
    ) {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL stage=post-init-channel channel={} \
             frequency={} operations={operations}",
            outcome.channel, outcome.frequency_mhz,
        ));
    }

    fn channel_failed(
        &mut self,
        failure: open_esp_radio::esp32s31::phy::phy_channel::PhyChipChannelFailure,
        operations: u32,
    ) {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=FAIL stage=post-init-channel \
             failure={failure:?} operations={operations}"
        ));
    }

    fn mac_channel_restarted(&mut self, channel_or_frequency: u16, cbw: u8, link: u8) {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL stage=mac-channel-restart \
             channel_or_frequency={channel_or_frequency} cbw={cbw} regdma_link={link}"
        ));
    }

    fn tx_dc_entry(&mut self) {
        emergency_log(format_args!("OPEN_RADIO_PHY_HIL stage=txdc-entry"));
    }

    fn tx_dc_comparator(&mut self, gain_index: u8, iteration: u8, comparator_high: [bool; 2]) {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL stage=txdc-comparator gain={gain_index} \
             iteration={iteration} comparator={comparator_high:?}"
        ));
    }

    fn power_detector_sample(
        &mut self,
        measurement_index: u8,
        sample_index: u8,
        sample_value: u16,
    ) {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL stage=pwdet-sample measurement={measurement_index} \
             sample={sample_index} value={sample_value}"
        ));
    }

    fn rf_boundary(&mut self, boundary: PhyRfBoundary) {
        let source = match boundary {
            PhyRfBoundary::BeforeRfInit => "open-before-rf-init",
            PhyRfBoundary::AfterPbusClear => "open-after-pbus-clear",
            PhyRfBoundary::BeforeI2cMasterRegisterInit => "open-before-i2cmst-reg-init",
            PhyRfBoundary::BeforePowerDetectorRegisterInit => "open-before-pwdet-reg-init",
            PhyRfBoundary::BeforeFrontEndRegisterInit => "open-before-fe-reg-init",
            PhyRfBoundary::BeforeTemperatureSensorReadInit => "open-before-tsens-read-init",
            PhyRfBoundary::BeforeTxPowerControlBackgroundInit => "open-before-tx-pwctrl-bg-init",
            PhyRfBoundary::BeforeChannelFrequencyInit => "open-before-chan-freq-init",
        };
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL stage=rf-boundary source={source}"
        ));
    }
}
