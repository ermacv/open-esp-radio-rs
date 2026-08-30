//! Bounded scheduler-window arithmetic for DTM events.
//!
//! Current `r_sym_ble_G4zC4UNjJYmyjOsZ3vNq` forms the initial window, while
//! current `r_sym_ble_huwoa5WRTRrAierQfN3B.part.1` advances recurring events.
//! The latter catches up a late transmitter one interval per CPU-loop
//! iteration. The source-open transition below preserves its wrapping signed
//! comparison and phase, but calculates the same skip count in constant time.
//! No operation samples a clock, waits for hardware or publishes an item.

#![forbid(unsafe_code)]

#[cfg(any(target_arch = "riscv32", test))]
use crate::BluetoothDtmTxSchedulerTiming;

#[cfg(any(target_arch = "riscv32", test))]
const INITIAL_ANCHOR_BASE_LEAD_TICKS: u32 = 440 + 500;
#[cfg(any(target_arch = "riscv32", test))]
const RX_RECURRING_ANCHOR_EXTRA_LEAD_TICKS: u32 = 15;
#[cfg(any(target_arch = "riscv32", test))]
const RX_EVENT_WINDOW_TICKS: u32 = 1_000;

/// One positional instant in the BLE software-scheduler domain.
///
/// This type is retained for internal DTM arithmetic; external callers cannot
/// manufacture an instant from a detached integer image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmSchedulerInstant(u32);

impl BluetoothDtmSchedulerInstant {
    /// Preserve one complete positional scheduler-time image.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn from_image(image: u32) -> Self {
        Self(image)
    }

    /// Return the complete positional scheduler-time image.
    pub(crate) const fn image(self) -> u32 {
        self.0
    }
}

/// The one-byte scheduler lead subtracted from a DTM event anchor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct BluetoothDtmSchedulerMargin(u8);

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothDtmSchedulerMargin {
    /// Construct the exact margin initialized by the reviewed standalone LLL.
    pub(crate) const fn reviewed_standalone() -> Self {
        Self(106)
    }

    const fn scheduler_delta(self) -> u32 {
        self.0 as u32
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
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn anchor(self) -> BluetoothDtmSchedulerInstant {
        self.anchor
    }

    /// Return the scheduler window start before raw-time projection.
    pub(crate) const fn start(self) -> BluetoothDtmSchedulerInstant {
        self.start
    }

    /// Return the scheduler window end before raw-time projection.
    pub(crate) const fn end(self) -> BluetoothDtmSchedulerInstant {
        self.end
    }
}

/// Constant-time result of advancing one recurring DTM transmitter event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub struct BluetoothDtmTxEventAdvance {
    window: BluetoothDtmTxEventWindow,
    intervals_advanced: u32,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothDtmTxEventAdvance {
    /// Return the next phase-preserving scheduler window.
    pub(crate) const fn window(self) -> BluetoothDtmTxEventWindow {
        self.window
    }

    /// Return one plus the number of intervals skipped to catch up.
    #[cfg(test)]
    pub(crate) const fn intervals_advanced(self) -> u32 {
        self.intervals_advanced
    }
}

/// Shared private scheduler positions carried by each distinct RX phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BluetoothDtmRxEventWindowData {
    anchor: BluetoothDtmSchedulerInstant,
    start: BluetoothDtmSchedulerInstant,
    end: BluetoothDtmSchedulerInstant,
}

impl BluetoothDtmRxEventWindowData {
    #[cfg(any(target_arch = "riscv32", test))]
    const fn at(anchor: u32, config: crate::BluetoothSchedulerSoftwareConfig) -> Self {
        let margin = config.dtm_scheduler_margin();
        Self {
            anchor: BluetoothDtmSchedulerInstant::from_image(anchor),
            start: BluetoothDtmSchedulerInstant::from_image(
                anchor.wrapping_sub(margin.scheduler_delta()),
            ),
            end: BluetoothDtmSchedulerInstant::from_image(
                anchor.wrapping_add(RX_EVENT_WINDOW_TICKS),
            ),
        }
    }
}

/// First DTM receiver window before scheduler-time-to-raw projection.
///
/// This type cannot represent a recurring receiver event. Its distinct type
/// makes the first-event admission edge phase-correct without a runtime test.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmRxInitialEventWindow(BluetoothDtmRxEventWindowData);

