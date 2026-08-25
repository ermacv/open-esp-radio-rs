//! Validation-only ED-DONE/TIMER0 `EVENT_STATUS` discriminator.
//!
//! The closed transaction keeps source 132 detached on both cores, enables
//! RX-ABORT for diagnosis plus the ED-DONE/TIMER0 discriminator pair, selects
//! exactly the three public ED abort reasons, programs ED duration eight, and
//! starts TIMER0 before ED. A successful trace observes both paired bits,
//! writes only ED-DONE and requires TIMER0 to remain, then writes only TIMER0
//! and requires zero. This is evidence collection, not a production
//! acknowledgement API or a PHY/RF/BTBB readiness transition.

use core::sync::atomic::{AtomicBool, Ordering};

use open_esp_radio_esp32s31_ieee802154_irq::Ieee802154RouteReadback;

const RX_ABORT_EVENT: u16 = 1 << 4;
const ED_DONE_EVENT: u16 = 1 << 6;
const TIMER0_EVENT: u16 = 1 << 8;
const ED_TIMER_EVENTS: u16 = ED_DONE_EVENT | TIMER0_EVENT;
const VALIDATION_EVENTS: u16 = RX_ABORT_EVENT | ED_TIMER_EVENTS;
const ED_ABORT_REASONS: u32 = (1 << 23) | (1 << 24) | (1 << 25);
const VALIDATION_ED_DURATION: u32 = 8;

static RESET_ISOLATION_CLAIMED: AtomicBool = AtomicBool::new(false);

/// Unique process-lifetime claim for the dedicated reset-isolated image.
///
/// The value is not clonable and is never released. The transaction still
/// samples both complete source-132 route words before event enable, while the
/// exact mask is active, and after cleanup.
#[must_use = "the reset-isolation claim must be consumed by the terminal validation transaction"]
pub struct Ieee802154EdEventProbeIsolation {
    _private: (),
}

impl Ieee802154EdEventProbeIsolation {
    /// Claim this diagnostic image's single reset-isolation capability.
    #[doc(hidden)]
    pub fn claim_for_reset_isolated_image() -> Option<Self> {
        RESET_ISOLATION_CLAIMED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self { _private: () })
    }
}

/// Finite bounds for one ED-DONE/TIMER0 discriminator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154EdEventProbeConfig {
    poll_limit: u32,
    timer_threshold: u32,
}

impl Ieee802154EdEventProbeConfig {
    /// Maximum samples made by the single paired-event wait.
    pub const MAX_POLL_LIMIT: u32 = 1_000_000;
    /// Maximum accepted TIMER0 threshold.
    pub const MAX_TIMER_THRESHOLD: u32 = 1_000;

    /// Construct one nondegenerate bounded probe configuration.
    pub const fn new(poll_limit: u32, timer_threshold: u32) -> Option<Self> {
        if poll_limit == 0
            || poll_limit > Self::MAX_POLL_LIMIT
            || timer_threshold == 0
            || timer_threshold > Self::MAX_TIMER_THRESHOLD
        {
            None
        } else {
            Some(Self {
                poll_limit,
                timer_threshold,
            })
        }
    }

    /// Maximum status samples in the paired-event wait.
    pub const fn poll_limit(self) -> u32 {
        self.poll_limit
    }

    /// Complete TIMER0 threshold written before the stimulus.
    pub const fn timer_threshold(self) -> u32 {
        self.timer_threshold
    }
}

