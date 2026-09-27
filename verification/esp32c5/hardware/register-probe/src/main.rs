//! Board observation of the ESP32-C5 IEEE 802.15.4 MAC register model.
//!
//! Opens the IEEE 802.15.4 MAC clocks as esp-radio does, then writes all ones
//! to each probed register through the generated raw PAC and reads the word
//! back. The implemented bits of a read-write register are the ones that
//! stay set, so every line checks the published field widths against the
//! silicon. The MAC is idle and no command is issued.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_hal::main;
use esp_println::println;
use oer_esp32c5_pac_raw::Ieee802154Mac;

esp_bootloader_esp_idf::esp_app_desc!();

/// One read-write word: its name and the implemented bits the model declares.
struct Probe {
    name: &'static str,
    expected: u32,
    write_read: fn(&oer_esp32c5_pac_raw::ieee802154_mac::RegisterBlock) -> u32,
}

const PROBES: &[Probe] = &[
    Probe {
        name: "CHANNEL (freq 6:0)",
        expected: 0x0000_007f,
        write_read: |mac| {
            // SAFETY: the idle MAC has no user; the probe restores nothing.
            mac.channel().modify(|_, w| unsafe { w.bits(u32::MAX) });
            mac.channel().read().bits()
        },
    },
    Probe {
        name: "TX_POWER (power 4:0)",
        expected: 0x0000_001f,
        write_read: |mac| {
            // SAFETY: as above.
            mac.tx_power().modify(|_, w| unsafe { w.bits(u32::MAX) });
            mac.tx_power().read().bits()
        },
    },
    Probe {
        name: "EVENT_ENABLE (events 12:0)",
        expected: 0x0000_1fff,
        write_read: |mac| {
            // SAFETY: as above; no event source is running.
            mac.event_enable()
                .modify(|_, w| unsafe { w.bits(u32::MAX) });
            mac.event_enable().read().bits()
        },
    },
    Probe {
        name: "COEX_PTI (pti 3:0, ack 7:4, close_rf_sel 8)",
        expected: 0x0000_01ff,
        write_read: |mac| {
            // SAFETY: as above.
            mac.coex_pti().modify(|_, w| unsafe { w.bits(u32::MAX) });
            mac.coex_pti().read().bits()
        },
    },
];

#[main]
fn main() -> ! {
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    // SAFETY: this image has no radio driver; only the probe touches the
    // modem clock register, as esp-radio's `enable_ieee802154` does.
    let syscon = unsafe { &*esp32c5::MODEM_SYSCON::ptr() };
    syscon
        .clk_conf()
        .modify(|_, w| w.clk_zb_apb_en().set_bit().clk_zbmac_en().set_bit());

    // SAFETY: the probe is the sole user of the IEEE 802.15.4 MAC.
    let mac = unsafe { Ieee802154Mac::steal() };
    let mut failures = 0;
    for probe in PROBES {
        let observed = (probe.write_read)(&mac);
        let verdict = if observed == probe.expected {
            "MATCH"
        } else {
            failures += 1;
            "DIFF"
        };
        println!(
            "PROBE {verdict} {}: observed {observed:#010x} expected {:#010x}",
            probe.name, probe.expected
        );
    }
    println!("PROBE-DONE failures={failures}");
    let delay = esp_hal::delay::Delay::new();
    loop {
        delay.delay_millis(1000);
    }
}
