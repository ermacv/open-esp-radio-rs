//! Validation-only discriminator for the unresolved IEEE 802.15.4
//! `EVENT_STATUS` write class.
//!
//! The transaction enables exactly the two validation timer events only after
//! read-only checks prove the source-132 CPU route controls are at their boot
//! reset values on both cores. It never writes a route. This remains evidence
//! collection, not an active IRQ owner, acknowledge capability, or MAC-ready
//! transition.

const TIMER0_EVENT: u16 = 1 << 8;
const TIMER1_EVENT: u16 = 1 << 9;
const TIMER_EVENTS: u16 = TIMER0_EVENT | TIMER1_EVENT;

use core::sync::atomic::{AtomicBool, Ordering};

use open_esp_radio_esp32s31_ieee802154_irq::Ieee802154RouteReadback;

static RESET_ISOLATION_CLAIMED: AtomicBool = AtomicBool::new(false);

/// Unique process-lifetime claim for the reset-isolated validation image.
///
/// The capability is available only with `validation-probes`, cannot be
/// constructed or cloned by callers, and is never released. It proves that at
/// most one transaction in the dedicated image may rely on both source-132
/// routes remaining untouched. The transaction still checks both complete raw
/// route words before and during the active phase.
#[must_use = "the reset-isolation claim must be consumed by the terminal validation transaction"]
pub struct Ieee802154EventStatusProbeIsolation {
    _private: (),
}

impl Ieee802154EventStatusProbeIsolation {
    /// Claim the dedicated image's one reset-isolation capability.
    #[doc(hidden)]
    pub fn claim_for_reset_isolated_image() -> Option<Self> {
        RESET_ISOLATION_CLAIMED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self { _private: () })
    }
}

/// Bounded inputs for one closed `EVENT_STATUS` validation transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154EventStatusProbeConfig {
    poll_limit: u32,
    timer_threshold: u32,
}

impl Ieee802154EventStatusProbeConfig {
    /// Wire-compatible upper bound for each individual status polling loop.
    pub const MAX_POLL_LIMIT: u32 = 1_000_000;
    /// Wire-compatible upper bound for the timer stimulus threshold.
    pub const MAX_TIMER_THRESHOLD: u32 = 1_000;

    /// Construct one finite probe configuration.
    ///
    /// Zero is rejected for both values: it would either perform no bounded
    /// observation or make the timer stimulus degenerate.  The upper bounds
    /// match the HIL wire contract and keep every busy wait tightly finite.
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

    /// Maximum status samples made by each individual wait.
    pub const fn poll_limit(self) -> u32 {
        self.poll_limit
    }

    /// Complete field value written to each validation timer threshold.
    pub const fn timer_threshold(self) -> u32 {
        self.timer_threshold
    }
}

/// Terminal classification for one closed validation transaction.
///
/// These variants report raw experimental control flow only.  Even
/// [`Complete`](Self::Complete) does not publish a production acknowledge
/// operation; reviewed HIL evidence must first classify the register.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154EventStatusProbeStop {
    /// Every expected timer-bit relation was observed and cleanup read zero.
    Complete,
    /// MAC event delivery was not masked at entry.
    UnsupportedSetup,
    /// A source-132 route word had a non-reset MAP or PASS_LEVEL field.
    RouteNotQuiesced,
    /// The post-reset entry sample was not clear.
    ResetNotClear,
    /// The exact timer-event enable image did not read back.
    EventEnableReadbackMismatch,
    /// Enabling delivery revealed a pre-existing status before timer start.
    PostEnableStatusNotClear,
    /// One or both timer counters did not change while their start commands
    /// were active and no dual event latch was observed.
    TimerActivityTimeout,
    /// Both independently started timers did not latch within the bound.
    DualLatchTimeout,
    /// A selected raw write did not clear only the selected timer event.
    SelectiveAcknowledgeMismatch,
    /// Timer zero did not latch alone within the bound.
    DistinctFirstLatchTimeout,
    /// Timer one did not join the retained timer-zero snapshot in time.
    DistinctSecondLatchTimeout,
    /// Best-effort stop and timer-bit cleanup did not leave the delivery mask,
    /// masked status observation, and both route controls clear.
    CleanupNotClear,
}

/// Raw trace from one closed validation transaction.
///
/// Every event image is the complete fourteen-bit `EVENT_STATUS` field.  The
/// fields intentionally carry observations rather than typed IRQ events so
/// the HIL oracle can test selective-W1C compatibility without publishing an
/// acknowledge access class or collapsing mismatches into invented semantics.
/// Route fields retain complete raw words and every nonzero bit fails the
/// reset-isolation gate. `dual_observed_events` is the bitwise union of every
/// sample in the first bounded wait, while `dual_latched_events` is its last
/// sample and therefore preserves the simultaneous two-bit condition.
/// `cleanup_pending_events` is sampled after delivery is masked and may hide a
/// retained latch; cleanup selection also retains every pre-mask timer sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154EventStatusProbeEvidence {
    pub stop: Ieee802154EventStatusProbeStop,
    pub event_enable_before: u16,
    pub event_enable_active: u16,
    pub event_enable_after: u16,
    pub route_core0_before_enable: u32,
    pub route_core1_before_enable: u32,
    pub route_core0_with_events_enabled: u32,
    pub route_core1_with_events_enabled: u32,
    pub route_core0_after_cleanup: u32,
    pub route_core1_after_cleanup: u32,
    pub post_enable_events: u16,
    pub timer0_value_before_start: u32,
    pub timer1_value_before_start: u32,
    pub timer0_value_min: u32,
    pub timer0_value_max: u32,
    pub timer1_value_min: u32,
    pub timer1_value_max: u32,
    pub timer0_value_after_stop: u32,
    pub timer1_value_after_stop: u32,
    pub reset_events: u16,
    pub dual_observed_events: u16,
    pub dual_latched_events: u16,
    pub after_timer0_ack_events: u16,
    pub after_timer1_ack_events: u16,
    pub distinct_snapshot_events: u16,
    pub distinct_before_ack_events: u16,
    pub distinct_after_ack_events: u16,
    pub cleanup_pending_events: u16,
    pub final_events: u16,
}

