//! Bounded scheduler-window arithmetic for DTM transmitter events.
//!
//! Current `r_sym_ble_G4zC4UNjJYmyjOsZ3vNq` forms the initial window, while
//! current `r_sym_ble_huwoa5WRTRrAierQfN3B.part.1` advances recurring events.
//! The latter catches up a late transmitter one interval per CPU-loop
//! iteration. The source-open transition below preserves its wrapping signed
//! comparison and phase, but calculates the same skip count in constant time.
//! No operation samples a clock, waits for hardware or publishes an item.

#![forbid(unsafe_code)]

use crate::BluetoothDtmTxSchedulerTiming;

const INITIAL_ANCHOR_BASE_LEAD_TICKS: u32 = 440 + 500;

/// One positional instant in the BLE software-scheduler domain.
///
/// Construction does not prove that the image came from a live clock.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmSchedulerInstant(u32);

impl BluetoothDtmSchedulerInstant {
    /// Preserve one complete positional scheduler-time image.
    pub const fn from_image(image: u32) -> Self {
        Self(image)
    }

    /// Return the complete positional scheduler-time image.
    pub const fn image(self) -> u32 {
        self.0
    }
}

/// The one-byte scheduler lead subtracted from a DTM event anchor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmSchedulerMargin(u8);

impl BluetoothDtmSchedulerMargin {
    /// Preserve the complete scheduler-environment byte image.
    pub const fn from_image(image: u8) -> Self {
        Self(image)
    }

    /// Return the complete scheduler-environment byte image.
    pub const fn image(self) -> u8 {
        self.0
    }
}

/// One pure DTM transmitter window before scheduler-time-to-raw projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmTxEventWindow {
    anchor: BluetoothDtmSchedulerInstant,
    start: BluetoothDtmSchedulerInstant,
    end: BluetoothDtmSchedulerInstant,
}

impl BluetoothDtmTxEventWindow {
    /// Return the phase anchor retained for the next transmitter event.
    pub const fn anchor(self) -> BluetoothDtmSchedulerInstant {
        self.anchor
    }

    /// Return the scheduler window start before raw-time projection.
    pub const fn start(self) -> BluetoothDtmSchedulerInstant {
        self.start
    }

    /// Return the scheduler window end before raw-time projection.
    pub const fn end(self) -> BluetoothDtmSchedulerInstant {
        self.end
    }
}

/// Constant-time result of advancing one recurring DTM transmitter event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmTxEventAdvance {
    window: BluetoothDtmTxEventWindow,
    intervals_advanced: u32,
}

impl BluetoothDtmTxEventAdvance {
    /// Return the next phase-preserving scheduler window.
    pub const fn window(self) -> BluetoothDtmTxEventWindow {
        self.window
    }

    /// Return one plus the number of intervals skipped to catch up.
    pub const fn intervals_advanced(self) -> u32 {
        self.intervals_advanced
    }
}

impl BluetoothDtmTxSchedulerTiming {
    /// Form the first TX window from ordered current-time and RF-ready images.
    ///
    /// The nominal anchor leads the sampled scheduler time by the source-owned
    /// 440-tick LLL setup image, the DTM body's literal 500 ticks and the
    /// scheduler margin. A later RF-ready image wins under the complete signed
    /// wrapping comparison.
    pub const fn initial_event_window(
        self,
        current: BluetoothDtmSchedulerInstant,
        rf_ready: BluetoothDtmSchedulerInstant,
        margin: BluetoothDtmSchedulerMargin,
    ) -> BluetoothDtmTxEventWindow {
        let nominal_anchor = current
            .image()
            .wrapping_add(INITIAL_ANCHOR_BASE_LEAD_TICKS)
            .wrapping_add(margin.image() as u32);
        let anchor = if is_before(nominal_anchor, rf_ready.image()) {
            rf_ready.image()
        } else {
            nominal_anchor
        };

        self.window_at(anchor, margin)
    }

    /// Advance one recurring TX window without the vendor's catch-up loop.
    ///
    /// At least one interval is always consumed. If that first start is late,
    /// unsigned ceiling division computes exactly how many additional phase-
    /// preserving intervals the complete body would add before returning.
    pub const fn advance_event_window(
        self,
        previous: BluetoothDtmTxEventWindow,
        current: BluetoothDtmSchedulerInstant,
        margin: BluetoothDtmSchedulerMargin,
    ) -> BluetoothDtmTxEventAdvance {
        let interval = self.interval_ticks();
        let first_anchor = previous.anchor().image().wrapping_add(interval);
        let first_start = first_anchor.wrapping_sub(margin.image() as u32);
        let delta = first_start.wrapping_sub(current.image()) as i32;
        let extra_intervals = if delta < 0 {
            (delta.wrapping_neg() as u32).div_ceil(interval)
        } else {
            0
        };
        let anchor = first_anchor.wrapping_add(extra_intervals * interval);

        BluetoothDtmTxEventAdvance {
            window: self.window_at(anchor, margin),
            intervals_advanced: extra_intervals + 1,
        }
    }

