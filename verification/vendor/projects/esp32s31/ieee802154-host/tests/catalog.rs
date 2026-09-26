//! Stand self-checks: each catalog scenario runs in its own process, and the
//! assertions are independent expectations read from the pinned ESP-IDF
//! source, not snapshots of the stand's own output.

use std::process::Command;

fn trace(scenario: &str) -> Vec<String> {
    let output = Command::new(env!("CARGO_BIN_EXE_oer-esp32s31-ieee802154-vendor-host"))
        .args(["run", scenario])
        .output()
        .expect("the stand binary runs");
    assert!(output.status.success(), "{scenario} failed: {output:?}");
    String::from_utf8(output.stdout)
        .expect("UTF-8 trace")
        .lines()
        .filter(|line| !line.contains("_critical"))
        .map(str::to_owned)
        .collect()
}

/// The records in `expected` appear in `trace` in this order.
fn assert_subsequence(trace: &[String], expected: &[&str]) {
    let mut remaining = trace.iter();
    for record in expected {
        assert!(
            remaining.any(|line| line == record),
            "missing or out of order: {record}\ntrace:\n{}",
            trace.join("\n")
        );
    }
}

#[test]
fn every_scenario_runs_without_a_driver_assertion() {
    let list = Command::new(env!("CARGO_BIN_EXE_oer-esp32s31-ieee802154-vendor-host"))
        .arg("list")
        .output()
        .expect("the stand binary lists its scenarios");
    for scenario in String::from_utf8(list.stdout).unwrap().lines() {
        let trace = trace(scenario);
        assert!(
            !trace
                .iter()
                .any(|line| line.starts_with("event assert_failed")),
            "{scenario} tripped a driver assertion:\n{}",
            trace.join("\n")
        );
    }
}

/// The production engine reproduces every catalog trace.
#[test]
fn every_scenario_matches_the_production_engine() {
    let list = Command::new(env!("CARGO_BIN_EXE_oer-esp32s31-ieee802154-vendor-host"))
        .arg("list")
        .output()
        .expect("the stand binary lists its scenarios");
    for scenario in String::from_utf8(list.stdout).unwrap().lines() {
        let output = Command::new(env!("CARGO_BIN_EXE_oer-esp32s31-ieee802154-vendor-host"))
            .args(["compare", scenario])
            .output()
            .expect("the stand binary compares");
        let verdict = String::from_utf8_lossy(&output.stdout);
        assert_eq!(verdict.trim(), "MATCH", "{scenario}: {verdict}");
    }
}

/// `esp_ieee802154_enable` then `ieee802154_mac_init` (esp_ieee802154.c
/// L35-L41, esp_ieee802154_dev.c L897-L956).
#[test]
fn enable_follows_the_public_order_and_mac_init() {
    assert_eq!(
        trace("enable"),
        [
            "ext modem_clock_module_enable(0x7)",
            "ext esp_phy_enable(0x4)",
            "ext esp_btbb_enable()",
            "ext modem_clock_module_mac_reset(0x7)",
            "ll ieee802154_ll_enable_events(0x3fff)",
            "ll ieee802154_ll_disable_events(0x100)",
            "ll ieee802154_ll_enable_tx_abort_events(0x1868000)",
            "ll ieee802154_ll_enable_rx_abort_events(0x28000)",
            "ll ieee802154_ll_set_ed_sample_mode(0x1)",
            "ll ieee802154_ll_disable_coex()",
            "ext ieee802154_txon_delay_set()",
            "ext esp_intr_alloc(0x84, 0x0)",
            "ext esp_phy_modem_init(0x2)",
            "return esp_ieee802154_enable = 0",
        ]
    );
}

