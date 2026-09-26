//! Platform reservation travels separately from concrete radio command states.

use super::{
    TeardownPendingPlatform,
    runtime_owner::{RuntimeOwnerLease, RuntimeOwnerSlot},
};

/// Exclusive lease of a powered platform reservation.
///
/// Dropping this lease leaves the reservation in its claimed static slot.
/// It never runs platform Drop or makes another cold start possible.
#[must_use = "retain the platform lease for the same Controller epoch"]
pub struct ControllerPlatformLease<'runtime, P> {
    lease: RuntimeOwnerLease<'runtime, TeardownPendingPlatform<P>>,
}

impl<'runtime, P> ControllerPlatformLease<'runtime, P> {
    pub(crate) fn claim(
        slot: &'runtime mut RuntimeOwnerSlot<TeardownPendingPlatform<P>>,
    ) -> Option<Self> {
        slot.lease().map(|lease| Self { lease })
    }

    /// Extract the reservation once `barrier` proves that the composition's
    /// radio and Host epoch retired; a failed barrier returns the unchanged
    /// lease.
    pub fn try_retire<Proof, Error>(
        mut self,
        barrier: impl FnOnce() -> Result<Proof, Error>,
    ) -> Result<(ControllerRetiredPlatform<'runtime, P>, Proof), (Error, Self)> {
        match self.lease.try_retire(barrier) {
            Ok((platform, proof)) => Ok((
                ControllerRetiredPlatform {
                    lease: self.lease,
                    _platform: platform,
                },
                proof,
            )),
            Err(error) => Err((error, self)),
        }
    }
}

/// Actual platform reservation extracted after its epoch retired.
///
/// The reservation remains protected against implicit Drop. Hardware teardown
/// must still precede access to the underlying platform or release of its lease.
#[must_use = "retain the powered platform until verified physical teardown"]
pub struct ControllerRetiredPlatform<'runtime, P> {
    #[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
    lease: RuntimeOwnerLease<'runtime, TeardownPendingPlatform<P>>,
    _platform: TeardownPendingPlatform<P>,
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, P> ControllerRetiredPlatform<'runtime, P> {
    pub fn platform_mut(&mut self) -> &mut P {
        self._platform.platform_mut()
    }
    pub fn into_platform_after_shutdown(
        self,
    ) -> (P, RuntimeOwnerLease<'runtime, TeardownPendingPlatform<P>>) {
        (self._platform.into_platform_after_shutdown(), self.lease)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        clock::ClockedResources,
        resources::{BluetoothRadioHardware, BluetoothStopped},
        runtime_resources::ControllerRuntimeResources,
    };
    use std::{boxed::Box, cell::Cell, rc::Rc};

    struct Platform(Rc<Cell<usize>>);
    impl Drop for Platform {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    #[test]
    fn the_platform_returns_only_after_its_barrier_without_running_drop() {
        let drops = Rc::new(Cell::new(0));
        let platform = Box::new(Platform(drops.clone()));
        let address = core::ptr::from_ref(&*platform);
        let stopped =
            BluetoothStopped::from_hardware(platform, BluetoothRadioHardware::for_validation());
        let (registers, platform) = stopped.into_parts();
        let mut scheduler = ClockedResources::for_validation(registers, platform)
            .initialize_controller_hal_with(|_, _| {})
            .initialize_scheduler_for_validation(ControllerRuntimeResources::<1>::new());
        let retired = {
            let (_, _, _, lease) = scheduler.split_runtime().unwrap();
            let Err(((), lease)) = lease.try_retire(|| Err::<(), ()>(())) else {
                panic!("a failed barrier keeps the reservation");
            };
            assert_eq!(drops.get(), 0);
            let Ok((retired, 7)) = lease.try_retire(|| Ok::<_, ()>(7)) else {
                panic!("a passed barrier extracts the reservation");
            };
            retired
        };
        assert_eq!(core::ptr::from_ref(&**retired._platform._platform), address);
        drop(retired);
        assert_eq!(
            drops.get(),
            0,
            "powered retirement must not run platform Drop"
        );
    }

    #[test]
    fn abandoning_the_platform_lease_never_reopens_the_slot_or_releases_p() {
        let drops = Rc::new(Cell::new(0));
        {
            let mut slot =
                RuntimeOwnerSlot::new(TeardownPendingPlatform::new(Platform(drops.clone())));
            {
                let _lease = ControllerPlatformLease::claim(&mut slot).unwrap();
            }
            assert!(ControllerPlatformLease::claim(&mut slot).is_none());
        }
        assert_eq!(drops.get(), 0);
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, P> ControllerPlatformLease<'runtime, P> {
    pub fn restore(
        mut lease: RuntimeOwnerLease<'runtime, TeardownPendingPlatform<P>>,
        platform: TeardownPendingPlatform<P>,
    ) -> Self {
        lease
            .restore(platform)
            .unwrap_or_else(|_| panic!("retired platform slot is empty"));
        Self { lease }
    }
}
