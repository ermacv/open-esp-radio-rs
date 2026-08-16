//! Ordered executor shutdown frontier for one connected station epoch.
//!
//! A returned radio runner proves only that it no longer starts new hardware
//! work. The interrupt route may still publish wakes and a protocol task may
//! still borrow staging/scratch resources. This transaction closes those two
//! frontiers in the only reusable order and preserves the complete runner on
//! every hardware-quiescence failure.

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_embassy_net::RawMutex as NetworkRawMutex;
use open_esp_radio_esp32s31_wifi_mac::crypto::{CcmpKeyHardware, StaGroupCcmpSlot};
use open_esp_radio_esp32s31_wifi_mac::irq::MacInterruptRoute;
use open_esp_radio_wifi_embassy::connected_tasks::{ConnectedTaskGroup, stop_connected_task_group};

use crate::{
    connected_sta_teardown::{
        Esp32s31ConnectedStaControlTeardown, Esp32s31ConnectedStaRxTeardown,
        Esp32s31ConnectedStaTeardownFailure, Esp32s31ConnectedStaTeardownPort,
        Esp32s31ConnectedStaTeardownSuccess, Esp32s31ConnectedStaTxTeardown,
    },
    embassy_irq::{
        Esp32s31MacInterruptEpoch, Esp32s31MacInterruptEpochDrain,
        Esp32s31MacInterruptEpochQuiesceError,
    },
    wdev::services::WdevServiceSet,
    wdev::{WdevRunner, WdevServices},
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

impl<
    'resources,
    'irq,
    M,
    N,
    B,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> Esp32s31ConnectedEpochRunnerOwner
    for WdevRunner<
        'resources,
        'irq,
        M,
        N,
        B,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
    >
where
    M: NetworkRawMutex,
    N: crate::wdev::WdevNetwork<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            RX_QUEUE_DEPTH,
            TX_QUEUE_DEPTH,
        >,
    B: WdevServices<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
{
    type Network = N;
    type Services = B;

    fn into_connected_epoch_parts(self) -> (Self::Network, Self::Services) {
        self.into_parts()
    }
}

/// Complete reusable frontier after connected executor activity has stopped.
pub struct Esp32s31ConnectedEpochQuiesced<I, N, S, T> {
    pub interrupt: I,
    pub interrupt_drain: Esp32s31MacInterruptEpochDrain,
    pub network: N,
    pub services: S,
    pub tasks: T,
}

impl<I, N, S, T> Esp32s31ConnectedEpochQuiesced<I, N, S, T> {
    /// Replace an observation/fault decorator without exposing any other
    /// returned owner. HIL uses this to remove its services wrapper before the
    /// same production teardown transaction as ordinary firmware.
    pub fn map_services<U>(
        self,
        map: impl FnOnce(S) -> U,
    ) -> Esp32s31ConnectedEpochQuiesced<I, N, U, T> {
        Esp32s31ConnectedEpochQuiesced {
            interrupt: self.interrupt,
            interrupt_drain: self.interrupt_drain,
            network: self.network,
            services: map(self.services),
            tasks: self.tasks,
        }
    }
}

/// Complete reusable connected frontier after driver teardown succeeds.
pub struct Esp32s31ConnectedEpochTeardown<I, N, T, D> {
    pub interrupt: I,
    pub interrupt_drain: Esp32s31MacInterruptEpochDrain,
    pub network: N,
    pub tasks: T,
    pub driver: D,
}

/// Owner-preserving quarantined frontier after IRQ/tasks stopped but driver
/// teardown could not complete.
pub struct Esp32s31ConnectedEpochTeardownFailure<I, N, T, E> {
    pub interrupt: I,
    pub interrupt_drain: Esp32s31MacInterruptEpochDrain,
    pub network: N,
    pub tasks: T,
    pub error: E,
}

impl<I, N, H, R, X, C, T> Esp32s31ConnectedEpochQuiesced<I, N, WdevServiceSet<H, R, X, C>, T>
where
    H: CcmpKeyHardware,
    C: Esp32s31ConnectedStaControlTeardown<H, X>,
    R: Esp32s31ConnectedStaRxTeardown<H>,
    X: Esp32s31ConnectedStaTxTeardown,
{
    /// Stop control, RX DMA and TX, then clear both association keys while
    /// retaining network and task owners on every failure.
    #[allow(clippy::type_complexity, clippy::result_large_err)]
    pub fn try_teardown(
        self,
        group_key: StaGroupCcmpSlot,
    ) -> Result<
        Esp32s31ConnectedEpochTeardown<
            I,
            N,
            T,
            Esp32s31ConnectedStaTeardownSuccess<
                H,
                R::Stopped,
                X::Resources,
                X::Aggregate,
                C::Report,
            >,
        >,
        Esp32s31ConnectedEpochTeardownFailure<
            I,
            N,
            T,
            Esp32s31ConnectedStaTeardownFailure<H, R, R::Stopped, X, C, C::Error, R::Error>,
        >,
    > {
        let Self {
            interrupt,
            interrupt_drain,
            network,
            services,
            tasks,
        } = self;
        match Esp32s31ConnectedStaTeardownPort::try_teardown(services, group_key) {
            Ok(driver) => Ok(Esp32s31ConnectedEpochTeardown {
                interrupt,
                interrupt_drain,
                network,
                tasks,
                driver,
            }),
            Err(error) => Err(Esp32s31ConnectedEpochTeardownFailure {
                interrupt,
                interrupt_drain,
                network,
                tasks,
                error,
            }),
        }
    }
}

/// Hardware-quiescence failure retaining the exact radio runner.
///
/// Software tasks stay in the explicit stopping transaction until they return
/// their owners. The only fallible edge here is disabling and draining the
/// hardware interrupt route.
pub enum Esp32s31ConnectedEpochQuiesceFailure<I, C, G, E> {
    Interrupt {
        error: Esp32s31MacInterruptEpochQuiesceError<E>,
        interrupt: I,
        runner: C,
        tasks: G,
    },
}

/// Close IRQ publication, return all attached task owners, then reveal the
/// radio runner's network and driver owners.
///
/// The runner has already reached its finite connected exit before this call.
/// Keeping it opaque until both later frontiers close prevents a composition
/// root from accidentally stopping RX DMA while an ISR or protocol task can
/// still observe the epoch.
pub async fn quiesce_esp32s31_connected_epoch<'runtime, R, M, C, G>(
    mut interrupt: Esp32s31MacInterruptEpoch<'runtime, R, M>,
    platform: &R::Platform,
    runner: C,
    mut tasks: G,
) -> Result<
    Esp32s31ConnectedEpochQuiesced<
        Esp32s31MacInterruptEpoch<'runtime, R, M>,
        C::Network,
        C::Services,
        G::Stopped,
    >,
    Esp32s31ConnectedEpochQuiesceFailure<Esp32s31MacInterruptEpoch<'runtime, R, M>, C, G, R::Error>,
