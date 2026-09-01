//! First-event timing for restricted legacy advertising.
//!
//! Same-chip `r_ble_lll_adv_init` establishes the 2000-microsecond initial
//! LLL delay. `r_ble_lll_adv_sched_first_pri_event` combines it with the
//! scheduler preparation lead and the LE 1M packet duration, then shifts the
//! complete window forward when the radio-ready observation is later. This
//! module retains that hardware-facing timing geometry without importing the
//! vendor callback, counter or state-machine policy.

#![forbid(unsafe_code)]

use crate::{
    BluetoothControllerSchedulerEpoch, BluetoothSchedulerInstant, BluetoothSchedulerRawWindow,
    BluetoothSchedulerSoftwareConfig,
};

const INITIAL_EVENT_DELAY_MICROS: u32 = 2_000;
const LE_1M_FIXED_PACKET_MICROS: u32 = 80;

/// One restricted legacy advertising event before raw-tick projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BluetoothLegacyAdvertisingEventWindow {
    anchor: BluetoothSchedulerInstant,
    start: BluetoothSchedulerInstant,
    end: BluetoothSchedulerInstant,
}

/// Opaque nominal advertising phase retained for recurring scheduling.
///
/// No integer conversion is public; only the chip scheduler can interpret the
/// phase inside its retained controller epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothLegacyAdvertisingEventPhase(BluetoothSchedulerInstant);

/// Ordered live timing capability for one first advertising event.
///
/// The type is public only so an affine controller owner can pass it across
/// the role boundary. Its fields and construction remain private to the chip
/// controller; external code cannot manufacture detached scheduler images or
/// an epoch.
#[must_use = "the live timing observation must be consumed or retained"]
pub struct BluetoothLegacyAdvertisingTimingObservation {
    pub(crate) current: BluetoothSchedulerInstant,
    pub(crate) radio_ready: BluetoothSchedulerInstant,
    pub(crate) epoch: BluetoothControllerSchedulerEpoch,
}

/// Retained scheduler epoch used to project a successor from its nominal phase.
///
/// A recurring event does not reuse the first event's `current` or radio-ready
/// observations. Its phase is already fixed by the completed event; only the
/// same initialized Controller epoch may project that phase into raw time.
#[must_use = "the recurring timing authority must remain attached to its controller epoch"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothLegacyAdvertisingRecurringTimingObservation {
    epoch: BluetoothControllerSchedulerEpoch,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothLegacyAdvertisingRecurringTimingObservation {
    pub(crate) const fn new(epoch: BluetoothControllerSchedulerEpoch) -> Self {
        Self { epoch }
    }

    pub(crate) const fn recurring_le_1m_window(
        self,
        previous_phase: BluetoothLegacyAdvertisingEventPhase,
        start_offset_micros: u64,
        config: BluetoothSchedulerSoftwareConfig,
        payload_length: u8,
        primary_channel_count: usize,
    ) -> Option<(
        BluetoothLegacyAdvertisingEventWindow,
        BluetoothSchedulerRawWindow,
        u32,
    )> {
        let window = BluetoothLegacyAdvertisingEventWindow::recurring_le_1m(
            previous_phase,
            start_offset_micros,
            config,
            payload_length,
        );
        match window.project_raw(self.epoch, primary_channel_count) {
            Some((raw, raw_item_duration)) => Some((window, raw, raw_item_duration)),
            None => None,
        }
    }
}

impl BluetoothLegacyAdvertisingTimingObservation {
    pub(crate) const fn first_le_1m_window(
        self,
        config: BluetoothSchedulerSoftwareConfig,
        payload_length: u8,
        primary_channel_count: usize,
    ) -> Option<(
        BluetoothLegacyAdvertisingEventWindow,
        BluetoothSchedulerRawWindow,
        u32,
    )> {
        let window = BluetoothLegacyAdvertisingEventWindow::first_le_1m(
            config,
            self.current,
            self.radio_ready,
            payload_length,
        );
        match window.project_raw(self.epoch, primary_channel_count) {
            Some((raw, raw_item_duration)) => Some((window, raw, raw_item_duration)),
            None => None,
        }
    }
}

impl BluetoothLegacyAdvertisingEventWindow {
    /// Form the first LE 1M advertising window from ordered scheduler samples.
    ///
    /// `payload_length` is the Link Layer payload byte count from the encoded
    /// advertising PDU header. It includes AdvA and advertising data, but not
    /// the two-byte header, preamble, Access Address or CRC.
    pub(crate) const fn first_le_1m(
        config: BluetoothSchedulerSoftwareConfig,
        current: BluetoothSchedulerInstant,
        radio_ready: BluetoothSchedulerInstant,
        payload_length: u8,
    ) -> Self {
        let nominal_start = current.wrapping_add(INITIAL_EVENT_DELAY_MICROS);
        let anchor = nominal_start.wrapping_add(config.preparation_lead_micros());
        let nominal_end = anchor.wrapping_add(
            (payload_length as u32)
                .wrapping_mul(8)
                .wrapping_add(LE_1M_FIXED_PACKET_MICROS),
        );

        if nominal_start.is_before(radio_ready) {
            let shift = radio_ready.image().wrapping_sub(nominal_start.image());
            Self {
                anchor,
                start: radio_ready,
                end: nominal_end.wrapping_add(shift),
            }
        } else {
            Self {
                anchor,
                start: nominal_start,
                end: nominal_end,
            }
        }
    }

