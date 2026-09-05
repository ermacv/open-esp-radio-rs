//! Exact owner transition from a successful join into connected RX.
//!
//! The first association owns raw PAC registers and one-time static resources.
//! A later association owns the exact cooperative hardware and persistent RX,
//! aggregate-TX and control resources returned by the preceding teardown.  This
//! module is the only place which may turn either frontier into the uniform
//! connected owner set.

use core::future::Future;

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_esp32s31_hal::RadioRuntimeOwner;
use open_esp_radio_esp32s31_hal::radio_arena::{
    Esp32s31RadioOwnerArena, Esp32s31RadioOwnerArenaError,
};
use open_esp_radio_esp32s31_wifi::cooperative_hardware::CooperativeRadioHardware;

use crate::{
    datapath::rx::dma::{Esp32s31RxEpochResources, Esp32s31StagedRxProducer},
    datapath::rx::frontier::{
        Esp32s31RxFrontier, Esp32s31RxFrontierDelay, Esp32s31RxFrontierError,
    },
    roles::station::epoch::{Esp32s31ReconnectedStaEpoch, Esp32s31ReconnectedStaEpochParts},
};

/// Whether RX promotion was attempted for the first or a later association.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ConnectedEpochStartPhase {
    Initial,
    Reconnected,
}

/// One-time resources consumed only by the first connected association.
///
/// Supplying these only to [`start_esp32s31_initial_connected_epoch`]
/// prevents reconnect from initializing the same static cells again.
pub struct Esp32s31InitialConnectedEpochResources<'arena, X, A, C> {
    registers: &'arena Esp32s31RadioOwnerArena,
    rx: X,
    aggregate_tx: A,
    control: C,
}

impl<'arena, X, A, C> Esp32s31InitialConnectedEpochResources<'arena, X, A, C> {
    pub const fn new(
        registers: &'arena Esp32s31RadioOwnerArena,
        rx: X,
        aggregate_tx: A,
        control: C,
    ) -> Self {
        Self {
            registers,
            rx,
            aggregate_tx,
            control,
        }
    }
}

/// Uniform connected owner set produced for both initial and reconnected
/// associations.
pub struct Esp32s31ConnectedEpochStarted<H, X, A, C> {
    pub hardware: H,
    pub rx: X,
    pub aggregate_tx: A,
    pub control: C,
}

/// Complete owner return when connected materialization cannot finish.
///
/// Register publication failure retains raw PAC ownership. RX promotion
/// failure retains the published cooperative owner together with the exact
/// pre-connected and persistent resources. Neither case can be mistaken for a
/// successfully running connected epoch.
#[allow(clippy::large_enum_variant)]
pub enum Esp32s31ConnectedEpochStartFailure<I, H, P, X, A, C, E> {
    RegisterPublication {
        error: Esp32s31RadioOwnerArenaError,
        hardware: RadioRuntimeOwner,
        receive: P,
        initial: I,
    },
    Receive {
        phase: Esp32s31ConnectedEpochStartPhase,
        error: E,
        hardware: H,
        receive: P,
        rx_resources: X,
        aggregate_tx: A,
        control: C,
    },
}

/// Internal capability for joining a pre-connected RX frontier to its
/// persistent connected resources.
///
/// The trait exists to keep every storage lifetime and capacity out of the
/// public connected-epoch transition. Its production implementation below is
/// still concrete and returns both owners on failure.
#[doc(hidden)]
pub trait Esp32s31ConnectedRxMaterializer<H, P>: Sized {
    type Connected;
    type Error;

    fn materialize(
        self,
        receive: P,
        hardware: &mut H,
    ) -> impl Future<Output = Result<Self::Connected, (P, Self, Self::Error)>>;
}

impl<
    'arena,
    'storage,
    'pool,
    'queue,
    PD,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>
    Esp32s31ConnectedRxMaterializer<
        CooperativeRadioHardware<'arena>,
        Esp32s31RxFrontier<'storage, PD, COUNT, DMA_BUFFER_SIZE>,
    >
    for Esp32s31RxEpochResources<
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
where
    PD: Esp32s31RxFrontierDelay,
{
    type Connected = Esp32s31StagedRxProducer<
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
    type Error = Esp32s31RxFrontierError;

    async fn materialize(
        self,
        receive: Esp32s31RxFrontier<'storage, PD, COUNT, DMA_BUFFER_SIZE>,
        hardware: &mut CooperativeRadioHardware<'arena>,
    ) -> Result<
        Self::Connected,
        (
            Esp32s31RxFrontier<'storage, PD, COUNT, DMA_BUFFER_SIZE>,
            Self,
            Self::Error,
        ),
    > {
        let storage = self.storage();
        match receive.try_into_live_with_storage(hardware, storage).await {
            Ok(ring) => Ok(self.with_live_ring(ring)),
            Err(failure) => Err((failure.owner, self, failure.error)),
        }
    }
}