>
where
    R: MacInterruptRoute,
    M: RawMutex,
    C: Esp32s31ConnectedEpochRunnerOwner,
    G: ConnectedTaskGroup,
{
    let interrupt_drain = match interrupt.quiesce(platform) {
        Ok(drain) => drain,
        Err(error) => {
            return Err(Esp32s31ConnectedEpochQuiesceFailure::Interrupt {
                error,
                interrupt,
                runner,
                tasks,
            });
        }
    };
    let stopped = stop_connected_task_group(&mut tasks).await;
    let (network, services) = runner.into_connected_epoch_parts();
    Ok(Esp32s31ConnectedEpochQuiesced {
        interrupt,
        interrupt_drain,
        network,
        services,
        tasks: stopped,
    })
}

#[cfg(test)]
mod tests {
    use core::future::ready;

    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use open_esp_radio_esp32s31_hal::types::MacKeyInstallOutcome;
    use open_esp_radio_esp32s31_wifi_mac::{
        crypto::{install_sta_group_ccmp, install_sta_pairwise_ccmp},
        irq::MacInterruptRoute,
    };

    use super::*;
    use crate::{
        aggregate_tx::Esp32s31ConnectedTxTeardownParts,
        embassy_irq::{EmbassyMacIrqRuntime, EmbassyPowerIrqRuntime},
    };

