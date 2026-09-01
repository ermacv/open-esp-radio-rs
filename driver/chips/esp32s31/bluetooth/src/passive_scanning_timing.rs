//! Source-owned timing for restricted passive LE scanning.
//!
//! The vendor scanner timer/callout graph is not reproduced. Fresh common
//! scheduler and always-awake BLE-PHY observations select a bounded window;
//! recurring events preserve the portable scan interval and skip expired
//! phases in constant time.

#![forbid(unsafe_code)]

use open_esp_radio_bluetooth_ll::scanning::LegacyPassiveScanParameters;
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothPassiveScanMemoryGraphCpuOwned, BluetoothPassiveScanPrimaryChannel,
};
use open_esp_radio_esp32s31_hal::BluetoothControllerLatchedTime;

use crate::{
    BluetoothControllerSchedulerEpoch, BluetoothPassiveScanFirstEventCandidate,
    BluetoothSchedulerInstant, BluetoothSchedulerRawWindow, BluetoothSchedulerSoftwareConfig,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BluetoothPassiveScanEventWindow {
    anchor: BluetoothSchedulerInstant,
    start: BluetoothSchedulerInstant,
    end: BluetoothSchedulerInstant,
}

/// Opaque receive-window phase retained across CPU reclamation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "retain the phase until the next scanner window is scheduled or scanning stops"]
pub struct BluetoothPassiveScanEventPhase(BluetoothSchedulerInstant);

impl BluetoothPassiveScanEventWindow {
    pub(crate) const fn first(
        config: BluetoothSchedulerSoftwareConfig,
        current: BluetoothSchedulerInstant,
        radio_ready: BluetoothSchedulerInstant,
        parameters: LegacyPassiveScanParameters,
    ) -> Self {
        let preparation = config.preparation_lead_micros();
        let nominal_anchor = current
            .wrapping_add(config.late_start_guard_micros())
            .wrapping_add(preparation);
        let anchor = nominal_anchor.later(radio_ready);
        Self {
            anchor,
            start: BluetoothSchedulerInstant::from_image(anchor.image().wrapping_sub(preparation)),
            end: anchor.wrapping_add(parameters.window().micros()),
        }
    }

    /// Select the first non-expired successor while preserving interval phase.
    pub(crate) const fn recurring(
        config: BluetoothSchedulerSoftwareConfig,
        current: BluetoothSchedulerInstant,
        radio_ready: BluetoothSchedulerInstant,
        previous: BluetoothPassiveScanEventPhase,
        parameters: LegacyPassiveScanParameters,
    ) -> Self {
        let preparation = config.preparation_lead_micros();
        let earliest_anchor = current
            .wrapping_add(config.late_start_guard_micros())
            .wrapping_add(preparation)
            .later(radio_ready);
        let interval = parameters.interval().micros();
        let first_anchor = previous.0.wrapping_add(interval);
        let lateness = earliest_anchor.image().wrapping_sub(first_anchor.image()) as i32;
        let intervals_to_skip = if lateness > 0 {
            (lateness as u32).div_ceil(interval)
        } else {
            0
        };
        let anchor = first_anchor.wrapping_add(interval.wrapping_mul(intervals_to_skip));
        Self {
            anchor,
            start: BluetoothSchedulerInstant::from_image(anchor.image().wrapping_sub(preparation)),
            end: anchor.wrapping_add(parameters.window().micros()),
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

    const fn phase(self) -> BluetoothPassiveScanEventPhase {
        BluetoothPassiveScanEventPhase(self.anchor)
    }
}

#[must_use = "consume the live scanner timing observation or retain it"]
pub(crate) struct BluetoothPassiveScanTimingObservation {
    pub(crate) current: BluetoothSchedulerInstant,
    pub(crate) radio_ready: BluetoothSchedulerInstant,
    pub(crate) epoch: BluetoothControllerSchedulerEpoch,
    pub(crate) controller_time: BluetoothControllerLatchedTime,
}

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
        parameters: LegacyPassiveScanParameters,
        config: BluetoothSchedulerSoftwareConfig,
    ) -> Result<
        (
            BluetoothPassiveScanFirstEventCandidate,
            BluetoothPassiveScanEventPhase,
        ),
        BluetoothPassiveScanTimingFailure,
    > {
        self.form_event_candidate(graph, channel, parameters, config, None)
    }

    pub(crate) fn form_recurring_event_candidate(
        self,
        graph: BluetoothPassiveScanMemoryGraphCpuOwned,
        channel: BluetoothPassiveScanPrimaryChannel,
        parameters: LegacyPassiveScanParameters,
        config: BluetoothSchedulerSoftwareConfig,
        previous: BluetoothPassiveScanEventPhase,
    ) -> Result<
        (
            BluetoothPassiveScanFirstEventCandidate,
            BluetoothPassiveScanEventPhase,
        ),
        BluetoothPassiveScanTimingFailure,
    > {
        self.form_event_candidate(graph, channel, parameters, config, Some(previous))
    }

    fn form_event_candidate(
        self,
        graph: BluetoothPassiveScanMemoryGraphCpuOwned,
        channel: BluetoothPassiveScanPrimaryChannel,
        parameters: LegacyPassiveScanParameters,
        config: BluetoothSchedulerSoftwareConfig,
        previous: Option<BluetoothPassiveScanEventPhase>,
    ) -> Result<
        (
            BluetoothPassiveScanFirstEventCandidate,
            BluetoothPassiveScanEventPhase,
        ),
        BluetoothPassiveScanTimingFailure,
    > {
        let event = match previous {
            Some(previous) => BluetoothPassiveScanEventWindow::recurring(
                config,
                self.current,
                self.radio_ready,
                previous,
                parameters,
            ),
            None => BluetoothPassiveScanEventWindow::first(
                config,
                self.current,
                self.radio_ready,
                parameters,
            ),
        };
        let Some(requested_window) = event.project_raw(self.epoch) else {
            return Err(BluetoothPassiveScanTimingFailure { graph });
        };
        Ok((
            BluetoothPassiveScanFirstEventCandidate::new(
                graph,
                channel,
                requested_window,
                self.controller_time,
            ),
            event.phase(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_bluetooth_ll::scanning::{
        LegacyPassiveScanParameters, LegacyScanInterval, LegacyScanWindow,
    };

    use super::BluetoothPassiveScanEventWindow;
    use crate::{BluetoothSchedulerInstant, BluetoothSchedulerSoftwareConfig};

    fn parameters(interval: u16, window: u16) -> LegacyPassiveScanParameters {
        LegacyPassiveScanParameters::new(
            LegacyScanInterval::new(interval).expect("the interval is valid"),
            LegacyScanWindow::new(window).expect("the window is valid"),
        )
        .expect("the window fits the interval")
    }

    #[test]
    fn later_readiness_moves_the_complete_window_without_shortening_reception() {
        let config = BluetoothSchedulerSoftwareConfig::reviewed_standalone();
        let parameters = parameters(16, 16);
        let current = BluetoothSchedulerInstant::from_image(1_000);
        let nominal = BluetoothPassiveScanEventWindow::first(
            config,
            current,
            BluetoothSchedulerInstant::from_image(1_000),
            parameters,
        );
        let delayed = BluetoothPassiveScanEventWindow::first(
            config,
            current,
            BluetoothSchedulerInstant::from_image(2_000),
            parameters,
        );

        assert_eq!(
            delayed.end.image().wrapping_sub(delayed.start.image()),
            nominal.end.image().wrapping_sub(nominal.start.image())
        );
        assert!(nominal.start.is_before(delayed.start));
    }

    #[test]
    fn recurring_window_preserves_phase_and_skips_expired_intervals() {
        let config = BluetoothSchedulerSoftwareConfig::reviewed_standalone();
        let parameters = parameters(16, 8);
        let first = BluetoothPassiveScanEventWindow::first(
            config,
            BluetoothSchedulerInstant::from_image(1_000),
            BluetoothSchedulerInstant::from_image(1_000),
            parameters,
        );
        let current = BluetoothSchedulerInstant::from_image(first.anchor.image() + 25_000);
        let next = BluetoothPassiveScanEventWindow::recurring(
            config,
            current,
            BluetoothSchedulerInstant::from_image(1_000),
            first.phase(),
            parameters,
        );

        assert_eq!(
            next.anchor.image().wrapping_sub(first.anchor.image()) % parameters.interval().micros(),
            0
        );
        assert!(!next.start.is_before(current));
    }
}
