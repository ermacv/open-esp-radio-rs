use core::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll},
};

use crate::{
    atomic_once::compare_exchange_once_relaxed,
    channel::{BoundedChannel, Receive, TrySendError},
    context::RadioContextGuard,
    queue::WakerCell,
};

/// Synthetic event identity used while the single radio owner handles an
/// application command.
pub const RADIO_COMMAND_CONTEXT_EVENT: u32 = u32::MAX - 32;

static COMMAND_BUDGET_SELF_WAKES: AtomicUsize = AtomicUsize::new(0);

/// Number of explicit owner self-wakes caused by a still-nonempty command
/// queue after consuming its bounded per-poll budget.
pub fn command_budget_self_wakes() -> usize {
    COMMAND_BUDGET_SELF_WAKES.load(Ordering::Acquire)
}

/// Fixed-capacity ownership channel for application-to-radio commands.
///
/// Producers never call the vendor API and never wait for the radio owner.
/// Queue saturation is explicit and leaves ownership with the producer.
pub struct RadioCommandQueue<C, const N: usize> {
    channel: BoundedChannel<C, N>,
    submitted: AtomicUsize,
    rejected: AtomicUsize,
    high_water: AtomicUsize,
    capacity_waker: WakerCell,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RadioCommandSnapshot {
    pub submitted: usize,
    pub rejected: usize,
    pub queued: usize,
    pub high_water: usize,
    pub capacity: usize,
}

fn record_high_water(counter: &AtomicUsize, value: usize) {
    let observed = counter.load(Ordering::Relaxed);
    if value > observed {
        // One failed diagnostic CAS is acceptable; retrying here would break
        // the fixed-cost command-producer contract.
        let _ = compare_exchange_once_relaxed(counter, observed, value);
    }
}

impl<C, const N: usize> RadioCommandQueue<C, N> {
    pub const fn new() -> Self {
        Self {
            channel: BoundedChannel::new(),
            submitted: AtomicUsize::new(0),
            rejected: AtomicUsize::new(0),
            high_water: AtomicUsize::new(0),
            capacity_waker: WakerCell::new(),
        }
    }

    // Keep an instantiated claim visible in final disassembly. The strict
    // audit verifies that its code generation contains no LR/SC retry cycle;
    // an unused generic instantiation may still be removed completely.
    #[inline(never)]
    pub fn try_submit(&self, command: C) -> Result<(), TrySendError<C>> {
        match self.channel.try_send(command) {
            Ok(()) => {
                self.submitted.fetch_add(1, Ordering::Relaxed);
                record_high_water(&self.high_water, self.channel.len());
                Ok(())
            }
            Err(error) => {
                self.rejected.fetch_add(1, Ordering::Relaxed);
                Err(error)
            }
        }
    }

    pub fn try_receive(&self) -> Option<C> {
        self.channel
            .try_receive()
            .inspect(|_| self.capacity_waker.wake())
    }

    pub fn receive(&self) -> Receive<'_, C, N> {
        self.channel.receive()
    }

    pub fn rejected(&self) -> usize {
        self.rejected.load(Ordering::Acquire)
    }

    pub fn len(&self) -> usize {
        self.channel.len()
    }

    pub fn is_empty(&self) -> bool {
        self.channel.is_empty()
    }

    pub fn snapshot(&self) -> RadioCommandSnapshot {
        RadioCommandSnapshot {
            submitted: self.submitted.load(Ordering::Acquire),
            rejected: self.rejected.load(Ordering::Acquire),
            queued: self.channel.len(),
            high_water: self.high_water.load(Ordering::Acquire),
            capacity: N,
        }
    }