/// Terminal classification for the closed discriminator.
///
/// Even [`Complete`](Self::Complete) is only an observed ED-DONE/TIMER0
/// relation. It does not publish a register-wide or production write class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154EdEventProbeStop {
    /// The paired bits and both exact selected-write relations were observed.
    Complete,
    /// Event or RX-abort delivery was already enabled at entry.
    UnsupportedSetup,
    /// Either complete source-132 route word was not reset-detached.
    RouteNotQuiesced,
    /// The entry status was not clear.
    ResetNotClear,
    /// Duration eight did not read back exactly.
    EdDurationReadbackMismatch,
    /// The exact `0x0150` event-enable image did not read back.
    EventEnableReadbackMismatch,
    /// The exact ED_ABORT/ED_STOP/ED_COEX_REJECT mask did not read back.
    RxAbortEnableReadbackMismatch,
    /// Enabling the exact mask exposed status before either stimulus started.
    PostEnableStatusNotClear,
    /// TIMER0 did not show bounded activity.
    TimerActivityTimeout,
    /// ED-DONE and TIMER0 did not become simultaneously latched in time.
    PairLatchTimeout,
    /// ED terminated through RX-ABORT; the complete raw RX status is retained.
    EdAborted,
    /// A bit outside ED-DONE/TIMER0 appeared and was not RX-ABORT alone.
    UnexpectedEvent,
    /// Either selected raw write did not produce the exact expected full image.
    SelectiveWriteMismatch,
    /// A nominally complete trace did not leave both delivery masks, route and
    /// status clear. ED duration deliberately remains at the terminal value.
    /// Failure traces retain their more specific primary stop.
    CleanupNotClear,
}

/// Complete raw evidence retained by the discriminator.
///
/// Every event field is the full fourteen-bit status image. `observed_events`
/// is the union of all sampled images, while `terminal_events` is the exact
/// sample that completed or ended the bounded wait. If RX-ABORT appears, the
/// complete raw RX status is captured and the transaction fails closed; no
/// interpretation or acknowledgement of the abort bit is attempted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154EdEventProbeEvidence {
    pub stop: Ieee802154EdEventProbeStop,
    pub event_enable_before: u16,
    pub event_enable_active: u16,
    pub event_enable_after: u16,
    pub rx_abort_enable_before: u32,
    pub rx_abort_enable_active: u32,
    pub rx_abort_enable_after: u32,
    pub route_core0_before_enable: u32,
    pub route_core1_before_enable: u32,
    pub route_core0_with_events_enabled: u32,
    pub route_core1_with_events_enabled: u32,
    pub route_core0_after_cleanup: u32,
    pub route_core1_after_cleanup: u32,
    pub ed_duration_before: u32,
    pub ed_duration_active: u32,
    pub ed_duration_after: u32,
    pub timer0_value_before_start: u32,
    pub timer0_value_min: u32,
    pub timer0_value_max: u32,
    pub timer0_value_after_stop: u32,
    pub reset_events: u16,
    pub post_enable_events: u16,
    pub observed_events: u16,
    pub terminal_events: u16,
    pub after_ed_done_write_events: u16,
    pub after_timer0_write_events: u16,
    pub cleanup_pending_events: u16,
    pub final_events: u16,
    pub rx_status_at_abort: Option<u32>,
    pub stop_command_issued: bool,
    pub cleanup_clear: bool,
}

impl Ieee802154EdEventProbeEvidence {
    const fn empty() -> Self {
        Self {
            stop: Ieee802154EdEventProbeStop::UnsupportedSetup,
            event_enable_before: 0,
            event_enable_active: 0,
            event_enable_after: 0,
            rx_abort_enable_before: 0,
            rx_abort_enable_active: 0,
            rx_abort_enable_after: 0,
            route_core0_before_enable: 0,
            route_core1_before_enable: 0,
            route_core0_with_events_enabled: 0,
            route_core1_with_events_enabled: 0,
            route_core0_after_cleanup: 0,
            route_core1_after_cleanup: 0,
            ed_duration_before: 0,
            ed_duration_active: 0,
            ed_duration_after: 0,
            timer0_value_before_start: 0,
            timer0_value_min: 0,
            timer0_value_max: 0,
            timer0_value_after_stop: 0,
            reset_events: 0,
            post_enable_events: 0,
            observed_events: 0,
            terminal_events: 0,
            after_ed_done_write_events: 0,
            after_timer0_write_events: 0,
            cleanup_pending_events: 0,
            final_events: 0,
            rx_status_at_abort: None,
            stop_command_issued: false,
            cleanup_clear: false,
        }
    }
}

