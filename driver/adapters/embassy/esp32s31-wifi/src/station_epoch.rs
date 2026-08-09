//! Persistent ESP32-S31 station resources between connected epochs.
//!
//! Connected shutdown returns a stopped RX service plus the peer-independent
//! network, hardware, aggregate-TX and control owners. A running scan may
//! temporarily split out only hardware and RX. Preparing the next join then
//! consumes the stopped RX service exactly once and separates its halted ring
//! from the staging resources needed by the next connected service.

use core::marker::PhantomData;

use embassy_sync::blocking_mutex::raw::RawMutex;

use crate::{
    preconnected_rx::{Esp32s31PreconnectedRx, Esp32s31PreconnectedRxDelay},
    rx_dma_service::{Esp32s31RxEpochResources, Esp32s31StoppedRx},
};

/// RX conversion required at the disconnected-to-reconnected boundary.
///
/// This interface exists so the station epoch does not expose every RX
/// storage lifetime and capacity as an argument of its own. The production
/// implementation below is the exact stopped ESP32-S31 RX owner.
pub trait Esp32s31StoppedStaRx {
    type Preconnected<D>
    where
        D: Esp32s31PreconnectedRxDelay;
    type Persistent;

    fn split_for_reconnect<D>(self) -> (Self::Preconnected<D>, Self::Persistent)
    where
        D: Esp32s31PreconnectedRxDelay;
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> Esp32s31StoppedStaRx
    for Esp32s31StoppedRx<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >
{
    type Preconnected<P>
        = Esp32s31PreconnectedRx<'storage, P, COUNT, DMA_BUFFER_SIZE>
    where
        P: Esp32s31PreconnectedRxDelay;
    type Persistent = Esp32s31RxEpochResources<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >;

    fn split_for_reconnect<P>(self) -> (Self::Preconnected<P>, Self::Persistent)
    where
        P: Esp32s31PreconnectedRxDelay,
    {
        let (ring, resources) = self.into_epoch_parts();
        (Esp32s31PreconnectedRx::from_halted(ring), resources)
    }
}

/// Complete peer-independent owner after a connected STA epoch has stopped.
pub struct Esp32s31DisconnectedStaEpoch<N, H, R, A, C> {
    network: N,
    hardware: H,
    rx: R,
    aggregate_tx: A,
    control: C,
}

impl<N, H, R, A, C> Esp32s31DisconnectedStaEpoch<N, H, R, A, C> {
    pub const fn new(network: N, hardware: H, rx: R, aggregate_tx: A, control: C) -> Self {
        Self {
            network,
            hardware,
            rx,
            aggregate_tx,
            control,
        }
    }

    pub const fn hardware(&self) -> &H {
        &self.hardware
    }

    pub const fn rx(&self) -> &R {
        &self.rx
    }

    /// Consume the peer-independent stopped frontier for role
    /// dematerialization or board-level resource regrouping.
    pub fn into_parts(self) -> Esp32s31DisconnectedStaEpochParts<N, H, R, A, C> {
        Esp32s31DisconnectedStaEpochParts {
            network: self.network,
            hardware: self.hardware,
            rx: self.rx,
            aggregate_tx: self.aggregate_tx,
            control: self.control,
        }
    }

