use core::{
    sync::atomic::{AtomicBool, Ordering},
    task::Waker,
};

use embassy_time_driver::Driver;
use embassy_time_queue_utils::Queue;
use esp_hal::{
    Blocking,
    interrupt::{InterruptHandler, Priority},
    time::{Duration, Instant},
    timer::{Error, OneShotTimer},
};
use esp_sync::NonReentrantMutex;

/// Hardware timer type accepted by [`init`].
pub type Timer = OneShotTimer<'static, Blocking>;

struct State {
    timer: Option<Timer>,
    queue: Queue,
    next_wakeup: u64,
    current_alarm: u64,
}

impl State {
    const fn new() -> Self {
        Self {
            timer: None,
            queue: Queue::new(),
            next_wakeup: u64::MAX,
            current_alarm: u64::MAX,
        }
    }

    fn arm_next_wakeup(&mut self, now: u64) {
        if self.next_wakeup == self.current_alarm {
            return;
        }

        let timer = self
            .timer
            .as_mut()
            .expect("esp32s31_embassy_runtime::init must be called before using embassy-time");

        self.current_alarm = self.next_wakeup;

        if self.next_wakeup == u64::MAX {
            timer.stop();
            return;
        }

        // A deadline in the past still has to cause an asynchronous wake-up.
        // One microsecond is the smallest portable timeout exposed by esp-hal.
        let timeout_us = self.next_wakeup.saturating_sub(now).max(1);
        let mut timeout = Duration::from_micros(timeout_us);

        loop {
            match timer.schedule(timeout) {
                Ok(()) => break,
                Err(Error::InvalidTimeout) if timeout > Duration::from_micros(1) => {
                    // Waking early is valid. The interrupt handler will re-arm
                    // the timer if the real deadline has not been reached.
                    timeout = timeout / 2;
                }
                Err(error) => panic!("failed to schedule Embassy timer: {error:?}"),
            }
        }
    }
}

struct EmbassyTimeDriver {
    state: NonReentrantMutex<State>,
}

#[used]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".critical.bss.embassy_time")]
static ESP32S31_EMBASSY_TIMER_FIRED: AtomicBool = AtomicBool::new(false);

impl EmbassyTimeDriver {
    const fn new() -> Self {
        Self {
            state: NonReentrantMutex::new(State::new()),
        }
    }

    #[inline(always)]
    fn acknowledge_interrupt(&self) {
        self.state.with(|state| {
            let timer = state
                .timer
                .as_mut()
                .expect("Embassy timer interrupt fired before initialization");
            timer.clear_interrupt();
            state.current_alarm = u64::MAX;
        });
    }

    fn dispatch_expired(&self) {
        self.state.with(|state| {
            let now = now();
            state.next_wakeup = state.queue.next_expiration(now);
            state.arm_next_wakeup(now);
        });
    }
}

#[used]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".critical.data.embassy_time")]
static ESP32S31_EMBASSY_TIME_DRIVER: EmbassyTimeDriver = EmbassyTimeDriver::new();

// This is the expansion of `embassy_time_driver::time_driver_impl!`, kept
// local so the driver state can carry the SRAM section contract above.
#[unsafe(no_mangle)]
fn _embassy_time_now() -> u64 {
    <EmbassyTimeDriver as Driver>::now(&ESP32S31_EMBASSY_TIME_DRIVER)
}

#[unsafe(no_mangle)]
fn _embassy_time_schedule_wake(at: u64, waker: &Waker) {
    <EmbassyTimeDriver as Driver>::schedule_wake(&ESP32S31_EMBASSY_TIME_DRIVER, at, waker);
}

impl Driver for EmbassyTimeDriver {
    #[inline]
    fn now(&self) -> u64 {
        now()
    }

    fn schedule_wake(&self, at: u64, waker: &Waker) {
        self.state.with(|state| {
            if state.queue.schedule_wake(at, waker) {
                state.next_wakeup = state.next_wakeup.min(at);
                state.arm_next_wakeup(now());
            }
        });
    }
}

/// Processes an acknowledged hardware alarm from executor thread mode.
///
/// The timer queue owns arbitrary task [`Waker`] values. Calling them here,
/// rather than in the hardware ISR, means PSRAM-resident Embassy executor
/// vtables are never part of the interrupt call closure.
pub(crate) fn dispatch_pending() {
    if ESP32S31_EMBASSY_TIMER_FIRED.swap(false, Ordering::AcqRel) {
        ESP32S31_EMBASSY_TIME_DRIVER.dispatch_expired();
    }
}

#[esp_hal::ram]
#[unsafe(export_name = "esp32s31_embassy_timer_interrupt")]
extern "C" fn timer_interrupt() {
    ESP32S31_EMBASSY_TIME_DRIVER.acknowledge_interrupt();
    ESP32S31_EMBASSY_TIMER_FIRED.store(true, Ordering::Release);
    // The global time driver is owned by Executor<0>. The hardware interrupt
    // itself wakes a sleeping local hart; this flag closes the race where the
    // interrupt arrives immediately before Executor<0> enters WFI.
    crate::executor::mark_work::<0>();
}

/// Initializes the global Embassy time driver with a hardware one-shot timer.
///
/// This function must be called exactly once, before polling tasks which use
/// `embassy-time`. The timer interrupt is bound to the core which calls this
/// function. The timer must be initialized on the same core that runs
/// [`crate::Executor<0>`].
pub fn init(mut timer: Timer) {
    timer.stop();
    timer.unlisten();
    timer.clear_interrupt();
    timer.set_interrupt_handler(InterruptHandler::new(timer_interrupt, Priority::Priority1));
    timer.listen();

    ESP32S31_EMBASSY_TIME_DRIVER.state.with(|state| {
        assert!(
            state.timer.is_none(),
            "Embassy time driver already initialized"
        );
        state.timer = Some(timer);
    });
}

#[inline]
fn now() -> u64 {
    Instant::now().duration_since_epoch().as_micros()
}