/// Closed register vocabulary consumed by the pure validation state machine.
///
/// The target adapter must be the unique PAC lease in a dedicated validation
/// build. Host tests implement the trait without exposing raw addresses or
/// broadening the two selected status writes.
pub(crate) trait Ieee802154EdEventProbeBackend {
    fn event_enable_events(&mut self) -> u16;
    fn enable_ed_timer_abort_events(&mut self);
    fn disable_all_events(&mut self);
    fn rx_abort_enable_events(&mut self) -> u32;
    fn enable_ed_abort_reasons(&mut self);
    fn disable_all_rx_abort_reasons(&mut self);
    fn interrupt_route_readback(&mut self) -> Ieee802154RouteReadback;
    fn event_status_events(&mut self) -> u16;
    fn rx_status_raw(&mut self) -> u32;
    fn ed_duration(&mut self) -> u32;
    fn set_ed_duration_eight(&mut self);
    fn timer0_value(&mut self) -> u32;
    fn set_timer0_threshold(&mut self, threshold: u32);
    fn start_timer0(&mut self);
    fn stop_timer0(&mut self);
    fn start_ed(&mut self);
    fn stop_operation(&mut self);
    fn write_ed_done_event(&mut self);
    fn write_timer0_event(&mut self);
}

#[cfg(feature = "validation-probes")]
impl Ieee802154EdEventProbeBackend for crate::ieee802154::Ieee802154PacHal<'_> {
    fn event_enable_events(&mut self) -> u16 {
        self.validation_ed_event_enable_events()
    }

    fn enable_ed_timer_abort_events(&mut self) {
        self.validation_enable_ed_timer_abort_events();
    }

    fn disable_all_events(&mut self) {
        self.validation_disable_ed_events();
    }

    fn rx_abort_enable_events(&mut self) -> u32 {
        self.validation_ed_rx_abort_enable_events()
    }

    fn enable_ed_abort_reasons(&mut self) {
        self.validation_enable_ed_abort_reasons();
    }

    fn disable_all_rx_abort_reasons(&mut self) {
        self.validation_disable_ed_abort_reasons();
    }

    fn interrupt_route_readback(&mut self) -> Ieee802154RouteReadback {
        self.interrupt_route_readback()
    }

    fn event_status_events(&mut self) -> u16 {
        self.validation_ed_event_status_events()
    }

    fn rx_status_raw(&mut self) -> u32 {
        self.validation_ed_rx_status_raw()
    }

    fn ed_duration(&mut self) -> u32 {
        self.validation_ed_duration()
    }

    fn set_ed_duration_eight(&mut self) {
        self.validation_set_ed_duration_eight();
    }

    fn timer0_value(&mut self) -> u32 {
        self.validation_ed_timer0_value()
    }

    fn set_timer0_threshold(&mut self, threshold: u32) {
        self.validation_set_ed_timer0_threshold(threshold);
    }

    fn start_timer0(&mut self) {
        self.validation_start_ed_timer0();
    }

    fn stop_timer0(&mut self) {
        self.validation_stop_ed_timer0();
    }

    fn start_ed(&mut self) {
        self.validation_start_ed();
    }

    fn stop_operation(&mut self) {
        self.validation_stop_ed_operation();
    }

    fn write_ed_done_event(&mut self) {
        self.validation_write_ed_done_event();
    }

    fn write_timer0_event(&mut self) {
        self.validation_write_ed_timer0_event();
    }
}

fn sample_events<B>(backend: &mut B, evidence: &mut Ieee802154EdEventProbeEvidence) -> u16
where
    B: Ieee802154EdEventProbeBackend,
{
    let events = backend.event_status_events();
    evidence.observed_events |= events;
    if events & RX_ABORT_EVENT != 0 && evidence.rx_status_at_abort.is_none() {
        evidence.rx_status_at_abort = Some(backend.rx_status_raw());
    }
    events
}

