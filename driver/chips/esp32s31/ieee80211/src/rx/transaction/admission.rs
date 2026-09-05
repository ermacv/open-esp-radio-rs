//! Value-only admission contract for a real completed DMA unit.

use super::Discard;

/// Value-only description of one real completed DMA unit before staging.
///
/// The policy never receives a descriptor pointer, payload view or ring
/// capability. It can narrow the admitted payload length, but ownership and
/// descriptor reclaim remain exclusively inside the physical transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompletedUnit {
    pub head_index: usize,
    pub descriptor_count: usize,
    pub payload_length: usize,
}

/// Fact-only traffic class visible at the DMA/staging admission boundary.
///
/// Only an IEEE 802.11 frame-control value copied from the completed unit is
/// interpreted. No association, authorization or hardware meaning is
/// inferred here. Protected data is the sole bulk class; management, control
/// and unprotected data (including pre-key EAPOL) remain critical.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IngressClass {
    BulkProtectedData,
    Critical,
    Unclassified,
}

/// Fact-only logical route for overload accounting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IngressRoute {
    Standalone,
    Station,
    AccessPoint,
    Foreign,
    Ambiguous,
    Malformed,
}

/// Value-only preview used before staging ownership is transferred.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Preview {
    pub unit: CompletedUnit,
    pub frame_control: Option<u16>,
    pub class: IngressClass,
    pub route: IngressRoute,
}

/// Policy decision when ordinary staging credits are unavailable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unavailable {
    /// Keep ordinary bulk data at the completed ring head until staging frees.
    PreserveForCapacity,
    /// Preserve the final staging credit for control/management/EAPOL input.
    PreserveForCriticalAdmission,
    /// Drop the upper copy but return the completed descriptor immediately.
    DiscardAndRecycle,
}

/// A completed ingress transaction observed after its ownership edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Observation {
    /// The unit was discarded and retained until the frozen-LAST reclaim.
    DiscardRetained {
        unit: CompletedUnit,
        reason: Discard,
    },
    /// The original DMA buffer was published into the bounded upper queue.
    Staged(CompletedUnit),
    /// A bulk unit could not acquire an upper-layer credit and followed the
    /// reviewed vendor discard/append path.
    OverloadDiscardedAndRecycled(Preview),
    /// A bulk unit remained at the completed ring head until upper capacity
    /// becomes available.
    BulkAdmissionBlocked(Preview),
    /// A critical unit consumed the reserved final staging credit.
    CriticalReserveAdmitted(Preview),
    /// No reserved credit remained for a critical unit. Descriptor ownership
    /// was deliberately not transferred, so a later capacity wake retries it.
    CriticalAdmissionBlocked(Preview),
}

/// Admission policy at the completed-DMA-unit/staging boundary.
///
/// This hook is intentionally narrower than a general RX observer. It may
/// only lower the maximum staged payload for the current real unit and then
/// observe the completed transaction. It cannot mutate descriptor metadata,
/// reclaim buffers, fabricate frames or retain hardware ownership.
pub trait Admission {
    fn maximum_payload_length(&self, _unit: CompletedUnit, physical_capacity: usize) -> usize {
        physical_capacity
    }

    fn observe(&self, _observation: Observation) {}

    /// Number of staging/queue credits unavailable to ordinary bulk data.
    fn critical_reserved_credits(&self) -> usize {
        1
    }

    /// Decide whether a unit may be discarded when only the critical reserve
    /// remains. The default is deliberately conservative for unknown input.
    fn unavailable_disposition(&self, preview: Preview) -> Unavailable {
        match preview.class {
            IngressClass::BulkProtectedData => Unavailable::DiscardAndRecycle,
            IngressClass::Critical | IngressClass::Unclassified => {
                Unavailable::PreserveForCriticalAdmission
            }
        }
    }
}

impl<T: Admission + ?Sized> Admission for &T {
    fn maximum_payload_length(&self, unit: CompletedUnit, physical_capacity: usize) -> usize {
        T::maximum_payload_length(*self, unit, physical_capacity)
    }

    fn observe(&self, observation: Observation) {
        T::observe(*self, observation);
    }

    fn critical_reserved_credits(&self) -> usize {
        T::critical_reserved_credits(*self)
    }

    fn unavailable_disposition(&self, preview: Preview) -> Unavailable {
        T::unavailable_disposition(*self, preview)
    }
}

/// Zero-sized production policy admitting the complete physical stage slot.
#[derive(Clone, Copy, Debug, Default)]
pub struct AdmitAll;

impl Admission for AdmitAll {}