impl BluetoothDtmRxInitialEventWindow {
    /// Form the first RX window from ordered current-time and RF-ready images.
    ///
    /// The first receiver event shares the reviewed 940-tick base lead with
    /// initial TX scheduling. This transform does not establish sample
    /// freshness.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn new(
        config: crate::BluetoothSchedulerSoftwareConfig,
        current: BluetoothDtmSchedulerInstant,
        rf_ready: BluetoothDtmSchedulerInstant,
    ) -> Self {
        let margin = config.dtm_scheduler_margin();
        let nominal_anchor = current
            .image()
            .wrapping_add(INITIAL_ANCHOR_BASE_LEAD_TICKS)
            .wrapping_add(margin.scheduler_delta());
        let anchor = later(nominal_anchor, rf_ready.image());

        Self(BluetoothDtmRxEventWindowData::at(anchor, config))
    }

    /// Return the first receiver phase anchor retained for publication.
    #[cfg(test)]
    pub(crate) const fn anchor(self) -> BluetoothDtmSchedulerInstant {
        self.0.anchor
    }

    /// Return the scheduler window start before raw-time projection.
    pub(crate) const fn start(self) -> BluetoothDtmSchedulerInstant {
        self.0.start
    }

    /// Return the scheduler window end before raw-time projection.
    pub(crate) const fn end(self) -> BluetoothDtmSchedulerInstant {
        self.0.end
    }
}

/// Recurring DTM receiver window before scheduler-time-to-raw projection.
///
/// RX recurrence samples a new clock and RF-ready instant instead of deriving
/// its phase from the preceding event. This type cannot cross the first-event
/// admission edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmRxRecurringEventWindow(BluetoothDtmRxEventWindowData);

