//! Stable one-shot placement for the final powered Bluetooth Controller.

use core::mem::MaybeUninit;

use crate::{BluetoothSystem, BluetoothSystemBuildError, compose_esp32s31_bluetooth_system};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

use oer_esp32s31_bluetooth::controller::hci::ControllerHciBound;

use oer_esp32s31_bluetooth_embassy::{
    controller::DtmAbsoluteRecheck, notification::RuntimeNotifications,
};

use oer_esp32s31_radio_platform_esp_hal::PublishedEspHalBluetoothInterruptOwners;

use static_cell::StaticCell;

type RuntimeWakers = RuntimeNotifications<CriticalSectionRawMutex>;

/// The final published Controller type accepted by product composition.
pub type BluetoothPublishedController<
    P,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> = ControllerHciBound<
    P,
    CriticalSectionRawMutex,
    PublishedEspHalBluetoothInterruptOwners,
    MODEM_TIMER_CAPACITY,
    SCHEDULER_CAPACITY,
    HOST_TO_CONTROLLER_DEPTH,
    CONTROLLER_TO_HOST_DEPTH,
    PACKET_CAPACITY,
>;

/// Stable process-lifetime storage for one final Bluetooth Controller epoch.
///
/// Final composition lends `&'static` references to interrupt dispatch and the
/// executor runners. The owner therefore cannot live on an application task
/// stack. A cold-start attempt must reserve the slot before claiming global
/// memory or touching MMIO; failed cold start deliberately leaves it consumed
/// because no complete Controller rollback has been proven.
pub struct BluetoothSystemStorage<
    P: 'static,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> {
    owner: StaticCell<
        BluetoothPublishedController<
            P,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    >,
    wakers: RuntimeWakers,
}

impl<
    P: 'static,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothSystemStorage<
        P,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
{
    /// Reserve empty storage without touching hardware or claiming resources.
    pub const fn new() -> Self {
        Self {
            owner: StaticCell::new(),
            wakers: RuntimeWakers::new(),
        }
    }

    /// Reserve this storage before beginning a non-cancellable cold start.
    pub fn reserve(
        &'static self,
    ) -> Result<
        BluetoothSystemSlot<
            P,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        BluetoothSystemStorageInUse,
    > {
        let owner = self.owner.try_uninit().ok_or(BluetoothSystemStorageInUse)?;
        Ok(BluetoothSystemSlot {
            owner,
            wakers: &self.wakers,
        })
    }
}

impl<
    P: 'static,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> Default
    for BluetoothSystemStorage<
        P,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
{
    fn default() -> Self {
        Self::new()
    }
}

/// Unique reservation for one final Controller cold-start attempt.
///
/// Dropping a slot does not make the backing storage reusable. This matches
/// the fail-stop hardware policy after any partially completed cold start.
#[must_use = "a reserved final slot must be filled or retained as a failed cold-start owner"]
pub struct BluetoothSystemSlot<
    P: 'static,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> {
    owner: &'static mut MaybeUninit<
        BluetoothPublishedController<
            P,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    >,
    wakers: &'static RuntimeWakers,
}

impl<
    P: 'static,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothSystemSlot<
        P,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
{
    /// Publish the final owner into this exact reservation and split it once.
    #[expect(
        clippy::result_large_err,
        reason = "construction failures retain the complete affine Controller epoch"
    )]
    pub fn compose(
        self,
        owner: BluetoothPublishedController<
            P,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        recheck: DtmAbsoluteRecheck,
    ) -> Result<
        BluetoothSystem<
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        BluetoothSystemBuildError<
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    > {
        let owner = self.owner.write(owner);
        compose_esp32s31_bluetooth_system(owner, self.wakers, recheck)
    }
}

/// The process-lifetime final Controller slot was already reserved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSystemStorageInUse;
