//! HIL-only PHY observations and diagnostic snapshots.

use super::*;

pub(super) struct EmbassyPhyDelay;

impl PhyAsyncDelay for EmbassyPhyDelay {
    fn after_micros(micros: u64) -> impl core::future::Future<Output = ()> {
        Timer::after_micros(micros)
    }
}

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
             channel_or_frequency={channel_or_frequency} cbw={cbw} \
             control={:#010x} regdma_link={link}",
            read_diagnostic_mmio(0x2010_4cac),
        ));
    }

    fn tx_dc_entry(&mut self) {
        log_open_txdc_entry_mmio();
    }

    fn tx_dc_comparator(&mut self, gain_index: u8, iteration: u8, comparator_high: [bool; 2]) {
        if gain_index == 0 && iteration == 0 {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL probe=txdc-first-environment \
                 bb_init={:#010x} pbus={:#010x}/{:#010x}/{:#010x} \
                 tone={:#010x}/{:#010x}/{:#010x}/{:#010x} control={:#010x}",
                read_diagnostic_mmio(0x2010_0800),
                read_diagnostic_mmio(0x2010_0884),
                read_diagnostic_mmio(0x2010_088c),
                read_diagnostic_mmio(0x2010_0890),
                read_diagnostic_mmio(0x2010_040c),
                read_diagnostic_mmio(0x2010_041c),
                read_diagnostic_mmio(0x2010_0420),
                read_diagnostic_mmio(0x2010_0428),
                read_diagnostic_mmio(0x2010_0418),
            ));
        }
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL probe=txdc-comparator gain={} iteration={} \
             comparator={:?} control={:#010x}",
            gain_index,
            iteration,
            comparator_high,
            read_diagnostic_mmio(0x2010_0418),
        ));
    }

    fn power_detector_sample(
        &mut self,
        measurement_index: u8,
        sample_index: u8,
        sample_value: u16,
    ) {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL probe=pwdet-sample measurement={} sample={} \
             value={} tone={:#010x}/{:#010x}/{:#010x} \
             sar={:#010x}/{:#010x} reference={:#010x}",
            measurement_index,
            sample_index,
            sample_value,
            read_diagnostic_mmio(0x2010_040c),
            read_diagnostic_mmio(0x2010_041c),
            read_diagnostic_mmio(0x2010_0420),
            read_diagnostic_mmio(0x2010_0808),
            read_diagnostic_mmio(0x2010_080c),
            read_diagnostic_mmio(0x2010_0818),
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
        log_open_rf_boundary_mmio(source);
    }
}

pub(super) async fn select_channel(
    state: &mut PhyColdState,
    channel_or_frequency: u16,
    cbw: u8,
    platform: &mut EspHalRadioPeripheral,
    registers: &mut RadioRegisters,
) -> Result<(), PhyTargetPortError> {
    let mut observer = HilPhyObserver;
    select_phy_channel::<EmbassyPhyDelay, _, _>(
        state,
        channel_or_frequency,
        cbw,
        platform,
        registers,
        &mut observer,
    )
    .await
}

pub(super) async fn switch_channel_with_mac_restart(
    state: &mut PhyColdState,
    channel_or_frequency: u16,
    cbw: u8,
    platform: &mut EspHalRadioPeripheral,
    registers: &mut RadioRegisters,
) -> Result<(), PhyTargetPortError> {
    let mut observer = HilPhyObserver;
    switch_phy_channel_with_mac_restart::<EmbassyPhyDelay, _, _>(
        state,
        channel_or_frequency,
        cbw,
        platform,
        registers,
        &mut observer,
    )
    .await
}

fn log_open_txdc_entry_mmio() {
    const ADDRESSES: [usize; 18] = [
        0x2010_001c,
        0x2010_0028,
        0x2010_040c,
        0x2010_0418,
        0x2010_041c,
        0x2010_0420,
        0x2010_0428,
        0x2010_0800,
        0x2010_081c,
        0x2010_0820,
        0x2010_0830,
        0x2010_0848,
        0x2010_084c,
        0x2010_0850,
        0x2010_0870,
        0x2010_0884,
        0x2010_088c,
        0x2010_0890,
    ];
    // SOURCE: keep this list and the page hash geometry identical to
    // `open_radio_vendor_oracle_hil::__wrap_phy_txdc_cal_init`. That linker
    // wrapper records the hardware immediately before the blob call proved by
    // `_oracles/libphy.a[phy_tx_cal.o]`; this function records the matching
    // open state before its first `ConfigurePbusDebugMode` action.
    let values: [u32; ADDRESSES.len()] =
        core::array::from_fn(|index| read_diagnostic_mmio(ADDRESSES[index]));
    emergency_log(format_args!(
        "OPEN_RADIO_TXDC_ENTRY source=open-before-txdc addresses={ADDRESSES:08x?}"
    ));
    emergency_log(format_args!(
        "OPEN_RADIO_TXDC_ENTRY source=open-before-txdc values={values:08x?}"
    ));

    const PAGE_OFFSETS: [u16; 28] = [
        0x0000, 0x0400, 0x0800, 0x0c00, 0x4000, 0x4100, 0x4200, 0x4300, 0x4400, 0x4800, 0x4c00,
        0x4d00, 0x5100, 0x5500, 0x5700, 0x7000, 0x7100, 0x7400, 0x7800, 0x7900, 0x7a00, 0x7c00,
        0x7d00, 0x8000, 0x9c00, 0xd800, 0xf000, 0xf800,
    ];
    for offset in PAGE_OFFSETS {
        let base = 0x2010_0000_usize + usize::from(offset);
        let mut hash = 0x811c_9dc5_u32;
        let mut word = 0_usize;
        while word != 64 {
            hash ^= read_diagnostic_mmio(base + word * 4);
            hash = hash.wrapping_mul(0x0100_0193);
            word += 1;
        }
        emergency_log(format_args!(
            "OPEN_RADIO_MMIO_PAGE source=open-before-txdc \
             offset={offset:#06x} hash={hash:#010x}"
        ));
    }

    // SOURCE: these are precisely the hash-mismatching pages from the
    // 2026-07-29 vendor/open cold-entry comparison above. Keep the output
    // geometry identical to the vendor oracle wrapper so it can be diffed
    // mechanically by address.
    const DIFFERING_PAGE_OFFSETS: [u16; 8] = [
        0x0400, 0x0800, 0x0c00, 0x4400, 0x5500, 0x7000, 0x7c00, 0xd800,
    ];
    for page in DIFFERING_PAGE_OFFSETS {
        let base = 0x2010_0000_usize + usize::from(page);
        for chunk in 0..4_u16 {
            let offset = page + chunk * 0x40;
            let values: [u32; 16] = core::array::from_fn(|word| {
                read_diagnostic_mmio(base + usize::from(chunk) * 0x40 + word * 4)
            });
            emergency_log(format_args!(
                "OPEN_RADIO_TXDC_WORDS source=open-before-txdc \
                 offset={offset:#06x} values={values:08x?}"
            ));
        }
    }
}

fn log_open_rf_boundary_mmio(source: &str) {
    emergency_log(format_args!(
        "OPEN_RADIO_RF_BOUNDARY source={source} \
         pbus={:#010x}/{:#010x}/{:#010x}/{:#010x}/{:#010x}/{:#010x}/{:#010x} \
         dac_scale={:#010x}",
        read_diagnostic_mmio(0x2010_0884),
        read_diagnostic_mmio(0x2010_088c),
        read_diagnostic_mmio(0x2010_0890),
        read_diagnostic_mmio(0x2010_0898),
        read_diagnostic_mmio(0x2010_089c),
        read_diagnostic_mmio(0x2010_08a0),
        read_diagnostic_mmio(0x2010_08a4),
        read_diagnostic_mmio(0x2010_0c04),
    ));
}

/// Capture the first open Authentication TX state without changing it.
///
/// The order is intentionally fixed and shared with the address-by-address
/// vendor/open comparison. Unknown words stay address-labelled in this
/// diagnostic instead of receiving speculative names; any stable difference
/// promoted into the driver must first be tied to blob/ROM control flow and
/// documented in the SVD.
pub(super) fn log_open_auth_register_snapshot() {
    // These raw addresses are deliberately confined to the HIL diagnostic
    // boundary above. A word is promoted to the PAC only after its identity
    // and fields are supported by the blob/ROM evidence recorded in the SVD.
    const ADDRESSES: [usize; 73] = [
        0x2010_4000,
        0x2010_4004,
        0x2010_4038,
        0x2010_403c,
        0x2010_42f4,
        0x2010_4300,
        0x2010_430c,
        0x2010_4310,
        0x2010_4314,
        0x2010_4318,
        0x2010_432c,
        0x2010_4330,
        0x2010_4334,
        0x2010_434c,
        0x2010_4350,
        0x2010_435c,
        0x2010_4360,
        0x2010_4364,
        0x2010_4370,
        0x2010_4388,
        0x2010_438c,
        0x2010_43b4,
        0x2010_43b8,
        0x2010_43bc,
        0x2010_4400,
        0x2010_4404,
        0x2010_443c,
        0x2010_4440,
        0x2010_4444,
        0x2010_4448,
        0x2010_444c,
        0x2010_4450,
        0x2010_4458,
        0x2010_445c,
        0x2010_448c,
        0x2010_4830,
        0x2010_4c04,
        0x2010_4c30,
        0x2010_4c54,
        0x2010_4c58,
        0x2010_4c60,
        0x2010_4c7c,
        0x2010_4c80,
        0x2010_4c8c,
        0x2010_4cac,
        0x2010_4dd4,
        0x2010_4dd8,
        0x2010_4ddc,
        0x2010_4e10,
        0x2010_4e24,
        0x2010_4e2c,
        0x2010_4e30,
        0x2010_4e34,
        0x2010_4e38,
        0x2010_4e44,
        0x2010_4e48,
        0x2010_4e4c,
        0x2010_4e58,
        0x2010_4e5c,
        0x2010_4e60,
        0x2010_54d8,
        0x2010_54dc,
        0x2010_54e0,
        0x2010_54e4,
        0x2010_54e8,
        0x2010_5500,
        0x2010_5504,
        0x2010_550c,
        0x2010_5510,
        0x2010_d814,
        0x2010_d818,
        0x2010_d81c,
        0x2010_d83c,
    ];
    let values: [u32; 73] = core::array::from_fn(|index| read_diagnostic_mmio(ADDRESSES[index]));
    for (chunk, words) in values.chunks(16).enumerate() {
        emergency_log(format_args!(
            "OPEN_AUTH_REGISTER_SNAPSHOT chunk={chunk} values={words:08x?}"
        ));
    }
}

