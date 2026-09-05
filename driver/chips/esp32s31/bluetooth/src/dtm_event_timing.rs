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

use crate::BluetoothSchedulerInstant;

#[cfg(any(target_arch = "riscv32", test))]
const INITIAL_ANCHOR_BASE_LEAD_MICROS: u32 = 440 + 500;
#[cfg(any(target_arch = "riscv32", test))]
const RX_RECURRING_ANCHOR_EXTRA_LEAD_MICROS: u32 = 15;
#[cfg(any(target_arch = "riscv32", test))]
const RX_EVENT_WINDOW_MICROS: u32 = 1_000;

/// One pure DTM transmitter window before microsecond-to-raw-tick projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmTxEventWindow {
    anchor: BluetoothSchedulerInstant,
    start: BluetoothSchedulerInstant,
    end: BluetoothSchedulerInstant,
}

impl BluetoothDtmTxEventWindow {
    /// Return the phase anchor retained for the next transmitter event.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn anchor(self) -> BluetoothSchedulerInstant {
        self.anchor
    }

    /// Return the microsecond window start before raw-tick projection.
    pub(crate) const fn start(self) -> BluetoothSchedulerInstant {
        self.start
    }

    /// Return the microsecond window end before raw-tick projection.
    pub(crate) const fn end(self) -> BluetoothSchedulerInstant {
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
    anchor: BluetoothSchedulerInstant,
    start: BluetoothSchedulerInstant,
    end: BluetoothSchedulerInstant,
}

impl BluetoothDtmRxEventWindowData {
    #[cfg(any(target_arch = "riscv32", test))]
    const fn at(anchor: u32, config: crate::BluetoothSchedulerSoftwareConfig) -> Self {
        let margin = config.preparation_lead_micros();
        Self {
            anchor: BluetoothSchedulerInstant::from_image(anchor),
            start: BluetoothSchedulerInstant::from_image(anchor.wrapping_sub(margin)),
            end: BluetoothSchedulerInstant::from_image(anchor.wrapping_add(RX_EVENT_WINDOW_MICROS)),
        }
    }
}

/// First DTM receiver window before microsecond-to-raw-tick projection.
///
/// This type cannot represent a recurring receiver event. Its distinct type
/// makes the first-event admission edge phase-correct without a runtime test.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmRxInitialEventWindow(BluetoothDtmRxEventWindowData);

impl BluetoothDtmRxInitialEventWindow {
    /// Form the first RX window from ordered current-time and post-enable images.
    ///
    /// The first receiver event shares the reviewed 940-microsecond base lead with
    /// initial TX scheduling. This transform does not establish sample
    /// freshness.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn new(
        config: crate::BluetoothSchedulerSoftwareConfig,
        current: BluetoothSchedulerInstant,
        timing_ready: BluetoothSchedulerInstant,
    ) -> Self {
        let margin = config.preparation_lead_micros();
        let nominal_anchor = current
            .image()
            .wrapping_add(INITIAL_ANCHOR_BASE_LEAD_MICROS)
            .wrapping_add(margin);
        let anchor = BluetoothSchedulerInstant::from_image(nominal_anchor)
            .later(timing_ready)
            .image();

        Self(BluetoothDtmRxEventWindowData::at(anchor, config))
    }

    /// Return the first receiver phase anchor retained for publication.
    #[cfg(test)]
    pub(crate) const fn anchor(self) -> BluetoothSchedulerInstant {
        self.0.anchor
    }

    /// Return the microsecond window start before raw-tick projection.
    pub(crate) const fn start(self) -> BluetoothSchedulerInstant {
        self.0.start
    }

    /// Return the microsecond window end before raw-tick projection.
    pub(crate) const fn end(self) -> BluetoothSchedulerInstant {
        self.0.end
    }
}

/// Recurring DTM receiver window before microsecond-to-raw-tick projection.
///
/// RX recurrence samples a new clock and post-enable timing instant instead of deriving
/// its phase from the preceding event. This type cannot cross the first-event
/// admission edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmRxRecurringEventWindow(BluetoothDtmRxEventWindowData);

impl BluetoothDtmRxRecurringEventWindow {
    /// Form one recurring RX window from ordered current-time and post-enable images.
    ///
    /// The nominal anchor adds the common late-start guard, scheduler margin
    /// and the reviewed RX recurrence lead to a fresh current-time sample. A
    /// later fresh post-enable timing image wins under the complete signed wrapping
    /// comparison. This pure transform does not establish sample freshness.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn new(
        config: crate::BluetoothSchedulerSoftwareConfig,
        current: BluetoothSchedulerInstant,
        timing_ready: BluetoothSchedulerInstant,
    ) -> Self {
        let margin = config.preparation_lead_micros();
        let nominal_anchor = current
            .image()
            .wrapping_add(config.late_start_guard_micros())
            .wrapping_add(margin)
            .wrapping_add(RX_RECURRING_ANCHOR_EXTRA_LEAD_MICROS);
        let anchor = BluetoothSchedulerInstant::from_image(nominal_anchor)
            .later(timing_ready)
            .image();

        Self(BluetoothDtmRxEventWindowData::at(anchor, config))
    }

    /// Return the recurring receiver phase anchor retained for publication.
    #[cfg(test)]
    pub(crate) const fn anchor(self) -> BluetoothSchedulerInstant {
        self.0.anchor
    }

    /// Return the microsecond window start before raw-tick projection.
    pub(crate) const fn start(self) -> BluetoothSchedulerInstant {
        self.0.start
    }

    /// Return the microsecond window end before raw-tick projection.
    pub(crate) const fn end(self) -> BluetoothSchedulerInstant {
        self.0.end
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothDtmTxSchedulerTiming {
    /// Form the first TX window from ordered current-time and post-enable images.
    ///
    /// The nominal anchor leads the sampled scheduler time by the source-owned
    /// 440-microsecond LLL setup image, the DTM body's literal 500 microseconds
    /// and the scheduler margin. A later post-enable timing image wins under
    /// the complete signed wrapping comparison.
    pub(crate) const fn initial_event_window(
        self,
        config: crate::BluetoothSchedulerSoftwareConfig,
        current: BluetoothSchedulerInstant,
        timing_ready: BluetoothSchedulerInstant,
    ) -> BluetoothDtmTxEventWindow {
        let margin = config.preparation_lead_micros();
        let nominal_anchor = current
            .image()
            .wrapping_add(INITIAL_ANCHOR_BASE_LEAD_MICROS)
            .wrapping_add(margin);
        let anchor = BluetoothSchedulerInstant::from_image(nominal_anchor)
            .later(timing_ready)
            .image();

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
        current: BluetoothSchedulerInstant,
    ) -> BluetoothDtmTxEventAdvance {
        let margin = config.preparation_lead_micros();
        let interval = self.interval_micros();
        let first_anchor = previous.anchor().image().wrapping_add(interval);
        let first_start = first_anchor.wrapping_sub(margin);
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
        let margin = config.preparation_lead_micros();
        BluetoothDtmTxEventWindow {
            anchor: BluetoothSchedulerInstant::from_image(anchor),
            start: BluetoothSchedulerInstant::from_image(anchor.wrapping_sub(margin)),
            end: BluetoothSchedulerInstant::from_image(
                anchor.wrapping_add(self.packet_window_micros()),
            ),
        }
    }
}

#[cfg(test)]
mod tests;