    struct TestRoute {
        active: bool,
    }

    impl MacInterruptRoute for TestRoute {
        type Platform = ();
        type Setup = u32;
        type Error = u8;

        fn activate(
            &mut self,
            _platform: &Self::Platform,
            setup: Self::Setup,
            _event_mask: open_esp_radio_esp32s31_hal::types::MacInterruptMask,
        ) -> Result<(), (Self::Error, Self::Setup)> {
            self.active = true;
            let _ = setup;
            Ok(())
        }

        fn quiesce(&mut self, _platform: &Self::Platform) -> Result<Self::Setup, Self::Error> {
            self.active = false;
            Ok(41)
        }
    }

    struct TestRunner(u32, u16);

    impl Esp32s31ConnectedEpochRunnerOwner for TestRunner {
        type Network = u32;
        type Services = u16;

        fn into_connected_epoch_parts(self) -> (Self::Network, Self::Services) {
            (self.0, self.1)
        }
    }

    struct ReadyTasks(Option<u8>);

    impl ConnectedTaskGroup for ReadyTasks {
        type Stopped = u8;

        fn request_stop(&mut self) {}

        async fn wait_stopped(&mut self) -> Self::Stopped {
            ready(self.0.take().expect("task owner returns once")).await
        }
    }

    fn active_epoch() -> Esp32s31MacInterruptEpoch<'static, TestRoute, CriticalSectionRawMutex> {
        static MAC: EmbassyMacIrqRuntime<CriticalSectionRawMutex> = EmbassyMacIrqRuntime::new();
        static POWER: EmbassyPowerIrqRuntime<CriticalSectionRawMutex> =
            EmbassyPowerIrqRuntime::new();
        let mut epoch =
            Esp32s31MacInterruptEpoch::new(TestRoute { active: false }, 40, &MAC, &POWER);
        epoch
            .activate(
                &(),
                open_esp_radio_esp32s31_hal::types::MacInterruptMask::COLD_RX,
            )
            .expect("test route activates");
        epoch
    }

    #[test]
    fn reusable_parts_are_revealed_only_after_irq_and_tasks_stop() {
        let epoch = active_epoch();
        let tasks = ReadyTasks(Some(9));
        let stopped = block_on(quiesce_esp32s31_connected_epoch(
            epoch,
            &(),
            TestRunner(7, 8),
            tasks,
        ))
        .unwrap_or_else(|_| panic!("ready shutdown must succeed"));
        assert_eq!(stopped.network, 7);
        assert_eq!(stopped.services, 8);
        assert_eq!(stopped.tasks, 9);
        assert!(!stopped.interrupt.is_active());
    }

    #[derive(Default)]
    struct TeardownHardware {
        cleared: std::vec::Vec<u8>,
    }

    impl CcmpKeyHardware for TeardownHardware {
        fn install_sta_ccmp_entry(&mut self, _index: u8, _words: [u32; 6]) -> MacKeyInstallOutcome {
            MacKeyInstallOutcome::Installed
        }

        fn clear_ccmp_entry(&mut self, index: u8) {
            self.cleared.push(index);
        }
    }

    struct TeardownControl {
        fail: bool,
    }

    impl Esp32s31ConnectedStaControlTeardown<TeardownHardware, TeardownTx> for TeardownControl {
        type Report = u8;
        type Error = u8;

        fn shutdown(
            &mut self,
            _hardware: &mut TeardownHardware,
            _tx: &mut TeardownTx,
        ) -> Result<Self::Report, Self::Error> {
            if self.fail { Err(1) } else { Ok(2) }
        }
    }

    struct TeardownRx {
        fail: bool,
    }

    impl Esp32s31ConnectedStaRxTeardown<TeardownHardware> for TeardownRx {
        type Stopped = u8;
        type Error = u8;

        fn try_stop(
            self,
            _hardware: &mut TeardownHardware,
        ) -> Result<Self::Stopped, (Self, Self::Error)> {
            if self.fail { Err((self, 3)) } else { Ok(4) }
        }
    }

    struct TeardownTx {
        active: bool,
        pairwise: Option<open_esp_radio_esp32s31_wifi_mac::crypto::StaPairwiseCcmpSlot>,
    }

    impl Esp32s31ConnectedStaTxTeardown for TeardownTx {
        type Resources = u8;
        type Aggregate = u8;

        fn try_return(
            mut self,
        ) -> Result<Esp32s31ConnectedTxTeardownParts<Self::Resources, Self::Aggregate>, Self>
        {
            if self.active {
                return Err(self);
            }
            Ok(Esp32s31ConnectedTxTeardownParts {
                resources: 5,
                pairwise_key: self.pairwise.take().expect("test TX owns its pairwise key"),
                sequences: open_esp_radio_ieee80211::station::StaTxSequenceCounters::new(6),
                aggregate: 7,
            })
        }
    }

    type TeardownServices =
        WdevServiceSet<TeardownHardware, TeardownRx, TeardownTx, TeardownControl>;

    fn teardown_frontier(
        control_failure: bool,
        rx_failure: bool,
        tx_active: bool,
    ) -> (
        Esp32s31ConnectedEpochQuiesced<u8, u32, TeardownServices, u16>,
        StaGroupCcmpSlot,
    ) {
        let mut hardware = TeardownHardware::default();
        let pairwise = install_sta_pairwise_ccmp(&mut hardware, [1, 2, 3, 4, 5, 6], &[0x11; 16])
            .expect("test hardware installs its pairwise key");
        let group = install_sta_group_ccmp(&mut hardware, 1, &[0x22; 16])
            .expect("test hardware installs its group key");
        let services = WdevServiceSet::with_control(
            hardware,
            TeardownRx { fail: rx_failure },
            TeardownTx {
                active: tx_active,
                pairwise: Some(pairwise),
            },
            TeardownControl {
                fail: control_failure,
            },
        );
        (
            Esp32s31ConnectedEpochQuiesced {
                interrupt: 16,
                interrupt_drain: Esp32s31MacInterruptEpochDrain::default(),
                network: 17,
                services,
                tasks: 18,
            },
            group,
        )
    }

    #[test]
    fn complete_teardown_returns_network_tasks_and_driver_frontier_together() {
        let (frontier, group) = teardown_frontier(false, false, false);
        let stopped = frontier
            .try_teardown(group)
            .unwrap_or_else(|_| panic!("idle connected frontier must stop"));
        assert_eq!(stopped.network, 17);
        assert_eq!(stopped.tasks, 18);
        assert_eq!(stopped.driver.stopped_rx, 4);
        assert_eq!(stopped.driver.tx_resources, 5);
        assert_eq!(stopped.driver.aggregate, 7);
        assert_eq!(stopped.driver.control, 2);
        assert_eq!(stopped.driver.hardware.cleared, [1, 4]);
    }

    #[test]
    fn every_driver_teardown_failure_retains_network_and_task_owners() {
        for (control, rx, tx, expected) in [
            (true, false, false, 1),
            (false, true, false, 2),
            (false, false, true, 3),
        ] {
            let (frontier, group) = teardown_frontier(control, rx, tx);
            let failure = frontier
                .try_teardown(group)
                .err()
                .expect("selected teardown stage must fail");
            assert_eq!(failure.network, 17);
            assert_eq!(failure.tasks, 18);
            let observed = match failure.error {
                Esp32s31ConnectedStaTeardownFailure::Control { error, .. } => {
                    assert_eq!(error, 1);
                    1
                }
                Esp32s31ConnectedStaTeardownFailure::Rx { error, .. } => {
                    assert_eq!(error, 3);
                    2
                }
                Esp32s31ConnectedStaTeardownFailure::TxActive { .. } => 3,
            };
            assert_eq!(observed, expected);
        }
    }
}