impl Ieee802154EventStatusProbeEvidence {
    const fn empty() -> Self {
        Self {
            stop: Ieee802154EventStatusProbeStop::UnsupportedSetup,
            event_enable_before: 0,
            event_enable_active: 0,
            event_enable_after: 0,
            route_core0_before_enable: 0,
            route_core1_before_enable: 0,
            route_core0_with_events_enabled: 0,
            route_core1_with_events_enabled: 0,
            route_core0_after_cleanup: 0,
            route_core1_after_cleanup: 0,
            post_enable_events: 0,
            timer0_value_before_start: 0,
            timer1_value_before_start: 0,
            timer0_value_min: 0,
            timer0_value_max: 0,
            timer1_value_min: 0,
            timer1_value_max: 0,
            timer0_value_after_stop: 0,
            timer1_value_after_stop: 0,
            reset_events: 0,
            dual_observed_events: 0,
            dual_latched_events: 0,
            after_timer0_ack_events: 0,
            after_timer1_ack_events: 0,
            distinct_snapshot_events: 0,
            distinct_before_ack_events: 0,
            distinct_after_ack_events: 0,
            cleanup_pending_events: 0,
            final_events: 0,
        }
    }
}

/// Closed register vocabulary consumed by the pure validation state machine.
///
/// The target implementation is the unique PAC lease in a validation build.
/// Tests implement this trait only inside this module; callers cannot provide
/// raw addresses or broaden the two selected writes.
pub(crate) trait Ieee802154EventStatusProbeBackend {
    fn event_enable_events(&mut self) -> u16;
    fn enable_timer_events(&mut self);
    fn disable_all_events(&mut self);
    fn interrupt_route_readback(&mut self) -> Ieee802154RouteReadback;
    fn event_status_events(&mut self) -> u16;
    fn timer0_value(&mut self) -> u32;
    fn timer1_value(&mut self) -> u32;
    fn set_timer_thresholds(&mut self, threshold: u32);
    fn start_timer0(&mut self);
    fn stop_timer0(&mut self);
    fn start_timer1(&mut self);
    fn stop_timer1(&mut self);
    fn write_timer0_event(&mut self);
    fn write_timer1_event(&mut self);
}

#[cfg(feature = "validation-probes")]
impl Ieee802154EventStatusProbeBackend for crate::ieee802154::Ieee802154PacHal<'_> {
    fn event_enable_events(&mut self) -> u16 {
        self.validation_event_enable_events()
    }

    fn enable_timer_events(&mut self) {
        self.validation_enable_timer_events();
    }

    fn disable_all_events(&mut self) {
        self.validation_disable_all_events();
    }

    fn interrupt_route_readback(&mut self) -> Ieee802154RouteReadback {
        self.validation_interrupt_route_readback()
    }

    fn event_status_events(&mut self) -> u16 {
        self.validation_event_status_events()
    }

    fn timer0_value(&mut self) -> u32 {
        self.validation_event_timer0_value()
    }

    fn timer1_value(&mut self) -> u32 {
        self.validation_event_timer1_value()
    }

    fn set_timer_thresholds(&mut self, threshold: u32) {
        self.validation_set_event_timer_thresholds(threshold);
    }

    fn start_timer0(&mut self) {
        self.validation_start_event_timer0();
    }

    fn stop_timer0(&mut self) {
        self.validation_stop_event_timer0();
    }

    fn start_timer1(&mut self) {
        self.validation_start_event_timer1();
    }

    fn stop_timer1(&mut self) {
        self.validation_stop_event_timer1();
    }

    fn write_timer0_event(&mut self) {
        self.validation_write_event_timer0();
    }

    fn write_timer1_event(&mut self) {
        self.validation_write_event_timer1();
    }
}

fn poll_events<B, F>(backend: &mut B, poll_limit: u32, predicate: F) -> (bool, u16)
where
    B: Ieee802154EventStatusProbeBackend,
    F: Fn(u16) -> bool,
{
    let mut observed = 0;
    for _ in 0..poll_limit {
        observed = backend.event_status_events();
        if predicate(observed) {
            return (true, observed);
        }
    }
    (false, observed)
}

