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
    fn real_hci_rejections_never_extract_the_hardware_owner() {
        use embassy_futures::block_on;
        use embassy_sync::blocking_mutex::raw::NoopRawMutex;
        use oer_bluetooth_hci::{
            BluetoothPublicDeviceAddress, LeControllerBootstrapConfig,
            LeControllerCommandReadyClaim, LeControllerHciResources,
            LeControllerHciRetirementError,
        };
        let config = LeControllerBootstrapConfig::new(
            BluetoothPublicDeviceAddress::from_canonical_bytes([2, 3, 5, 7, 11, 13]),
            27,
            1,
        )
        .unwrap();
        let mut resources =
            LeControllerHciResources::<NoopRawMutex, 2, 2, 80>::new(config).unwrap();
        let mut endpoints = resources.split();
        let LeControllerCommandReadyClaim::Ready(ready) =
            endpoints.controller.claim_initial_command_ready(())
        else {
            panic!("initial HCI authority");
        };
        let credits = endpoints.host.acl_credit_sender();
        block_on(credits.return_completed_packets(&[])).unwrap();
        let owner = Box::new(47);
        let address = core::ptr::from_ref(&*owner);
        let mut slot = RuntimeOwnerSlot::new(owner);
        {
            let mut lease = slot.lease().unwrap();
            let (error, ready) = lease
                .try_retire(|| endpoints.controller.try_retire_transport(ready))
                .err()
                .expect("accepted Host credits must drain");
            assert_eq!(error, LeControllerHciRetirementError::HostPacketsPending);
            assert_eq!(address, core::ptr::from_ref(&**lease));
            endpoints.controller.close_transport();
            let (error, _ready) = lease
                .try_retire(|| endpoints.controller.try_retire_transport(ready))
                .err()
                .expect("terminal closure cannot return graceful hardware ownership");
            assert_eq!(error, LeControllerHciRetirementError::Closed);
            assert_eq!(address, core::ptr::from_ref(&**lease));
        }
        assert_eq!(slot.owner.as_deref(), Some(&47));
        assert!(slot.lease().is_none());
    }
    #[test]
    fn joint_retirement_keeps_both_owners_until_the_original_hci_epoch_drains() {
        use embassy_futures::block_on;
        use embassy_sync::blocking_mutex::raw::NoopRawMutex;
        use oer_bluetooth_hci::{
            BluetoothPublicDeviceAddress, LeControllerBootstrapConfig,
            LeControllerCommandReadyClaim, LeControllerHciResources,
            LeControllerHciRetirementError,
        };
        use std::{cell::Cell, rc::Rc};
        struct Owner(Rc<Cell<usize>>);
        impl Drop for Owner {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let drops = Rc::new(Cell::new(0));
        let config = LeControllerBootstrapConfig::new(
            BluetoothPublicDeviceAddress::from_canonical_bytes([2, 3, 5, 7, 11, 13]),
            27,
            1,
        )
        .unwrap();
        let mut original = LeControllerHciResources::<NoopRawMutex, 2, 2, 80>::new(config).unwrap();
        let mut foreign = LeControllerHciResources::<NoopRawMutex, 2, 2, 80>::new(config).unwrap();
        let mut original = original.split();
        let mut foreign = foreign.split();
        let LeControllerCommandReadyClaim::Ready(ready) =
            original.controller.claim_initial_command_ready(())
        else {
            panic!("initial HCI authority");
        };
        let first = Box::new(Owner(drops.clone()));
        let second = Box::new(Owner(drops.clone()));
        let first_address = core::ptr::from_ref(&*first);
        let second_address = core::ptr::from_ref(&*second);
        let (first, second, proof) = {
            let mut first = RuntimeOwnerSlot::new(first);
            let mut second = RuntimeOwnerSlot::new(second);
            let retired = {
                let mut a = first.lease().unwrap();
                let mut b = second.lease().unwrap();
                let (error, ready) = a
                    .try_retire_with(&mut b, || foreign.controller.try_retire_transport(ready))
                    .err()
                    .expect("foreign endpoint must not extract either owner");
                assert_eq!(error, LeControllerHciRetirementError::EndpointMismatch);
                block_on(
                    original
                        .host
                        .acl_credit_sender()
                        .return_completed_packets(&[]),
                )
                .unwrap();
                let (error, ready) = a
                    .try_retire_with(&mut b, || original.controller.try_retire_transport(ready))
                    .err()
                    .expect("accepted credits must drain before either owner moves");
                assert_eq!(error, LeControllerHciRetirementError::HostPacketsPending);
                assert_eq!(core::ptr::from_ref(&**a), first_address);
                assert_eq!(core::ptr::from_ref(&**b), second_address);
                assert_eq!(drops.get(), 0);
                let mut buffer = [0; 80];
                let oer_bluetooth_hci::LeControllerActivePeripheralIntake::HostCompletedPackets {
                    ready,
                    command,
                    ..
                } = original
                    .controller
                    .try_receive_active_peripheral_with_buffer(
                        ready,
                        None,
                        true,
                        &mut buffer,
                        |_, _| panic!("expected credits, not ACL"),
                    )
                else {
                    panic!("accepted Host credits retained across rejection");
                };
                assert!(command.is_ok());
                a.try_retire_with(&mut b, || original.controller.try_retire_transport(ready))
                    .unwrap_or_else(|_| panic!("same drained epoch"))
            };
            assert!(first.owner.is_none());
            assert!(second.owner.is_none());
            assert!(first.lease().is_none());
            assert!(second.lease().is_none());
            retired
        };
        assert!(proof.matches_endpoint(&original.controller));
        assert!(!proof.matches_endpoint(&foreign.controller));
        assert_eq!(core::ptr::from_ref(&*first), first_address);
        assert_eq!(core::ptr::from_ref(&*second), second_address);
        assert_eq!(drops.get(), 0);
        assert!(
            block_on(
                foreign
                    .host
                    .acl_credit_sender()
                    .return_completed_packets(&[])
            )
            .is_ok()
        );
        assert!(
            block_on(
                original
                    .host
                    .acl_credit_sender()
                    .return_completed_packets(&[])
            )
            .is_err()
        );
        drop((first, second));
        assert_eq!(drops.get(), 2);
    }
}