    /// Form one recurring event from the previous nominal phase.
    pub(crate) const fn recurring_le_1m(
        previous_phase: BluetoothLegacyAdvertisingEventPhase,
        start_offset_micros: u64,
        config: BluetoothSchedulerSoftwareConfig,
        payload_length: u8,
    ) -> Self {
        let anchor = previous_phase.0.wrapping_add(start_offset_micros as u32);
        let start = BluetoothSchedulerInstant::from_image(
            anchor
                .image()
                .wrapping_sub(config.preparation_lead_micros()),
        );
        let end = anchor.wrapping_add(
            (payload_length as u32)
                .wrapping_mul(8)
                .wrapping_add(LE_1M_FIXED_PACKET_MICROS),
        );
        Self { anchor, start, end }
    }

    #[cfg(test)]
    pub(crate) const fn anchor(self) -> BluetoothSchedulerInstant {
        self.anchor
    }

    #[cfg(test)]
    pub(crate) const fn start(self) -> BluetoothSchedulerInstant {
        self.start
    }

    #[cfg(test)]
    pub(crate) const fn end(self) -> BluetoothSchedulerInstant {
        self.end
    }

    /// Project the accepted scheduler positions into controller raw time.
    pub(crate) const fn project_raw(
        self,
        epoch: BluetoothControllerSchedulerEpoch,
        primary_channel_count: usize,
    ) -> Option<(BluetoothSchedulerRawWindow, u32)> {
        if primary_channel_count == 0 || primary_channel_count > 3 {
            return None;
        }
        let raw_start = epoch.raw_ticks_for_micros(self.start.image());
        let raw_first_end = epoch.raw_ticks_for_micros(self.end.image());
        let raw_item_duration = raw_first_end.wrapping_sub(raw_start);
        let raw_event_end =
            raw_start.wrapping_add(raw_item_duration.wrapping_mul(primary_channel_count as u32));
        match BluetoothSchedulerRawWindow::from_projected_scheduler_window(raw_start, raw_event_end)
        {
            Some(window) => Some((window, raw_item_duration)),
            None => None,
        }
    }

    pub(crate) const fn phase(self) -> BluetoothLegacyAdvertisingEventPhase {
        BluetoothLegacyAdvertisingEventPhase(self.anchor)
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_pac::BluetoothControllerHalInitConfig;

    use super::BluetoothLegacyAdvertisingEventWindow;
    use crate::{
        BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample,
        BluetoothSchedulerInstant, BluetoothSchedulerSoftwareConfig,
    };

    const fn instant(image: u32) -> BluetoothSchedulerInstant {
        BluetoothSchedulerInstant::from_image(image)
    }

    #[test]
    fn first_event_retains_lll_delay_preparation_lead_and_le_1m_airtime() {
        let window = BluetoothLegacyAdvertisingEventWindow::first_le_1m(
            BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            instant(10_000),
            instant(11_999),
            9,
        );

        assert_eq!(window.start().image(), 12_000);
        assert_eq!(window.anchor().image(), 12_107);
        assert_eq!(window.end().image(), 12_259);
    }

    #[test]
    fn later_radio_ready_shifts_the_complete_window_without_changing_duration() {
        let window = BluetoothLegacyAdvertisingEventWindow::first_le_1m(
            BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            instant(10_000),
            instant(12_050),
            9,
        );

        assert_eq!(window.anchor().image(), 12_107);
        assert_eq!(window.start().image(), 12_050);
        assert_eq!(window.end().image(), 12_309);
        assert_eq!(
            window.end().image().wrapping_sub(window.start().image()),
            259
        );
    }

    #[test]
    fn first_event_uses_signed_wrapping_order_and_live_epoch_projection() {
        let window = BluetoothLegacyAdvertisingEventWindow::first_le_1m(
            BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            instant(0xffff_ff00),
            instant(1_800),
            6,
        );
        assert_eq!(window.start().image(), 1_800);
        assert_eq!(window.anchor().image(), 1_851);
        assert_eq!(window.end().image(), 2_035);

        let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
        let epoch = BluetoothControllerSchedulerEpoch::new(
            BluetoothControllerTimeSample::for_validation(100),
            1_000,
            scale,
        );
        let (raw, item_duration) = window
            .project_raw(epoch, 3)
            .expect("bounded three-channel event window");
        assert_eq!(item_duration, 58);
        assert_eq!(raw.duration(), 174);
    }

    #[test]
    fn recurring_event_advances_nominal_phase_and_reserves_the_complete_chain() {
        let first = BluetoothLegacyAdvertisingEventWindow::first_le_1m(
            BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            instant(10_000),
            instant(11_999),
            9,
        );
        let recurring = BluetoothLegacyAdvertisingEventWindow::recurring_le_1m(
            first.phase(),
            20_000,
            BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            9,
        );
        assert_eq!(recurring.start().image(), 32_000);
        assert_eq!(recurring.anchor().image(), 32_107);
        assert_eq!(recurring.end().image(), 32_259);

        let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
        let epoch = BluetoothControllerSchedulerEpoch::new(
            BluetoothControllerTimeSample::for_validation(100),
            1_000,
            scale,
        );
        let (raw, item_duration) = recurring
            .project_raw(epoch, 3)
            .expect("the recurring chain fits one raw epoch");
        assert_eq!(raw.duration(), item_duration * 3);
    }
}
