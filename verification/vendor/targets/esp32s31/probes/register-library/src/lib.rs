#![no_std]

//! Small, low-level probes kept independent from AP/STA runtime adapters.
//!
//! Register conformance must remain buildable while higher-level driver work
//! is in progress. Fat LTO still inlines the production HAL/PAC leaves into
//! the retained ABI symbol inspected by the Workbench.

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

fn mac_address_from_words(low: u32, high: u32) -> [u8; 6] {
    let low = low.to_le_bytes();
    let high = high.to_le_bytes();
    [low[0], low[1], low[2], low[3], high[0], high[1]]
}

/// Register-only production projection of vendor `wifi_set_rx_policy` cases
/// six and eight.
///
/// Arguments one and two carry the reviewed global-context address into the
/// Rust probe. Argument three selects the closed case-six register submode;
/// case eight deliberately ignores it.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn open_wifi_sta_ap_trace_wifi_set_rx_policy(
    policy: u32,
    address_low: u32,
    address_high: u32,
    mode: u32,
) -> u32 {
    let address = mac_address_from_words(address_low, address_high);
    let mut registers = open_esp_radio_esp32s31_pac::validation::radio_registers();
    let mut hal = open_esp_radio_esp32s31_hal::wifi_mac::WifiMacHal::new(&mut registers);
    let policy = match policy {
        6 => {
            let mode = if mode == 2 {
                open_esp_radio_esp32s31_pac::MacStaPolicyMode::Mode2
            } else {
                open_esp_radio_esp32s31_pac::MacStaPolicyMode::Mode1
            };
            open_esp_radio_esp32s31_pac::MacRoleReceivePolicy::Station {
                bssid: address,
                mode,
            }
        }
        8 => open_esp_radio_esp32s31_pac::MacRoleReceivePolicy::AccessPoint { address },
        _ => return 0,
    };
    hal.configure_role_receive_policy(policy);
    1
}

pub fn retain_all_probes() {
    core::hint::black_box(open_wifi_sta_ap_trace_wifi_set_rx_policy as *const ());
}