/// `tx_init` stops the current operation, applies the pending PIB, publishes
/// the frame and the ACK receive buffer, then issues `TX_START`; `TX_DONE` of
/// an ACK-requesting frame starts the 200 ms timer-zero watchdog and
/// `ACK_RX_DONE` stops it before reporting the ACK (esp_ieee802154_dev.c
/// L507-L536, L595-L602, L992-L1026).
// The default build's trace; multi-PAN adds identity reads and interface
// indices.
#[cfg(not(feature = "multipan"))]
#[test]
fn transmit_with_ack_arms_and_disarms_the_ack_watchdog() {
    assert_subsequence(
        &trace("transmit-with-ack"),
        &[
            "ll ieee802154_ll_set_cmd(0x45)",
            "ll ieee802154_ll_set_freq(0x3)",
            "ll ieee802154_ll_set_pending_mode(0x0)",
            "ll ieee802154_ll_set_tx_addr(tx#0)",
            "ll ieee802154_ll_set_rx_addr(buf#0)",
            "ll ieee802154_ll_set_cmd(0x41)",
            "ll ieee802154_ll_clear_events(0x1)",
            "ll ieee802154_ll_enable_events(0x100)",
            "ll ieee802154_ll_timer0_set_threshold(0x30d40)",
            "ll ieee802154_ll_set_cmd(0x4c)",
            "ll ieee802154_ll_clear_events(0x8)",
            "ll ieee802154_ll_set_cmd(0x4d)",
            "ll ieee802154_ll_disable_events(0x100)",
            "event transmit_done(0x0, 0x1, 0xb, 0xffffffffffffffc4, 0xc8, 0x0, 0x1, 0x0) \
             [0c6188013412ffff7856aa0000] [05020001c4c8]",
        ],
    );
}

/// A CRC abort is not reported; `next_operation` re-arms receive into the
/// same buffer (esp_ieee802154_dev.c L488-L505, L604-L640).
#[test]
fn receive_crc_error_restarts_into_the_same_buffer() {
    let trace = trace("receive-crc-error-restarts");
    assert!(!trace.iter().any(|line| line.starts_with("event ")));
    assert_subsequence(
        &trace,
        &[
            "ll ieee802154_ll_set_rx_addr(buf#0)",
            "ll ieee802154_ll_set_cmd(0x42)",
            "ll ieee802154_ll_clear_events(0x10)",
            "ll ieee802154_ll_get_rx_status()",
            "ll ieee802154_ll_set_rx_addr(buf#0)",
            "ll ieee802154_ll_set_cmd(0x42)",
        ],
    );
}

/// `RX_DONE` of an ACK-requesting 2006 frame with TX auto-ACK sets the pending
/// bit before the ACK leaves; `ACK_TX_DONE` delivers the frame and
/// `next_operation` re-arms receive into the next buffer
/// (esp_ieee802154_dev.c L538-L593).
// The default build's trace; multi-PAN adds identity reads and interface
// indices.
#[cfg(not(feature = "multipan"))]
#[test]
fn receive_with_auto_ack_selects_pending_before_the_ack() {
    assert_subsequence(
        &trace("receive-with-auto-ack"),
        &[
            "ll ieee802154_ll_set_rx_addr(buf#0)",
            "ll ieee802154_ll_clear_events(0x2)",
            "ll ieee802154_ll_get_tx_auto_ack()",
            "ll ieee802154_ll_set_pending_bit(0x1)",
            "ll ieee802154_ll_clear_events(0x4)",
            "event receive_done(0x1, 0x1, 0xb, 0xffffffffffffffc9, 0xb4, 0x0, 0x1, 0x0) \
             [0c6188013412ffff7856aac9b4] []",
            "ll ieee802154_ll_set_rx_addr(buf#1)",
            "ll ieee802154_ll_set_cmd(0x42)",
            "return esp_ieee802154_receive_handle_done = 0",
        ],
    );
}

/// ED and standalone CCA share `ED_START`; `ED_DONE` reports the RSS code
/// plus the S31 compensation 0, or the CCA busy bit
/// (esp_ieee802154_dev.c L761-L770, L985-L990, L1221-L1252).
#[test]
fn energy_detection_and_cca_report_their_ed_done_sample() {
    assert_subsequence(
        &trace("energy-detect"),
        &[
            "ll ieee802154_ll_enable_events(0x40)",
            "ll ieee802154_ll_set_ed_duration(0x8)",
            "ll ieee802154_ll_set_cmd(0x44)",
            "ll ieee802154_ll_get_ed_rss()",
            "event energy_detect_done(0xffffffffffffffba) [] []",
        ],
    );
    assert_subsequence(
        &trace("cca-busy"),
        &[
            "ll ieee802154_ll_set_cmd(0x44)",
            "ll ieee802154_ll_is_cca_busy()",
            "event cca_done(0x1) [] []",
        ],
    );
}