    const fn window_at(
        self,
        anchor: u32,
        margin: BluetoothDtmSchedulerMargin,
    ) -> BluetoothDtmTxEventWindow {
        BluetoothDtmTxEventWindow {
            anchor: BluetoothDtmSchedulerInstant::from_image(anchor),
            start: BluetoothDtmSchedulerInstant::from_image(
                anchor.wrapping_sub(margin.image() as u32),
            ),
            end: BluetoothDtmSchedulerInstant::from_image(
                anchor.wrapping_add(self.packet_window_ticks()),
            ),
        }
    }
}

const fn is_before(lhs: u32, rhs: u32) -> bool {
    (lhs.wrapping_sub(rhs) as i32) < 0
}

#[cfg(test)]
mod tests {
    use super::{
        BluetoothDtmSchedulerInstant, BluetoothDtmSchedulerMargin, BluetoothDtmTxEventWindow,
    };
    use crate::{BluetoothDtmPayloadLength, BluetoothDtmPhy, BluetoothDtmTxTimingMicros};

    fn timing(
        length: u8,
        phy: BluetoothDtmPhy,
        request: u16,
    ) -> crate::BluetoothDtmTxSchedulerTiming {
        BluetoothDtmTxTimingMicros::new(
            BluetoothDtmPayloadLength::from_hci_image(length),
            phy,
            request,
        )
        .scheduler_timing()
    }

    fn instant(image: u32) -> BluetoothDtmSchedulerInstant {
        BluetoothDtmSchedulerInstant::from_image(image)
    }

    fn margin(image: u8) -> BluetoothDtmSchedulerMargin {
        BluetoothDtmSchedulerMargin::from_image(image)
    }

    #[test]
    fn initial_window_selects_the_later_nominal_or_rf_ready_anchor() {
        let timing = timing(0, BluetoothDtmPhy::Le1M, 0);

        let nominal = timing.initial_event_window(instant(1_000), instant(1_900), margin(20));
        assert_eq!(nominal.anchor().image(), 1_960);
        assert_eq!(nominal.start().image(), 1_940);
        assert_eq!(nominal.end().image(), 4_096);

        let rf_limited = timing.initial_event_window(instant(1_000), instant(2_000), margin(20));
        assert_eq!(rf_limited.anchor().image(), 2_000);
        assert_eq!(rf_limited.start().image(), 1_980);
        assert_eq!(rf_limited.end().image(), 4_136);
    }

    #[test]
    fn on_time_recurring_window_advances_exactly_one_interval() {
        let timing = timing(0, BluetoothDtmPhy::Le1M, 0);
        let previous = timing.initial_event_window(instant(1_000), instant(2_000), margin(20));
        let advance = timing.advance_event_window(previous, instant(2_600), margin(20));

        assert_eq!(advance.intervals_advanced(), 1);
        assert_eq!(advance.window().anchor().image(), 2_625);
        assert_eq!(advance.window().start().image(), 2_605);
        assert_eq!(advance.window().end().image(), 4_761);
    }

    #[test]
    fn late_recurring_window_preserves_phase_and_skips_in_constant_time() {
        let timing = timing(0, BluetoothDtmPhy::Le1M, 0);
        let previous = timing.initial_event_window(instant(1_000), instant(2_000), margin(20));
        let advance = timing.advance_event_window(previous, instant(4_000), margin(20));

        assert_eq!(advance.intervals_advanced(), 4);
        assert_eq!(advance.window().anchor().image(), 4_500);
        assert_eq!(advance.window().start().image(), 4_480);
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
                    for margin in [0, 1, 31, u8::MAX] {
                        let previous = BluetoothDtmTxEventWindow {
                            anchor: instant(0xffff_f000),
                            start: instant(0),
                            end: instant(0),
                        };
                        for offset in current_offsets {
                            let current = instant(previous.anchor().image().wrapping_add(offset));
                            let actual = timing.advance_event_window(
                                previous,
                                current,
                                super::BluetoothDtmSchedulerMargin::from_image(margin),
                            );

                            let mut expected_anchor = previous
                                .anchor()
                                .image()
                                .wrapping_add(timing.interval_ticks());
                            let mut intervals = 1;
                            while (expected_anchor
                                .wrapping_sub(u32::from(margin))
                                .wrapping_sub(current.image())
                                as i32)
                                < 0
                            {
                                expected_anchor =
                                    expected_anchor.wrapping_add(timing.interval_ticks());
                                intervals += 1;
                            }

                            assert_eq!(actual.window().anchor().image(), expected_anchor);
                            assert_eq!(actual.intervals_advanced(), intervals);
                        }
                    }
                }
            }
        }
    }
}
