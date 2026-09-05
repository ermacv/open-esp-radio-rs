use super::{
    BluetoothDtmRxInitialEventWindow, BluetoothDtmRxRecurringEventWindow, BluetoothDtmTxEventWindow,
};
use crate::{
    BluetoothDtmPayloadLength, BluetoothDtmPhy, BluetoothDtmTxTimingMicros,
    BluetoothSchedulerInstant, BluetoothSchedulerSoftwareConfig,
};

fn timing(length: u8, phy: BluetoothDtmPhy, request: u16) -> crate::BluetoothDtmTxSchedulerTiming {
    BluetoothDtmTxTimingMicros::new(
        BluetoothDtmPayloadLength::from_hci_image(length),
        phy,
        request,
    )
    .scheduler_timing()
}

fn instant(image: u32) -> BluetoothSchedulerInstant {
    BluetoothSchedulerInstant::from_image(image)
}

const fn config() -> BluetoothSchedulerSoftwareConfig {
    BluetoothSchedulerSoftwareConfig::reviewed_standalone()
}

#[test]
fn initial_receiver_window_selects_the_later_fresh_anchor() {
    let nominal = BluetoothDtmRxInitialEventWindow::new(config(), instant(1_000), instant(2_045));
    assert_eq!(nominal.anchor().image(), 2_047);
    assert_eq!(nominal.start().image(), 1_940);
    assert_eq!(nominal.end().image(), 3_047);

    let rf_limited =
        BluetoothDtmRxInitialEventWindow::new(config(), instant(1_000), instant(2_047));
    assert_eq!(rf_limited.anchor().image(), 2_047);
    assert_eq!(rf_limited.start().image(), 1_940);
    assert_eq!(rf_limited.end().image(), 3_047);
}

#[test]
fn initial_receiver_window_uses_signed_wrapping_order() {
    let nominal =
        BluetoothDtmRxInitialEventWindow::new(config(), instant(0xffff_ffe0), instant(1_013));
    assert_eq!(nominal.anchor().image(), 1_015);
    assert_eq!(nominal.start().image(), 908);
    assert_eq!(nominal.end().image(), 2_015);

    let rf_limited =
        BluetoothDtmRxInitialEventWindow::new(config(), instant(0xffff_ffe0), instant(1_015));
    assert_eq!(rf_limited.anchor().image(), 1_015);
}

#[test]
fn recurring_receiver_window_selects_the_later_fresh_anchor() {
    let config = BluetoothSchedulerSoftwareConfig::reviewed_standalone();

    let nominal = BluetoothDtmRxRecurringEventWindow::new(config, instant(1_000), instant(1_160));
    assert_eq!(nominal.anchor().image(), 1_162);
    assert_eq!(nominal.start().image(), 1_055);
    assert_eq!(nominal.end().image(), 2_162);

    let rf_limited =
        BluetoothDtmRxRecurringEventWindow::new(config, instant(1_000), instant(1_162));
    assert_eq!(rf_limited.anchor().image(), 1_162);
    assert_eq!(rf_limited.start().image(), 1_055);
    assert_eq!(rf_limited.end().image(), 2_162);
}

#[test]
fn recurring_receiver_window_uses_signed_wrapping_order() {
    let config = BluetoothSchedulerSoftwareConfig::reviewed_standalone();

    let nominal =
        BluetoothDtmRxRecurringEventWindow::new(config, instant(0xffff_ffe0), instant(128));
    assert_eq!(nominal.anchor().image(), 130);
    assert_eq!(nominal.start().image(), 23);
    assert_eq!(nominal.end().image(), 1_130);

    let rf_limited =
        BluetoothDtmRxRecurringEventWindow::new(config, instant(0xffff_ffe0), instant(130));
    assert_eq!(rf_limited.anchor().image(), 130);
}

#[test]
fn initial_window_selects_the_later_nominal_or_post_enable_anchor() {
    let timing = timing(0, BluetoothDtmPhy::Le1M, 0);

    let nominal = timing.initial_event_window(config(), instant(1_000), instant(2_045));
    assert_eq!(nominal.anchor().image(), 2_047);
    assert_eq!(nominal.start().image(), 1_940);
    assert_eq!(nominal.end().image(), 4_183);

    let rf_limited = timing.initial_event_window(config(), instant(1_000), instant(2_100));
    assert_eq!(rf_limited.anchor().image(), 2_100);
    assert_eq!(rf_limited.start().image(), 1_993);
    assert_eq!(rf_limited.end().image(), 4_236);
}

#[test]
fn on_time_recurring_window_advances_exactly_one_interval() {
    let timing = timing(0, BluetoothDtmPhy::Le1M, 0);
    let previous = timing.initial_event_window(config(), instant(1_000), instant(2_100));
    let advance = timing.advance_event_window(config(), previous, instant(2_600));

    assert_eq!(advance.intervals_advanced(), 1);
    assert_eq!(advance.window().anchor().image(), 2_725);
    assert_eq!(advance.window().start().image(), 2_618);
    assert_eq!(advance.window().end().image(), 4_861);
}

#[test]
fn late_recurring_window_preserves_phase_and_skips_in_constant_time() {
    let timing = timing(0, BluetoothDtmPhy::Le1M, 0);
    let previous = timing.initial_event_window(config(), instant(1_000), instant(2_100));
    let advance = timing.advance_event_window(config(), previous, instant(4_000));

    assert_eq!(advance.intervals_advanced(), 4);
    assert_eq!(advance.window().anchor().image(), 4_600);
    assert_eq!(advance.window().start().image(), 4_493);
}

#[test]
fn constant_time_catch_up_matches_the_complete_vendor_loop() {
    let phys = [
        BluetoothDtmPhy::Le1M,
        BluetoothDtmPhy::Le2M,
        BluetoothDtmPhy::LeCoded,
        BluetoothDtmPhy::LeCodedS2,
    ];
    let current_offsets = [0, 1, 624, 625, 626, 10_000, 1_000_000];

    for phy in phys {
        for length in [0, 1, 37, 254, 255] {
            for request in [0, 626, 17_501, u16::MAX] {
                let timing = timing(length, phy, request);
                let previous = BluetoothDtmTxEventWindow {
                    anchor: instant(0xffff_f000),
                    start: instant(0),
                    end: instant(0),
                };
                let margin = config().preparation_lead_micros();
                for offset in current_offsets {
                    let current = instant(previous.anchor().image().wrapping_add(offset));
                    let actual = timing.advance_event_window(config(), previous, current);

                    let mut expected_anchor = previous
                        .anchor()
                        .image()
                        .wrapping_add(timing.interval_micros());
                    let mut intervals = 1;
                    while (expected_anchor
                        .wrapping_sub(margin)
                        .wrapping_sub(current.image()) as i32)
                        < 0
                    {
                        expected_anchor = expected_anchor.wrapping_add(timing.interval_micros());
                        intervals += 1;
                    }

                    assert_eq!(actual.window().anchor().image(), expected_anchor);
                    assert_eq!(actual.intervals_advanced(), intervals);
                }
            }
        }
    }
}
