#[cfg(feature = "task-poll-telemetry")]
use crate::diagnostics::core0_rx_cycles::{Core0RxCyclePhase, Core0RxCycleProfile};
#[cfg(all(
    feature = "core0-rx-coarse-telemetry",
    not(feature = "task-poll-telemetry")
))]
use crate::diagnostics::core0_rx_performance::Core0PerformanceDmaProfile as Core0RxCycleProfile;
#[cfg(any(feature = "diagnostics", test))]
use crate::diagnostics::rx_pipeline::{RxPipelineObservation, RxPipelineObserver};
use open_esp_radio_esp32s31_wifi::rx::transaction::{Discard, Hooks, Phase, ServiceObservation};
#[cfg(feature = "diagnostics")]
use open_esp_radio_esp32s31_wifi_mac::rx::RxRingError;

pub(super) struct Context<'a> {
    #[cfg(any(feature = "diagnostics", test))]
    observer: Option<&'a dyn RxPipelineObserver>,
    #[cfg(any(feature = "diagnostics", test))]
    started: Option<u64>,
    #[cfg(any(feature = "task-poll-telemetry", feature = "core0-rx-coarse-telemetry"))]
    cycles: Core0RxCycleProfile,
    marker: core::marker::PhantomData<&'a ()>,
}

impl<'a> Context<'a> {
    #[inline(always)]
    pub(super) fn new(
        #[cfg(any(feature = "diagnostics", test))] observer: Option<&'a dyn RxPipelineObserver>,
    ) -> Self {
        #[cfg(any(feature = "task-poll-telemetry", feature = "core0-rx-coarse-telemetry"))]
        let cycles = Core0RxCycleProfile::begin();
        #[cfg(any(feature = "diagnostics", test))]
        let started = observer.map(|observer| observer.begin_service());
        Self {
            #[cfg(any(feature = "diagnostics", test))]
            observer,
            #[cfg(any(feature = "diagnostics", test))]
            started,
            #[cfg(any(feature = "task-poll-telemetry", feature = "core0-rx-coarse-telemetry"))]
            cycles,
            marker: core::marker::PhantomData,
        }
    }
}