    /// Wait asynchronously until a bounded command slot may be available.
    ///
    /// A producer still has to use [`try_submit`](Self::try_submit) after this
    /// returns because another producer can win the slot. The future never
    /// spins or sleeps; command consumption wakes it.
    pub fn ready(&self) -> RadioCommandReady<'_, C, N> {
        RadioCommandReady { queue: self }
    }

    /// Submit with Rust-async backpressure while retaining command ownership.
    pub async fn submit(&self, mut command: C) {
        loop {
            match self.try_submit(command) {
                Ok(()) => return,
                Err(error) => command = error.0,
            }
            self.ready().await;
        }
    }
}

impl<C, const N: usize> Default for RadioCommandQueue<C, N> {
    fn default() -> Self {
        Self::new()
    }
}

pub struct RadioCommandReady<'a, C, const N: usize> {
    queue: &'a RadioCommandQueue<C, N>,
}

impl<C, const N: usize> Future for RadioCommandReady<'_, C, N> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.queue.capacity_waker.register(cx.waker());
        if self.queue.len() < N {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

/// Run-to-completion handler owned exclusively by [`RadioOwnerFuture`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PendingCommandAction {
    Retry,
    Cancel,
}

pub trait RadioCommandHandler<C> {
    type Error;

    fn handle(&mut self, command: C) -> Result<(), Self::Error>;

    /// Advance one fixed-capacity batch of event-driven internal continuations.
    ///
    /// Implementations may register `cx.waker()` and return without work. They
    /// must not inspect a device status in a retry loop or manufacture their
    /// own wakeups merely to poll again.
    fn poll_internal(&mut self, _cx: &mut Context<'_>) -> bool {
        false
    }

    /// Recover an owned command from a transient handler failure.
    ///
    /// The default keeps the existing fail-fast contract. Specialized
    /// handlers may return the command and arrange an event-driven readiness
    /// edge through `poll_retry_ready`.
    fn recover_retry(&mut self, error: Self::Error) -> Result<C, Self::Error> {
        Err(error)
    }

    fn poll_retry_ready(&mut self, _cx: &mut Context<'_>) -> Poll<PendingCommandAction> {
        Poll::Ready(PendingCommandAction::Retry)
    }

    /// Cancel one command retained after a transient failure.
    ///
    /// This runs under the same logical radio identity as `handle`. The
    /// default simply drops the owned command; specialized handlers may first
    /// revoke state that made the command admissible.
    fn cancel_retry(&mut self, _command: C) {}
}

/// Polls application commands and the Wi-Fi runtime on the same executor
/// stack. This is the only application-facing location that enters logical
/// Wi-Fi task identity.
pub struct RadioOwnerFuture<'a, W, C, H, const N: usize> {
    wifi: W,
    commands: &'a RadioCommandQueue<C, N>,
    handler: H,
    command_budget: usize,
    pending_command: Option<C>,
}

impl<'a, W, C, H, const N: usize> RadioOwnerFuture<'a, W, C, H, N> {
    pub fn new(
        wifi: W,
        commands: &'a RadioCommandQueue<C, N>,
        handler: H,
        command_budget: usize,
    ) -> Self {
        assert!(command_budget > 0);
        Self {
            wifi,
            commands,
            handler,
            command_budget,
            pending_command: None,
        }
    }

    pub fn handler(&self) -> &H {
        &self.handler
    }

    pub fn handler_mut(&mut self) -> &mut H {
        &mut self.handler
    }

    pub fn wifi(&self) -> &W {
        &self.wifi
    }
}

