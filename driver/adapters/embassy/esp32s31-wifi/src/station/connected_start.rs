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
use open_esp_radio_esp32s31_wifi_sta::cooperative_hardware::CooperativeRadioHardware;

use crate::{
    preconnected_rx::{
        Esp32s31PreconnectedRx, Esp32s31PreconnectedRxDelay, Esp32s31PreconnectedRxError,
    },
    rx_dma_service::{Esp32s31ConnectedRx, Esp32s31RxEpochResources},
    station_epoch::{Esp32s31ReconnectedStaEpoch, Esp32s31ReconnectedStaEpochParts},
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
        Esp32s31PreconnectedRx<'storage, PD, COUNT, DMA_BUFFER_SIZE>,
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
    PD: Esp32s31PreconnectedRxDelay,
{
    type Connected = Esp32s31ConnectedRx<
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
    type Error = Esp32s31PreconnectedRxError;

    async fn materialize(
        self,
        receive: Esp32s31PreconnectedRx<'storage, PD, COUNT, DMA_BUFFER_SIZE>,
        hardware: &mut CooperativeRadioHardware<'arena>,
    ) -> Result<
        Self::Connected,
        (
            Esp32s31PreconnectedRx<'storage, PD, COUNT, DMA_BUFFER_SIZE>,
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
mod tests {
    use embassy_futures::block_on;
    use open_esp_radio_esp32s31_hal::{Radio, wifi_bb::PhyWifiBbControl};

    use crate::station_epoch::{Esp32s31DisconnectedStaEpoch, Esp32s31StoppedStaRx};

    use super::*;

    struct TestRxResources {
        value: u8,
        fail: bool,
    }

    struct TestRadioPeripheral;

    impl PhyWifiBbControl for TestRadioPeripheral {
        fn clear_cold_start_wifi_control(&mut self) {}
        fn wifi_baseband_is_enabled(&self) -> bool {
            false
        }
        fn set_wifi_baseband_enabled(&mut self, _enabled: bool) {}
        fn set_bss_cbw_40_digital(&mut self, _enabled: bool) {}
        fn set_bb_agc_update_encoding(&mut self, _encoding: u8) {}
        fn set_mac_baseband_enabled(&mut self, _enabled: bool) {}
    }

    impl<'arena> Esp32s31ConnectedRxMaterializer<CooperativeRadioHardware<'arena>, u8>
        for TestRxResources
    {
        type Connected = (u8, u8);
        type Error = u8;

        async fn materialize(
            self,
            receive: u8,
            _hardware: &mut CooperativeRadioHardware<'arena>,
        ) -> Result<Self::Connected, (u8, Self, Self::Error)> {
            if self.fail {
                Err((receive, self, 99))
            } else {
                Ok((receive, self.value))
            }
        }
    }

    struct TestPreconnectedRx;
    struct TestDelay;

    impl Esp32s31PreconnectedRxDelay for TestDelay {
        async fn after_micros(_micros: u32) {}
    }

    impl Esp32s31StoppedStaRx for TestPreconnectedRx {
        type Preconnected<D>
            = u8
        where
            D: Esp32s31PreconnectedRxDelay;
        type Persistent = TestRxResources;

        fn split_for_reconnect<D>(self) -> (Self::Preconnected<D>, Self::Persistent)
        where
            D: Esp32s31PreconnectedRxDelay,
        {
            (
                17,
                TestRxResources {
                    value: 18,
                    fail: false,
                },
            )
        }
    }

    #[test]
    fn connected_start_unifies_initial_and_reconnected_owner_frontiers() {
        let radio = Radio::claim(TestRadioPeripheral)
            .unwrap_or_else(|_| panic!("radio singleton must be free for connected-start test"))
            .assume_powered_after_external_initialization()
            .into_running();
        let (_platform, registers, _interrupt_setup) = radio.into_runtime_parts();
        let arena = Esp32s31RadioOwnerArena::new();

        let started = block_on(start_esp32s31_initial_connected_epoch(
            registers,
            7,
            Esp32s31InitialConnectedEpochResources::new(
                &arena,
                TestRxResources {
                    value: 8,
                    fail: false,
                },
                9_u16,
                10_u32,
            ),
        ))
        .unwrap_or_else(|_| panic!("initial owner transition must succeed"));
        assert_eq!(started.rx, (7, 8));
        assert_eq!(started.aggregate_tx, 9);
        assert_eq!(started.control, 10);
        let reclaimed = started
            .hardware
            .try_into_reclaimed_registers()
            .unwrap_or_else(|_| {
                panic!("initial transition must return the PAC owner and arena binding")
            });
        let published = reclaimed.try_republish().unwrap_or_else(|_| {
            panic!("reconnected test must use the exact returned arena binding")
        });
        let disconnected = Esp32s31DisconnectedStaEpoch::new(
            (),
            CooperativeRadioHardware::new(published),
            TestPreconnectedRx,
            19_u16,
            20_u32,
        );
        let (_, reconnected) = disconnected.prepare_reconnect::<TestDelay>();
        let started = block_on(start_esp32s31_reconnected_connected_epoch(reconnected))
            .unwrap_or_else(|_| panic!("reconnected owner transition must succeed"));
        assert_eq!(started.rx, (17, 18));
        assert_eq!(started.aggregate_tx, 19);
        assert_eq!(started.control, 20);
        let registers = started
            .hardware
            .try_into_reclaimed_registers()
            .unwrap_or_else(|_| panic!("reconnected transition must retain its arena binding"))
            .into_owner();

        let failure = block_on(start_esp32s31_initial_connected_epoch(
            registers,
            21,
            Esp32s31InitialConnectedEpochResources::new(
                &arena,
                TestRxResources {
                    value: 22,
                    fail: true,
                },
                23_u16,
                24_u32,
            ),
        ))
        .err()
        .unwrap_or_else(|| panic!("RX promotion failure must return every owner"));
        match failure {
            Esp32s31ConnectedEpochStartFailure::Receive {
                phase,
                error,
                hardware,
                receive,
                rx_resources,
                aggregate_tx,
                control,
            } => {
                assert_eq!(phase, Esp32s31ConnectedEpochStartPhase::Initial);
                assert_eq!(error, 99);
                assert_eq!(receive, 21);
                assert_eq!(rx_resources.value, 22);
                assert_eq!(aggregate_tx, 23);
                assert_eq!(control, 24);
                let _registers = hardware.try_into_registers().unwrap_or_else(|_| {
                    panic!("failed RX promotion must retain the published PAC owner")
                });
            }
            Esp32s31ConnectedEpochStartFailure::RegisterPublication { .. } => {
                panic!("empty arena must accept the returned PAC owner")
            }
        }
    }
}
