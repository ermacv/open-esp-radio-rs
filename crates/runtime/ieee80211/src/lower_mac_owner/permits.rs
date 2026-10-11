//! The transmit permits, their guards and their keeper, in one module so
//! that only a keeper makes a guard and takes its permit back. A guard
//! admitted into another keeper of the install than the one that made it
//! panics at admission.
//!
//! No caller makes a guard:
//!
//! ```compile_fail,E0451
//! # use oer_ieee80211_runtime::lower_mac_owner::QueuePermit;
//! fn forge<'q, 'id>() -> QueuePermit<'q, 'id> {
//!     QueuePermit { permit: None, queue: 3, access_category: todo!(), keeper: todo!() }
//! }
//! ```
//!
//! nor takes its permit out:
//!
//! ```compile_fail,E0624
//! # use oer_ieee80211_runtime::lower_mac_owner::{QueuePermit, TxQueueKind};
//! # use oer_radio_port::Permit;
//! fn take<'q, 'id>(grant: QueuePermit<'q, 'id>) -> Permit<'id, TxQueueKind> {
//!     grant.into_permit()
//! }
//! ```
//!
//! Their compiling twin reads the queue a grant names:
//!
//! ```
//! # use oer_ieee80211_lower_mac::QueueGrant;
//! # use oer_ieee80211_runtime::lower_mac_owner::QueuePermit;
//! fn queue_of(grant: &QueuePermit<'_, '_>) -> u8 {
//!     grant.queue()
//! }
//! ```

use core::{cell::RefCell, marker::PhantomData};

use oer_ieee80211_lower_mac::{Ieee80211Capabilities, QueueGrant, TxId};
use oer_ieee80211_mac::qos::WmmAccessCategory;
use oer_radio_port::{Mint, Permit, PermitPool};

/// The permit kind of one transmit queue: a queue holds one attempt in
/// flight, so each queue's pool holds one permit.
pub enum TxQueueKind {}

/// The permit kind of one transmit buffer.
pub enum TxBufferKind {}

/// What a guard gives its permit back to: the [`TxPermits`] that made it.
trait Keeper<'id> {
    fn release_queue(&self, queue: u8, permit: Permit<'id, TxQueueKind>);
    fn release_buffer(&self, permit: Permit<'id, TxBufferKind>);
}

/// The grant of the queue an attempt of one access category occupies,
/// which only [`TxPermits::queue`] makes, from the pool of the queue the
/// capabilities assign the access category. Dropped before a submission,
/// it gives the queue back; [`TxPermits::admit`] moves it in.
pub struct QueuePermit<'q, 'id> {
    permit: Option<Permit<'id, TxQueueKind>>,
    queue: u8,
    access_category: WmmAccessCategory,
    keeper: &'q dyn Keeper<'id>,
}

impl<'id> QueuePermit<'_, 'id> {
    fn into_permit(mut self) -> Permit<'id, TxQueueKind> {
        self.permit
            .take()
            .expect("a guard holds its permit until a submission or its drop")
    }
}

impl QueueGrant for QueuePermit<'_, '_> {
    fn queue(&self) -> u8 {
        self.queue
    }

    fn access_category(&self) -> WmmAccessCategory {
        self.access_category
    }
}

impl Drop for QueuePermit<'_, '_> {
    fn drop(&mut self) {
        if let Some(permit) = self.permit.take() {
            self.keeper.release_queue(self.queue, permit);
        }
    }
}

/// The permit of one transmit buffer, which only [`TxPermits::buffer`]
/// makes; its index names the buffer's memory. Dropped before a
/// submission, it gives the buffer back; [`TxPermits::admit`] moves it in.
pub struct BufferPermit<'q, 'id> {
    permit: Option<Permit<'id, TxBufferKind>>,
    keeper: &'q dyn Keeper<'id>,
}

impl<'id> BufferPermit<'_, 'id> {
    /// The buffer's index in its pool.
    pub fn index(&self) -> u8 {
        self.permit.as_ref().map_or(0, Permit::index)
    }

    fn into_permit(mut self) -> Permit<'id, TxBufferKind> {
        self.permit
            .take()
            .expect("a guard holds its permit until a submission or its drop")
    }
}

impl Drop for BufferPermit<'_, '_> {
    fn drop(&mut self) {
        if let Some(permit) = self.permit.take() {
            self.keeper.release_buffer(permit);
        }
    }
}

