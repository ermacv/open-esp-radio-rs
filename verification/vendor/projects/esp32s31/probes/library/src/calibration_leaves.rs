//! Capability and scalar ABI setup for shipping calibration leaves.

/// Profiles select TX-gain restore (enabled domain), forced gain, temperature
/// conversion and post-init AGC. The wrapper only acquires the validation owner
/// and lowers scalar arguments; production owns every computation and MMIO access.
/// Unknown profiles return 0x10003 without touching hardware.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_phy_calibration_leaf(profile: u32, a: u32, b: u32, c: u32) -> u32 {
    if profile == 2 {
        return super::open_phy_calibration_trace_temperature_to_power(a, b, c) as u32;
    }
    if profile > 3 {
        return 0x10003;
    }
    let (mut registers, _interrupts) = oer_esp32s31_pac::RadioHardware::for_validation()
        .into_wifi()
        .into_running();
    match profile {
        0 => super::open_phy_calibration_trace_restore_tx_gain_compensation(
            a,
            registers.radio_phy_mut(),
        ),
        1 => {
            super::open_phy_calibration_trace_force_digital_gain(a, b, c, registers.radio_phy_mut())
        }
        3 => super::open_phy_calibration_trace_post_init_agc(registers.radio_phy_mut()),
        _ => unreachable!(),
    }
    0
}

pub fn retain() {
    core::hint::black_box(open_phy_calibration_leaf as *const ());
}