/// Materialize the first connected owner from raw PAC and the complete
/// initial-only resource graph.
///
/// Reconnect cannot call this function, so absence or repeated construction
/// of initial static resources is not representable at this boundary.
#[allow(clippy::type_complexity)]
pub async fn start_esp32s31_initial_connected_epoch<'arena, P, X, A, C>(
    hardware: RadioRuntimeOwner,
    receive: P,
    initial: Esp32s31InitialConnectedEpochResources<'arena, X, A, C>,
) -> Result<
    Esp32s31ConnectedEpochStarted<CooperativeRadioHardware<'arena>, X::Connected, A, C>,
    Esp32s31ConnectedEpochStartFailure<
        Esp32s31InitialConnectedEpochResources<'arena, X, A, C>,
        CooperativeRadioHardware<'arena>,
        P,
        X,
        A,
        C,
        X::Error,
    >,
>
where
    X: Esp32s31ConnectedRxMaterializer<CooperativeRadioHardware<'arena>, P>,
{
    let published = match initial.registers.publish(hardware) {
        Ok(published) => published,
        Err(failure) => {
            return Err(Esp32s31ConnectedEpochStartFailure::RegisterPublication {
                error: failure.error,
                hardware: failure.owner,
                receive,
                initial,
            });
        }
    };
    materialize_esp32s31_connected_rx(
        Esp32s31ConnectedEpochStartPhase::Initial,
        CooperativeRadioHardware::new(published),
        receive,
        initial.rx,
        initial.aggregate_tx,
        initial.control,
    )
    .await
}

/// Restore a later connected owner exclusively from a completed disconnected
/// epoch. No initial resource factory exists on this path.
#[allow(clippy::type_complexity)]
pub async fn start_esp32s31_reconnected_connected_epoch<'arena, P, X, A, C>(
    epoch: Esp32s31ReconnectedStaEpoch<CooperativeRadioHardware<'arena>, P, X, A, C>,
) -> Result<
    Esp32s31ConnectedEpochStarted<CooperativeRadioHardware<'arena>, X::Connected, A, C>,
    Esp32s31ConnectedEpochStartFailure<
        Esp32s31InitialConnectedEpochResources<'arena, X, A, C>,
        CooperativeRadioHardware<'arena>,
        P,
        X,
        A,
        C,
        X::Error,
    >,
>
where
    X: Esp32s31ConnectedRxMaterializer<CooperativeRadioHardware<'arena>, P>,
{
    let Esp32s31ReconnectedStaEpochParts {
        hardware,
        rx,
        rx_resources,
        aggregate_tx,
        control,
    } = epoch.into_parts();
    materialize_esp32s31_connected_rx(
        Esp32s31ConnectedEpochStartPhase::Reconnected,
        hardware,
        rx,
        rx_resources,
        aggregate_tx,
        control,
    )
    .await
}

async fn materialize_esp32s31_connected_rx<'arena, P, X, A, C>(
    phase: Esp32s31ConnectedEpochStartPhase,
    mut hardware: CooperativeRadioHardware<'arena>,
    receive: P,
    rx_resources: X,
    aggregate_tx: A,
    control: C,
) -> Result<
    Esp32s31ConnectedEpochStarted<CooperativeRadioHardware<'arena>, X::Connected, A, C>,
    Esp32s31ConnectedEpochStartFailure<
        Esp32s31InitialConnectedEpochResources<'arena, X, A, C>,
        CooperativeRadioHardware<'arena>,
        P,
        X,
        A,
        C,
        X::Error,
    >,
>
where
    X: Esp32s31ConnectedRxMaterializer<CooperativeRadioHardware<'arena>, P>,
{
    match rx_resources.materialize(receive, &mut hardware).await {
        Ok(rx) => Ok(Esp32s31ConnectedEpochStarted {
            hardware,
            rx,
            aggregate_tx,
            control,
        }),
        Err((receive, rx_resources, error)) => Err(Esp32s31ConnectedEpochStartFailure::Receive {
            phase,
            error,
            hardware,
            receive,
            rx_resources,
            aggregate_tx,
            control,
        }),
    }
}

#[cfg(test)]
mod tests;
