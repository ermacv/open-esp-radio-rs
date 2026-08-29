//! Finite execution transaction for one composed connected station epoch.
//!
//! Composition roots may choose network services, task placement, observers
//! and fault decorators, but they must not choose a different shutdown order.
//! This module keeps the live runner opaque across its terminal edge, maps the
//! exit while its service observations are still available, then proves IRQ
//! quiescence before returning any reusable owner.

use core::future::Future;

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_esp32s31_wifi_mac::{crypto::CcmpKeyHardware, irq::MacInterruptRoute};
use open_esp_radio_wifi_embassy::await_stack_boundary;

use crate::roles::station::teardown::Esp32s31ConnectedStaGroupSecurity;
use crate::{datapath::DatapathRunner, datapath::irq::Esp32s31MacInterruptEpoch};
use crate::{
    datapath::irq::Esp32s31MacInterruptEpochDrain,
    datapath::services::SingleRoleServices,
    roles::station::teardown::{
        Esp32s31ConnectedStaControlTeardown, Esp32s31ConnectedStaRxPark,
        Esp32s31ConnectedStaTeardownFailure, Esp32s31ConnectedStaTeardownSuccess,
        Esp32s31ConnectedStaTxTeardown,
    },
};
use open_esp_radio_esp32s31_wifi_sta::connected_control_hardware::ConnectedControlHardware;

use super::{
    Esp32s31ConnectedEpochQuiesceFailure, Esp32s31ConnectedEpochQuiesced,
    Esp32s31ConnectedEpochRunnerOwner, Esp32s31ConnectedStationExit,
    Esp32s31StationCommandReceiver, quiesce_esp32s31_connected_epoch,
    run_esp32s31_connected_station_epoch,
};

/// Runner interface consumed by the common connected execution transaction.
///
/// The trait hides network geometry from the transaction without weakening
/// ownership: its only production implementation is the concrete
/// [`DatapathRunner`], and it inherits the consuming shutdown interface.
#[doc(hidden)]
pub trait Esp32s31ConnectedStationRunner<M: RawMutex>: Esp32s31ConnectedEpochRunnerOwner {
    type Error;

    fn run_station_epoch<'a>(
        &'a mut self,
        control: &'a mut Esp32s31StationCommandReceiver<'_, M>,
    ) -> impl Future<Output = Esp32s31ConnectedStationExit<Self::Error>> + 'a;

    /// Revoke station RX admission after the finite runner exits but before
    /// its interrupt route is quiesced.
    ///
    /// This is a logical role boundary. It must not begin the outer-MAC
    /// channel-stop transaction: vendor `ic_mac_deinit` pairs that request
    /// synchronously with `ic_mac_init` around an actual PHY retune.
    fn close_station_rx_admission(&mut self);

    /// Observe natural MAC activity drain after station admission is closed.
    /// IRQ and DMA ownership remain live until the last accepted RX/TX work
    /// reaches the idle frontier.
    fn station_rx_frontend_quiescent(&mut self) -> bool;
}

/// Service graph operation required at the connected RX ingress frontier.
#[doc(hidden)]
pub trait Esp32s31ConnectedStationIngress {
    fn close_station_rx_admission(&mut self);
    fn station_rx_frontend_quiescent(&mut self) -> bool;
}

impl<H, R, X, C> Esp32s31ConnectedStationIngress for SingleRoleServices<H, R, X, C>
where
    H: ConnectedControlHardware,
{
    fn close_station_rx_admission(&mut self) {
        self.hardware_mut().disable_station_receive_policy();
    }

    fn station_rx_frontend_quiescent(&mut self) -> bool {
        self.hardware_mut().mac_runtime_active_state() == 0
    }
}

impl<
    'resources,
    'irq,
    RM,
    CM,
    N,
    B,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> Esp32s31ConnectedStationRunner<CM>
    for DatapathRunner<
        'resources,
        'irq,
        RM,
        N,
        B,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
    >
