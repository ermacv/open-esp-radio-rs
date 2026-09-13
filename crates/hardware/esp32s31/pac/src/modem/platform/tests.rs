use super::{PlatformClockPowerState, PlatformPllSourceBaseline, WifiPowerBaseline};
use crate::modem::syscon::ModemSysconPowerBaseline;

const BASELINE: PlatformPllSourceBaseline = PlatformPllSourceBaseline {
    ref_160m_clock_enabled: true,
    modem_apb_clock_enabled: false,
    modem_reset_asserted: true,
    modem_source_clock_enabled: true,
    modem_pll_selected: false,
    modem_pll_clock_enabled: false,
    modem_xtal_clock_enabled: true,
};

#[test]
fn nested_retain_restores_only_after_last_release() {
    let mut state = PlatformClockPowerState::new();
    assert!(state.retain(BASELINE));
    assert!(!state.retain(BASELINE));
    assert_eq!(state.release(), None);
    assert_eq!(state.release(), Some(BASELINE));
}

#[test]
fn wifi_power_retry_preserves_the_original_cold_baseline_until_commit() {
    let original = WifiPowerBaseline::new(false, BASELINE, ModemSysconPowerBaseline::default());
    let retry_observation = WifiPowerBaseline::new(
        true,
        PlatformPllSourceBaseline {
            modem_apb_clock_enabled: true,
            modem_reset_asserted: false,
            ..BASELINE
        },
        ModemSysconPowerBaseline::default(),
    );
    let mut state = PlatformClockPowerState::new();

    state.capture_wifi_power_baseline(original);
    state.capture_wifi_power_baseline(retry_observation);
    assert_eq!(state.wifi_power_baseline, Some(original));

    state.complete_wifi_power_restore();
    assert_eq!(state.wifi_power_baseline, None);

    state.capture_wifi_power_baseline(retry_observation);
    assert_eq!(state.wifi_power_baseline, Some(retry_observation));
}
