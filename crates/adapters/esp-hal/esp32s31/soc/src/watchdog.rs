//! Finite system-deadline protection, independent of any peripheral client.
//!
//! No periodic feed, implicit renewal, or Drop-based disarm exists. A caller
//! completes a lease only after its own physical postconditions hold. This is
//! engineering protection in the awake XTAL clock domain, not a qualified
//! interrupt-to-reset or reset-to-RF-off bound. Sleep and external clock changes
//! while armed are unsupported. No other TIMG1/Wdt constructor may be used.

/// Explicit caller-selected hardware timeout. There is deliberately no default.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeadlineBudget(core::num::NonZeroU32);

impl DeadlineBudget {
    /// Microseconds from arming, including the caller's restoration work.
    pub const fn from_micros(micros: core::num::NonZeroU32) -> Self {
        Self(micros)
    }

    pub const fn as_micros(self) -> u32 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeadlineError {
    AlreadyArmed,
    TimelineExhausted,
    StaleCompletion,
    Expired,
}

#[cfg(any(test, feature = "esp32s31"))]
trait Backend {
    fn now_micros(&self) -> u64;
    fn arm(&mut self, micros: u32);
    fn disarm(&mut self);
}

#[cfg(any(test, feature = "esp32s31"))]
struct Controller<B> {
    backend: B,
    generation: u64,
    armed: Option<(u64, u64)>,
}

#[cfg(any(test, feature = "esp32s31"))]
impl<B: Backend> Controller<B> {
    const fn new(backend: B) -> Self {
        Self {
            backend,
            generation: 0,
            armed: None,
        }
    }

    fn arm(&mut self, budget: DeadlineBudget) -> Result<u64, DeadlineError> {
        if self.armed.is_some() {
            return Err(DeadlineError::AlreadyArmed);
        }
        let generation = self
            .generation
            .checked_add(1)
            .ok_or(DeadlineError::TimelineExhausted)?;
        let deadline = self
            .backend
            .now_micros()
            .checked_add(u64::from(budget.as_micros()))
            .ok_or(DeadlineError::TimelineExhausted)?;
        self.generation = generation;
        self.armed = Some((generation, deadline));
        self.backend.arm(budget.as_micros());
        Ok(generation)
    }

    fn complete(&mut self, generation: u64) -> Result<(), DeadlineError> {
        let (active, deadline) = self.armed.ok_or(DeadlineError::StaleCompletion)?;
        if generation != active {
            return Err(DeadlineError::StaleCompletion);
        }
        if self.backend.now_micros() >= deadline {
            return Err(DeadlineError::Expired);
        }
        self.backend.disarm();
        self.armed = None;
        Ok(())
    }
}

#[cfg(feature = "esp32s31")]
mod target {
    use super::*;
    use core::{cell::RefCell, mem::ManuallyDrop};
    use critical_section::Mutex;
    use esp_hal::{
        peripherals::TIMG1,
        time::{Duration, Instant},
        timer::timg::{MwdtStage, TimerGroup},
    };

    struct Hardware(ManuallyDrop<TimerGroup<'static, TIMG1<'static>>>);
    impl Backend for Hardware {
        fn now_micros(&self) -> u64 {
            Instant::now().duration_since_epoch().as_micros()
        }
        fn arm(&mut self, micros: u32) {
            self.0
                .wdt
                .set_timeout(MwdtStage::Stage0, Duration::from_micros(u64::from(micros)));
            self.0.wdt.feed();
            self.0.wdt.enable();
        }
        fn disarm(&mut self) {
            self.0.wdt.disable();
        }
    }

    /// Exclusive TIMG1 owner. Share this service, never the peripheral itself.
    /// A critical section serializes finite programming only; expiry needs no
    /// ISR, executor, Host progress or access to this service.
    pub struct DeadlineWatchdog(Mutex<RefCell<Controller<Hardware>>>);

    impl DeadlineWatchdog {
        pub fn new(timg: TIMG1<'static>) -> Self {
            let mut group = TimerGroup::new(timg);
            group.wdt.disable();
            Self(Mutex::new(RefCell::new(Controller::new(Hardware(
                ManuallyDrop::new(group),
            )))))
        }

        /// Arm once. Reject an existing obligation without changing its timer.
        /// The pinned HAL selects XTAL for S31. The logical completion deadline
        /// is sampled before programming; physical expiry additionally includes
        /// programming/latch/reset latency, which requires hardware measurement.
        pub fn arm(&self, budget: DeadlineBudget) -> Result<DeadlineLease<'_>, DeadlineError> {
            let generation =
                critical_section::with(|cs| self.0.borrow(cs).borrow_mut().arm(budget))?;
            Ok(DeadlineLease {
                owner: self,
                generation,
            })
        }
    }