where
    RM: open_esp_radio_embassy_net::RawMutex,
    CM: RawMutex,
    N: crate::datapath::network::DatapathNetwork<
            'resources,
            RM,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            RX_QUEUE_DEPTH,
            TX_QUEUE_DEPTH,
        >,
    B: crate::datapath::DatapathServices<
            'resources,
            RM,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
            Exit = open_esp_radio_esp32s31_wifi_sta::connected_control::ConnectedDisconnectReason,
        > + Esp32s31ConnectedStationIngress,
{
    type Error = B::Error;

    fn run_station_epoch<'a>(
        &'a mut self,
        control: &'a mut Esp32s31StationCommandReceiver<'_, CM>,
    ) -> impl Future<Output = Esp32s31ConnectedStationExit<Self::Error>> + 'a {
        run_esp32s31_connected_station_epoch(self, control)
    }

    fn close_station_rx_admission(&mut self) {
        self.services_mut().close_station_rx_admission();
    }

    fn station_rx_frontend_quiescent(&mut self) -> bool {
        self.services_mut().station_rx_frontend_quiescent()
    }
}

/// Observation-only wrapper around the DATAPATH runner future.
///
/// Implementations may count polls or measure residence time, but cannot
/// replace the future's result or gain access to the runner owners. Generic
/// dispatch keeps the ordinary firmware path free of a vtable and permits the
/// compiler to erase [`NoopEsp32s31ConnectedRunObserver`] completely.
pub trait Esp32s31ConnectedRunObserver {
    fn observe<'a, F>(&'a mut self, future: F) -> impl Future<Output = F::Output> + 'a
    where
        F: Future + 'a;
}

/// Production observer which adds no work to the DATAPATH runner.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopEsp32s31ConnectedRunObserver;

impl Esp32s31ConnectedRunObserver for NoopEsp32s31ConnectedRunObserver {
    async fn observe<'a, F>(&'a mut self, future: F) -> F::Output
    where
        F: Future + 'a,
    {
        future.await
    }
}

/// A classified finite runner exit paired with its proved quiescent owners.
pub struct Esp32s31ConnectedEpochStopped<X, I, N, S> {
    pub exit: X,
    pub quiesced: Esp32s31ConnectedEpochQuiesced<I, N, S>,
}

/// Incomplete hardware stop retaining the already-classified runner exit and
/// every owner needed to continue the same quiescence transaction.
pub struct Esp32s31ConnectedRunQuiesceFailure<X, I, C, E> {
    pub exit: X,
    pub error: crate::datapath::irq::Esp32s31MacInterruptEpochQuiesceError<E>,
    interrupt: I,
    runner: C,
}

impl<'runtime, X, R, IM, C>
    Esp32s31ConnectedRunQuiesceFailure<X, Esp32s31MacInterruptEpoch<'runtime, R, IM>, C, R::Error>
where
    R: MacInterruptRoute,
    IM: RawMutex,
    C: Esp32s31ConnectedEpochRunnerOwner,
{
    /// Retry only the unfinished IRQ shutdown edge. The DATAPATH runner
    /// is never polled a second time and its previously classified exit is
    /// preserved across every failed attempt.
    #[allow(
        clippy::type_complexity,
        reason = "retry returns the exact stopped owners or the complete retry frontier"
    )]
    pub fn retry_quiesce(
        self,
        platform: &R::Platform,
    ) -> Result<
        Esp32s31ConnectedEpochStopped<
            X,
            Esp32s31MacInterruptEpoch<'runtime, R, IM>,
            C::Network,
            C::Services,
        >,
        Self,
    > {
        match quiesce_esp32s31_connected_epoch(self.interrupt, platform, self.runner) {
            Ok(quiesced) => Ok(Esp32s31ConnectedEpochStopped {
                exit: self.exit,
                quiesced,
            }),
            Err(Esp32s31ConnectedEpochQuiesceFailure::Interrupt {
                error,
                interrupt,
                runner,
            }) => Err(Self {
                exit: self.exit,
                error,
                interrupt,
                runner,
            }),
        }
    }
}

impl<X, I, N, S> Esp32s31ConnectedEpochStopped<X, I, N, S> {
    /// Remove an observation/fault decorator without releasing any other
    /// stopped owner or separating the classified runner exit.
    pub fn map_services<U>(
        self,
        map: impl FnOnce(S) -> U,
    ) -> Esp32s31ConnectedEpochStopped<X, I, N, U> {
        Esp32s31ConnectedEpochStopped {
            exit: self.exit,
            quiesced: self.quiesced.map_services(map),
        }
    }
}