impl<W, C, H, const N: usize> Future for RadioOwnerFuture<'_, W, C, H, N>
where
    W: Future + Unpin,
    C: Unpin,
    H: RadioCommandHandler<C> + Unpin,
{
    type Output = Result<W::Output, H::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut received = 0;
        let mut retry_blocked = false;

        {
            let _radio_context = RadioContextGuard::enter(RADIO_COMMAND_CONTEXT_EVENT);
            self.handler.poll_internal(cx);
        }

        if self.pending_command.is_some() {
            match self.handler.poll_retry_ready(cx) {
                Poll::Pending => retry_blocked = true,
                Poll::Ready(PendingCommandAction::Cancel) => {
                    let command = self
                        .pending_command
                        .take()
                        .expect("pending command checked above");
                    {
                        let _radio_context = RadioContextGuard::enter(RADIO_COMMAND_CONTEXT_EVENT);
                        self.handler.cancel_retry(command);
                    }
                    received = 1;
                }
                Poll::Ready(PendingCommandAction::Retry) => {
                    let command = self
                        .pending_command
                        .take()
                        .expect("pending command checked above");
                    let result = {
                        let _radio_context = RadioContextGuard::enter(RADIO_COMMAND_CONTEXT_EVENT);
                        self.handler.handle(command)
                    };
                    if let Err(error) = result {
                        match self.handler.recover_retry(error) {
                            Ok(command) => {
                                self.pending_command = Some(command);
                                retry_blocked = true;
                            }
                            Err(error) => return Poll::Ready(Err(error)),
                        }
                    } else {
                        received = 1;
                    }
                }
            }
        }

        let mut receive = self.commands.receive();

        while !retry_blocked && received < self.command_budget {
            let command = if received == 0 {
                match Pin::new(&mut receive).poll(cx) {
                    Poll::Ready(command) => {
                        self.commands.capacity_waker.wake();
                        command
                    }
                    Poll::Pending => break,
                }
            } else {
                match self.commands.try_receive() {
                    Some(command) => command,
                    None => break,
                }
            };

            let result = {
                let _radio_context = RadioContextGuard::enter(RADIO_COMMAND_CONTEXT_EVENT);
                self.handler.handle(command)
            };
            if let Err(error) = result {
                match self.handler.recover_retry(error) {
                    Ok(command) => {
                        self.pending_command = Some(command);
                        break;
                    }
                    Err(error) => return Poll::Ready(Err(error)),
                }
            }
            received += 1;
        }

        if received == self.command_budget && !self.commands.is_empty() {
            COMMAND_BUDGET_SELF_WAKES.fetch_add(1, Ordering::Relaxed);
            cx.waker().wake_by_ref();
        }
        match Pin::new(&mut self.wifi).poll(cx) {
            Poll::Ready(output) => Poll::Ready(Ok(output)),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use core::{
        future::{pending, Future},
        pin::Pin,
        task::{Context, Poll, Waker},
    };

    use super::{PendingCommandAction, RadioCommandHandler, RadioCommandQueue, RadioOwnerFuture};
    use crate::context::in_radio_context;

    #[derive(Default)]
    struct Handler {
        sum: u32,
        all_in_radio_context: bool,
        internal_polls: usize,
        internal_in_radio_context: bool,
    }

    #[derive(Default)]
    struct RetryHandler {
        attempts: usize,
        sum: u32,
        ready: bool,
        cancel: bool,
        cancelled: u32,
        cancel_in_radio_context: bool,
    }

    impl RadioCommandHandler<u32> for RetryHandler {
        type Error = (u8, u32);

        fn handle(&mut self, command: u32) -> Result<(), Self::Error> {
            self.attempts += 1;
            if self.attempts == 1 {
                Err((1, command))
            } else {
                self.sum += command;
                Ok(())
            }
        }

        fn recover_retry(&mut self, error: Self::Error) -> Result<u32, Self::Error> {
            if error.0 == 1 {
                Ok(error.1)
            } else {
                Err(error)
            }
        }

        fn poll_retry_ready(&mut self, _cx: &mut Context<'_>) -> Poll<PendingCommandAction> {
            if self.cancel {
                Poll::Ready(PendingCommandAction::Cancel)
            } else if self.ready {
                Poll::Ready(PendingCommandAction::Retry)
            } else {
                Poll::Pending
            }
        }

        fn cancel_retry(&mut self, command: u32) {
            self.cancelled += command;
            self.cancel_in_radio_context = in_radio_context();
        }
    }

    impl RadioCommandHandler<u32> for Handler {
        type Error = ();

        fn poll_internal(&mut self, _cx: &mut Context<'_>) -> bool {
            self.internal_polls += 1;
            self.internal_in_radio_context = in_radio_context();
            false
        }

        fn handle(&mut self, command: u32) -> Result<(), Self::Error> {
            self.sum += command;
            self.all_in_radio_context = in_radio_context();
            Ok(())
        }
    }

    #[test]
    fn commands_only_run_under_the_radio_owner() {
        let commands = RadioCommandQueue::<u32, 2>::new();
        commands.try_submit(2).unwrap();
        commands.try_submit(3).unwrap();
        let mut owner = RadioOwnerFuture::new(pending::<()>(), &commands, Handler::default(), 2);
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);

        assert_eq!(Pin::new(&mut owner).poll(&mut context), Poll::Pending);
        assert_eq!(owner.handler().sum, 5);
        assert!(owner.handler().all_in_radio_context);
        assert_eq!(owner.handler().internal_polls, 1);
        assert!(owner.handler().internal_in_radio_context);
        assert!(!in_radio_context());
        let snapshot = commands.snapshot();
        assert_eq!(snapshot.submitted, 2);
        assert_eq!(snapshot.rejected, 0);
        assert_eq!(snapshot.queued, 0);
        assert_eq!(snapshot.high_water, 2);
        assert_eq!(snapshot.capacity, 2);
    }

    #[test]
    fn async_submit_retains_command_until_capacity_is_woken() {
        let commands = RadioCommandQueue::<u32, 1>::new();
        commands.try_submit(7).unwrap();
        let mut submit = core::pin::pin!(commands.submit(9));
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);

        assert_eq!(submit.as_mut().poll(&mut context), Poll::Pending);
        assert_eq!(commands.try_receive(), Some(7));
        assert_eq!(submit.as_mut().poll(&mut context), Poll::Ready(()));
        assert_eq!(commands.try_receive(), Some(9));
    }

    #[test]
    fn owner_retains_retryable_command_until_event_readiness() {
        let commands = RadioCommandQueue::<u32, 1>::new();
        commands.try_submit(7).unwrap();
        let mut owner =
            RadioOwnerFuture::new(pending::<()>(), &commands, RetryHandler::default(), 1);
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);

        assert_eq!(Pin::new(&mut owner).poll(&mut context), Poll::Pending);
        assert_eq!(owner.handler().attempts, 1);
        assert_eq!(owner.handler().sum, 0);
        assert_eq!(Pin::new(&mut owner).poll(&mut context), Poll::Pending);
        assert_eq!(owner.handler().attempts, 1);

        owner.handler_mut().ready = true;
        assert_eq!(Pin::new(&mut owner).poll(&mut context), Poll::Pending);
        assert_eq!(owner.handler().attempts, 2);
        assert_eq!(owner.handler().sum, 7);
    }

    #[test]
    fn owner_cancels_pending_command_and_continues_the_queue() {
        let commands = RadioCommandQueue::<u32, 2>::new();
        commands.try_submit(7).unwrap();
        commands.try_submit(9).unwrap();
        let mut owner =
            RadioOwnerFuture::new(pending::<()>(), &commands, RetryHandler::default(), 2);
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);

        assert_eq!(Pin::new(&mut owner).poll(&mut context), Poll::Pending);
        assert_eq!(owner.handler().attempts, 1);
        assert_eq!(owner.handler().sum, 0);

        owner.handler_mut().cancel = true;
        assert_eq!(Pin::new(&mut owner).poll(&mut context), Poll::Pending);
        assert_eq!(owner.handler().attempts, 2);
        assert_eq!(owner.handler().cancelled, 7);
        assert!(owner.handler().cancel_in_radio_context);
        assert_eq!(owner.handler().sum, 9);
        assert_eq!(commands.len(), 0);
    }
}