fn finish<B>(
    backend: &mut B,
    mut evidence: Ieee802154EventStatusProbeEvidence,
    stop: Ieee802154EventStatusProbeStop,
    retained_events: u16,
    status_cleanup_allowed: bool,
) -> Ieee802154EventStatusProbeEvidence
where
    B: Ieee802154EventStatusProbeBackend,
{
    backend.stop_timer0();
    backend.stop_timer1();
    backend.disable_all_events();
    evidence.event_enable_after = backend.event_enable_events();

    let pending = backend.event_status_events();
    evidence.cleanup_pending_events = pending;
    if evidence.event_enable_after == 0 && status_cleanup_allowed {
        let selected = (retained_events | pending) & TIMER_EVENTS;
        if selected & TIMER0_EVENT != 0 {
            backend.write_timer0_event();
        }
        if selected & TIMER1_EVENT != 0 {
            backend.write_timer1_event();
        }
    }

    evidence.final_events = backend.event_status_events();
    let route = backend.interrupt_route_readback();
    evidence.route_core0_after_cleanup = route.core0().raw_bits();
    evidence.route_core1_after_cleanup = route.core1().raw_bits();
    evidence.stop = if evidence.final_events == 0
        && evidence.event_enable_after == 0
        && route.is_full_reset()
    {
        stop
    } else {
        Ieee802154EventStatusProbeStop::CleanupNotClear
    };
    evidence
}

fn finish_without_writes<B>(
    backend: &mut B,
    mut evidence: Ieee802154EventStatusProbeEvidence,
    stop: Ieee802154EventStatusProbeStop,
) -> Ieee802154EventStatusProbeEvidence
where
    B: Ieee802154EventStatusProbeBackend,
{
    evidence.final_events = backend.event_status_events();
    evidence.event_enable_after = backend.event_enable_events();
    let route = backend.interrupt_route_readback();
    evidence.route_core0_after_cleanup = route.core0().raw_bits();
    evidence.route_core1_after_cleanup = route.core1().raw_bits();
    evidence.stop = stop;
    evidence
}