fn finish_without_writes<B>(
    backend: &mut B,
    mut evidence: Ieee802154EdEventProbeEvidence,
    stop: Ieee802154EdEventProbeStop,
) -> Ieee802154EdEventProbeEvidence
where
    B: Ieee802154EdEventProbeBackend,
{
    evidence.final_events = sample_events(backend, &mut evidence);
    evidence.event_enable_after = backend.event_enable_events();
    evidence.rx_abort_enable_after = backend.rx_abort_enable_events();
    evidence.ed_duration_after = backend.ed_duration();
    let route = backend.interrupt_route_readback();
    evidence.route_core0_after_cleanup = route.core0().raw_bits();
    evidence.route_core1_after_cleanup = route.core1().raw_bits();
    evidence.stop = stop;
    evidence
}

fn finish<B>(
    backend: &mut B,
    mut evidence: Ieee802154EdEventProbeEvidence,
    stop: Ieee802154EdEventProbeStop,
    stop_ed_for_cleanup: bool,
) -> Ieee802154EdEventProbeEvidence
where
    B: Ieee802154EdEventProbeBackend,
{
    // Quiesce both delivery gates before issuing any best-effort stop. A route
    // readback is still required below, but no newly completed event should be
    // deliberately exposed during cleanup if external state drifted.
    backend.disable_all_events();
    evidence.event_enable_after = backend.event_enable_events();
    backend.disable_all_rx_abort_reasons();
    evidence.rx_abort_enable_after = backend.rx_abort_enable_events();

    backend.stop_timer0();
    evidence.timer0_value_after_stop = backend.timer0_value();
    if stop_ed_for_cleanup {
        backend.stop_operation();
        evidence.stop_command_issued = true;
    }

    evidence.ed_duration_after = backend.ed_duration();

    evidence.cleanup_pending_events = sample_events(backend, &mut evidence);
    let route = backend.interrupt_route_readback();
    evidence.route_core0_after_cleanup = route.core0().raw_bits();
    evidence.route_core1_after_cleanup = route.core1().raw_bits();

    evidence.final_events = sample_events(backend, &mut evidence);
    evidence.cleanup_clear = evidence.event_enable_after == 0
        && evidence.rx_abort_enable_after == 0
        && evidence.final_events == 0
        && route.is_full_reset();
    evidence.stop = if stop == Ieee802154EdEventProbeStop::Complete && !evidence.cleanup_clear {
        Ieee802154EdEventProbeStop::CleanupNotClear
    } else {
        stop
    };
    evidence
}

