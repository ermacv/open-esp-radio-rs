//! Source-owned first-window timing for restricted passive LE scanning.
//!
//! The vendor scanner timer/callout graph is not reproduced. A fresh common
//! scheduler current and a later always-awake BLE-PHY observation establish
//! one bounded preparation interval followed by the exact Link Layer scan
//! window. Only the retained Controller epoch projects that window to raw
//! hardware ticks.

#![forbid(unsafe_code)]

use open_esp_radio_bluetooth_ll::scanning::LegacyScanWindow;
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothPassiveScanMemoryGraphCpuOwned, BluetoothPassiveScanPrimaryChannel,
};
use open_esp_radio_esp32s31_hal::BluetoothControllerLatchedTime;

use crate::{
    BluetoothControllerSchedulerEpoch, BluetoothPassiveScanFirstEventCandidate,
    BluetoothSchedulerInstant, BluetoothSchedulerRawWindow, BluetoothSchedulerSoftwareConfig,
};

/// One passive receive window before projection into raw Controller ticks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BluetoothPassiveScanEventWindow {
    start: BluetoothSchedulerInstant,
    end: BluetoothSchedulerInstant,
}

impl BluetoothPassiveScanEventWindow {
    /// Form a first scanner item with a source-owned preparation prefix.
    ///
    /// `radio_ready` is the later always-awake observation, not a claim about
    /// an undocumented vendor callback. The receive interval begins at the
    /// selected anchor; the scheduler item begins one common preparation lead
    /// earlier and remains active for the complete requested receive window.
    pub(crate) const fn first(
        config: BluetoothSchedulerSoftwareConfig,
        current: BluetoothSchedulerInstant,
        radio_ready: BluetoothSchedulerInstant,
        scan_window: LegacyScanWindow,
    ) -> Self {
        let preparation = config.preparation_lead_micros();
        let nominal_anchor = current
            .wrapping_add(config.late_start_guard_micros())
            .wrapping_add(preparation);
        let anchor = nominal_anchor.later(radio_ready);
        Self {
            start: BluetoothSchedulerInstant::from_image(anchor.image().wrapping_sub(preparation)),
            end: anchor.wrapping_add(scan_window.micros()),
        }
    }

    const fn project_raw(
        self,
        epoch: BluetoothControllerSchedulerEpoch,
    ) -> Option<BluetoothSchedulerRawWindow> {
        BluetoothSchedulerRawWindow::from_projected_scheduler_window(
            epoch.raw_ticks_for_micros(self.start.image()),
            epoch.raw_ticks_for_micros(self.end.image()),
        )
    }
}

/// Ordered timing authority retained while the first scanner event is formed.
#[must_use = "consume the live scanner timing observation or retain it"]
pub(crate) struct BluetoothPassiveScanTimingObservation {
    pub(crate) current: BluetoothSchedulerInstant,
    pub(crate) radio_ready: BluetoothSchedulerInstant,
    pub(crate) epoch: BluetoothControllerSchedulerEpoch,
    pub(crate) controller_time: BluetoothControllerLatchedTime,
}

/// Failure to project a valid Link Layer window into the Controller domain.
#[must_use = "return the unchanged scanner graph to its production owner"]
pub(crate) struct BluetoothPassiveScanTimingFailure {
    graph: BluetoothPassiveScanMemoryGraphCpuOwned,
}

impl BluetoothPassiveScanTimingFailure {
    pub(crate) fn into_graph(self) -> BluetoothPassiveScanMemoryGraphCpuOwned {
        self.graph
    }
}

impl BluetoothPassiveScanTimingObservation {
    pub(crate) fn form_first_event_candidate(
        self,
        graph: BluetoothPassiveScanMemoryGraphCpuOwned,
        channel: BluetoothPassiveScanPrimaryChannel,
        scan_window: LegacyScanWindow,
        config: BluetoothSchedulerSoftwareConfig,
    ) -> Result<BluetoothPassiveScanFirstEventCandidate, BluetoothPassiveScanTimingFailure> {
        let Some(requested_window) = BluetoothPassiveScanEventWindow::first(
            config,
            self.current,
            self.radio_ready,
            scan_window,
        )
        .project_raw(self.epoch) else {
            return Err(BluetoothPassiveScanTimingFailure { graph });
        };
        Ok(BluetoothPassiveScanFirstEventCandidate::new(
            graph,
            channel,
            requested_window,
            self.controller_time,
        ))
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_bluetooth_ll::scanning::LegacyScanWindow;

    use super::BluetoothPassiveScanEventWindow;
    use crate::{BluetoothSchedulerInstant, BluetoothSchedulerSoftwareConfig};

    #[test]
    fn later_readiness_moves_the_complete_window_without_shortening_reception() {
        let config = BluetoothSchedulerSoftwareConfig::reviewed_standalone();
        let scan_window = LegacyScanWindow::new(16).expect("the scan window is valid");
        let current = BluetoothSchedulerInstant::from_image(1_000);
        let nominal = BluetoothPassiveScanEventWindow::first(
            config,
            current,
            BluetoothSchedulerInstant::from_image(1_000),
            scan_window,
        );
        let delayed = BluetoothPassiveScanEventWindow::first(
            config,
            current,
            BluetoothSchedulerInstant::from_image(2_000),
            scan_window,
        );

        assert_eq!(
            delayed.end.image().wrapping_sub(delayed.start.image()),
            nominal.end.image().wrapping_sub(nominal.start.image())
        );
        assert!(nominal.start.is_before(delayed.start));
    }
}
