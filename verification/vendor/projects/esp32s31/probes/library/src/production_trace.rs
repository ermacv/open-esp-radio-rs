//! Thin wrappers around exact compiled production entries.
//!
//! Keep ABI conversion and isolated platform construction here. Operation
//! ordering belongs to the production driver function being traced.

use core::future::{Future, ready};

struct ProductionTraceDelay;

impl open_esp_radio_esp32s31_phy::target_executor::PhyAsyncDelay for ProductionTraceDelay {
    fn after_micros(micros: u64) -> impl Future<Output = ()> {
        super::ets_delay_us(micros as u32);
        ready(())
    }
}

/// Exact compiled production channel entry used by vendor comparison.
///
/// The wrapper owns only the isolated probe image's peripheral tokens and ABI
/// conversion. Channel sequencing remains entirely in the production PHY;
/// platform MMIO is provided by the same ESP-HAL adapter used by firmware.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_production_trace_phy_chip_set_chan(
    channel_or_frequency: u32,
    cbw: u32,
) -> u32 {
    // SAFETY: the verifier executes this entry in an isolated image and never
    // creates a second peripheral owner during the same execution.
    let peripherals = unsafe { esp_hal::peripherals::Peripherals::steal() };
    let platform = open_esp_radio_esp32s31_wifi_esp_hal::EspHalRadioPeripheral::new(
        peripherals.WIFI,
        peripherals.MODEM_SYSCON,
        peripherals.MODEM_LPCON,
        peripherals.HP_SYS_CLKRST,
        peripherals.PMU,
        peripherals.LP_AON_CLK_RST,
        peripherals.LP_PERI,
        peripherals.LP_TSENS,
        peripherals.I2C_ANA_MST,
    );
    let radio = open_esp_radio_esp32s31_hal::Radio::claim_for_validation(platform);
    let mut radio = radio.assume_powered_for_validation();
    let mut channel = radio.channel_hal();
    let mut state = open_esp_radio_esp32s31_phy::PhyState::default();
    let mut observer = open_esp_radio_esp32s31_phy::target_port::NoopPhyTargetObserver;
    embassy_futures::block_on(
        open_esp_radio_esp32s31_phy::target_port::select_phy_channel_with_hal::<
            ProductionTraceDelay,
            _,
            _,
        >(
            &mut state,
            channel_or_frequency as u16,
            cbw as u8,
            &mut channel,
            &mut observer,
        ),
    )
    .map_or(1, |()| 0)
}
