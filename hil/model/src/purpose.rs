//! A scenario's one purpose, as a type.
//!
//! The purpose is a marker type of [`Spec::Purpose`](crate::Spec::Purpose),
//! so the executor can admit a scenario by its purpose at compile time: the
//! host model executes only conformance scenarios, because interop and
//! measurement need real peers and real hardware. There is deliberately no
//! protocol purpose: protocol behaviour is proven on the host against the
//! model, and a stand scenario that only repeats it cannot name one.

use core::{marker::PhantomData, num::NonZeroU32};

use serde::{Deserialize, Serialize};

/// The purpose as a value, for records.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "purpose")]
pub enum Purpose {
    Conformance,
    Interop,
    Measurement,
    Investigation { issue: NonZeroU32 },
}

mod sealed {
    pub trait Sealed {}
}

/// One of the purpose markers.
pub trait PurposeKind: sealed::Sealed {
    const VALUE: Purpose;
}

/// The backend keeps its port contract: the port's conformance suite, the
/// same one the host model passes.
pub enum Conformance {}

/// The backend works with a real peer (hostapd, wpa_supplicant, OpenWrt,
/// BlueZ, OpenThread).
pub enum Interop {}

/// A physical budget: a step or answer bound, a latency, a rate.
pub enum Measurement {}

/// The issue an investigation answers. Without an open issue an
/// investigation is deleted (#761, owner decision 2).
pub trait OpenQuestion {
    const ISSUE: NonZeroU32;
}

/// An open question, named by its issue. Investigations never qualify.
pub struct Investigation<Q: OpenQuestion>(PhantomData<fn() -> Q>);

impl sealed::Sealed for Conformance {}
impl sealed::Sealed for Interop {}
impl sealed::Sealed for Measurement {}
impl<Q: OpenQuestion> sealed::Sealed for Investigation<Q> {}

impl PurposeKind for Conformance {
    const VALUE: Purpose = Purpose::Conformance;
}

impl PurposeKind for Interop {
    const VALUE: Purpose = Purpose::Interop;
}

impl PurposeKind for Measurement {
    const VALUE: Purpose = Purpose::Measurement;
}

impl<Q: OpenQuestion> PurposeKind for Investigation<Q> {
    const VALUE: Purpose = Purpose::Investigation { issue: Q::ISSUE };
}