/// Execute the pure sequence shared by the real PAC lease and host fake.
pub(crate) fn run_ieee802154_event_status_probe<B>(
    backend: &mut B,
    config: Ieee802154EventStatusProbeConfig,
) -> Ieee802154EventStatusProbeEvidence
where
    B: Ieee802154EventStatusProbeBackend,
{
    let mut evidence = Ieee802154EventStatusProbeEvidence::empty();

    evidence.event_enable_before = backend.event_enable_events();
    if evidence.event_enable_before != 0 {
        return finish_without_writes(
            backend,
            evidence,
            Ieee802154EventStatusProbeStop::UnsupportedSetup,
        );
    }

    let route = backend.interrupt_route_readback();
    evidence.route_core0_before_enable = route.core0().raw_bits();
    evidence.route_core1_before_enable = route.core1().raw_bits();
    if !route.is_full_reset() {
        return finish_without_writes(
            backend,
            evidence,
            Ieee802154EventStatusProbeStop::RouteNotQuiesced,
        );
    }

    evidence.reset_events = backend.event_status_events();
    if evidence.reset_events != 0 {
        return finish_without_writes(
            backend,
            evidence,
            Ieee802154EventStatusProbeStop::ResetNotClear,
        );
    }

    backend.set_timer_thresholds(config.timer_threshold());
    backend.enable_timer_events();
    evidence.event_enable_active = backend.event_enable_events();
    if evidence.event_enable_active != TIMER_EVENTS {
        return finish(
            backend,
            evidence,
            Ieee802154EventStatusProbeStop::EventEnableReadbackMismatch,
            0,
            false,
        );
    }

    evidence.post_enable_events = backend.event_status_events();
    let mut retained_events = evidence.post_enable_events;
    if evidence.post_enable_events != 0 {
        return finish(
            backend,
            evidence,
            Ieee802154EventStatusProbeStop::PostEnableStatusNotClear,
            retained_events,
            true,
        );
    }

    let route = backend.interrupt_route_readback();
    evidence.route_core0_with_events_enabled = route.core0().raw_bits();
    evidence.route_core1_with_events_enabled = route.core1().raw_bits();
    if !route.is_full_reset() {
        return finish(
            backend,
            evidence,
            Ieee802154EventStatusProbeStop::RouteNotQuiesced,
            retained_events,
            false,
        );
    }

    evidence.timer0_value_before_start = backend.timer0_value();
    evidence.timer1_value_before_start = backend.timer1_value();
    evidence.timer0_value_min = evidence.timer0_value_before_start;
    evidence.timer0_value_max = evidence.timer0_value_before_start;
    evidence.timer1_value_min = evidence.timer1_value_before_start;
    evidence.timer1_value_max = evidence.timer1_value_before_start;
    backend.start_timer0();
    backend.start_timer1();
    let mut timer0_changed = false;
    let mut timer1_changed = false;
    let mut dual_latched = false;
    for _ in 0..config.poll_limit() {
        let timer0_value = backend.timer0_value();
        let timer1_value = backend.timer1_value();
        evidence.timer0_value_min = evidence.timer0_value_min.min(timer0_value);
        evidence.timer0_value_max = evidence.timer0_value_max.max(timer0_value);
        evidence.timer1_value_min = evidence.timer1_value_min.min(timer1_value);
        evidence.timer1_value_max = evidence.timer1_value_max.max(timer1_value);
        if !timer0_changed && timer0_value != evidence.timer0_value_before_start {
            timer0_changed = true;
        }
        if !timer1_changed && timer1_value != evidence.timer1_value_before_start {
            timer1_changed = true;
        }
        evidence.dual_latched_events = backend.event_status_events();
        evidence.dual_observed_events |= evidence.dual_latched_events;
        retained_events |= evidence.dual_latched_events;
        if evidence.dual_latched_events & TIMER_EVENTS == TIMER_EVENTS {
            dual_latched = true;
            break;
        }
    }

    // Freeze both independent stimuli before classifying the bounded wait or
    // testing selected raw writes. Counter samples distinguish timer activity
    // from absent status observations while exactly the timer events remain
    // enabled and the CPU route remains reset-detached.
    backend.stop_timer0();
    backend.stop_timer1();
    evidence.timer0_value_after_stop = backend.timer0_value();
    evidence.timer1_value_after_stop = backend.timer1_value();
    if !dual_latched {
        let stop = if timer0_changed && timer1_changed {
            Ieee802154EventStatusProbeStop::DualLatchTimeout
        } else {
            Ieee802154EventStatusProbeStop::TimerActivityTimeout
        };
        return finish(backend, evidence, stop, retained_events, true);
    }

    backend.write_timer0_event();
    evidence.after_timer0_ack_events = backend.event_status_events();
    retained_events |= evidence.after_timer0_ack_events;
    if evidence.after_timer0_ack_events & TIMER_EVENTS != TIMER1_EVENT {
        return finish(
            backend,
            evidence,
            Ieee802154EventStatusProbeStop::SelectiveAcknowledgeMismatch,
            retained_events,
            true,
        );
    }

    backend.write_timer1_event();
    evidence.after_timer1_ack_events = backend.event_status_events();
    retained_events |= evidence.after_timer1_ack_events;
    if evidence.after_timer1_ack_events & TIMER_EVENTS != 0 {
        return finish(
            backend,
            evidence,
            Ieee802154EventStatusProbeStop::SelectiveAcknowledgeMismatch,
            retained_events,
            true,
        );
    }

    // Re-arm timer zero alone and retain the successful polling read as the
    // snapshot whose selected bit will be acknowledged later.
    backend.set_timer_thresholds(config.timer_threshold());
    backend.start_timer0();
    let (first_latched, first_events) = poll_events(backend, config.poll_limit(), |events| {
        events & TIMER_EVENTS == TIMER0_EVENT
    });
    evidence.distinct_snapshot_events = first_events;
    retained_events |= first_events;
    if !first_latched {
        return finish(
            backend,
            evidence,
            Ieee802154EventStatusProbeStop::DistinctFirstLatchTimeout,
            retained_events,
            true,
        );
    }
    backend.stop_timer0();

    // Timer one is introduced strictly after the retained timer-zero sample
    // and before the raw write of timer zero's earlier selected bit.
    backend.start_timer1();
    let (second_latched, second_events) = poll_events(backend, config.poll_limit(), |events| {
        events & TIMER_EVENTS == TIMER_EVENTS
    });
    evidence.distinct_before_ack_events = second_events;
    retained_events |= second_events;
    if !second_latched {
        return finish(
            backend,
            evidence,
            Ieee802154EventStatusProbeStop::DistinctSecondLatchTimeout,
            retained_events,
            true,
        );
    }
    backend.stop_timer1();

    backend.write_timer0_event();
    evidence.distinct_after_ack_events = backend.event_status_events();
    retained_events |= evidence.distinct_after_ack_events;
    if evidence.distinct_after_ack_events & TIMER_EVENTS != TIMER1_EVENT {
        return finish(
            backend,
            evidence,
            Ieee802154EventStatusProbeStop::SelectiveAcknowledgeMismatch,
            retained_events,
            true,
        );
    }

    finish(
        backend,
        evidence,
        Ieee802154EventStatusProbeStop::Complete,
        retained_events,
        true,
    )
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, vec, vec::Vec};

    use super::{
        Ieee802154EventStatusProbeBackend, Ieee802154EventStatusProbeConfig,
        Ieee802154EventStatusProbeIsolation, Ieee802154EventStatusProbeStop, TIMER_EVENTS,
        TIMER0_EVENT, TIMER1_EVENT, run_ieee802154_event_status_probe,
    };
    use open_esp_radio_esp32s31_ieee802154_irq::{Ieee802154RouteReadback, Ieee802154RouteWord};

    const fn route(core0: u32, core1: u32) -> Ieee802154RouteReadback {
        Ieee802154RouteReadback::new(
            Ieee802154RouteWord::from_raw(core0),
            Ieee802154RouteWord::from_raw(core1),
        )
    }

    const fn reset_route() -> Ieee802154RouteReadback {
        route(0, 0)
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        EventEnable,
        EnableTimerEvents,
        DisableAllEvents,
        RouteReadback,
        EventStatus,
        Timer0Value,
        Timer1Value,
        Thresholds(u32),
        StartTimer0,
        StopTimer0,
        StartTimer1,
        StopTimer1,
        WriteTimer0,
        WriteTimer1,
    }

    struct FakeBackend {
        event_enable: u16,
        enabled_writeback: u16,
        disabled_writeback: u16,
        route_reads: VecDeque<Ieee802154RouteReadback>,
        route_default: Ieee802154RouteReadback,
        status_reads: VecDeque<u16>,
        timer0_advances: bool,
        timer1_advances: bool,
        timer0_value: u32,
        timer1_value: u32,
        operations: Vec<Operation>,
    }

    impl FakeBackend {
        fn new(event_enable: u16, status_reads: &[u16]) -> Self {
            Self {
                event_enable,
                enabled_writeback: TIMER_EVENTS,
                disabled_writeback: 0,
                route_reads: VecDeque::new(),
                route_default: reset_route(),
                status_reads: status_reads.iter().copied().collect(),
                timer0_advances: true,
                timer1_advances: true,
                timer0_value: 0,
                timer1_value: 0,
                operations: Vec::new(),
            }
        }

        fn with_inactive_timers(mut self) -> Self {
            self.timer0_advances = false;
            self.timer1_advances = false;
            self
        }

        fn with_inactive_timer0(mut self) -> Self {
            self.timer0_advances = false;
            self
        }

        fn with_inactive_timer1(mut self) -> Self {
            self.timer1_advances = false;
            self
        }

        fn with_enabled_writeback(mut self, writeback: u16) -> Self {
            self.enabled_writeback = writeback;
            self
        }

        fn with_disabled_writeback(mut self, writeback: u16) -> Self {
            self.disabled_writeback = writeback;
            self
        }

        fn with_routes(mut self, routes: &[Ieee802154RouteReadback]) -> Self {
            self.route_reads = routes.iter().copied().collect();
            self
        }

        fn assert_consumed(&self) {
            assert!(
                self.status_reads.is_empty(),
                "unused fake status reads: {:?}",
                self.status_reads
            );
        }
    }

    impl Ieee802154EventStatusProbeBackend for FakeBackend {
        fn event_enable_events(&mut self) -> u16 {
            self.operations.push(Operation::EventEnable);
            self.event_enable
        }

        fn enable_timer_events(&mut self) {
            self.operations.push(Operation::EnableTimerEvents);
            self.event_enable = self.enabled_writeback;
        }

        fn disable_all_events(&mut self) {
            self.operations.push(Operation::DisableAllEvents);
            self.event_enable = self.disabled_writeback;
        }

        fn interrupt_route_readback(&mut self) -> Ieee802154RouteReadback {
            self.operations.push(Operation::RouteReadback);
            self.route_reads.pop_front().unwrap_or(self.route_default)
        }

        fn event_status_events(&mut self) -> u16 {
            self.operations.push(Operation::EventStatus);
            self.status_reads
                .pop_front()
                .expect("fake must provide every requested raw status sample")
        }

        fn timer0_value(&mut self) -> u32 {
            self.operations.push(Operation::Timer0Value);
            self.timer0_value
        }

        fn timer1_value(&mut self) -> u32 {
            self.operations.push(Operation::Timer1Value);
            self.timer1_value
        }

        fn set_timer_thresholds(&mut self, threshold: u32) {
            self.operations.push(Operation::Thresholds(threshold));
        }

        fn start_timer0(&mut self) {
            self.operations.push(Operation::StartTimer0);
            if self.timer0_advances {
                self.timer0_value = self.timer0_value.wrapping_add(1);
            }
        }

        fn stop_timer0(&mut self) {
            self.operations.push(Operation::StopTimer0);
        }

        fn start_timer1(&mut self) {
            self.operations.push(Operation::StartTimer1);
            if self.timer1_advances {
                self.timer1_value = self.timer1_value.wrapping_add(1);
            }
        }

        fn stop_timer1(&mut self) {
            self.operations.push(Operation::StopTimer1);
        }

        fn write_timer0_event(&mut self) {
            self.operations.push(Operation::WriteTimer0);
        }

        fn write_timer1_event(&mut self) {
            self.operations.push(Operation::WriteTimer1);
        }
    }

    fn config(poll_limit: u32) -> Ieee802154EventStatusProbeConfig {
        Ieee802154EventStatusProbeConfig::new(poll_limit, 37).expect("nonzero finite test config")
    }

    #[test]
    fn config_rejects_degenerate_inputs_and_preserves_bounds() {
        assert_eq!(Ieee802154EventStatusProbeConfig::new(0, 1), None);
        assert_eq!(Ieee802154EventStatusProbeConfig::new(1, 0), None);
        assert_eq!(
            Ieee802154EventStatusProbeConfig::new(
                Ieee802154EventStatusProbeConfig::MAX_POLL_LIMIT + 1,
                1,
            ),
            None
        );
        assert_eq!(
            Ieee802154EventStatusProbeConfig::new(
                1,
                Ieee802154EventStatusProbeConfig::MAX_TIMER_THRESHOLD + 1,
            ),
            None
        );
        let maximum = Ieee802154EventStatusProbeConfig::new(
            Ieee802154EventStatusProbeConfig::MAX_POLL_LIMIT,
            Ieee802154EventStatusProbeConfig::MAX_TIMER_THRESHOLD,
        )
        .expect("wire maxima are supported");
        assert_eq!(
            maximum.poll_limit(),
            Ieee802154EventStatusProbeConfig::MAX_POLL_LIMIT
        );
        assert_eq!(
            maximum.timer_threshold(),
            Ieee802154EventStatusProbeConfig::MAX_TIMER_THRESHOLD
        );
    }

    #[test]
    fn reset_isolation_capability_is_process_lifetime_unique() {
        {
            let _isolation = Ieee802154EventStatusProbeIsolation::claim_for_reset_isolated_image()
                .expect("first validation transaction owns reset isolation");
            assert!(
                Ieee802154EventStatusProbeIsolation::claim_for_reset_isolated_image().is_none()
            );
        }
        assert!(Ieee802154EventStatusProbeIsolation::claim_for_reset_isolated_image().is_none());
    }

    #[test]
    fn complete_trace_orders_detached_route_enabled_stimulus_and_masked_cleanup() {
        let mut backend = FakeBackend::new(
            0,
            &[
                0,
                0,
                TIMER_EVENTS,
                TIMER1_EVENT,
                0,
                TIMER0_EVENT,
                TIMER_EVENTS,
                TIMER1_EVENT,
                0,
                0,
            ],
        );

        let evidence = run_ieee802154_event_status_probe(&mut backend, config(2));

        assert_eq!(evidence.stop, Ieee802154EventStatusProbeStop::Complete);
        assert_eq!(evidence.event_enable_before, 0);
        assert_eq!(evidence.event_enable_active, TIMER_EVENTS);
        assert_eq!(evidence.event_enable_after, 0);
        assert_eq!(evidence.route_core0_before_enable, 0);
        assert_eq!(evidence.route_core1_before_enable, 0);
        assert_eq!(evidence.route_core0_with_events_enabled, 0);
        assert_eq!(evidence.route_core1_with_events_enabled, 0);
        assert_eq!(evidence.route_core0_after_cleanup, 0);
        assert_eq!(evidence.route_core1_after_cleanup, 0);
        assert_eq!(evidence.post_enable_events, 0);
        assert_eq!(evidence.timer0_value_before_start, 0);
        assert_eq!(evidence.timer0_value_min, 0);
        assert_eq!(evidence.timer0_value_max, 1);
        assert_eq!(evidence.timer0_value_after_stop, 1);
        assert_eq!(evidence.timer1_value_before_start, 0);
        assert_eq!(evidence.timer1_value_min, 0);
        assert_eq!(evidence.timer1_value_max, 1);
        assert_eq!(evidence.timer1_value_after_stop, 1);
        assert_eq!(evidence.reset_events, 0);
        assert_eq!(evidence.dual_observed_events, TIMER_EVENTS);
        assert_eq!(evidence.dual_latched_events, TIMER_EVENTS);
        assert_eq!(evidence.after_timer0_ack_events, TIMER1_EVENT);
        assert_eq!(evidence.after_timer1_ack_events, 0);
        assert_eq!(evidence.distinct_snapshot_events, TIMER0_EVENT);
        assert_eq!(evidence.distinct_before_ack_events, TIMER_EVENTS);
        assert_eq!(evidence.distinct_after_ack_events, TIMER1_EVENT);
        assert_eq!(evidence.cleanup_pending_events, 0);
        assert_eq!(evidence.final_events, 0);
        assert_eq!(
            backend.operations,
            vec![
                Operation::EventEnable,
                Operation::RouteReadback,
                Operation::EventStatus,
                Operation::Thresholds(37),
                Operation::EnableTimerEvents,
                Operation::EventEnable,
                Operation::EventStatus,
                Operation::RouteReadback,
                Operation::Timer0Value,
                Operation::Timer1Value,
                Operation::StartTimer0,
                Operation::StartTimer1,
                Operation::Timer0Value,
                Operation::Timer1Value,
                Operation::EventStatus,
                Operation::StopTimer0,
                Operation::StopTimer1,
                Operation::Timer0Value,
                Operation::Timer1Value,
                Operation::WriteTimer0,
                Operation::EventStatus,
                Operation::WriteTimer1,
                Operation::EventStatus,
                Operation::Thresholds(37),
                Operation::StartTimer0,
                Operation::EventStatus,
                Operation::StopTimer0,
                Operation::StartTimer1,
                Operation::EventStatus,
                Operation::StopTimer1,
                Operation::WriteTimer0,
                Operation::EventStatus,
                Operation::StopTimer0,
                Operation::StopTimer1,
                Operation::DisableAllEvents,
                Operation::EventEnable,
                Operation::EventStatus,
                Operation::WriteTimer0,
                Operation::WriteTimer1,
                Operation::EventStatus,
                Operation::RouteReadback,
            ]
        );
        backend.assert_consumed();
    }

    #[test]
    fn unsupported_setup_never_programs_or_writes_the_mac() {
        let mut backend = FakeBackend::new(1, &[0x0123]);
        let evidence = run_ieee802154_event_status_probe(&mut backend, config(1));

        assert_eq!(
            evidence.stop,
            Ieee802154EventStatusProbeStop::UnsupportedSetup
        );
        assert_eq!(evidence.final_events, 0x0123);
        assert_eq!(
            backend.operations,
            [
                Operation::EventEnable,
                Operation::EventStatus,
                Operation::EventEnable,
                Operation::RouteReadback,
            ]
        );
        backend.assert_consumed();
    }

    #[test]
    fn reset_not_clear_returns_without_any_register_write() {
        let mut backend = FakeBackend::new(0, &[TIMER0_EVENT, TIMER0_EVENT]);
        let evidence = run_ieee802154_event_status_probe(&mut backend, config(1));

        assert_eq!(evidence.stop, Ieee802154EventStatusProbeStop::ResetNotClear);
        assert_eq!(evidence.reset_events, TIMER0_EVENT);
        assert_eq!(evidence.final_events, TIMER0_EVENT);
        assert!(backend.operations.iter().all(|operation| {
            !matches!(
                operation,
                Operation::Thresholds(_)
                    | Operation::EnableTimerEvents
                    | Operation::DisableAllEvents
                    | Operation::StartTimer0
                    | Operation::StartTimer1
                    | Operation::StopTimer0
                    | Operation::StopTimer1
                    | Operation::WriteTimer0
                    | Operation::WriteTimer1
            )
        }));
        backend.assert_consumed();
    }

    #[test]
    fn active_route_at_entry_returns_without_any_register_write() {
        let active = route(1, 0);
        let mut backend = FakeBackend::new(0, &[0]).with_routes(&[active, active]);

        let evidence = run_ieee802154_event_status_probe(&mut backend, config(1));

        assert_eq!(
            evidence.stop,
            Ieee802154EventStatusProbeStop::RouteNotQuiesced
        );
        assert_eq!(evidence.route_core0_before_enable, 1);
        assert_eq!(evidence.route_core0_after_cleanup, 1);
        assert!(backend.operations.iter().all(|operation| {
            matches!(
                operation,
                Operation::EventEnable | Operation::EventStatus | Operation::RouteReadback
            )
        }));
        backend.assert_consumed();
    }

    #[test]
    fn enable_readback_mismatch_restores_zero_before_any_start_or_status_write() {
        let mut backend =
            FakeBackend::new(0, &[0, TIMER0_EVENT, 0]).with_enabled_writeback(TIMER0_EVENT);

        let evidence = run_ieee802154_event_status_probe(&mut backend, config(1));

        assert_eq!(
            evidence.stop,
            Ieee802154EventStatusProbeStop::EventEnableReadbackMismatch
        );
        assert_eq!(evidence.event_enable_active, TIMER0_EVENT);
        assert_eq!(evidence.event_enable_after, 0);
        assert!(!backend.operations.contains(&Operation::StartTimer0));
        assert!(!backend.operations.contains(&Operation::StartTimer1));
        assert!(!backend.operations.contains(&Operation::WriteTimer0));
        assert!(!backend.operations.contains(&Operation::WriteTimer1));
        backend.assert_consumed();
    }

    #[test]
    fn status_revealed_by_enable_is_not_counted_as_fresh_stimulus() {
        let mut backend = FakeBackend::new(0, &[0, TIMER0_EVENT, 0, 0]);

        let evidence = run_ieee802154_event_status_probe(&mut backend, config(1));

        assert_eq!(
            evidence.stop,
            Ieee802154EventStatusProbeStop::PostEnableStatusNotClear
        );
        assert_eq!(evidence.post_enable_events, TIMER0_EVENT);
        assert_eq!(evidence.dual_observed_events, 0);
        assert!(!backend.operations.contains(&Operation::StartTimer0));
        assert!(!backend.operations.contains(&Operation::StartTimer1));
        let disable = backend
            .operations
            .iter()
            .position(|operation| *operation == Operation::DisableAllEvents)
            .expect("cleanup masks delivery");
        let write = backend
            .operations
            .iter()
            .position(|operation| *operation == Operation::WriteTimer0)
            .expect("retained revealed bit is cleaned best-effort");
        assert!(disable < write);
        backend.assert_consumed();
    }

    #[test]
    fn route_change_while_enabled_restores_mask_before_any_timer_start() {
        let active = route(0, 0x100);
        let mut backend = FakeBackend::new(0, &[0, 0, TIMER0_EVENT, 0]).with_routes(&[
            reset_route(),
            active,
            reset_route(),
        ]);

        let evidence = run_ieee802154_event_status_probe(&mut backend, config(1));

        assert_eq!(
            evidence.stop,
            Ieee802154EventStatusProbeStop::RouteNotQuiesced
        );
        assert_eq!(evidence.route_core1_with_events_enabled, 0x100);
        assert_eq!(evidence.event_enable_after, 0);
        assert!(!backend.operations.contains(&Operation::StartTimer0));
        assert!(!backend.operations.contains(&Operation::StartTimer1));
        assert!(!backend.operations.contains(&Operation::WriteTimer0));
        assert!(!backend.operations.contains(&Operation::WriteTimer1));
        backend.assert_consumed();
    }

    #[test]
    fn dual_latch_timeout_retains_the_last_bounded_sample() {
        let mut backend = FakeBackend::new(0, &[0, 0, 0, TIMER0_EVENT, TIMER0_EVENT, 0]);
        let evidence = run_ieee802154_event_status_probe(&mut backend, config(2));

        assert_eq!(
            evidence.stop,
            Ieee802154EventStatusProbeStop::DualLatchTimeout
        );
        assert_eq!(evidence.dual_latched_events, TIMER0_EVENT);
        assert_eq!(evidence.dual_observed_events, TIMER0_EVENT);
        assert_eq!(evidence.cleanup_pending_events, TIMER0_EVENT);
        assert_eq!(evidence.final_events, 0);
        backend.assert_consumed();
    }

    #[test]
    fn dual_wait_union_retains_a_transient_separately_from_the_last_sample() {
        let mut backend = FakeBackend::new(0, &[0, 0, TIMER0_EVENT, 0, 0, 0]);
        let evidence = run_ieee802154_event_status_probe(&mut backend, config(2));

        assert_eq!(
            evidence.stop,
            Ieee802154EventStatusProbeStop::DualLatchTimeout
        );
        assert_eq!(evidence.dual_observed_events, TIMER0_EVENT);
        assert_eq!(evidence.dual_latched_events, 0);
        assert_eq!(evidence.cleanup_pending_events, 0);
        assert_eq!(evidence.final_events, 0);
        assert!(backend.operations.contains(&Operation::WriteTimer0));
        assert!(!backend.operations.contains(&Operation::WriteTimer1));
        backend.assert_consumed();
    }

    #[test]
    fn inactive_timer_counters_are_distinguished_from_masked_status() {
        let mut backend = FakeBackend::new(0, &[0, 0, 0, 0, 0, 0]).with_inactive_timers();
        let evidence = run_ieee802154_event_status_probe(&mut backend, config(2));

        assert_eq!(
            evidence.stop,
            Ieee802154EventStatusProbeStop::TimerActivityTimeout
        );
        assert_eq!(
            evidence.timer0_value_before_start,
            evidence.timer0_value_min
        );
        assert_eq!(
            evidence.timer1_value_before_start,
            evidence.timer1_value_max
        );
        assert_eq!(evidence.final_events, 0);
        backend.assert_consumed();
    }

    #[test]
    fn either_inactive_timer_is_fail_closed_with_individual_extrema() {
        let mut timer0_inactive = FakeBackend::new(0, &[0, 0, 0, 0, 0, 0]).with_inactive_timer0();
        let evidence = run_ieee802154_event_status_probe(&mut timer0_inactive, config(2));
        assert_eq!(
            evidence.stop,
            Ieee802154EventStatusProbeStop::TimerActivityTimeout
        );
        assert_eq!(evidence.timer0_value_min, evidence.timer0_value_max);
        assert!(evidence.timer1_value_min < evidence.timer1_value_max);
        timer0_inactive.assert_consumed();

        let mut timer1_inactive = FakeBackend::new(0, &[0, 0, 0, 0, 0, 0]).with_inactive_timer1();
        let evidence = run_ieee802154_event_status_probe(&mut timer1_inactive, config(2));
        assert_eq!(
            evidence.stop,
            Ieee802154EventStatusProbeStop::TimerActivityTimeout
        );
        assert!(evidence.timer0_value_min < evidence.timer0_value_max);
        assert_eq!(evidence.timer1_value_min, evidence.timer1_value_max);
        timer1_inactive.assert_consumed();
    }

    #[test]
    fn first_selected_write_mismatch_is_fail_closed() {
        let mut backend = FakeBackend::new(0, &[0, 0, TIMER_EVENTS, TIMER_EVENTS, TIMER_EVENTS, 0]);
        let evidence = run_ieee802154_event_status_probe(&mut backend, config(1));

        assert_eq!(
            evidence.stop,
            Ieee802154EventStatusProbeStop::SelectiveAcknowledgeMismatch
        );
        assert_eq!(evidence.after_timer0_ack_events, TIMER_EVENTS);
        assert_eq!(evidence.final_events, 0);
        backend.assert_consumed();
    }

    #[test]
    fn second_selected_write_mismatch_is_fail_closed() {
        let mut backend = FakeBackend::new(
            0,
            &[
                0,
                0,
                TIMER_EVENTS,
                TIMER1_EVENT,
                TIMER1_EVENT,
                TIMER1_EVENT,
                0,
            ],
        );
        let evidence = run_ieee802154_event_status_probe(&mut backend, config(1));

        assert_eq!(
            evidence.stop,
            Ieee802154EventStatusProbeStop::SelectiveAcknowledgeMismatch
        );
        assert_eq!(evidence.after_timer1_ack_events, TIMER1_EVENT);
        assert_eq!(evidence.final_events, 0);
        backend.assert_consumed();
    }

    #[test]
    fn distinct_first_latch_timeout_retains_the_last_sample() {
        let mut backend = FakeBackend::new(0, &[0, 0, TIMER_EVENTS, TIMER1_EVENT, 0, 0, 0, 0, 0]);
        let evidence = run_ieee802154_event_status_probe(&mut backend, config(2));

        assert_eq!(
            evidence.stop,
            Ieee802154EventStatusProbeStop::DistinctFirstLatchTimeout
        );
        assert_eq!(evidence.distinct_snapshot_events, 0);
        assert_eq!(evidence.final_events, 0);
        backend.assert_consumed();
    }

    #[test]
    fn distinct_second_latch_timeout_retains_timer_zero() {
        let mut backend = FakeBackend::new(
            0,
            &[
                0,
                0,
                TIMER_EVENTS,
                TIMER1_EVENT,
                0,
                TIMER0_EVENT,
                TIMER0_EVENT,
                TIMER0_EVENT,
                TIMER0_EVENT,
                0,
            ],
        );
        let evidence = run_ieee802154_event_status_probe(&mut backend, config(2));

        assert_eq!(
            evidence.stop,
            Ieee802154EventStatusProbeStop::DistinctSecondLatchTimeout
        );
        assert_eq!(evidence.distinct_before_ack_events, TIMER0_EVENT);
        assert_eq!(evidence.final_events, 0);
        backend.assert_consumed();
    }

    #[test]
    fn distinct_selected_write_mismatch_is_fail_closed() {
        let mut backend = FakeBackend::new(
            0,
            &[
                0,
                0,
                TIMER_EVENTS,
                TIMER1_EVENT,
                0,
                TIMER0_EVENT,
                TIMER_EVENTS,
                TIMER_EVENTS,
                TIMER_EVENTS,
                0,
            ],
        );
        let evidence = run_ieee802154_event_status_probe(&mut backend, config(1));

        assert_eq!(
            evidence.stop,
            Ieee802154EventStatusProbeStop::SelectiveAcknowledgeMismatch
        );
        assert_eq!(evidence.distinct_after_ack_events, TIMER_EVENTS);
        assert_eq!(evidence.final_events, 0);
        backend.assert_consumed();
    }

    #[test]
    fn failed_mask_restore_skips_raw_cleanup_and_overrides_the_probe_stop() {
        let mut backend =
            FakeBackend::new(0, &[0, 0, 0, 0, 0]).with_disabled_writeback(TIMER_EVENTS);
        let evidence = run_ieee802154_event_status_probe(&mut backend, config(1));

        assert_eq!(
            evidence.stop,
            Ieee802154EventStatusProbeStop::CleanupNotClear
        );
        assert_eq!(evidence.event_enable_after, TIMER_EVENTS);
        assert!(!backend.operations.contains(&Operation::WriteTimer0));
        assert!(!backend.operations.contains(&Operation::WriteTimer1));
        backend.assert_consumed();
    }
}
