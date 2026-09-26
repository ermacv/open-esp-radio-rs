//! Exclusive runtime lease and finite, lossless return of a hardware owner.

use core::ops::{Deref, DerefMut};

pub struct RuntimeOwnerSlot<T> {
    owner: Option<T>,
    claimed: bool,
}

impl<T> RuntimeOwnerSlot<T> {
    pub const fn new(owner: T) -> Self {
        Self {
            owner: Some(owner),
            claimed: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn as_ref(&self) -> Option<&T> {
        if self.claimed {
            None
        } else {
            self.owner.as_ref()
        }
    }

    pub fn as_mut(&mut self) -> Option<&mut T> {
        if self.claimed {
            None
        } else {
            self.owner.as_mut()
        }
    }

    pub fn lease(&mut self) -> Option<RuntimeOwnerLease<'_, T>> {
        if self.claimed {
            return None;
        }
        self.claimed = true;
        Some(RuntimeOwnerLease { slot: self })
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn into_unclaimed(self) -> T {
        assert!(!self.claimed, "movable initialization has not been split");
        self.owner.expect("unclaimed initialized owner")
    }
}

/// Dropping the lease leaves the slot claimed and its hardware retained.
/// Only successful retirement removes the owner; no pointer is reconstructed.
pub struct RuntimeOwnerLease<'slot, T> {
    slot: &'slot mut RuntimeOwnerSlot<T>,
}

impl<T> RuntimeOwnerLease<'_, T> {
    /// Only the original exclusive borrow can refill its emptied runtime slot.
    /// The caller supplies the owner from a completed physical initialization.
    #[cfg(any(target_arch = "riscv32", test, feature = "test-support"))]
    pub fn restore(&mut self, owner: T) -> Result<(), T> {
        if self.slot.owner.is_some() {
            return Err(owner);
        }
        self.slot.owner = Some(owner);
        Ok(())
    }
    /// A single barrier governs both owners; rejection extracts neither.
    pub fn try_retire_with<U, Proof, Error>(
        &mut self,
        other: &mut RuntimeOwnerLease<'_, U>,
        barrier: impl FnOnce() -> Result<Proof, Error>,
    ) -> Result<(T, U, Proof), Error> {
        self.try_retire(|| other.try_retire(barrier))
            .map(|(owner, (other, proof))| (owner, other, proof))
    }

    pub fn try_retire<Proof, Error>(
        &mut self,
        barrier: impl FnOnce() -> Result<Proof, Error>,
    ) -> Result<(T, Proof), Error> {
        let proof = barrier()?;
        let owner = self
            .slot
            .owner
            .take()
            .expect("a live lease retains its owner");
        Ok((owner, proof))
    }
}

impl<T> Deref for RuntimeOwnerLease<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.slot.owner.as_ref().expect("live runtime owner")
    }
}

impl<T> DerefMut for RuntimeOwnerLease<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.slot.owner.as_mut().expect("live runtime owner")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::boxed::Box;

    #[test]
    fn only_empty_original_lease_accepts_the_returned_owner() {
        let mut slot = RuntimeOwnerSlot::new(Box::new(23));
        {
            let mut lease = slot.lease().unwrap();
            assert_eq!(lease.restore(Box::new(29)), Err(Box::new(29)));
            let (owner, ()) = lease.try_retire(|| Ok::<_, ()>(())).unwrap();
            let address = core::ptr::from_ref(&*owner);
            lease.restore(owner).unwrap();
            assert_eq!(address, core::ptr::from_ref(&**lease));
            **lease = 31;
            assert_eq!(**lease, 31);
        }
        assert!(
            slot.lease().is_none(),
            "restoration never reopens outer splitting"
        );
    }

    #[test]
    fn failed_barrier_preserves_owner_and_permits_same_lease_retry() {
        let owner = Box::new(31);
        let address = core::ptr::from_ref(&*owner);
        let mut slot = RuntimeOwnerSlot::new(owner);
        {
            let mut lease = slot.lease().unwrap();
            assert_eq!(
                lease.try_retire::<(), _>(|| Err("packets pending")),
                Err("packets pending")
            );
            assert_eq!(address, core::ptr::from_ref(&**lease));
            **lease = 37;
            let (owner, proof) = lease.try_retire(|| Ok::<_, ()>("drained")).unwrap();
            assert_eq!(proof, "drained");
            assert_eq!(address, core::ptr::from_ref(&*owner));
            assert_eq!(*owner, 37);
        }
        assert!(slot.lease().is_none());
        assert!(slot.owner.is_none());
    }

    #[test]
    fn dropped_lease_retains_hardware_and_never_reopens_admission() {
        let mut slot = RuntimeOwnerSlot::new(Box::new(41));
        {
            let mut lease = slot.lease().unwrap();
            **lease = 43;
        }
        assert_eq!(slot.owner.as_deref(), Some(&43));
        assert!(slot.lease().is_none());
        assert!(slot.as_mut().is_none());
        assert!(slot.as_ref().is_none());
    }
    #[test]
    fn joint_retirement_moves_both_owners_only_after_one_barrier() {
        let mut first = RuntimeOwnerSlot::new(Box::new(3));
        let mut second = RuntimeOwnerSlot::new(Box::new(5));
        let mut a = first.lease().unwrap();
        let mut b = second.lease().unwrap();
        assert_eq!(
            a.try_retire_with(&mut b, || Err::<(), _>("pending")),
            Err("pending")
        );
        assert_eq!((**a, **b), (3, 5));
        let (one, two, proof) = a
            .try_retire_with(&mut b, || Ok::<_, ()>("retired"))
            .unwrap();
        assert_eq!((*one, *two, proof), (3, 5, "retired"));
    }
}
