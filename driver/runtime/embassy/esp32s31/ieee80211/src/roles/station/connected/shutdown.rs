//! Ordered executor shutdown frontier for one connected station epoch.
//!
//! A returned radio runner proves only that it no longer starts new hardware
//! work. The interrupt route may still publish wakes. Protocol ownership is
//! part of the runner's RX service, so closing IRQ publication is the sole
//! asynchronous frontier before driver teardown.

use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::blocking_mutex::raw::RawMutex as NetworkRawMutex;
use open_esp_radio_esp32s31_wifi_mac::crypto::CcmpKeyHardware;
use open_esp_radio_esp32s31_wifi_mac::irq::MacInterruptRoute;

use crate::{
    datapath::irq::{
        Esp32s31MacInterruptEpoch, Esp32s31MacInterruptEpochDrain,
        Esp32s31MacInterruptEpochQuiesceError,
    },
    datapath::services::SingleRoleServices,
    datapath::{DatapathRunner, DatapathServices},
    roles::station::teardown::{
        Esp32s31ConnectedStaControlTeardown, Esp32s31ConnectedStaGroupSecurity,
        Esp32s31ConnectedStaRxPark, Esp32s31ConnectedStaTeardownFailure,
        Esp32s31ConnectedStaTeardownPort, Esp32s31ConnectedStaTeardownSuccess,
        Esp32s31ConnectedStaTxTeardown,
    },
};

/// Consuming owner interface needed by the common shutdown transaction.
///
/// The trait deliberately exposes no service methods. A caller can only
/// recover the network and driver owners after IRQ publication and every
/// attached task have been proved quiescent.
pub trait Esp32s31ConnectedEpochRunnerOwner: Sized {
    type Network;
    type Services;

    fn into_connected_epoch_parts(self) -> (Self::Network, Self::Services);
}

impl<'irq, M, N, B, RX> Esp32s31ConnectedEpochRunnerOwner for DatapathRunner<'irq, M, N, B, RX>
where
    M: NetworkRawMutex,
    N: crate::datapath::network::DatapathNetwork,
    B: DatapathServices<N::TxFrame, N::PhysicalTxFrame>,
    RX: crate::datapath::network::DatapathNetworkRxSet,
{
    type Network = N;
    type Services = B;

    fn into_connected_epoch_parts(self) -> (Self::Network, Self::Services) {
        self.into_parts()
    }
}

/// Complete reusable frontier after connected executor activity has stopped.
pub struct Esp32s31ConnectedEpochQuiesced<I, N, S> {
    pub interrupt: I,
    pub interrupt_drain: Esp32s31MacInterruptEpochDrain,
    pub network: N,
    pub services: S,
}

impl<I, N, S> Esp32s31ConnectedEpochQuiesced<I, N, S> {
    /// Replace an observation/fault decorator without exposing any other
    /// returned owner. HIL uses this to remove its services wrapper before the
    /// same production teardown transaction as ordinary firmware.
    pub fn map_services<U>(
        self,
        map: impl FnOnce(S) -> U,
    ) -> Esp32s31ConnectedEpochQuiesced<I, N, U> {
        Esp32s31ConnectedEpochQuiesced {
            interrupt: self.interrupt,
            interrupt_drain: self.interrupt_drain,
            network: self.network,
            services: map(self.services),
        }
    }
}

/// Complete reusable connected frontier after driver teardown succeeds.
pub struct Esp32s31ConnectedEpochTeardown<I, N, D> {
    pub interrupt: I,
    pub interrupt_drain: Esp32s31MacInterruptEpochDrain,
    pub network: N,
    pub driver: D,
}

/// Owner-preserving quarantined frontier after IRQ stopped but driver
/// teardown could not complete.
pub struct Esp32s31ConnectedEpochTeardownFailure<I, N, E> {
    pub interrupt: I,
    pub interrupt_drain: Esp32s31MacInterruptEpochDrain,
    pub network: N,
    pub error: E,
}

impl<I, N, H, R, X, C> Esp32s31ConnectedEpochQuiesced<I, N, SingleRoleServices<H, R, X, C>>
where
    H: CcmpKeyHardware,
    C: Esp32s31ConnectedStaControlTeardown<H, X>,
    R: Esp32s31ConnectedStaRxPark<H>,
    X: Esp32s31ConnectedStaTxTeardown,
{
    /// Stop control, RX DMA and TX, then clear both association keys while
    /// retaining network and task owners on every failure.
    #[allow(clippy::type_complexity, clippy::result_large_err)]
    pub fn try_teardown(
        self,
        group_security: Esp32s31ConnectedStaGroupSecurity,
    ) -> Result<
        Esp32s31ConnectedEpochTeardown<
            I,
            N,
            Esp32s31ConnectedStaTeardownSuccess<
                H,
                R::Parked,
                X::Resources,
                X::Aggregate,
                C::Report,
            >,
        >,
        Esp32s31ConnectedEpochTeardownFailure<
            I,
            N,
            Esp32s31ConnectedStaTeardownFailure<H, R, R::Parked, X, C, C::Error, R::Error>,
        >,
    > {
        let Self {
            interrupt,
            interrupt_drain,
            network,
            services,
        } = self;
        match Esp32s31ConnectedStaTeardownPort::try_teardown(services, group_security) {
            Ok(driver) => Ok(Esp32s31ConnectedEpochTeardown {
                interrupt,
                interrupt_drain,
                network,
                driver,
            }),
            Err(error) => Err(Esp32s31ConnectedEpochTeardownFailure {
                interrupt,
                interrupt_drain,
                network,
                error,
            }),
        }
    }
}

/// Hardware-quiescence failure retaining the exact radio runner.
///
/// The only fallible edge here is disabling and draining the hardware
/// interrupt route.
pub enum Esp32s31ConnectedEpochQuiesceFailure<I, C, E> {
    Interrupt {
        error: Esp32s31MacInterruptEpochQuiesceError<E>,
        interrupt: I,
        runner: C,
    },
}

/// Park the logical IRQ consumer, then reveal the radio runner's network and
/// driver owners.
///
/// The runner has already reached its finite connected exit before this call.
/// The physical MAC route remains installed across role cutovers. Keeping the
/// runner opaque until its coalesced publications are drained prevents the
/// next consumer from interpreting stale work while preserving service for a
/// still-powered MAC.
#[allow(
    clippy::type_complexity,
    reason = "the public result retains the exact IRQ, network, services, and retry owners"
)]
pub fn quiesce_esp32s31_connected_epoch<'runtime, R, M, C>(
    mut interrupt: Esp32s31MacInterruptEpoch<'runtime, R, M>,
    platform: &R::Platform,
    runner: C,
) -> Result<
    Esp32s31ConnectedEpochQuiesced<
        Esp32s31MacInterruptEpoch<'runtime, R, M>,
        C::Network,
        C::Services,
    >,
    Esp32s31ConnectedEpochQuiesceFailure<Esp32s31MacInterruptEpoch<'runtime, R, M>, C, R::Error>,
>
where
    R: MacInterruptRoute,
    M: RawMutex,
    C: Esp32s31ConnectedEpochRunnerOwner,
{
    let _ = platform;
    let interrupt_drain = match interrupt.park() {
        Ok(drain) => drain,
        Err(error) => {
            return Err(Esp32s31ConnectedEpochQuiesceFailure::Interrupt {
                error,
                interrupt,
                runner,
            });
        }
    };
    let (network, services) = runner.into_connected_epoch_parts();
    Ok(Esp32s31ConnectedEpochQuiesced {
        interrupt,
        interrupt_drain,
        network,
        services,
    })
}

#[cfg(test)]
mod tests;