/// Successful completion of the complete run, quiesce and driver teardown
/// transaction.
pub struct Esp32s31ConnectedEpochCompleted<X, I, N, D> {
    pub exit: X,
    pub interrupt: I,
    pub interrupt_drain: Esp32s31MacInterruptEpochDrain,
    pub network: N,
    pub driver: D,
}

/// Owner-preserving quarantined frontier after asynchronous quiescence
/// succeeded but control/RX/TX/key teardown did not.
pub struct Esp32s31ConnectedServiceTeardownFailure<X, I, N, E> {
    pub exit: X,
    pub interrupt: I,
    pub interrupt_drain: Esp32s31MacInterruptEpochDrain,
    pub network: N,
    pub error: E,
}

impl<X, I, N, H, R, A, C> Esp32s31ConnectedEpochStopped<X, I, N, SingleRoleServices<H, R, A, C>>
where
    H: CcmpKeyHardware,
    C: Esp32s31ConnectedStaControlTeardown<H, A>,
    R: Esp32s31ConnectedStaRxPark<H>,
    A: Esp32s31ConnectedStaTxTeardown,
{
    /// Complete the mandatory control/RX/TX/key teardown while retaining the
    /// classified runner exit and all reusable owners on success or failure.
    #[allow(clippy::type_complexity, clippy::result_large_err)]
    pub fn try_teardown(
        self,
        group_security: Esp32s31ConnectedStaGroupSecurity,
    ) -> Result<
        Esp32s31ConnectedEpochCompleted<
            X,
            I,
            N,
            Esp32s31ConnectedStaTeardownSuccess<
                H,
                R::Parked,
                A::Resources,
                A::Aggregate,
                C::Report,
            >,
        >,
        Esp32s31ConnectedServiceTeardownFailure<
            X,
            I,
            N,
            Esp32s31ConnectedStaTeardownFailure<H, R, R::Parked, A, C, C::Error, R::Error>,
        >,
    > {
        let Self { exit, quiesced } = self;
        match quiesced.try_teardown(group_security) {
            Ok(teardown) => Ok(Esp32s31ConnectedEpochCompleted {
                exit,
                interrupt: teardown.interrupt,
                interrupt_drain: teardown.interrupt_drain,
                network: teardown.network,
                driver: teardown.driver,
            }),
            Err(failure) => Err(Esp32s31ConnectedServiceTeardownFailure {
                exit,
                interrupt: failure.interrupt,
                interrupt_drain: failure.interrupt_drain,
                network: failure.network,
                error: failure.error,
            }),
        }
    }
}

/// Run one connected station owner and close every asynchronous publication
/// frontier in the mandatory order.
///
/// `classify` executes immediately after the runner returns and before IRQ or
/// task shutdown. It may derive copy-only policy/qualification evidence from
/// `runner`, but cannot move any of its owners. A quiesce failure returns the
/// complete runner through [`Esp32s31ConnectedEpochQuiesceFailure`] and never
/// exposes a partial teardown result.
#[allow(clippy::too_many_arguments)]
pub async fn run_and_quiesce_esp32s31_connected_epoch<'runtime, R, IM, CM, C, O, X>(
    interrupt: Esp32s31MacInterruptEpoch<'runtime, R, IM>,
    platform: &R::Platform,
    mut runner: C,
    control: &mut Esp32s31StationCommandReceiver<'_, CM>,
    observer: &mut O,
    classify: impl FnOnce(Esp32s31ConnectedStationExit<C::Error>, &C) -> X,
) -> Result<
    Esp32s31ConnectedEpochStopped<
        X,
        Esp32s31MacInterruptEpoch<'runtime, R, IM>,
        C::Network,
        C::Services,
    >,
    Esp32s31ConnectedRunQuiesceFailure<X, Esp32s31MacInterruptEpoch<'runtime, R, IM>, C, R::Error>,