impl Hooks for Context<'_> {
    const LOG_ERRORS: bool = cfg!(feature = "diagnostics");
    const SAMPLE_ENTRY_REMAINING: bool = cfg!(any(
        feature = "task-poll-telemetry",
        feature = "core0-rx-coarse-telemetry"
    ));

    #[inline(always)]
    fn observing(&self) -> bool {
        #[cfg(any(feature = "diagnostics", test))]
        {
            self.observer.is_some()
        }
        #[cfg(not(any(feature = "diagnostics", test)))]
        {
            false
        }
    }

    #[inline(always)]
    fn entry_remaining(&mut self, _remaining: Option<usize>) {
        #[cfg(any(feature = "task-poll-telemetry", feature = "core0-rx-coarse-telemetry"))]
        crate::diagnostics::core0_rx_performance::CORE0_PERFORMANCE
            .record_dma_entry_remaining(_remaining);
    }

    #[inline(always)]
    fn phase(&mut self, _phase: Phase) {
        #[cfg(feature = "task-poll-telemetry")]
        self.cycles.switch_to(match _phase {
            Phase::Frontier => Core0RxCyclePhase::Frontier,
            Phase::Admission => Core0RxCyclePhase::Admission,
            Phase::StageTake => Core0RxCyclePhase::StageTake,
            Phase::StagePool => Core0RxCyclePhase::StagePool,
            Phase::Recycle => Core0RxCyclePhase::Recycle,
            Phase::Reload => Core0RxCyclePhase::Reload,
            Phase::Publish => Core0RxCyclePhase::Publish,
            Phase::Tail => Core0RxCyclePhase::Tail,
        });
    }

    #[inline(always)]
    fn stage_discarded(&self, _discard: Discard) {
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.observer {
            observer.observe(RxPipelineObservation::StageDiscarded(_discard));
        }
    }

    #[inline(always)]
    fn now_micros(&self) -> Option<u64> {
        #[cfg(any(feature = "diagnostics", test))]
        {
            self.observer.map(RxPipelineObserver::now_micros)
        }
        #[cfg(not(any(feature = "diagnostics", test)))]
        {
            None
        }
    }

    #[inline(always)]
    fn reload_completed(&self, _started: Option<u64>) {
        #[cfg(any(feature = "diagnostics", test))]
        if let (Some(observer), Some(started)) = (self.observer, _started) {
            observer.observe(RxPipelineObservation::ReloadCompleted {
                micros: observer.elapsed_micros_since(started),
            });
        }
    }

    #[inline(always)]
    fn elapsed_service_micros(&self) -> u64 {
        #[cfg(any(feature = "diagnostics", test))]
        if let (Some(observer), Some(started)) = (self.observer, self.started) {
            return observer.elapsed_micros_since(started);
        }
        0
    }

    #[inline(always)]
    fn service_completed(&self, _observation: ServiceObservation) {
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.observer {
            observer.observe(RxPipelineObservation::ServiceCompleted(_observation));
        }
    }

    #[inline(always)]
    fn recycled_probe_pending(&self, recycled_descriptors: usize) -> bool {
        #[cfg(feature = "core0-rx-coarse-telemetry")]
        {
            recycled_descriptors != 0
                && !super::super::interrupt_driven_recycled_append_for_diagnostics()
        }
        #[cfg(not(feature = "core0-rx-coarse-telemetry"))]
        {
            recycled_descriptors != 0
        }
    }

    #[inline(always)]
    fn probe_reasons(
        &self,
        _recycled: bool,
        _frontier: bool,
        _writeback: bool,
        _republication: bool,
    ) {
        #[cfg(feature = "core0-rx-coarse-telemetry")]
        crate::diagnostics::core0_rx_performance::CORE0_PERFORMANCE.record_dma_probe_reasons(
            _recycled,
            _frontier,
            _writeback,
            _republication,
        );
    }

    #[inline(always)]
    fn finish(self, _units: usize) {
        #[cfg(any(feature = "task-poll-telemetry", feature = "core0-rx-coarse-telemetry"))]
        self.cycles.finish(_units);
    }

    #[cfg(feature = "diagnostics")]
    #[inline(always)]
    fn log_busy(
        &self,
        stage: &'static str,
        error: RxRingError,
        head: usize,
        descriptors: usize,
        detached: usize,
        released: usize,
    ) {
        log_rx_service_busy(stage, error, head, descriptors, detached, released);
    }

    #[cfg(feature = "diagnostics")]
    #[inline(always)]
    fn log_recycle_refused(
        &self,
        descriptors: usize,
        recycle_start: usize,
        accepted_tail: usize,
        observed: u32,
        last: u32,
        next: u32,
        software_reload_pending: bool,
        hardware_reload_pending: bool,
    ) {
        log_frozen_rx_recycle_refused(
            descriptors,
            recycle_start,
            accepted_tail,
            observed,
            last,
            next,
            software_reload_pending,
            hardware_reload_pending,
        );
    }
}

#[cfg(feature = "diagnostics")]
#[inline(never)]
fn log_rx_service_busy(
    stage: &'static str,
    error: RxRingError,
    head: usize,
    descriptors: usize,
    detached: usize,
    released: usize,
) {
    log::error!(
        "open-radio: RX service failed stage={stage} error={error:?} head={head} descriptors={descriptors} detached={detached} released={released}"
    );
}

#[cfg(feature = "diagnostics")]
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn log_frozen_rx_recycle_refused(
    descriptor_count: usize,
    recycle_start: usize,
    accepted_tail: usize,
    observed: u32,
    cursor_last: u32,
    cursor_next: u32,
    software_reload_pending: bool,
    hardware_reload_pending: bool,
) {
    log::error!(
        "open-radio: RX service failed stage=frozen-recycle count={descriptor_count} recycle_start={recycle_start} accepted_tail={accepted_tail} observed={observed} cursor_last={cursor_last:#07x} cursor_next={cursor_next:#07x} software_reload_pending={software_reload_pending} hardware_reload_pending={hardware_reload_pending}"
    );
}
