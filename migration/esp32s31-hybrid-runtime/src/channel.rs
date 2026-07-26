use core::{
    cell::UnsafeCell,
    future::Future,
    mem::MaybeUninit,
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll},
};

use crate::{
    atomic_once::{compare_exchange_once_acquire, compare_exchange_once_relaxed},
    queue::WakerCell,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TrySendError<T>(pub T);

struct Slot<T> {
    sequence: AtomicUsize,
    value: UnsafeCell<MaybeUninit<T>>,
}

impl<T> Slot<T> {
    const fn new(sequence: usize) -> Self {
        Self {
            sequence: AtomicUsize::new(sequence),
            value: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }
}

// The sequence-number protocol transfers exclusive ownership of `value`
// between producers and the consumer.
unsafe impl<T: Send> Sync for Slot<T> {}

/// Allocation-free bounded multi-producer channel with one async consumer.
///
/// `try_send` and `try_receive` make one atomic claim attempt. Contention is
/// reported like capacity exhaustion instead of retrying, so the producer is
/// wait-free and safe to call from an interrupt. Waking the consumer also
/// never spins on its waker lock.
pub struct BoundedChannel<T, const N: usize> {
    enqueue: AtomicUsize,
    dequeue: AtomicUsize,
    slots: [Slot<T>; N],
    waker: WakerCell,
}

impl<T, const N: usize> BoundedChannel<T, N> {
    pub const fn new() -> Self {
        assert!(N > 0);

        let mut slots = [const { Slot::new(0) }; N];
        let mut index = 0;
        while index < N {
            slots[index] = Slot::new(index);
            index += 1;
        }

        Self {
            enqueue: AtomicUsize::new(0),
            dequeue: AtomicUsize::new(0),
            slots,
            waker: WakerCell::new(),
        }
    }

    /// Attempt to transfer ownership into the channel without waiting.
    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        // The ordinary sequence-number protocol needs at least two slots.
        // Use an explicit four-state handoff for the useful one-slot case:
        // 0 empty, 1 producer owns it, 2 full, 3 consumer owns it.
        if N == 1 {
            let slot = &self.slots[0];
            if compare_exchange_once_acquire(&slot.sequence, 0, 1).is_err() {
                return Err(TrySendError(value));
            }
            unsafe { (*slot.value.get()).write(value) };
            self.enqueue.fetch_add(1, Ordering::Release);
            slot.sequence.store(2, Ordering::Release);
            self.waker.wake();
            return Ok(());
        }

        let position = self.enqueue.load(Ordering::Relaxed);
        let slot = &self.slots[position % N];
        let sequence = slot.sequence.load(Ordering::Acquire);
        if sequence.wrapping_sub(position) as isize != 0
            || compare_exchange_once_relaxed(&self.enqueue, position, position.wrapping_add(1))
                .is_err()
        {
            return Err(TrySendError(value));
        }

        unsafe { (*slot.value.get()).write(value) };
        slot.sequence
            .store(position.wrapping_add(1), Ordering::Release);
        self.waker.wake();
        Ok(())
    }

    pub fn try_receive(&self) -> Option<T> {
        if N == 1 {
            let slot = &self.slots[0];
            if slot.sequence.load(Ordering::Acquire) != 2 {
                return None;
            }
            // The channel has exactly one consumer, so observing the full
            // state gives it exclusive ownership without a retrying CAS.
            slot.sequence.store(3, Ordering::Relaxed);
            let value = unsafe { (*slot.value.get()).assume_init_read() };
            self.dequeue.fetch_add(1, Ordering::Release);
            slot.sequence.store(0, Ordering::Release);
            return Some(value);
        }

        let position = self.dequeue.load(Ordering::Relaxed);
        let slot = &self.slots[position % N];
        let expected = position.wrapping_add(1);
        let sequence = slot.sequence.load(Ordering::Acquire);
        if sequence.wrapping_sub(expected) as isize != 0 {
            return None;
        }

        self.dequeue
            .store(position.wrapping_add(1), Ordering::Relaxed);
        let value = unsafe { (*slot.value.get()).assume_init_read() };
        slot.sequence
            .store(position.wrapping_add(N), Ordering::Release);
        Some(value)
    }

    pub fn receive(&self) -> Receive<'_, T, N> {
        Receive { channel: self }
    }

    pub fn is_empty(&self) -> bool {
        self.dequeue.load(Ordering::Acquire) == self.enqueue.load(Ordering::Acquire)
    }

    pub fn len(&self) -> usize {
        self.enqueue
            .load(Ordering::Acquire)
            .wrapping_sub(self.dequeue.load(Ordering::Acquire))
    }
}

impl<T, const N: usize> Default for BoundedChannel<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const N: usize> Drop for BoundedChannel<T, N> {
    fn drop(&mut self) {
        // Exclusive `&mut self` guarantees producers and consumers no longer
        // access the channel. At most N initialized values can exist, so the
        // destructor has a manifest static bound and never waits for state.
        for _ in 0..N {
            if self.try_receive().is_none() {
                break;
            }
        }
    }
}

pub struct Receive<'a, T, const N: usize> {
    channel: &'a BoundedChannel<T, N>,
}

impl<T, const N: usize> Future for Receive<'_, T, N> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Register first so a producer racing the empty check either sees the
        // waker or leaves the WakerCell pending bit set.
        self.channel.waker.register(cx.waker());
        self.channel
            .try_receive()
            .map_or(Poll::Pending, Poll::Ready)
    }
}

#[cfg(test)]
mod tests {
    use core::{
        future::Future,
        pin::Pin,
        task::{Context, Poll, Waker},
    };

    use super::BoundedChannel;

    #[test]
    fn bounded_channel_transfers_owned_values() {
        let channel = BoundedChannel::<[u8; 4], 2>::new();
        channel.try_send(*b"one!").unwrap();
        channel.try_send(*b"two!").unwrap();
        assert_eq!(channel.try_send(*b"full").unwrap_err().0, *b"full");
        assert_eq!(channel.try_receive(), Some(*b"one!"));
        assert_eq!(channel.try_receive(), Some(*b"two!"));
        assert_eq!(channel.try_receive(), None);
    }

    #[test]
    fn receive_future_observes_interrupt_style_send() {
        let channel = BoundedChannel::<u32, 1>::new();
        let mut receive = channel.receive();
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        assert_eq!(Pin::new(&mut receive).poll(&mut context), Poll::Pending);
        channel.try_send(7).unwrap();
        assert_eq!(channel.try_send(8).unwrap_err().0, 8);
        assert_eq!(Pin::new(&mut receive).poll(&mut context), Poll::Ready(7));
        channel.try_send(9).unwrap();
        assert_eq!(channel.try_receive(), Some(9));
    }
}