>
where
    R: MacInterruptRoute,
    IM: RawMutex,
    CM: RawMutex,
    C: Esp32s31ConnectedStationRunner<CM>,
    O: Esp32s31ConnectedRunObserver,
{
    // The runner's batching/select future is retained inside this transaction
    // for ownership, but its poll frame is isolated from the quiesce frame.
    // Without this boundary fat LTO combines both unrelated call chains into
    // one 10-KiB CPU stack frame.
    let raw_exit = await_stack_boundary!(observer.observe(runner.run_station_epoch(control)));
    let exit = classify(raw_exit, &runner);
    runner.close_station_rx_admission();
    // Let already-accepted RX/TX work leave the MAC pipeline before closing
    // its ordinary interrupt route. The physical descriptor walker remains
    // live, but this logical role handoff deliberately does not assert the
    // channel-stop request: that request is legal only inside the paired
    // stop/retune/restart transaction owned by a real channel switch.
    embassy_time::Timer::after_micros(20).await;
    while !runner.station_rx_frontend_quiescent() {
        embassy_time::Timer::after_micros(1).await;
    }
    match quiesce_esp32s31_connected_epoch(interrupt, platform, runner) {
        Ok(quiesced) => Ok(Esp32s31ConnectedEpochStopped { exit, quiesced }),
        Err(Esp32s31ConnectedEpochQuiesceFailure::Interrupt {
            error,
            interrupt,
            runner,
        }) => Err(Esp32s31ConnectedRunQuiesceFailure {
            exit,
            error,
            interrupt,
            runner,
        }),
    }
}

#[cfg(test)]
mod tests {
    use core::future::ready;
    use std::{cell::Cell, rc::Rc};

    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;

    use super::*;
    use crate::{
        datapath::irq::{EmbassyMacIrqRuntime, EmbassyPowerIrqRuntime},
        roles::station::Esp32s31StationControlResources,
    };