    /// Unique completion authority for one operation. Dropping or forgetting it
    /// leaves the watchdog armed and prevents any later operation from renewing
    /// the deadline. This token is bound to the service that issued it.
    #[must_use = "only proven restoration may complete the lease; dropping it leaves reset armed"]
    pub struct DeadlineLease<'a> {
        owner: &'a DeadlineWatchdog,
        generation: u64,
    }

    impl DeadlineLease<'_> {
        /// Disarm after the client's physical postconditions hold. Equality or
        /// lateness rejects completion and leaves the hardware armed.
        pub fn complete(self) -> Result<(), DeadlineError> {
            critical_section::with(|cs| {
                self.owner
                    .0
                    .borrow(cs)
                    .borrow_mut()
                    .complete(self.generation)
            })
        }
    }
}

#[cfg(feature = "esp32s31")]
pub use target::{DeadlineLease, DeadlineWatchdog};

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Model {
        now: u64,
        armed: bool,
        arms: u32,
        disarms: u32,
    }
    impl Backend for Model {
        fn now_micros(&self) -> u64 {
            self.now
        }
        fn arm(&mut self, _: u32) {
            self.armed = true;
            self.arms += 1;
        }
        fn disarm(&mut self) {
            self.armed = false;
            self.disarms += 1;
        }
    }
    fn budget() -> DeadlineBudget {
        DeadlineBudget::from_micros(core::num::NonZeroU32::new(100).unwrap())
    }
    #[test]
    fn abandoned_operation_cannot_be_renewed() {
        let mut c = Controller::new(Model::default());
        let _abandoned = c.arm(budget()).unwrap();
        c.backend.now = 20;
        assert_eq!(c.arm(budget()), Err(DeadlineError::AlreadyArmed));
        assert_eq!(c.backend.arms, 1);
        assert!(c.backend.armed);
        assert_eq!(c.backend.disarms, 0);
    }

    #[test]
    fn restoration_must_finish_before_original_not_refreshed_deadline() {
        let mut c = Controller::new(Model::default());
        let ticket = c.arm(budget()).unwrap();
        // PHY returned at 20, but the client has not completed restoration.
        c.backend.now = 20;
        assert!(c.backend.armed);
        assert_eq!(c.arm(budget()), Err(DeadlineError::AlreadyArmed));
        c.backend.now = 100;
        assert_eq!(c.complete(ticket), Err(DeadlineError::Expired));
        assert_eq!(c.backend.arms, 1);
        assert_eq!(c.backend.disarms, 0);
    }

    #[test]
    fn safe_rejection_can_complete_and_does_not_poison_next_transaction() {
        let mut c = Controller::new(Model::default());
        let rejected = c.arm(budget()).unwrap();
        c.backend.now = 1;
        c.complete(rejected).unwrap();
        assert!(!c.backend.armed);
        let next = c.arm(budget()).unwrap();
        assert_ne!(next, rejected);
        c.backend.now = 100;
        c.complete(next).unwrap();
        assert!(!c.backend.armed);
    }
    #[test]
    fn equality_and_late_completion_never_disable_reset() {
        for now in [100, 101] {
            let mut c = Controller::new(Model::default());
            let ticket = c.arm(budget()).unwrap();
            c.backend.now = now;
            assert_eq!(c.complete(ticket), Err(DeadlineError::Expired));
            assert!(c.backend.armed);
            assert_eq!(c.backend.disarms, 0);
        }
    }
    #[test]
    fn old_completion_cannot_disarm_a_new_obligation() {
        let mut c = Controller::new(Model::default());
        let first = c.arm(budget()).unwrap();
        c.backend.now = 99;
        c.complete(first).unwrap();
        let second = c.arm(budget()).unwrap();
        assert_eq!(c.complete(first), Err(DeadlineError::StaleCompletion));
        assert!(c.backend.armed);
        c.complete(second).unwrap();
        assert_eq!(c.backend.disarms, 2);
    }
    #[test]
    fn timeline_overflow_rejects_before_hardware_programming() {
        let mut c = Controller::new(Model {
            now: u64::MAX,
            ..Model::default()
        });
        assert_eq!(c.arm(budget()), Err(DeadlineError::TimelineExhausted));
        assert_eq!(c.backend.arms, 0);
    }
}