impl BluetoothDtmRxRecurringEventWindow {
    /// Form one recurring RX window from ordered current-time and RF-ready images.
    ///
    /// The nominal anchor adds the common late-start guard, scheduler margin
    /// and the reviewed RX recurrence lead to a fresh current-time sample. A
    /// later fresh RF-ready image wins under the complete signed wrapping
    /// comparison. This pure transform does not establish sample freshness.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn new(
        config: crate::BluetoothSchedulerSoftwareConfig,
        current: BluetoothDtmSchedulerInstant,
        rf_ready: BluetoothDtmSchedulerInstant,
    ) -> Self {
        let margin = config.dtm_scheduler_margin();
        let nominal_anchor = current
            .image()
            .wrapping_add(config.late_start_guard_scheduler_delta())
            .wrapping_add(margin.scheduler_delta())
            .wrapping_add(RX_RECURRING_ANCHOR_EXTRA_LEAD_TICKS);
        let anchor = later(nominal_anchor, rf_ready.image());

        Self(BluetoothDtmRxEventWindowData::at(anchor, config))
    }

    /// Return the recurring receiver phase anchor retained for publication.
    #[cfg(test)]
    pub(crate) const fn anchor(self) -> BluetoothDtmSchedulerInstant {
        self.0.anchor
    }

    /// Return the scheduler window start before raw-time projection.
    pub(crate) const fn start(self) -> BluetoothDtmSchedulerInstant {
        self.0.start
    }

    /// Return the scheduler window end before raw-time projection.
    pub(crate) const fn end(self) -> BluetoothDtmSchedulerInstant {
        self.0.end
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothDtmTxSchedulerTiming {
    /// Form the first TX window from ordered current-time and RF-ready images.
    ///
    /// The nominal anchor leads the sampled scheduler time by the source-owned
    /// 440-tick LLL setup image, the DTM body's literal 500 ticks and the
    /// scheduler margin. A later RF-ready image wins under the complete signed
    /// wrapping comparison.
    pub(crate) const fn initial_event_window(
        self,
        config: crate::BluetoothSchedulerSoftwareConfig,
        current: BluetoothDtmSchedulerInstant,
        rf_ready: BluetoothDtmSchedulerInstant,
    ) -> BluetoothDtmTxEventWindow {
        let margin = config.dtm_scheduler_margin();
        let nominal_anchor = current
            .image()
            .wrapping_add(INITIAL_ANCHOR_BASE_LEAD_TICKS)
            .wrapping_add(margin.scheduler_delta());
        let anchor = if is_before(nominal_anchor, rf_ready.image()) {
            rf_ready.image()
        } else {
            nominal_anchor
        };

        self.window_at(config, anchor)
    }

    /// Advance one recurring TX window without the vendor's catch-up loop.
    ///
    /// At least one interval is always consumed. If that first start is late,
    /// unsigned ceiling division computes exactly how many additional phase-
    /// preserving intervals the complete body would add before returning.
    pub(crate) const fn advance_event_window(
        self,
        config: crate::BluetoothSchedulerSoftwareConfig,
        previous: BluetoothDtmTxEventWindow,
        current: BluetoothDtmSchedulerInstant,
    ) -> BluetoothDtmTxEventAdvance {
        let margin = config.dtm_scheduler_margin();
        let interval = self.interval_ticks();
        let first_anchor = previous.anchor().image().wrapping_add(interval);
        let first_start = first_anchor.wrapping_sub(margin.scheduler_delta());
        let delta = first_start.wrapping_sub(current.image()) as i32;
        let extra_intervals = if delta < 0 {
            (delta.wrapping_neg() as u32).div_ceil(interval)
        } else {
            0
        };
        let anchor = first_anchor.wrapping_add(extra_intervals * interval);

        BluetoothDtmTxEventAdvance {
            window: self.window_at(config, anchor),
            intervals_advanced: extra_intervals + 1,
        }
    }

    const fn window_at(
        self,
        config: crate::BluetoothSchedulerSoftwareConfig,
        anchor: u32,
    ) -> BluetoothDtmTxEventWindow {
        let margin = config.dtm_scheduler_margin();
        BluetoothDtmTxEventWindow {
            anchor: BluetoothDtmSchedulerInstant::from_image(anchor),
            start: BluetoothDtmSchedulerInstant::from_image(
                anchor.wrapping_sub(margin.scheduler_delta()),
            ),
            end: BluetoothDtmSchedulerInstant::from_image(
                anchor.wrapping_add(self.packet_window_ticks()),
            ),
        }
    }
}

#[cfg(any(target_arch = "riscv32", test))]
const fn is_before(lhs: u32, rhs: u32) -> bool {
    (lhs.wrapping_sub(rhs) as i32) < 0
}

#[cfg(any(target_arch = "riscv32", test))]
const fn later(lhs: u32, rhs: u32) -> u32 {
    if is_before(lhs, rhs) { rhs } else { lhs }
}

#[cfg(test)]
mod tests {
    use super::{
        BluetoothDtmRxInitialEventWindow, BluetoothDtmRxRecurringEventWindow,
        BluetoothDtmSchedulerInstant, BluetoothDtmTxEventWindow,
    };
    use crate::{
        BluetoothDtmPayloadLength, BluetoothDtmPhy, BluetoothDtmTxTimingMicros,
        BluetoothSchedulerSoftwareConfig,
    };

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

    const fn config() -> BluetoothSchedulerSoftwareConfig {
        BluetoothSchedulerSoftwareConfig::reviewed_standalone()
    }

    #[test]
    fn initial_receiver_window_selects_the_later_fresh_anchor() {
        let nominal =
            BluetoothDtmRxInitialEventWindow::new(config(), instant(1_000), instant(2_045));
        assert_eq!(nominal.anchor().image(), 2_046);
        assert_eq!(nominal.start().image(), 1_940);
        assert_eq!(nominal.end().image(), 3_046);

        let rf_limited =
            BluetoothDtmRxInitialEventWindow::new(config(), instant(1_000), instant(2_047));
        assert_eq!(rf_limited.anchor().image(), 2_047);
        assert_eq!(rf_limited.start().image(), 1_941);
        assert_eq!(rf_limited.end().image(), 3_047);
    }

    #[test]
    fn initial_receiver_window_uses_signed_wrapping_order() {
        let nominal =
            BluetoothDtmRxInitialEventWindow::new(config(), instant(0xffff_ffe0), instant(1_013));
        assert_eq!(nominal.anchor().image(), 1_014);
        assert_eq!(nominal.start().image(), 908);
        assert_eq!(nominal.end().image(), 2_014);

        let rf_limited =
            BluetoothDtmRxInitialEventWindow::new(config(), instant(0xffff_ffe0), instant(1_015));
        assert_eq!(rf_limited.anchor().image(), 1_015);
    }

    #[test]
    fn recurring_receiver_window_selects_the_later_fresh_anchor() {
        let config = BluetoothSchedulerSoftwareConfig::reviewed_standalone();

        let nominal =
            BluetoothDtmRxRecurringEventWindow::new(config, instant(1_000), instant(1_160));
        assert_eq!(nominal.anchor().image(), 1_161);
        assert_eq!(nominal.start().image(), 1_055);
        assert_eq!(nominal.end().image(), 2_161);

        let rf_limited =
            BluetoothDtmRxRecurringEventWindow::new(config, instant(1_000), instant(1_162));
        assert_eq!(rf_limited.anchor().image(), 1_162);
        assert_eq!(rf_limited.start().image(), 1_056);
        assert_eq!(rf_limited.end().image(), 2_162);
    }

    #[test]
    fn recurring_receiver_window_uses_signed_wrapping_order() {
        let config = BluetoothSchedulerSoftwareConfig::reviewed_standalone();

        let nominal =
            BluetoothDtmRxRecurringEventWindow::new(config, instant(0xffff_ffe0), instant(128));
        assert_eq!(nominal.anchor().image(), 129);
        assert_eq!(nominal.start().image(), 23);
        assert_eq!(nominal.end().image(), 1_129);

        let rf_limited =
            BluetoothDtmRxRecurringEventWindow::new(config, instant(0xffff_ffe0), instant(130));
        assert_eq!(rf_limited.anchor().image(), 130);
    }

    #[test]
    fn initial_window_selects_the_later_nominal_or_rf_ready_anchor() {
        let timing = timing(0, BluetoothDtmPhy::Le1M, 0);

        let nominal = timing.initial_event_window(config(), instant(1_000), instant(2_045));
        assert_eq!(nominal.anchor().image(), 2_046);
        assert_eq!(nominal.start().image(), 1_940);
        assert_eq!(nominal.end().image(), 4_182);

        let rf_limited = timing.initial_event_window(config(), instant(1_000), instant(2_100));
        assert_eq!(rf_limited.anchor().image(), 2_100);
        assert_eq!(rf_limited.start().image(), 1_994);
        assert_eq!(rf_limited.end().image(), 4_236);
    }

    #[test]
    fn on_time_recurring_window_advances_exactly_one_interval() {
        let timing = timing(0, BluetoothDtmPhy::Le1M, 0);
        let previous = timing.initial_event_window(config(), instant(1_000), instant(2_100));
        let advance = timing.advance_event_window(config(), previous, instant(2_600));

        assert_eq!(advance.intervals_advanced(), 1);
        assert_eq!(advance.window().anchor().image(), 2_725);
        assert_eq!(advance.window().start().image(), 2_619);
        assert_eq!(advance.window().end().image(), 4_861);
    }

    #[test]
    fn late_recurring_window_preserves_phase_and_skips_in_constant_time() {
        let timing = timing(0, BluetoothDtmPhy::Le1M, 0);
        let previous = timing.initial_event_window(config(), instant(1_000), instant(2_100));
        let advance = timing.advance_event_window(config(), previous, instant(4_000));

        assert_eq!(advance.intervals_advanced(), 4);
        assert_eq!(advance.window().anchor().image(), 4_600);
        assert_eq!(advance.window().start().image(), 4_494);
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
                    let margin = config().dtm_scheduler_margin().scheduler_delta();
                    for offset in current_offsets {
                        let current = instant(previous.anchor().image().wrapping_add(offset));
                        let actual = timing.advance_event_window(config(), previous, current);

                        let mut expected_anchor = previous
                            .anchor()
                            .image()
                            .wrapping_add(timing.interval_ticks());
                        let mut intervals = 1;
                        while (expected_anchor
                            .wrapping_sub(margin)
                            .wrapping_sub(current.image()) as i32)
                            < 0
                        {
                            expected_anchor = expected_anchor.wrapping_add(timing.interval_ticks());
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
