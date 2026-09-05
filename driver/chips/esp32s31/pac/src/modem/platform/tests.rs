use super::{PlatformClockPowerState, PlatformPllSourceBaseline};

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