    struct TestRoute(Rc<Cell<bool>>);

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
            let _ = setup;
            Ok(())
        }

        fn quiesce(&mut self, _platform: &Self::Platform) -> Result<Self::Setup, Self::Error> {
            assert!(self.0.get(), "RX admission must close before IRQ quiesce");
            Ok(19)
        }
    }

    struct RetryRoute {
        attempts: Rc<Cell<u8>>,
        admission_closed: Rc<Cell<bool>>,
    }

    impl MacInterruptRoute for RetryRoute {
        type Platform = ();
        type Setup = u32;
        type Error = u8;

        fn activate(
            &mut self,
            _platform: &Self::Platform,
            _setup: Self::Setup,
            _event_mask: open_esp_radio_esp32s31_hal::types::MacInterruptMask,
        ) -> Result<(), (Self::Error, Self::Setup)> {
            Ok(())
        }

        fn quiesce(&mut self, _platform: &Self::Platform) -> Result<Self::Setup, Self::Error> {
            assert!(
                self.admission_closed.get(),
                "RX admission must close before every IRQ quiesce attempt"
            );
            let attempt = self.attempts.get();
            self.attempts.set(attempt + 1);
            if attempt == 0 { Err(9) } else { Ok(19) }
        }
    }

    struct TestRunner {
        network: u32,
        services: u16,
        admission_closed: Rc<Cell<bool>>,
    }

    impl Esp32s31ConnectedEpochRunnerOwner for TestRunner {
        type Network = u32;
        type Services = u16;

        fn into_connected_epoch_parts(self) -> (Self::Network, Self::Services) {
            (self.network, self.services)
        }
    }

    impl Esp32s31ConnectedStationRunner<NoopRawMutex> for TestRunner {
        type Error = u8;

        fn run_station_epoch<'a>(
            &'a mut self,
            _control: &'a mut Esp32s31StationCommandReceiver<'_, NoopRawMutex>,
        ) -> impl Future<Output = Esp32s31ConnectedStationExit<Self::Error>> + 'a {
            ready(Esp32s31ConnectedStationExit::Disconnected(
                open_esp_radio_esp32s31_wifi_sta::connected_control::ConnectedDisconnectReason::BeaconLoss,
            ))
        }

        fn close_station_rx_admission(&mut self) {
            assert!(!self.admission_closed.replace(true));
        }

        fn station_rx_frontend_quiescent(&mut self) -> bool {
            self.admission_closed.get()
        }
    }

    struct OrderObserver(Rc<Cell<u8>>);

    impl Esp32s31ConnectedRunObserver for OrderObserver {
        async fn observe<'a, F>(&'a mut self, future: F) -> F::Output
        where
            F: Future + 'a,
        {
            assert_eq!(self.0.get(), 0);
            self.0.set(1);
            let output = future.await;
            self.0.set(2);
            output
        }
    }

    #[test]
    fn transaction_classifies_the_live_exit_before_revealing_parked_owners() {
        let mac = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
        let power = EmbassyPowerIrqRuntime::<NoopRawMutex>::new();
        let admission_closed = Rc::new(Cell::new(false));
        let mut interrupt =
            Esp32s31MacInterruptEpoch::new(TestRoute(admission_closed.clone()), 18, &mac, &power);
        interrupt
            .activate(
                &(),
                open_esp_radio_esp32s31_hal::types::MacInterruptMask::COLD_RX,
            )
            .expect("test interrupt epoch activates");
        let control = Esp32s31StationControlResources::<NoopRawMutex>::new();
        let (_controller, mut receiver) = control.split().expect("fresh control owner");
        let order = Rc::new(Cell::new(0));
        let mut observer = OrderObserver(order.clone());
        let stopped = block_on(run_and_quiesce_esp32s31_connected_epoch(
            interrupt,
            &(),
            TestRunner {
                network: 29,
                services: 31,
                admission_closed: admission_closed.clone(),
            },
            &mut receiver,
            &mut observer,
            |exit, runner| {
                assert!(matches!(
                    exit,
                    Esp32s31ConnectedStationExit::Disconnected(
                        open_esp_radio_esp32s31_wifi_sta::connected_control::ConnectedDisconnectReason::BeaconLoss
                    )
                ));
                assert_eq!(runner.services, 31);
                assert_eq!(order.get(), 2);
                order.set(3);
                37_u8
            },
        ))
        .unwrap_or_else(|_| panic!("ready transaction must quiesce"));

        assert_eq!(order.get(), 3);
        assert_eq!(stopped.exit, 37);
        assert_eq!(stopped.quiesced.network, 29);
        assert_eq!(stopped.quiesced.services, 31);
        assert!(stopped.quiesced.interrupt.is_active());
        assert!(admission_closed.get());
    }

    #[test]
    fn logical_park_does_not_call_physical_route_quiesce() {
        let mac = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
        let power = EmbassyPowerIrqRuntime::<NoopRawMutex>::new();
        let route_attempts = Rc::new(Cell::new(0));
        let admission_closed = Rc::new(Cell::new(false));
        let mut interrupt = Esp32s31MacInterruptEpoch::new(
            RetryRoute {
                attempts: route_attempts.clone(),
                admission_closed: admission_closed.clone(),
            },
            18,
            &mac,
            &power,
        );
        interrupt
            .activate(
                &(),
                open_esp_radio_esp32s31_hal::types::MacInterruptMask::COLD_RX,
            )
            .expect("test interrupt epoch activates");
        let control = Esp32s31StationControlResources::<NoopRawMutex>::new();
        let (_controller, mut receiver) = control.split().expect("fresh control owner");
        let order = Rc::new(Cell::new(0));
        let mut observer = OrderObserver(order.clone());
        let stopped = block_on(run_and_quiesce_esp32s31_connected_epoch(
            interrupt,
            &(),
            TestRunner {
                network: 29,
                services: 31,
                admission_closed: admission_closed.clone(),
            },
            &mut receiver,
            &mut observer,
            |_exit, _runner| {
                order.set(3);
                37_u8
            },
        ))
        .unwrap_or_else(|_| panic!("logical park must not call the route quiesce hook"));

        assert_eq!(stopped.exit, 37);
        assert_eq!(route_attempts.get(), 0);
        assert_eq!(
            order.get(),
            3,
            "classified exit must remain stable after IRQ failure"
        );
        assert_eq!(stopped.quiesced.network, 29);
        assert_eq!(stopped.quiesced.services, 31);
        assert!(stopped.quiesced.interrupt.is_active());
        assert_eq!(order.get(), 3);
    }
}