    /// Split out only the capabilities used by a finite running scan.
    pub fn into_running_scan_parts(self) -> Esp32s31RunningScanEpochParts<N, H, R, A, C> {
        Esp32s31RunningScanEpochParts {
            retained: Esp32s31RunningScanRetained {
                network: self.network,
                aggregate_tx: self.aggregate_tx,
                control: self.control,
                _split: PhantomData,
            },
            hardware: self.hardware,
            rx: self.rx,
        }
    }
}

/// Complete value returned when the disconnected station epoch is
/// dematerialized. No field is optional because connected teardown has
/// already returned every child owner.
pub struct Esp32s31DisconnectedStaEpochParts<N, H, R, A, C> {
    pub network: N,
    pub hardware: H,
    pub rx: R,
    pub aggregate_tx: A,
    pub control: C,
}

impl<N, H, R, A, C> Esp32s31DisconnectedStaEpoch<N, H, R, A, C>
where
    R: Esp32s31StoppedStaRx,
{
    /// Consume the stopped RX service and form the next finite join epoch.
    pub fn prepare_reconnect<D>(
        self,
    ) -> (
        N,
        Esp32s31ReconnectedStaEpoch<H, R::Preconnected<D>, R::Persistent, A, C>,
    )
    where
        D: Esp32s31PreconnectedRxDelay,
    {
        let (rx, rx_resources) = self.rx.split_for_reconnect::<D>();
        (
            self.network,
            Esp32s31ReconnectedStaEpoch::new(
                self.hardware,
                rx,
                rx_resources,
                self.aggregate_tx,
                self.control,
            ),
        )
    }
}

/// Named split used while running scan temporarily owns hardware and RX.
pub struct Esp32s31RunningScanEpochParts<N, H, R, A, C> {
    pub retained: Esp32s31RunningScanRetained<N, H, R, A, C>,
    pub hardware: H,
    pub rx: R,
}

/// Owners which running scan cannot observe or replace.
pub struct Esp32s31RunningScanRetained<N, H, R, A, C> {
    network: N,
    aggregate_tx: A,
    control: C,
    _split: PhantomData<(H, R)>,
}

impl<N, H, R, A, C> Esp32s31RunningScanRetained<N, H, R, A, C> {
    /// Reunite the exact hardware and stopped RX returned by running scan.
    pub fn restore(self, hardware: H, rx: R) -> Esp32s31DisconnectedStaEpoch<N, H, R, A, C> {
        Esp32s31DisconnectedStaEpoch::new(
            self.network,
            hardware,
            rx,
            self.aggregate_tx,
            self.control,
        )
    }
}

/// Radio resources for Authentication, Association and WPA2 after reconnect.
pub struct Esp32s31ReconnectedStaEpoch<H, R, E, A, C> {
    hardware: H,
    rx: R,
    rx_resources: E,
    aggregate_tx: A,
    control: C,
}

impl<H, R, E, A, C> Esp32s31ReconnectedStaEpoch<H, R, E, A, C> {
    /// Reassemble the exact stopped reconnect frontier after a role-neutral
    /// Wi-Fi transition returned and republished its hardware owner.
    pub const fn new(hardware: H, rx: R, rx_resources: E, aggregate_tx: A, control: C) -> Self {
        Self {
            hardware,
            rx,
            rx_resources,
            aggregate_tx,
            control,
        }
    }

    /// Borrow the only resources finite join phases are allowed to mutate.
    pub fn hardware_and_rx_mut(&mut self) -> (&mut H, &mut R) {
        (&mut self.hardware, &mut self.rx)
    }

    /// Consume the successful join frontier for connected-service assembly.
    pub fn into_parts(self) -> Esp32s31ReconnectedStaEpochParts<H, R, E, A, C> {
        Esp32s31ReconnectedStaEpochParts {
            hardware: self.hardware,
            rx: self.rx,
            rx_resources: self.rx_resources,
            aggregate_tx: self.aggregate_tx,
            control: self.control,
        }
    }
}

/// Named connected-assembly frontier returned after a successful finite join.
pub struct Esp32s31ReconnectedStaEpochParts<H, R, E, A, C> {
    pub hardware: H,
    pub rx: R,
    pub rx_resources: E,
    pub aggregate_tx: A,
    pub control: C,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDelay;

    impl Esp32s31PreconnectedRxDelay for TestDelay {
        async fn after_micros(_micros: u32) {}
    }

    struct StoppedRx(u8);

    impl Esp32s31StoppedStaRx for StoppedRx {
        type Preconnected<D>
            = (u8, PhantomData<D>)
        where
            D: Esp32s31PreconnectedRxDelay;
        type Persistent = u16;

        fn split_for_reconnect<D>(self) -> (Self::Preconnected<D>, Self::Persistent)
        where
            D: Esp32s31PreconnectedRxDelay,
        {
            ((self.0, PhantomData), u16::from(self.0) + 100)
        }
    }

    #[test]
    fn running_scan_round_trip_and_reconnect_preserve_every_owner() {
        let disconnected =
            Esp32s31DisconnectedStaEpoch::new("network", "hardware", StoppedRx(7), 8, 9);
        let scan = disconnected.into_running_scan_parts();
        assert_eq!(scan.hardware, "hardware");
        assert_eq!(scan.rx.0, 7);

        let disconnected = scan.retained.restore(scan.hardware, scan.rx);
        assert_eq!(disconnected.hardware(), &"hardware");
        assert_eq!(disconnected.rx().0, 7);

        let (network, reconnected) = disconnected.prepare_reconnect::<TestDelay>();
        assert_eq!(network, "network");
        let parts = reconnected.into_parts();
        assert_eq!(parts.hardware, "hardware");
        assert_eq!(parts.rx.0, 7);
        assert_eq!(parts.rx_resources, 107);
        assert_eq!(parts.aggregate_tx, 8);
        assert_eq!(parts.control, 9);
    }
}
