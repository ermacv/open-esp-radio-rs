//! One issuance of a backend's declared pool set.

use core::marker::PhantomData;

use super::{Brand, Line, Permit, Seat, Slot, index_of};

/// The complete pool layout of one backend installation.
///
/// A backend declares this as its port's `Pools` associated type. Tuples
/// compose slot and permit sets; a block can implement this trait to wrap
/// the resulting tokens in its own resource types. The builder is consumed
/// once, so a factory cannot issue another copy of a declared pool.
pub trait PoolSet {
    /// The owned pools, branded by the installation that issued them.
    type Pools<'id>;

    /// Assemble the complete set from its single construction authority.
    fn build<'id>(builder: PoolBuilder<'id>) -> Self::Pools<'id>;
}

/// The resources produced by layout `P` for installation `'id`.
pub type Pools<'id, P> = <P as PoolSet>::Pools<'id>;

/// The one issuance authority for an installation's declared layout `P`.
///
/// It has no individual pool constructors. Consuming it returns the entire
/// set; the backend then moves each pool to its held state or body resources.
///
/// ```compile_fail,E0382
/// use oer_radio_port::{Mint, Slots};
/// enum Keys {}
/// fn duplicate<'id>(mint: Mint<'id, Slots<Keys, 2>>) {
///     let first = mint.into_pools();
///     let second = mint.into_pools();
///     drop((first, second));
/// }
/// ```
///
/// ```
/// use oer_radio_port::{Mint, Slot, Slots};
/// enum Keys {}
/// fn once<'id>(mint: Mint<'id, Slots<Keys, 2>>) -> [Slot<'id, Keys>; 2] {
///     mint.into_pools()
/// }
/// ```
///
/// An entire pool set retains the same invariant installation brand:
///
/// ```compile_fail
/// use oer_radio_port::{Pools, Slots};
/// enum Keys {}
/// fn foreign<'a: 'b, 'b>(pools: Pools<'a, Slots<Keys, 2>>) -> Pools<'b, Slots<Keys, 2>> {
///     pools
/// }
/// ```
///
/// ```
/// use oer_radio_port::{Pools, Slots};
/// enum Keys {}
/// fn same<'id>(pools: Pools<'id, Slots<Keys, 2>>) -> Pools<'id, Slots<Keys, 2>> {
///     pools
/// }
/// ```
pub struct Mint<'id, P: PoolSet> {
    brand: Brand<'id>,
    layout: PhantomData<fn() -> P>,
}

impl<'id, P: PoolSet> Mint<'id, P> {
    pub(crate) const fn new() -> Self {
        Self {
            brand: PhantomData,
            layout: PhantomData,
        }
    }

    /// Issue the declared pool set exactly once, consuming its authority.
    pub fn into_pools(self) -> P::Pools<'id> {
        P::build(PoolBuilder { brand: self.brand })
    }
}

/// Run an installation with a fresh brand and one mint of its layout `P`.
/// Nothing branded by `'id` can leave the callback. A nested call has its
/// own brand, so its pools cannot be substituted for this installation's.
pub async fn branded<P: PoolSet, R>(install: impl for<'id> AsyncFnOnce(Mint<'id, P>) -> R) -> R {
    install(Mint::new()).await
}

/// Single-use authority supplied only to a [`PoolSet`] factory.
///
/// A custom layout expands one tuple of component layouts, then wraps or
/// distributes the returned owners. Only tuple construction inside this
/// crate can divide this authority among its declared components.
///
/// ```compile_fail,E0382
/// use oer_radio_port::{PoolBuilder, Slots};
/// enum Keys {}
/// fn duplicate<'id>(builder: PoolBuilder<'id>) {
///     let first = builder.into_pools::<Slots<Keys, 1>>();
///     let second = builder.into_pools::<Slots<Keys, 1>>();
///     drop((first, second));
/// }
/// ```
pub struct PoolBuilder<'id> {
    brand: Brand<'id>,
}

impl<'id> PoolBuilder<'id> {
    /// Expand one declared component set, consuming this factory authority.
    pub fn into_pools<P: PoolSet>(self) -> P::Pools<'id> {
        P::build(self)
    }
}

/// A layout of `N` synchronously owned slots of kind `Kind`.
///
/// Slot indices fit in one byte; larger layouts fail at instantiation:
///
/// ```compile_fail,E0080
/// use oer_radio_port::{Mint, Slots};
/// fn oversized(mint: Mint<'static, Slots<(), 257>>) {
///     let _ = mint.into_pools();
/// }
/// fn main() { let _: fn(Mint<'static, Slots<(), 257>>) = oversized; }
/// ```
pub struct Slots<Kind, const N: usize>(PhantomData<fn() -> Kind>);

impl<Kind, const N: usize> PoolSet for Slots<Kind, N> {
    type Pools<'id> = [Slot<'id, Kind>; N];

    fn build<'id>(builder: PoolBuilder<'id>) -> Self::Pools<'id> {
        const { assert!(N <= 256, "a pool holds at most 256 tokens") };
        core::array::from_fn(|index| Slot {
            index: index_of(index),
            brand: builder.brand,
            kind: PhantomData,
        })
    }
}

/// A layout of `N` awaited permits and the `S` seats of their line.
pub struct Permits<Kind, const N: usize, const S: usize>(PhantomData<fn() -> Kind>);

/// One permit pool's initial owners, line and caller seats.
pub struct PermitTokens<'id, Kind, const N: usize, const S: usize> {
    /// All permits, to be retained by the backend's permit pool.
    pub permits: [Permit<'id, Kind>; N],
    /// One queue place for every issued seat.
    pub line: Line<'id, Kind, S>,
    /// One seat for each allowed waiting caller.
    pub seats: [Seat<'id, Kind>; S],
}

impl<Kind, const N: usize, const S: usize> PoolSet for Permits<Kind, N, S> {
    type Pools<'id> = PermitTokens<'id, Kind, N, S>;

    fn build<'id>(builder: PoolBuilder<'id>) -> Self::Pools<'id> {
        const {
            assert!(N <= 256, "a pool holds at most 256 tokens");
            assert!(S <= 256, "a line holds at most 256 seats");
        };
        PermitTokens {
            permits: core::array::from_fn(|index| Permit {
                index: index_of(index),
                brand: builder.brand,
                kind: PhantomData,
            }),
            line: Line {
                brand: builder.brand,
                kind: PhantomData,
            },
            seats: core::array::from_fn(|index| Seat {
                index: index_of(index),
                brand: builder.brand,
                kind: PhantomData,
            }),
        }
    }
}

impl PoolSet for () {
    type Pools<'id> = ();

    fn build<'id>(_: PoolBuilder<'id>) {}
}

macro_rules! tuple_set {
    ($($part:ident),+) => {
        impl<$($part: PoolSet),+> PoolSet for ($($part,)+) {
            type Pools<'id> = ($($part::Pools<'id>,)+);

            fn build<'id>(builder: PoolBuilder<'id>) -> Self::Pools<'id> {
                ($($part::build(PoolBuilder { brand: builder.brand }),)+)
            }
        }
    };
}

tuple_set!(A);
tuple_set!(A, B);
tuple_set!(A, B, C);
tuple_set!(A, B, C, D);
tuple_set!(A, B, C, D, E);
tuple_set!(A, B, C, D, E, F);
tuple_set!(A, B, C, D, E, F, G);
tuple_set!(A, B, C, D, E, F, G, H);
tuple_set!(A, B, C, D, E, F, G, H, I);
tuple_set!(A, B, C, D, E, F, G, H, I, J);
