use crate::{
    SchedulerInstant,
    le::dtm::{DtmPayloadLength, DtmPhy, DtmTxTimingMicros},
    scheduler::SchedulerSoftwareConfig,
};

use super::{DtmRxInitialEventWindow, DtmRxRecurringEventWindow, DtmTxEventWindow};

fn timing(length: u8, phy: DtmPhy, request: u16) -> crate::le::dtm::DtmTxSchedulerTiming {
    DtmTxTimingMicros::new(DtmPayloadLength::from_hci_image(length), phy, request)
        .scheduler_timing()
}

fn instant(image: u32) -> SchedulerInstant {
    SchedulerInstant::from_image(image)
}

const fn config() -> SchedulerSoftwareConfig {
    SchedulerSoftwareConfig::reviewed_standalone()
}

#[test]
fn initial_receiver_window_selects_the_later_fresh_anchor() {
    let nominal = DtmRxInitialEventWindow::new(config(), instant(1_000), instant(2_045));
    assert_eq!(nominal.anchor().image(), 2_047);
    assert_eq!(nominal.start().image(), 1_940);
    assert_eq!(nominal.end().image(), 3_047);

    let rf_limited = DtmRxInitialEventWindow::new(config(), instant(1_000), instant(2_047));
    assert_eq!(rf_limited.anchor().image(), 2_047);
    assert_eq!(rf_limited.start().image(), 1_940);
    assert_eq!(rf_limited.end().image(), 3_047);
}

#[test]
fn initial_receiver_window_uses_signed_wrapping_order() {
    let nominal = DtmRxInitialEventWindow::new(config(), instant(0xffff_ffe0), instant(1_013));
    assert_eq!(nominal.anchor().image(), 1_015);
    assert_eq!(nominal.start().image(), 908);
    assert_eq!(nominal.end().image(), 2_015);

    let rf_limited = DtmRxInitialEventWindow::new(config(), instant(0xffff_ffe0), instant(1_015));
    assert_eq!(rf_limited.anchor().image(), 1_015);
}

#[test]
fn recurring_receiver_window_selects_the_later_fresh_anchor() {
    let config = SchedulerSoftwareConfig::reviewed_standalone();
    let nominal = DtmRxRecurringEventWindow::new(config, instant(1_000), instant(1_205));
    assert_eq!(nominal.anchor().image(), 1_207);
    assert_eq!(nominal.start().image(), 1_100);
    assert_eq!(nominal.end().image(), 2_207);

    let rf_limited = DtmRxRecurringEventWindow::new(config, instant(1_000), instant(1_250));
    assert_eq!(rf_limited.anchor().image(), 1_250);
    assert_eq!(rf_limited.start().image(), 1_143);
    assert_eq!(rf_limited.end().image(), 2_250);
}

#[test]
fn recurring_receiver_window_uses_signed_wrapping_order() {
    let config = SchedulerSoftwareConfig::reviewed_standalone();
    let nominal = DtmRxRecurringEventWindow::new(config, instant(0xffff_ffe0), instant(173));
    assert_eq!(nominal.anchor().image(), 175);
    assert_eq!(nominal.start().image(), 68);
    assert_eq!(nominal.end().image(), 1_175);

    let rf_limited = DtmRxRecurringEventWindow::new(config, instant(0xffff_ffe0), instant(200));
    assert_eq!(rf_limited.anchor().image(), 200);
}

#[test]
fn recurring_receiver_retains_dtm_setup_budget_above_common_admission_guard() {
    let current = instant(10_000);
    let window = DtmRxRecurringEventWindow::new(config(), current, current);
    // Vendor DTM setup 85 us + recurrence lead 15 us. A sequence sample
    // taken 50 us later must still clear the common 40 us admission guard.
    assert_eq!(window.start().image() - current.image(), 100);
    assert!(window.start().image() - 10_050 >= config().late_start_guard_micros());
}

#[test]
fn runtime_receiver_reserves_publication_time_without_shortening_rx_window() {
    let current = instant(10_000);
    let reference = DtmRxRecurringEventWindow::new(config(), current, current);
    let runtime = DtmRxRecurringEventWindow::for_runtime(config(), current, current);
    assert_eq!(runtime.start().image() - reference.start().image(), 500);
    assert_eq!(runtime.end().image() - runtime.anchor().image(), 1_000);
    assert!(runtime.start().image() - 10_300 >= config().late_start_guard_micros());

    let rf_limited = DtmRxRecurringEventWindow::for_runtime(config(), current, instant(12_000));
    assert_eq!(rf_limited.anchor().image(), 12_000);
    let wrapping =
        DtmRxRecurringEventWindow::for_runtime(config(), instant(u32::MAX - 99), instant(0));
    assert_eq!(wrapping.start().image(), 500);
}

#[test]
fn initial_window_selects_the_later_nominal_or_post_enable_anchor() {
    let timing = timing(0, DtmPhy::Le1M, 0);

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
    let timing = timing(0, DtmPhy::Le1M, 0);
    let previous = timing.initial_event_window(config(), instant(1_000), instant(2_100));
    let advance = timing.advance_event_window(config(), previous, instant(2_600));

    assert_eq!(advance.intervals_advanced(), 1);
    assert_eq!(advance.window().anchor().image(), 2_725);
    assert_eq!(advance.window().start().image(), 2_618);
    assert_eq!(advance.window().end().image(), 4_861);
}

#[test]
fn late_recurring_window_preserves_phase_and_skips_in_constant_time() {
    let timing = timing(0, DtmPhy::Le1M, 0);
    let previous = timing.initial_event_window(config(), instant(1_000), instant(2_100));
    let advance = timing.advance_event_window(config(), previous, instant(4_000));

    assert_eq!(advance.intervals_advanced(), 4);
    assert_eq!(advance.window().anchor().image(), 4_600);
    assert_eq!(advance.window().start().image(), 4_493);
}

#[test]
fn constant_time_catch_up_matches_the_complete_vendor_loop() {
    let phys = [
        DtmPhy::Le1M,
        DtmPhy::Le2M,
        DtmPhy::LeCoded,
        DtmPhy::LeCodedS2,
    ];
    let current_offsets = [0, 1, 624, 625, 626, 10_000, 1_000_000];

    for phy in phys {
        for length in [0, 1, 37, 254, 255] {
            for request in [0, 626, 17_501, u16::MAX] {
                let timing = timing(length, phy, request);
                let previous = DtmTxEventWindow {
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
