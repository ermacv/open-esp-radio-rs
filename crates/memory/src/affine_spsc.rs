//! Audited bounded SPSC ownership transfer.
//!
//! The queue moves affine values between one producer and one consumer. It is
//! intentionally executor-agnostic: higher layers may wrap its non-blocking
//! endpoints in their own wake and telemetry policy without reopening unsafe
//! slot access.

use core::{
    cell::{Cell, UnsafeCell},
    marker::PhantomData,
    mem::MaybeUninit,
    sync::atomic::{AtomicU8, AtomicUsize, Ordering},
};

const PRODUCER_ACTIVE: u8 = 1 << 0;
const CONSUMER_ACTIVE: u8 = 1 << 1;
const BOTH_ENDPOINTS_ACTIVE: u8 = PRODUCER_ACTIVE | CONSUMER_ACTIVE;

#[repr(align(64))]
struct CacheLine<T>(T);

/// Full value returned when a bounded SPSC publication finds no free slot.
#[derive(Debug)]
pub struct AffineSpscTrySendError<T>(pub T);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AffineSpscTryReceiveError {
    Empty,
}

/// Static storage for one bounded single-producer/single-consumer stream.
pub struct AffineSpscQueue<T, const DEPTH: usize> {
    values: [UnsafeCell<MaybeUninit<T>>; DEPTH],
    producer: CacheLine<AtomicUsize>,
    consumer: CacheLine<AtomicUsize>,
    active_endpoints: AtomicU8,
}

// SAFETY: each slot has exactly one producer before its Release publication
// and exactly one consumer after the matching Acquire load. The consumer's
// Release cursor prevents reuse until the affine value has moved out.
#[allow(
    unsafe_code,
    reason = "bounded SPSC cursor ownership serializes every value slot"
)]
unsafe impl<T: Send, const DEPTH: usize> Sync for AffineSpscQueue<T, DEPTH> {}

impl<T, const DEPTH: usize> AffineSpscQueue<T, DEPTH> {
    pub const fn new() -> Self {
        assert!(DEPTH != 0, "affine SPSC queue must not be empty");
        assert!(
            DEPTH <= usize::MAX / 2,
            "affine SPSC cursor domain must fit usize"
        );
        Self {
            values: [const { UnsafeCell::new(MaybeUninit::uninit()) }; DEPTH],
            producer: CacheLine(AtomicUsize::new(0)),
            consumer: CacheLine(AtomicUsize::new(0)),
            active_endpoints: AtomicU8::new(0),
        }
    }

    /// Acquire the only producer and consumer pair for one ownership epoch.
    pub fn split(
        &self,
    ) -> (
        AffineSpscSender<'_, T, DEPTH>,
        AffineSpscReceiver<'_, T, DEPTH>,
    ) {
        assert_eq!(
            self.len(),
            0,
            "affine SPSC queue must be drained before a new owner epoch"
        );
        assert_eq!(
            self.active_endpoints.compare_exchange(
                0,
                BOTH_ENDPOINTS_ACTIVE,
                Ordering::AcqRel,
                Ordering::Acquire,
            ),
            Ok(0),
            "affine SPSC queue already has an active producer or consumer"
        );
        (
            AffineSpscSender {
                queue: self,
                _not_sync: PhantomData,
            },
            AffineSpscReceiver {
                queue: self,
                _not_sync: PhantomData,
            },
        )
    }