/// The pool of one transmit queue: it holds the queue's one permit, which
/// [`TxPermits::new`] mints and hands to [`QueuePool::of`]; it has no other
/// way in, so a queue never holds two attempts.
pub trait QueuePool<'id>: PermitPool<'id, TxQueueKind> {
    /// The pool of the queue whose one permit is `permit`.
    fn of(permit: Permit<'id, TxQueueKind>) -> Self;
}

/// The transmit permits of one install, which a backend keeps in its held
/// part: a pool per queue the capabilities `K` state, each holding the one
/// permit this value mints for it, the pool of buffers, and the attempts in
/// flight, indexed by their queue, so a queue holds at most one. A client
/// awaits a queue and a buffer, in each pool's order, never refused.
pub struct TxPermits<'id, K, Q, B, const N: usize> {
    queues: [Q; N],
    buffers: B,
    in_flight: RefCell<[Option<InFlight<'id>>; N]>,
    capabilities: PhantomData<fn() -> K>,
}

/// The permits of one attempt in flight.
struct InFlight<'id> {
    id: TxId,
    queue: Permit<'id, TxQueueKind>,
    buffer: Permit<'id, TxBufferKind>,
}

impl<'id, K, Q, B, const N: usize> TxPermits<'id, K, Q, B, N>
where
    K: Ieee80211Capabilities,
    Q: QueuePool<'id>,
    B: PermitPool<'id, TxBufferKind>,
{
    /// The permits of the install `mint` serves: one queue permit minted
    /// for each queue of `K`, each in its own pool, and `buffers`.
    pub fn new(mint: &mut Mint<'id>, buffers: B) -> Self {
        const {
            assert!(
                N == K::CAPS.tx_queues as usize,
                "one pool per transmit queue"
            )
        };
        let permits: [Permit<'id, TxQueueKind>; N] = mint.permits();
        Self {
            queues: permits.map(Q::of),
            buffers,
            in_flight: RefCell::new([const { None }; N]),
            capabilities: PhantomData,
        }
    }

    /// The grant of the queue `access_category` occupies, once no attempt
    /// holds it.
    pub async fn queue(&self, access_category: WmmAccessCategory) -> QueuePermit<'_, 'id> {
        let queue = K::CAPS.tx_queue(access_category);
        let permit = self.queues[usize::from(queue)].take().await;
        QueuePermit {
            permit: Some(permit),
            queue,
            access_category,
            keeper: self,
        }
    }

    /// The permit of a free buffer, once one is.
    pub async fn buffer(&self) -> BufferPermit<'_, 'id> {
        BufferPermit {
            permit: Some(self.buffers.take().await),
            keeper: self,
        }
    }

    /// Keep the permits of the admitted attempt `id` until its terminal;
    /// a backend's `submit` calls it with the guards this keeper made.
    ///
    /// # Panics
    ///
    /// A guard came from another keeper of the install: a backend that
    /// keeps two would cross their pools.
    pub fn admit(&self, id: TxId, queue: QueuePermit<'_, 'id>, buffer: BufferPermit<'_, 'id>) {
        let this: &dyn Keeper<'id> = self;
        assert!(
            core::ptr::addr_eq(queue.keeper, this) && core::ptr::addr_eq(buffer.keeper, this),
            "a guard goes back into the keeper that made it"
        );
        let index = usize::from(queue.queue);
        let queue = queue.into_permit();
        let buffer = buffer.into_permit();
        // Each queue's pool holds the one permit `new` minted for it, so the
        // queue's entry is free while that permit was out.
        self.in_flight.borrow_mut()[index] = Some(InFlight { id, queue, buffer });
    }

    /// Give back the permits of the attempt `id`, whose terminal the
    /// backend took; it calls it before it hands the event out.
    pub fn complete(&self, id: TxId) {
        let ended =
            self.in_flight
                .borrow_mut()
                .iter_mut()
                .enumerate()
                .find_map(|(queue, entry)| {
                    entry
                        .take_if(|held| held.id == id)
                        .map(|held| (queue, held))
                });
        if let Some((queue, held)) = ended {
            self.queues[queue].release(held.queue);
            self.buffers.release(held.buffer);
        }
    }

    /// Whether no attempt is in flight: every permit is back, as a
    /// finished disable leaves it.
    pub fn idle(&self) -> bool {
        self.in_flight.borrow().iter().all(Option::is_none)
    }
}

impl<'id, K, Q, B, const N: usize> Keeper<'id> for TxPermits<'id, K, Q, B, N>
where
    Q: QueuePool<'id>,
    B: PermitPool<'id, TxBufferKind>,
{
    fn release_queue(&self, queue: u8, permit: Permit<'id, TxQueueKind>) {
        self.queues[usize::from(queue)].release(permit);
    }

    fn release_buffer(&self, permit: Permit<'id, TxBufferKind>) {
        self.buffers.release(permit);
    }
}

#[cfg(test)]
mod tests {
    use core::{
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };

    use oer_ieee80211_lower_mac::{Ieee80211Radio, TxId};
    use oer_ieee80211_mac::qos::WmmAccessCategory;
    use oer_radio_port::{
        ClockError, Disabled, EnableResult, Ended, EventsLost, Lent, Mint, Owner, PhaseParts,
        Poisoned, PortBackend, PortBody, PortResult, RadioPortEvents, Retire, run_port,
    };
    use oer_time::RadioInstant;

    use super::TxPermits;
    use crate::lower_mac_owner::model::{ModelCaps, ModelPermitPool, OWNED_MODEL_TX_QUEUES};

    fn ready<F: Future>(future: F) -> F::Output {
        match pin!(future)
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("a free permit is served at once"),
        }
    }

    type Keeper<'id> = TxPermits<
        'id,
        ModelCaps,
        ModelPermitPool<'id, super::TxQueueKind>,
        ModelPermitPool<'id, super::TxBufferKind>,
        OWNED_MODEL_TX_QUEUES,
    >;

    /// A backend that keeps two keepers of one install and crosses them.
    struct TwoKeepers;

    /// No event ever.
    struct NoEvents;

    impl RadioPortEvents for NoEvents {
        type Event = ();
        type Fault = ();

        fn next_event(&mut self) -> impl Future<Output = PortResult<(), EventsLost, ()>> + '_ {
            core::future::pending()
        }
    }

    impl PortBackend for TwoKeepers {
        type Event = ();
        type Domain = Ieee80211Radio;
        type Fault = ();
        type Refusal = ();
        type Setup<'a, 'id> = ();
        type Events<'a, 'id>
            = NoEvents
        where
            'id: 'a;
        type Control<'a, 'id>
            = ()
        where
            'id: 'a;
        type Clients<'a, 'id>
            = ()
        where
            'id: 'a;
        type Resources<'id> = ();
        type Held<'id> = ();
        type State = ();
        type Uninstall = ();
        type Uninstalled = ();

        fn enable(&mut self) -> impl Future<Output = EnableResult<(), ()>> + '_ {
            core::future::ready(Ok(Ok(())))
        }

        fn disable<'a, 'id>(
            &'a mut self,
            _: &'a (),
            _: &'a mut dyn FnMut(()),
        ) -> impl Future<Output = Result<(), Poisoned<()>>> + 'a
        where
            'id: 'a,
        {
            core::future::ready(Ok(()))
        }

        fn now(
            &self,
        ) -> impl Future<Output = PortResult<RadioInstant<Ieee80211Radio>, ClockError, ()>> + '_
        {
            core::future::ready(Ok(Err(ClockError::Unavailable)))
        }

        fn resources<'id>(&mut self, mut mint: Mint<'id>) -> ((), ()) {
            let buffers = |mint: &mut Mint<'id>| {
                let [buffer] = mint.permits();
                ModelPermitPool::new([buffer])
            };
            let first: Keeper<'id> = {
                let pool = buffers(&mut mint);
                TxPermits::new(&mut mint, pool)
            };
            let second: Keeper<'id> = {
                let pool = buffers(&mut mint);
                TxPermits::new(&mut mint, pool)
            };
            let queue = ready(first.queue(WmmAccessCategory::BestEffort));
            let buffer = ready(first.buffer());
            // A guard of the first keeper, admitted into the second.
            second.admit(TxId(0), queue, buffer);
            ((), ())
        }

        fn setup<'a, 'id>(&'a mut self) -> Lent<'a, Self::Setup<'a, 'id>> {
            Lent::new(())
        }

        fn split<'a, 'id>(
            &'a mut self,
            _: &'a (),
        ) -> PhaseParts<'a, Self::Events<'a, 'id>, Self::Control<'a, 'id>, Self::Clients<'a, 'id>>
        where
            'id: 'a,
        {
            PhaseParts {
                events: Lent::new(NoEvents),
                control: Lent::new(()),
                clients: Lent::new(()),
            }
        }

        fn drive<'p, 'id>(
            &mut self,
            _: &'p (),
            _: Retire<'p>,
        ) -> impl Future<Output = ()> + use<'p, 'id> {
            core::future::ready(())
        }

        fn retire_bound(&self) -> impl Future<Output = ()> + '_ {
            core::future::pending()
        }

        fn detach<'id>(&mut self, (): &()) {}

        fn uninstall<'id>(self, (): (), (): (), (): ()) {}

        fn abandon(&mut self) {}
    }

    struct Uninstalls;

    impl PortBody<TwoKeepers> for Uninstalls {
        async fn run<'p, 'id>(
            self,
            owner: Owner<'p, 'id, TwoKeepers, Disabled>,
            (): (),
        ) -> Ended<'id, TwoKeepers> {
            owner.uninstall(())
        }
    }

    #[test]
    #[should_panic(expected = "a guard goes back into the keeper that made it")]
    fn a_guard_admitted_into_another_keeper_of_the_install_panics() {
        let _ = ready(run_port(TwoKeepers, Uninstalls));
    }
}
