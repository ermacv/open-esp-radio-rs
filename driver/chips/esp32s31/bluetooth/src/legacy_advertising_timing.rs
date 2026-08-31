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

/// One restricted legacy advertising event before raw-time projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BluetoothLegacyAdvertisingEventWindow {
    anchor: BluetoothSchedulerInstant,
    start: BluetoothSchedulerInstant,
    end: BluetoothSchedulerInstant,
}

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

impl BluetoothLegacyAdvertisingTimingObservation {
    pub(crate) const fn first_le_1m_window(
        self,
        config: BluetoothSchedulerSoftwareConfig,
        payload_length: u8,
    ) -> Option<(
        BluetoothLegacyAdvertisingEventWindow,
        BluetoothSchedulerRawWindow,
    )> {
        let window = BluetoothLegacyAdvertisingEventWindow::first_le_1m(
            config,
            self.current,
            self.radio_ready,
            payload_length,
        );
        match window.project_raw(self.epoch) {
            Some(raw) => Some((window, raw)),
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
        let anchor = nominal_start.wrapping_add(config.preparation_lead_scheduler_delta());
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
    ) -> Option<BluetoothSchedulerRawWindow> {
        BluetoothSchedulerRawWindow::from_projected_scheduler_window(
            epoch.raw_time_for_scheduler_time(self.start.image()),
            epoch.raw_time_for_scheduler_time(self.end.image()),
        )
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
        let raw = window.project_raw(epoch).expect("bounded event window");
        assert_eq!(raw.duration(), 58);
    }
}
