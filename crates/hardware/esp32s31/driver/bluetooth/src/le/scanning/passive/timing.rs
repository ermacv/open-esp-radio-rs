//! Source-owned timing for restricted passive LE scanning.
//!
//! The vendor scanner timer/callout graph is not reproduced. Fresh common
//! scheduler and always-awake BLE-PHY observations select a bounded window;
//! recurring events preserve the portable scan interval and skip expired
//! phases in constant time.

#![forbid(unsafe_code)]

#[cfg(target_arch = "riscv32")]
use crate::{
    ControllerSchedulerEpoch,
    scheduler::{PassiveScanFirstEventCandidate, SchedulerRawWindow},
};
use oer_bluetooth_ll::scanning::LegacyPassiveScanParameters;
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_bluetooth_memory::{PassiveScanMemoryGraphCpuOwned, PassiveScanPrimaryChannel};
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::bluetooth::BluetoothControllerLatchedTime;

use crate::{SchedulerInstant, scheduler::SchedulerSoftwareConfig};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PassiveScanEventWindow {
    anchor: SchedulerInstant,
    start: SchedulerInstant,
    end: SchedulerInstant,
}

/// Opaque receive-window phase retained across CPU reclamation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "retain the phase until the next scanner window is scheduled or scanning stops"]
pub struct PassiveScanEventPhase(SchedulerInstant);

impl PassiveScanEventWindow {
    pub(crate) const fn first(
        config: SchedulerSoftwareConfig,
        current: SchedulerInstant,
        radio_ready: SchedulerInstant,
        parameters: LegacyPassiveScanParameters,
    ) -> Self {
        let preparation = config.preparation_lead_micros();
        let nominal_anchor = current
            .wrapping_add(config.late_start_guard_micros())
            .wrapping_add(preparation);
        let anchor = nominal_anchor.later(radio_ready);
        Self {
            anchor,
            start: SchedulerInstant::from_image(anchor.image().wrapping_sub(preparation)),
            end: anchor.wrapping_add(parameters.window().micros()),
        }
    }

    /// Select the first non-expired successor while preserving interval phase.
    pub(crate) const fn recurring(
        config: SchedulerSoftwareConfig,
        current: SchedulerInstant,
        radio_ready: SchedulerInstant,
        previous: PassiveScanEventPhase,
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
            start: SchedulerInstant::from_image(anchor.image().wrapping_sub(preparation)),
            end: anchor.wrapping_add(parameters.window().micros()),
        }
    }

    #[cfg(target_arch = "riscv32")]
    const fn project_raw(self, epoch: ControllerSchedulerEpoch) -> Option<SchedulerRawWindow> {
        SchedulerRawWindow::from_projected_scheduler_window(
            epoch.raw_ticks_for_micros(self.start.image()),
            epoch.raw_ticks_for_micros(self.end.image()),
        )
    }

    const fn phase(self) -> PassiveScanEventPhase {
        PassiveScanEventPhase(self.anchor)
    }
}

#[must_use = "consume the live scanner timing observation or retain it"]
#[cfg(target_arch = "riscv32")]
pub(crate) struct PassiveScanTimingObservation {
    pub(crate) current: SchedulerInstant,
    pub(crate) radio_ready: SchedulerInstant,
    pub(crate) epoch: ControllerSchedulerEpoch,
    pub(crate) controller_time: BluetoothControllerLatchedTime,
}

#[must_use = "return the unchanged scanner graph to its production owner"]
#[cfg(target_arch = "riscv32")]
pub(crate) struct PassiveScanTimingFailure {
    graph: PassiveScanMemoryGraphCpuOwned,
}

#[cfg(target_arch = "riscv32")]
impl PassiveScanTimingFailure {
    pub(crate) fn into_graph(self) -> PassiveScanMemoryGraphCpuOwned {
        self.graph
    }
}

#[cfg(target_arch = "riscv32")]
impl PassiveScanTimingObservation {
    pub(crate) fn form_first_event_candidate(
        self,
        graph: PassiveScanMemoryGraphCpuOwned,
        channel: PassiveScanPrimaryChannel,
        parameters: LegacyPassiveScanParameters,
        config: SchedulerSoftwareConfig,
    ) -> Result<(PassiveScanFirstEventCandidate, PassiveScanEventPhase), PassiveScanTimingFailure>
    {
        self.form_event_candidate(graph, channel, parameters, config, None)
    }

    pub(crate) fn form_recurring_event_candidate(
        self,
        graph: PassiveScanMemoryGraphCpuOwned,
        channel: PassiveScanPrimaryChannel,
        parameters: LegacyPassiveScanParameters,
        config: SchedulerSoftwareConfig,
        previous: PassiveScanEventPhase,
    ) -> Result<(PassiveScanFirstEventCandidate, PassiveScanEventPhase), PassiveScanTimingFailure>
    {
        self.form_event_candidate(graph, channel, parameters, config, Some(previous))
    }

    fn form_event_candidate(
        self,
        graph: PassiveScanMemoryGraphCpuOwned,
        channel: PassiveScanPrimaryChannel,
        parameters: LegacyPassiveScanParameters,
        config: SchedulerSoftwareConfig,
        previous: Option<PassiveScanEventPhase>,
    ) -> Result<(PassiveScanFirstEventCandidate, PassiveScanEventPhase), PassiveScanTimingFailure>
    {
        let event = match previous {
            Some(previous) => PassiveScanEventWindow::recurring(
                config,
                self.current,
                self.radio_ready,
                previous,
                parameters,
            ),
            None => {
                PassiveScanEventWindow::first(config, self.current, self.radio_ready, parameters)
            }
        };
        let Some(requested_window) = event.project_raw(self.epoch) else {
            return Err(PassiveScanTimingFailure { graph });
        };
        Ok((
            PassiveScanFirstEventCandidate::new(
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
mod tests;