    /// Reacquire the sole consumer beside a producer retained across a
    /// lifecycle boundary.
    ///
    /// The queue must be empty because the old consumer is the only owner
    /// allowed to drain its epoch before it releases the endpoint. Keeping the
    /// producer alive preserves the physical DMA owner's affine publication
    /// capability without manufacturing a second producer.
    fn resume_consumer(&self) -> AffineSpscReceiver<'_, T, DEPTH> {
        assert_eq!(
            self.len(),
            0,
            "affine SPSC queue must be drained before resuming its consumer"
        );
        assert_eq!(
            self.active_endpoints.compare_exchange(
                PRODUCER_ACTIVE,
                BOTH_ENDPOINTS_ACTIVE,
                Ordering::AcqRel,
                Ordering::Acquire,
            ),
            Ok(PRODUCER_ACTIVE),
            "affine SPSC consumer can only resume beside its retained producer"
        );
        AffineSpscReceiver {
            queue: self,
            _not_sync: PhantomData,
        }
    }

    fn release_endpoint(&self, endpoint: u8) {
        let previous = self.active_endpoints.fetch_and(!endpoint, Ordering::AcqRel);
        assert!(
            previous & endpoint != 0,
            "affine SPSC endpoint released more than once"
        );
    }

    fn len(&self) -> usize {
        let consumer = self.consumer.0.load(Ordering::Acquire);
        let producer = self.producer.0.load(Ordering::Acquire);
        Self::cursor_distance(consumer, producer).min(DEPTH)
    }

    const fn cursor_distance(consumer: usize, producer: usize) -> usize {
        if producer >= consumer {
            producer - consumer
        } else {
            producer + DEPTH * 2 - consumer
        }
    }

    const fn advance(cursor: usize) -> usize {
        if cursor + 1 == DEPTH * 2 {
            0
        } else {
            cursor + 1
        }
    }

    #[inline]
    fn try_send(&self, value: T) -> Result<(), AffineSpscTrySendError<T>> {
        let producer = self.producer.0.load(Ordering::Relaxed);
        let consumer = self.consumer.0.load(Ordering::Acquire);
        if Self::cursor_distance(consumer, producer) >= DEPTH {
            return Err(AffineSpscTrySendError(value));
        }
        // SAFETY: only the producer writes the slot at its private cursor, and
        // the Acquire consumer cursor proved the previous value moved out.
        #[allow(
            unsafe_code,
            reason = "the single producer owns this unpublished SPSC slot"
        )]
        unsafe {
            (*self.values[producer % DEPTH].get()).write(value);
        }
        self.producer
            .0
            .store(Self::advance(producer), Ordering::Release);
        Ok(())
    }

    #[inline]
    fn try_receive(&self) -> Result<T, AffineSpscTryReceiveError> {
        let consumer = self.consumer.0.load(Ordering::Relaxed);
        let producer = self.producer.0.load(Ordering::Acquire);
        if consumer == producer {
            return Err(AffineSpscTryReceiveError::Empty);
        }
        // SAFETY: the producer's Release cursor initialized this slot, and
        // only the consumer moves its affine value out before returning it.
        #[allow(
            unsafe_code,
            reason = "the single consumer owns this published SPSC slot"
        )]
        let value = unsafe { (*self.values[consumer % DEPTH].get()).assume_init_read() };
        self.consumer
            .0
            .store(Self::advance(consumer), Ordering::Release);
        Ok(value)
    }
}

impl<T, const DEPTH: usize> Default for AffineSpscQueue<T, DEPTH> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const DEPTH: usize> Drop for AffineSpscQueue<T, DEPTH> {
    fn drop(&mut self) {
        let mut consumer = *self.consumer.0.get_mut();
        let producer = *self.producer.0.get_mut();
        while consumer != producer {
            // SAFETY: exclusive queue ownership proves no endpoint exists;
            // every cursor-visible slot still contains one initialized value.
            #[allow(
                unsafe_code,
                reason = "queue drop releases every still-published affine value"
            )]
            unsafe {
                self.values[consumer % DEPTH].get_mut().assume_init_drop();
            }
            consumer = Self::advance(consumer);
        }
        *self.consumer.0.get_mut() = producer;
    }
}

/// The only producer endpoint for one [`AffineSpscQueue`] ownership epoch.
pub struct AffineSpscSender<'queue, T, const DEPTH: usize> {
    queue: &'queue AffineSpscQueue<T, DEPTH>,
    // Moving the endpoint is valid, but safe code must not share one endpoint
    // between concurrent producers and violate the SPSC cursor proof.
    _not_sync: PhantomData<Cell<()>>,
}

impl<'queue, T, const DEPTH: usize> AffineSpscSender<'queue, T, DEPTH> {
    #[inline]
    pub fn try_send(&self, value: T) -> Result<(), AffineSpscTrySendError<T>> {
        self.queue.try_send(value)
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn free_capacity(&self) -> usize {
        DEPTH.saturating_sub(self.len())
    }

    /// Reacquire the sole consumer after the preceding consumer drained and
    /// ended its lifecycle epoch while this producer remained active.
    pub fn resume_consumer(&self) -> AffineSpscReceiver<'queue, T, DEPTH> {
        self.queue.resume_consumer()
    }
}

impl<T, const DEPTH: usize> Drop for AffineSpscSender<'_, T, DEPTH> {
    fn drop(&mut self) {
        self.queue.release_endpoint(PRODUCER_ACTIVE);
    }
}

/// The only consumer endpoint for one [`AffineSpscQueue`] ownership epoch.
pub struct AffineSpscReceiver<'queue, T, const DEPTH: usize> {
    queue: &'queue AffineSpscQueue<T, DEPTH>,
    // Moving the endpoint is valid, but safe code must not share one endpoint
    // between concurrent consumers and violate the SPSC cursor proof.
    _not_sync: PhantomData<Cell<()>>,
}

impl<T, const DEPTH: usize> AffineSpscReceiver<'_, T, DEPTH> {
    #[inline]
    pub fn try_receive(&self) -> Result<T, AffineSpscTryReceiveError> {
        self.queue.try_receive()
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<T, const DEPTH: usize> Drop for AffineSpscReceiver<'_, T, DEPTH> {
    fn drop(&mut self) {
        self.queue.release_endpoint(CONSUMER_ACTIVE);
    }
}

#[cfg(test)]
mod tests;