/// Execute the pure sequence shared by the future PAC adapter and host fake.
pub(crate) fn run_ieee802154_ed_event_probe<B>(
    backend: &mut B,
    config: Ieee802154EdEventProbeConfig,
) -> Ieee802154EdEventProbeEvidence
where
    B: Ieee802154EdEventProbeBackend,
{
    let mut evidence = Ieee802154EdEventProbeEvidence::empty();

    evidence.event_enable_before = backend.event_enable_events();
    evidence.rx_abort_enable_before = backend.rx_abort_enable_events();
    evidence.ed_duration_before = backend.ed_duration();
    if evidence.event_enable_before != 0 || evidence.rx_abort_enable_before != 0 {
        return finish_without_writes(
            backend,
            evidence,
            Ieee802154EdEventProbeStop::UnsupportedSetup,
        );
    }

    let route = backend.interrupt_route_readback();
    evidence.route_core0_before_enable = route.core0().raw_bits();
    evidence.route_core1_before_enable = route.core1().raw_bits();
    if !route.is_full_reset() {
        return finish_without_writes(
            backend,
            evidence,
            Ieee802154EdEventProbeStop::RouteNotQuiesced,
        );
    }

    evidence.reset_events = sample_events(backend, &mut evidence);
    if evidence.reset_events != 0 {
        return finish_without_writes(backend, evidence, Ieee802154EdEventProbeStop::ResetNotClear);
    }

    backend.set_ed_duration_eight();
    evidence.ed_duration_active = backend.ed_duration();
    if evidence.ed_duration_active != VALIDATION_ED_DURATION {
        return finish(
            backend,
            evidence,
            Ieee802154EdEventProbeStop::EdDurationReadbackMismatch,
            false,
        );
    }

    backend.enable_ed_abort_reasons();
    evidence.rx_abort_enable_active = backend.rx_abort_enable_events();
    if evidence.rx_abort_enable_active != ED_ABORT_REASONS {
        return finish(
            backend,
            evidence,
            Ieee802154EdEventProbeStop::RxAbortEnableReadbackMismatch,
            false,
        );
    }

    backend.enable_ed_timer_abort_events();
    evidence.event_enable_active = backend.event_enable_events();
    if evidence.event_enable_active != VALIDATION_EVENTS {
        return finish(
            backend,
            evidence,
            Ieee802154EdEventProbeStop::EventEnableReadbackMismatch,
            false,
        );
    }

    evidence.post_enable_events = sample_events(backend, &mut evidence);
    if evidence.post_enable_events != 0 {
        return finish(
            backend,
            evidence,
            Ieee802154EdEventProbeStop::PostEnableStatusNotClear,
            false,
        );
    }

    let route = backend.interrupt_route_readback();
    evidence.route_core0_with_events_enabled = route.core0().raw_bits();
    evidence.route_core1_with_events_enabled = route.core1().raw_bits();
    if !route.is_full_reset() {
        return finish(
            backend,
            evidence,
            Ieee802154EdEventProbeStop::RouteNotQuiesced,
            false,
        );
    }

    evidence.timer0_value_before_start = backend.timer0_value();
    evidence.timer0_value_min = evidence.timer0_value_before_start;
    evidence.timer0_value_max = evidence.timer0_value_before_start;
    backend.set_timer0_threshold(config.timer_threshold());
    backend.start_timer0();
    backend.start_ed();

    let mut timer_changed = false;
    let mut paired = false;
    for _ in 0..config.poll_limit() {
        let timer = backend.timer0_value();
        evidence.timer0_value_min = evidence.timer0_value_min.min(timer);
        evidence.timer0_value_max = evidence.timer0_value_max.max(timer);
        timer_changed |= timer != evidence.timer0_value_before_start;

        evidence.terminal_events = sample_events(backend, &mut evidence);
        if evidence.terminal_events & !ED_TIMER_EVENTS != 0 {
            let stop_ed = evidence.terminal_events & (ED_DONE_EVENT | RX_ABORT_EVENT) == 0;
            let stop = if evidence.terminal_events == RX_ABORT_EVENT {
                Ieee802154EdEventProbeStop::EdAborted
            } else {
                Ieee802154EdEventProbeStop::UnexpectedEvent
            };
            return finish(backend, evidence, stop, stop_ed);
        }
        if evidence.terminal_events & ED_TIMER_EVENTS == ED_TIMER_EVENTS {
            paired = true;
            break;
        }
    }

    if !paired {
        let stop = if timer_changed {
            Ieee802154EdEventProbeStop::PairLatchTimeout
        } else {
            Ieee802154EdEventProbeStop::TimerActivityTimeout
        };
        let stop_ed = evidence.terminal_events & ED_DONE_EVENT == 0;
        return finish(backend, evidence, stop, stop_ed);
    }

    backend.stop_timer0();
    evidence.timer0_value_after_stop = backend.timer0_value();

    backend.write_ed_done_event();
    evidence.after_ed_done_write_events = sample_events(backend, &mut evidence);
    if evidence.after_ed_done_write_events != TIMER0_EVENT {
        return finish(
            backend,
            evidence,
            Ieee802154EdEventProbeStop::SelectiveWriteMismatch,
            false,
        );
    }

    backend.write_timer0_event();
    evidence.after_timer0_write_events = sample_events(backend, &mut evidence);
    if evidence.after_timer0_write_events != 0 {
        return finish(
            backend,
            evidence,
            Ieee802154EdEventProbeStop::SelectiveWriteMismatch,
            false,
        );
    }

    finish(
        backend,
        evidence,
        Ieee802154EdEventProbeStop::Complete,
        false,
    )
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, vec::Vec};

    use open_esp_radio_esp32s31_ieee802154_irq::{Ieee802154RouteReadback, Ieee802154RouteWord};

    use super::{
        ED_ABORT_REASONS, ED_DONE_EVENT, ED_TIMER_EVENTS, Ieee802154EdEventProbeBackend,
        Ieee802154EdEventProbeConfig, Ieee802154EdEventProbeStop, RX_ABORT_EVENT, TIMER0_EVENT,
        VALIDATION_ED_DURATION, VALIDATION_EVENTS, run_ieee802154_ed_event_probe,
    };

    const fn reset_route() -> Ieee802154RouteReadback {
        Ieee802154RouteReadback::new(
            Ieee802154RouteWord::from_raw(0),
            Ieee802154RouteWord::from_raw(0),
        )
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        EventEnable,
        EnableEvents,
        DisableEvents,
        RxAbortEnable,
        EnableEdAbortReasons,
        DisableRxAbortReasons,
        Route,
        EventStatus,
        RxStatus,
        EdDuration,
        SetEdDurationEight,
        TimerValue,
        TimerThreshold(u32),
        StartTimer,
        StopTimer,
        StartEd,
        StopEd,
        WriteEdDone,
        WriteTimer,
    }

    struct FakeBackend {
        event_enable: u16,
        rx_abort_enable: u32,
        ed_duration: u32,
        status_reads: VecDeque<u16>,
        route: Ieee802154RouteReadback,
        timer_running: bool,
        timer_value: u32,
        rx_status: u32,
        operations: Vec<Operation>,
    }

    impl FakeBackend {
        fn new(status_reads: &[u16]) -> Self {
            Self {
                event_enable: 0,
                rx_abort_enable: 0,
                ed_duration: 5,
                status_reads: status_reads.iter().copied().collect(),
                route: reset_route(),
                timer_running: false,
                timer_value: 0,
                rx_status: 0,
                operations: Vec::new(),
            }
        }

        fn assert_consumed(&self) {
            assert!(
                self.status_reads.is_empty(),
                "unused fake status reads: {:?}",
                self.status_reads
            );
        }
    }

    impl Ieee802154EdEventProbeBackend for FakeBackend {
        fn event_enable_events(&mut self) -> u16 {
            self.operations.push(Operation::EventEnable);
            self.event_enable
        }

        fn enable_ed_timer_abort_events(&mut self) {
            self.operations.push(Operation::EnableEvents);
            self.event_enable = VALIDATION_EVENTS;
        }

        fn disable_all_events(&mut self) {
            self.operations.push(Operation::DisableEvents);
            self.event_enable = 0;
        }

        fn rx_abort_enable_events(&mut self) -> u32 {
            self.operations.push(Operation::RxAbortEnable);
            self.rx_abort_enable
        }

        fn enable_ed_abort_reasons(&mut self) {
            self.operations.push(Operation::EnableEdAbortReasons);
            self.rx_abort_enable = ED_ABORT_REASONS;
        }

        fn disable_all_rx_abort_reasons(&mut self) {
            self.operations.push(Operation::DisableRxAbortReasons);
            self.rx_abort_enable = 0;
        }

        fn interrupt_route_readback(&mut self) -> Ieee802154RouteReadback {
            self.operations.push(Operation::Route);
            self.route
        }

        fn event_status_events(&mut self) -> u16 {
            self.operations.push(Operation::EventStatus);
            self.status_reads
                .pop_front()
                .expect("fake must provide every complete status sample")
        }

        fn rx_status_raw(&mut self) -> u32 {
            self.operations.push(Operation::RxStatus);
            self.rx_status
        }

        fn ed_duration(&mut self) -> u32 {
            self.operations.push(Operation::EdDuration);
            self.ed_duration
        }

        fn set_ed_duration_eight(&mut self) {
            self.operations.push(Operation::SetEdDurationEight);
            self.ed_duration = VALIDATION_ED_DURATION;
        }

        fn timer0_value(&mut self) -> u32 {
            self.operations.push(Operation::TimerValue);
            if self.timer_running {
                self.timer_value = self.timer_value.wrapping_add(1);
            }
            self.timer_value
        }

        fn set_timer0_threshold(&mut self, threshold: u32) {
            self.operations.push(Operation::TimerThreshold(threshold));
        }

        fn start_timer0(&mut self) {
            self.operations.push(Operation::StartTimer);
            self.timer_running = true;
        }

        fn stop_timer0(&mut self) {
            self.operations.push(Operation::StopTimer);
            self.timer_running = false;
        }

        fn start_ed(&mut self) {
            self.operations.push(Operation::StartEd);
        }

        fn stop_operation(&mut self) {
            self.operations.push(Operation::StopEd);
        }

        fn write_ed_done_event(&mut self) {
            self.operations.push(Operation::WriteEdDone);
        }

        fn write_timer0_event(&mut self) {
            self.operations.push(Operation::WriteTimer);
        }
    }

    fn config(poll_limit: u32) -> Ieee802154EdEventProbeConfig {
        Ieee802154EdEventProbeConfig::new(poll_limit, 37)
            .expect("nonzero bounded test configuration")
    }

    #[test]
    fn complete_trace_selects_ed_done_and_retains_timer0() {
        let mut backend = FakeBackend::new(&[0, 0, ED_TIMER_EVENTS, TIMER0_EVENT, 0, 0, 0]);

        let evidence = run_ieee802154_ed_event_probe(&mut backend, config(2));

        assert_eq!(evidence.stop, Ieee802154EdEventProbeStop::Complete);
        assert_eq!(evidence.event_enable_active, 0x0150);
        assert_eq!(evidence.rx_abort_enable_active, ED_ABORT_REASONS);
        assert_eq!(evidence.rx_abort_enable_after, 0);
        assert_eq!(evidence.ed_duration_active, VALIDATION_ED_DURATION);
        assert_eq!(evidence.terminal_events, ED_DONE_EVENT | TIMER0_EVENT);
        assert_eq!(evidence.after_ed_done_write_events, TIMER0_EVENT);
        assert_eq!(evidence.after_timer0_write_events, 0);
        assert_eq!(evidence.observed_events, ED_TIMER_EVENTS);
        assert_eq!(evidence.ed_duration_before, 5);
        assert_eq!(evidence.ed_duration_after, VALIDATION_ED_DURATION);
        assert!(!evidence.stop_command_issued);
        assert!(evidence.cleanup_clear);
        assert_eq!(evidence.rx_status_at_abort, None);
        assert!(
            backend
                .operations
                .windows(2)
                .any(|operations| { operations == [Operation::StartTimer, Operation::StartEd] })
        );
        assert!(backend.operations.windows(4).any(|operations| {
            operations
                == [
                    Operation::DisableEvents,
                    Operation::EventEnable,
                    Operation::DisableRxAbortReasons,
                    Operation::RxAbortEnable,
                ]
        }));
        backend.assert_consumed();
    }

    #[test]
    fn nonzero_entry_abort_mask_performs_no_mutating_operation() {
        let mut backend = FakeBackend::new(&[0]);
        backend.rx_abort_enable = 1;

        let evidence = run_ieee802154_ed_event_probe(&mut backend, config(1));

        assert_eq!(evidence.stop, Ieee802154EdEventProbeStop::UnsupportedSetup);
        assert_eq!(evidence.rx_abort_enable_before, 1);
        assert!(backend.operations.iter().all(|operation| matches!(
            operation,
            Operation::EventEnable
                | Operation::RxAbortEnable
                | Operation::Route
                | Operation::EventStatus
                | Operation::RxStatus
                | Operation::EdDuration
                | Operation::TimerValue
        )));
        backend.assert_consumed();
    }

    #[test]
    fn selected_ed_done_write_mismatch_fails_closed_without_more_status_writes() {
        let mut backend = FakeBackend::new(&[
            0,
            0,
            ED_TIMER_EVENTS,
            ED_TIMER_EVENTS,
            ED_TIMER_EVENTS,
            ED_TIMER_EVENTS,
        ]);

        let evidence = run_ieee802154_ed_event_probe(&mut backend, config(1));

        assert_eq!(
            evidence.stop,
            Ieee802154EdEventProbeStop::SelectiveWriteMismatch
        );
        assert_eq!(evidence.after_ed_done_write_events, ED_TIMER_EVENTS);
        assert_eq!(evidence.final_events, ED_TIMER_EVENTS);
        assert!(!evidence.cleanup_clear);
        assert_eq!(
            backend
                .operations
                .iter()
                .filter(|operation| **operation == Operation::WriteEdDone)
                .count(),
            1
        );
        assert!(!backend.operations.contains(&Operation::WriteTimer));
        backend.assert_consumed();
    }

    #[test]
    fn selected_timer_write_mismatch_never_repeats_either_raw_write() {
        let mut backend = FakeBackend::new(&[
            0,
            0,
            ED_TIMER_EVENTS,
            TIMER0_EVENT,
            TIMER0_EVENT,
            TIMER0_EVENT,
            TIMER0_EVENT,
        ]);

        let evidence = run_ieee802154_ed_event_probe(&mut backend, config(1));

        assert_eq!(
            evidence.stop,
            Ieee802154EdEventProbeStop::SelectiveWriteMismatch
        );
        assert_eq!(evidence.after_ed_done_write_events, TIMER0_EVENT);
        assert_eq!(evidence.after_timer0_write_events, TIMER0_EVENT);
        assert_eq!(evidence.final_events, TIMER0_EVENT);
        assert!(!evidence.cleanup_clear);
        assert_eq!(
            backend
                .operations
                .iter()
                .filter(|operation| **operation == Operation::WriteEdDone)
                .count(),
            1
        );
        assert_eq!(
            backend
                .operations
                .iter()
                .filter(|operation| **operation == Operation::WriteTimer)
                .count(),
            1
        );
        backend.assert_consumed();
    }

    #[test]
    fn pair_timeout_preserves_last_and_union_status_and_stops_ed() {
        let mut backend = FakeBackend::new(&[0, 0, 0, TIMER0_EVENT, TIMER0_EVENT, TIMER0_EVENT]);

        let evidence = run_ieee802154_ed_event_probe(&mut backend, config(2));

        assert_eq!(evidence.stop, Ieee802154EdEventProbeStop::PairLatchTimeout);
        assert_eq!(evidence.terminal_events, TIMER0_EVENT);
        assert_eq!(evidence.observed_events, TIMER0_EVENT);
        assert_eq!(evidence.cleanup_pending_events, TIMER0_EVENT);
        assert_eq!(evidence.final_events, TIMER0_EVENT);
        assert!(evidence.stop_command_issued);
        assert!(!evidence.cleanup_clear);
        assert!(backend.operations.contains(&Operation::StopEd));
        backend.assert_consumed();
    }

    #[test]
    fn rx_abort_is_retained_with_raw_status_and_never_acknowledged() {
        let mut backend = FakeBackend::new(&[0, 0, RX_ABORT_EVENT, RX_ABORT_EVENT, RX_ABORT_EVENT]);
        backend.rx_status = 25 << 4;

        let evidence = run_ieee802154_ed_event_probe(&mut backend, config(1));

        assert_eq!(evidence.stop, Ieee802154EdEventProbeStop::EdAborted);
        assert_eq!(evidence.terminal_events, RX_ABORT_EVENT);
        assert_eq!(evidence.observed_events, RX_ABORT_EVENT);
        assert_eq!(evidence.rx_status_at_abort, Some(25 << 4));
        assert!(!backend.operations.contains(&Operation::WriteEdDone));
        assert!(!backend.operations.contains(&Operation::WriteTimer));
        assert!(!evidence.cleanup_clear);
        backend.assert_consumed();
    }
}
