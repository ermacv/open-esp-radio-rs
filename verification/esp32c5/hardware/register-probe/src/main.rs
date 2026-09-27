//! Board observation of the ESP32-C5 IEEE 802.15.4 MAC register model.
//!
//! Opens the IEEE 802.15.4 MAC clocks as esp-radio does, then writes all ones
//! to each probed register through the generated raw PAC and reads the word
//! back. The implemented bits of a read-write register are the ones that
//! stay set, so every line checks the published field widths against the
//! silicon. The MAC is idle and no command is issued.
//!
//! It then drives every modem clock device and the IEEE 802.15.4 MAC reset
//! through the production PAC owner and checks each transition, and that the
//! modem clock words keep every bit outside the transitioned fields.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_hal::main;
use esp_println::println;
use oer_esp32c5_pac::{ModemClockDevice, ModemClockRegisters, RadioPartitions};
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
        // Bit 7 is implemented although the struct declares it reserved;
        // the first board run (2026-09-27) observed it.
        name: "CHANNEL (freq 6:0, unclassified 7)",
        expected: 0x0000_00ff,
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

const DEVICES: [ModemClockDevice; 5] = [
    ModemClockDevice::Coexistence,
    ModemClockDevice::Etm,
    ModemClockDevice::BluetoothApb,
    ModemClockDevice::BluetoothIeee802154CommonBaseband,
    ModemClockDevice::Ieee802154Mac,
];

/// Bits of MODEM_SYSCON CLK_CONF, MODEM_RST_CONF and CLK_CONF1 and of
/// MODEM_LPCON CLK_CONF that the published fields own.
const OWNED: [u32; 4] = [0x11c0_0000, 0x0180_0000, 0x0003_0000, 0x0000_0002];

/// The four modem clock words, read through esp-pacs so the owner under test
/// is not also the observer.
fn modem_clock_words() -> [u32; 4] {
    // SAFETY: shared read-only views; reads have no side effects.
    let syscon = unsafe { &*esp32c5::MODEM_SYSCON::ptr() };
    // SAFETY: as above.
    let lpcon = unsafe { &*esp32c5::MODEM_LPCON::ptr() };
    [
        syscon.clk_conf().read().bits(),
        syscon.modem_rst_conf().read().bits(),
        syscon.clk_conf1().read().bits(),
        lpcon.clk_conf().read().bits(),
    ]
}

fn check(failures: &mut u32, name: &str, ok: bool) {
    let verdict = if ok {
        "MATCH"
    } else {
        *failures += 1;
        "DIFF"
    };
    println!("PROBE {verdict} {name}");
}

/// Toggle every device off and on, pulse the MAC reset, and leave every
/// device enabled for the MAC probes.
fn probe_modem_clock(clock: &mut ModemClockRegisters, failures: &mut u32) {
    let before = modem_clock_words();
    for device in DEVICES {
        clock.configure_modem_clock_device(device, false);
        let off = !clock.modem_clock_device_enabled(device);
        clock.configure_modem_clock_device(device, true);
        let on = clock.modem_clock_device_enabled(device);
        println!("PROBE {:?}", device);
        check(failures, "  disable then enable", off && on);
    }
    clock.pulse_ieee802154_mac_reset();
    let reset = clock.ieee802154_reset_observation();
    check(
        failures,
        "IEEE 802.15.4 MAC reset pulse releases both lines",
        reset.mac_released && reset.apb_released,
    );
    let after = modem_clock_words();
    for (index, owned) in OWNED.iter().enumerate() {
        println!(
            "PROBE word {index}: before {:#010x} after {:#010x}",
            before[index], after[index]
        );
        check(
            failures,
            "  bits outside the published fields preserved",
            before[index] & !owned == after[index] & !owned,
        );
    }
}

#[main]
fn main() -> ! {
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    let mut failures = 0;
    let RadioPartitions {
        ieee802154: _ieee802154,
        mut modem_clock,
    } = RadioPartitions::take().expect("first radio partition acquisition");
    probe_modem_clock(&mut modem_clock, &mut failures);

    // SAFETY: the probe is the sole user of the IEEE 802.15.4 MAC.
    let mac = unsafe { Ieee802154Mac::steal() };
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
