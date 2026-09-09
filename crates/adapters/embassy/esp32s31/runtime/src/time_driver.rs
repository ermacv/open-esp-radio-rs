use core::{
    sync::atomic::{AtomicBool, Ordering},
    task::Waker,
};

use crate::timer_queue::WakeQueue;
use embassy_time_driver::Driver;
use esp_hal::{
    Blocking,
    interrupt::{InterruptHandler, Priority},
    time::{Duration, Instant},
    timer::{Error, OneShotTimer},
};
use esp_sync::NonReentrantMutex;

/// ESP-HAL timer capability accepted by [`init`].
pub type Timer = OneShotTimer<'static, Blocking>;

struct State {
    timer: Option<Timer>,
    queue: WakeQueue,
    current_alarm: u64,
    #[cfg(feature = "timer-observation")]
    observation: crate::timer_observation::Recorder,
}

impl State {
    const fn new() -> Self {
        Self {
            timer: None,
            queue: WakeQueue::new(),
            current_alarm: u64::MAX,
            #[cfg(feature = "timer-observation")]
            observation: crate::timer_observation::Recorder::new(),
        }
    }

    fn arm_next_wakeup(&mut self, now: u64) {
        let next_deadline = self.queue.next_deadline();
        if next_deadline == self.current_alarm {
            return;
        }
        let timer = self
            .timer
            .as_mut()
            .expect("oer_esp32s31_embassy_runtime::init must run first");
        self.current_alarm = next_deadline;
        if next_deadline == u64::MAX {
            timer.stop();
            #[cfg(feature = "timer-observation")]
            self.observation.stop();
            return;
        }

        let mut timeout = Duration::from_micros(next_deadline.saturating_sub(now).max(1));
        #[cfg(feature = "timer-observation")]
        let program_start = self.observation.enabled().then(crate::time_driver::now);
        loop {
            match timer.schedule(timeout) {
                Ok(()) => break,
                Err(Error::InvalidTimeout) if timeout > Duration::from_micros(1) => {
                    timeout = timeout / 2;
                }
                Err(error) => panic!("failed to schedule Embassy timer: {error:?}"),
            }
        }
        #[cfg(feature = "timer-observation")]
        if let Some(start) = program_start {
            self.observation
                .arm(next_deadline, start, crate::time_driver::now());
        }
    }
}

struct EmbassyTimeDriver {
    state: NonReentrantMutex<State>,
}

#[used]
#[allow(
    unsafe_code,
    reason = "board linker owns this exported timer interrupt-state section"
)]
#[unsafe(link_section = ".critical.bss.embassy_time")]
static ESP32S31_EMBASSY_TIMER_FIRED: AtomicBool = AtomicBool::new(false);

impl EmbassyTimeDriver {
    const fn new() -> Self {
        Self {
            state: NonReentrantMutex::new(State::new()),
        }
    }

    #[inline(always)]
    fn acknowledge_interrupt(&self, #[cfg(feature = "timer-observation")] entered: u64) {
        self.state.with(|state| {
            let timer = state
                .timer
                .as_mut()
                .expect("Embassy timer interrupt fired before initialization");
            timer.clear_interrupt();
            state.current_alarm = u64::MAX;
            #[cfg(feature = "timer-observation")]
            state.observation.interrupt(entered, now());
        });
    }

    fn dispatch_expired(&self) {
        self.state.with(|state| {
            let now = now();
            state.queue.dispatch_expired(now);
            #[cfg(feature = "timer-observation")]
            if state.observation.enabled() {
                state.observation.dispatch(now, crate::time_driver::now());
            }
            state.arm_next_wakeup(now);
        });
    }
}

#[used]
#[allow(
    unsafe_code,
    reason = "Embassy requires one exported global time-driver instance"
)]
#[unsafe(link_section = ".critical.data.embassy_time")]
static ESP32S31_EMBASSY_TIME_DRIVER: EmbassyTimeDriver = EmbassyTimeDriver::new();

#[allow(
    unsafe_code,
    reason = "Embassy time ABI requires this unique global symbol"
)]
#[unsafe(no_mangle)]
fn _embassy_time_now() -> u64 {
    <EmbassyTimeDriver as Driver>::now(&ESP32S31_EMBASSY_TIME_DRIVER)
}

#[allow(
    unsafe_code,
    reason = "Embassy time ABI requires this unique global symbol"
)]
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
            #[cfg(feature = "timer-observation")]
            if state.observation.enabled() {
                state.observation.registration(at, now());
            }
            if let Some(now) = state.queue.schedule_wake(at, waker, now) {
                state.arm_next_wakeup(now);
            }
        });
    }
}

pub(crate) fn dispatch_pending() {
    if ESP32S31_EMBASSY_TIMER_FIRED.swap(false, Ordering::AcqRel) {
        ESP32S31_EMBASSY_TIME_DRIVER.dispatch_expired();
    }
}

#[esp_hal::ram]
extern "C" fn timer_interrupt() {
    ESP32S31_EMBASSY_TIME_DRIVER.acknowledge_interrupt(
        #[cfg(feature = "timer-observation")]
        now(),
    );
    ESP32S31_EMBASSY_TIMER_FIRED.store(true, Ordering::Release);
    crate::executor::mark_work::<0>();
}

/// Install the global Embassy time driver on the calling core.
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

#[cfg(feature = "timer-observation")]
pub(crate) fn begin_observation() -> bool {
    ESP32S31_EMBASSY_TIME_DRIVER
        .state
        .with(|state| state.timer.is_some() && state.observation.begin(now()))
}
#[cfg(feature = "timer-observation")]
pub(crate) fn finish_observation() -> crate::timer_observation::Report {
    ESP32S31_EMBASSY_TIME_DRIVER
        .state
        .with(|state| state.observation.finish(now()))
}
