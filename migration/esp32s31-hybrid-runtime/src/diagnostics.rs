use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockingCall {
    None = 0,
    QueueReceive = 1,
    QueueSendWithTimeout = 2,
    SemaphoreTake = 3,
    MutexLock = 4,
    EventGroupWait = 5,
    TaskDelay = 6,
    EventPostWithTimeout = 7,
    NvsCommit = 8,
    Sleep = 9,
    EtsDelayUs = 10,
    UnknownTaskCreate = 11,
    UnsupportedQueue = 12,
    SemaphorePoolExhausted = 13,
    TimerPoolExhausted = 14,
    TimerWithoutClock = 15,
    MutexPoolExhausted = 16,
    Net80211TimerRejected = 17,
    EsfBufferRejected = 18,
    ManagementTxRejected = 19,
    TimerSetCallbackRejected = 20,
    TimerArmRejected = 21,
    TimerDisarmRejected = 22,
    TimerDoneRejected = 23,
}

/// Allocation-free recorder for forbidden calls reached from a supposedly
/// run-to-completion vendor handler.
pub struct BlockingCallProbe {
    call: AtomicU32,
    event: AtomicU32,
    argument: AtomicUsize,
}

impl BlockingCallProbe {
    pub const fn new() -> Self {
        Self {
            call: AtomicU32::new(BlockingCall::None as u32),
            event: AtomicU32::new(u32::MAX),
            argument: AtomicUsize::new(0),
        }
    }

    pub fn record(&self, call: BlockingCall, event: u32, argument: usize) {
        self.argument.store(argument, Ordering::Relaxed);
        self.event.store(event, Ordering::Relaxed);
        self.call.store(call as u32, Ordering::Release);
    }

    pub fn raw(&self) -> (u32, u32, usize) {
        (
            self.call.load(Ordering::Acquire),
            self.event.load(Ordering::Relaxed),
            self.argument.load(Ordering::Relaxed),
        )
    }

    pub fn clear(&self) {
        self.call
            .store(BlockingCall::None as u32, Ordering::Release);
    }
}

impl Default for BlockingCallProbe {
    fn default() -> Self {
        Self::new()
    }
}
