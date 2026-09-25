//! Platform reservation travels separately from concrete radio command states.

use super::{
    TeardownPendingPlatform,
    runtime_owner::{RuntimeOwnerLease, RuntimeOwnerSlot},
};
use oer_bluetooth_hci::{HciEpochIdentity, LeControllerHciRetired};

/// Exclusive lease of a powered platform reservation before HCI binding.
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

    pub fn bind(self, epoch: HciEpochIdentity<'runtime>) -> ControllerRuntimePlatform<'runtime, P> {
        ControllerRuntimePlatform {
            lease: self.lease,
            epoch,
        }
    }
}

/// Powered platform reservation tied to the HCI epoch issued by its final split.
///
/// Keep this beside the hardware runner, outside radio command state machines.
/// It provides neither mutable platform access nor permission to release PHY.
/// Dropping it retains the actual platform in static storage, permanently claimed.
#[must_use = "retain the platform lease until its own Controller retires"]
pub struct ControllerRuntimePlatform<'runtime, P> {
    lease: RuntimeOwnerLease<'runtime, TeardownPendingPlatform<P>>,
    epoch: HciEpochIdentity<'runtime>,
}

/// Actual platform reservation extracted after its matching HCI retirement.
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

impl<'runtime, P> ControllerRuntimePlatform<'runtime, P> {
    #[cfg(target_arch = "riscv32")]
    pub fn platform_mut_for_epoch(&mut self, epoch: HciEpochIdentity<'_>) -> Option<&mut P> {
        self.epoch
            .same_epoch(epoch)
            .then(|| self.lease.platform_mut())
    }

    /// Extract this reservation only with the retirement proof of its own HCI
    /// epoch. A foreign proof returns the unchanged lease without extracting P.
    pub fn try_retire<Owner>(
        mut self,
        retired: &LeControllerHciRetired<'_, Owner>,
    ) -> Result<ControllerRetiredPlatform<'runtime, P>, Self> {
        match self
            .lease
            .try_retire(|| retired.matches_epoch(self.epoch).then_some(()).ok_or(()))
        {
            Ok((platform, ())) => Ok(ControllerRetiredPlatform {
                lease: self.lease,
                _platform: platform,
            }),
            Err(()) => Err(self),
        }
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
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use oer_bluetooth_hci::{
        BluetoothPublicDeviceAddress, LeControllerBootstrapConfig, LeControllerCommandReadyClaim,
        LeControllerHciResources,
    };
    use std::{boxed::Box, cell::Cell, rc::Rc};

    type Resources = LeControllerHciResources<NoopRawMutex, 2, 2, 80>;
    struct Platform(Rc<Cell<usize>>);
    impl Drop for Platform {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    fn resources() -> Resources {
        Resources::new(
            LeControllerBootstrapConfig::new(
                BluetoothPublicDeviceAddress::from_canonical_bytes([2, 3, 5, 7, 11, 13]),
                27,
                1,
            )
            .unwrap(),
        )
        .unwrap()
    }
    fn retire<'epoch>(
        endpoint: &mut oer_bluetooth_hci::LeControllerCommandEndpoint<
            'epoch,
            NoopRawMutex,
            2,
            2,
            80,
        >,
    ) -> LeControllerHciRetired<'epoch, ()> {
        let LeControllerCommandReadyClaim::Ready(ready) = endpoint.claim_initial_command_ready(())
        else {
            panic!("initial authority");
        };
        endpoint
            .try_retire_transport(ready)
            .unwrap_or_else(|_| panic!("drained epoch"))
    }

    #[test]
    fn scheduler_platform_returns_only_for_its_hci_epoch_without_releasing_reservation() {
        let drops = Rc::new(Cell::new(0));
        let platform = Box::new(Platform(drops.clone()));
        let address = core::ptr::from_ref(&*platform);
        let mut original = resources();
        let mut foreign = resources();
        let mut original = original.split();
        let mut foreign = foreign.split();
        let stopped =
            BluetoothStopped::from_hardware(platform, BluetoothRadioHardware::for_validation());
        let (registers, platform) = stopped.into_parts();
        let mut scheduler = ClockedResources::for_validation(registers, platform)
            .initialize_controller_hal_with(|_, _| {})
            .initialize_scheduler_for_validation(ControllerRuntimeResources::<1, 1>::new());
        let retired_platform = {
            let (_, _, _, lease) = scheduler.split_runtime().unwrap();
            let platform = lease.bind(original.controller.epoch_identity());
            let foreign_proof = retire(&mut foreign.controller);
            let platform = platform
                .try_retire(&foreign_proof)
                .err()
                .expect("foreign retirement must preserve reservation");
            assert_eq!(drops.get(), 0);
            let proof = retire(&mut original.controller);
            platform
                .try_retire(&proof)
                .unwrap_or_else(|_| panic!("matching epoch"))
        };
        // The real P is extracted; its empty original slot remains exclusively leased.
        assert_eq!(
            core::ptr::from_ref(&**retired_platform._platform._platform),
            address
        );
        assert_eq!(drops.get(), 0);
        drop(retired_platform);
        assert_eq!(
            drops.get(),
            0,
            "powered retirement must not run platform Drop"
        );
    }

    #[test]
    fn abandoning_bound_platform_lease_never_reopens_the_slot_or_releases_p() {
        let drops = Rc::new(Cell::new(0));
        let mut hci = resources();
        let endpoints = hci.split();
        {
            let mut slot =
                RuntimeOwnerSlot::new(TeardownPendingPlatform::new(Platform(drops.clone())));
            {
                let _bound = ControllerPlatformLease::claim(&mut slot)
                    .unwrap()
                    .bind(endpoints.controller.epoch_identity());
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
