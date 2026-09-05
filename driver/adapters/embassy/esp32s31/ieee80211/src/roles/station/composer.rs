#![expect(
    clippy::large_enum_variant,
    reason = "no-alloc station phase enums carry concrete affine owners"
)]
#![expect(
    clippy::manual_async_fn,
    reason = "station composer implementations preserve explicit borrowed Future contracts"
)]
#![expect(
    clippy::type_complexity,
    reason = "phase transitions expose the complete static owner graph without dynamic erasure"
)]

//! Complete phase owner and dispatch shared by the production station composer.
//!
//! The outer STA lifecycle must move one coherent resource graph through the
//! initial join, connected/disconnected scan and reconnected join frontiers.
//! Keeping runtime, target and security beside that phase prevents an example
//! or HIL adapter from rebuilding them independently between attempts.

use core::{future::Future, marker::PhantomData};

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_esp32s31_wifi_sta::attempt::{
    Esp32s31StaAttemptSecurity, Esp32s31StaAttemptStation, Esp32s31StaIdentity,
};
use open_esp_radio_ieee80211::scan::ScanRecord;
use open_esp_radio_wifi_embassy::await_stack_boundary;
use open_esp_radio_wifi_sta::request::StationDiscovery;
use open_esp_radio_wifi_sta::station::{
    StaAttemptContext, StaAttemptFailure, StaAttemptOutcome, StaBackoffReason,
    StaFailureDisposition, StaLifecycleStage,
};

use super::{Esp32s31StationAttemptRunner, Esp32s31StationCommand, Esp32s31StationCommandReceiver};

/// Hardware/network frontier owned by one outer station lifecycle phase.
///
/// `InitialScan` is the only state without a selected AP. `InitialJoin` is the
/// only selected-peer state that may carry the cold register owner.
/// `RunningScan` contains the complete disconnected epoch returned by a clean
/// connected teardown. `Reconnected` may only be constructed from the exact
/// epoch returned by that running scan.
pub enum Esp32s31StationServicePhase<H, S, R, N, D, E, C> {
    InitialScan {
        hardware: H,
        receive: S,
        network: N,
        identity: Esp32s31StaIdentity,
    },
    InitialJoin {
        hardware: H,
        receive: R,
        network: N,
        station: Esp32s31StaAttemptStation,
    },
    RunningScan {
        disconnected: D,
        station: Esp32s31StaAttemptStation,
    },
    Reconnected {
        epoch: E,
        network: N,
        station: Esp32s31StaAttemptStation,
    },
    Connected {
        connected: C,
    },
}

/// Value-only identity of the currently owned station phase.
///
/// This may be copied into diagnostics without exposing any phase owner or
/// turning driver state into a string protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StationServicePhaseKind {
    InitialScan,
    InitialJoin,
    RunningScan,
    Reconnected,
    Connected,
}

/// Complete movable owner for the board-independent station lifecycle.
///
/// Runtime resources and WPA2/sequence state never live beside a phase as
/// reconstructible copies. Candidate identity belongs to the phase which can
/// legally use it. Every normal and failed attempt returns this exact value at
/// the hardware frontier it reached.
pub struct Esp32s31StationServiceOwner<'security, R, P> {
    pub phase: P,
    pub runtime: R,
    pub security: Esp32s31StaAttemptSecurity<'security>,
}

impl<'security, R, P> Esp32s31StationServiceOwner<'security, R, P> {
    pub const fn new(
        runtime: R,
        phase: P,
        security: Esp32s31StaAttemptSecurity<'security>,
    ) -> Self {
        Self {
            phase,
            runtime,
            security,
        }
    }

    pub fn into_parts(self) -> (R, P, Esp32s31StaAttemptSecurity<'security>) {
        (self.runtime, self.phase, self.security)
    }
}

/// Exact owner set entering cold candidate selection.
pub struct Esp32s31StationInitialScanPhase<'security, R, H, S, N> {
    runtime: R,
    hardware: H,
    receive: S,
    network: N,
    identity: Esp32s31StaIdentity,
    security: Esp32s31StaAttemptSecurity<'security>,
}

impl<'security, R, H, S, N> Esp32s31StationInitialScanPhase<'security, R, H, S, N> {
    fn new(
        runtime: R,
        hardware: H,
        receive: S,
        network: N,
        identity: Esp32s31StaIdentity,
        security: Esp32s31StaAttemptSecurity<'security>,
    ) -> Self {
        Self {
            runtime,
            hardware,
            receive,
            network,
            identity,
            security,
        }
    }

    pub fn into_parts(
        self,
    ) -> (
        R,
        H,
        S,
        N,
        Esp32s31StaIdentity,
        Esp32s31StaAttemptSecurity<'security>,
    ) {
        (
            self.runtime,
            self.hardware,
            self.receive,
            self.network,
            self.identity,
            self.security,
        )
    }
}

/// Exact owner set entering the first join transaction.
pub struct Esp32s31StationInitialJoinPhase<'security, R, H, X, N> {
    runtime: R,
    hardware: H,
    receive: X,
    network: N,
    station: Esp32s31StaAttemptStation,
    security: Esp32s31StaAttemptSecurity<'security>,
}

impl<'security, R, H, X, N> Esp32s31StationInitialJoinPhase<'security, R, H, X, N> {
    fn new(
        runtime: R,
        hardware: H,
        receive: X,
        network: N,
        station: Esp32s31StaAttemptStation,
        security: Esp32s31StaAttemptSecurity<'security>,
    ) -> Self {
        Self {
            runtime,
            hardware,
            receive,
            network,
            station,
            security,
        }
    }

    pub fn into_parts(
        self,
    ) -> (
        R,
        H,
        X,
        N,
        Esp32s31StaAttemptStation,
        Esp32s31StaAttemptSecurity<'security>,
    ) {
        (
            self.runtime,
            self.hardware,
            self.receive,
            self.network,
            self.station,
            self.security,
        )
    }
}

/// Exact owner set entering a disconnected candidate-refresh transaction.
pub struct Esp32s31StationRunningScanPhase<'security, R, D> {
    runtime: R,
    disconnected: D,
    station: Esp32s31StaAttemptStation,
    security: Esp32s31StaAttemptSecurity<'security>,
}

impl<'security, R, D> Esp32s31StationRunningScanPhase<'security, R, D> {
    fn new(
        runtime: R,
        disconnected: D,
        station: Esp32s31StaAttemptStation,
        security: Esp32s31StaAttemptSecurity<'security>,
    ) -> Self {
        Self {
            runtime,
            disconnected,
            station,
            security,
        }
    }

    pub fn into_parts(
        self,
    ) -> (
        R,
        D,
        Esp32s31StaAttemptStation,
        Esp32s31StaAttemptSecurity<'security>,
    ) {
        (self.runtime, self.disconnected, self.station, self.security)
    }
}

/// Exact owner set entering a join transaction after a completed rescan.
pub struct Esp32s31StationReconnectedPhase<'security, R, E, N> {
    runtime: R,
    epoch: E,
    network: N,
    station: Esp32s31StaAttemptStation,
    security: Esp32s31StaAttemptSecurity<'security>,
}

/// Exact owner set entering one connected data-plane epoch.
///
/// Join and connected service are deliberately separate async transactions:
/// a join future returns this frontier, then the outer lifecycle polls a new
/// future. This prevents both protocol state machines and their owner-return
/// temporaries from occupying one live CPU stack frame.
pub struct Esp32s31StationConnectedPhase<'security, R, C> {
    runtime: R,
    connected: C,
    security: Esp32s31StaAttemptSecurity<'security>,
}

impl<'security, R, C> Esp32s31StationConnectedPhase<'security, R, C> {
    fn new(runtime: R, connected: C, security: Esp32s31StaAttemptSecurity<'security>) -> Self {
        Self {
            runtime,
            connected,
            security,
        }
    }

    pub fn into_parts(self) -> (R, C, Esp32s31StaAttemptSecurity<'security>) {
        (self.runtime, self.connected, self.security)
    }
}

impl<'security, R, E, N> Esp32s31StationReconnectedPhase<'security, R, E, N> {
    fn new(
        runtime: R,
        epoch: E,
        network: N,
        station: Esp32s31StaAttemptStation,
        security: Esp32s31StaAttemptSecurity<'security>,
    ) -> Self {
        Self {
            runtime,
            epoch,
            network,
            station,
            security,
        }
    }

    pub fn into_parts(
        self,
    ) -> (
        R,
        E,
        N,
        Esp32s31StaAttemptStation,
        Esp32s31StaAttemptSecurity<'security>,
    ) {
        (
            self.runtime,
            self.epoch,
            self.network,
            self.station,
            self.security,
        )
    }
}

/// Finite result of cold candidate selection.
pub enum Esp32s31StationInitialScanExit<'security, R, H, X, N, O, E, F = core::convert::Infallible>
{
    JoinReady(Esp32s31StationInitialJoinPhase<'security, R, H, X, N>),
    Complete(StaAttemptOutcome<O, E, F>),
}

impl<'security, R, H, X, N, O, E, F>
    Esp32s31StationInitialScanExit<'security, R, H, X, N, O, E, F>
{
    pub fn join_ready(
        runtime: R,
        hardware: H,
        receive: X,
        network: N,
        station: Esp32s31StaAttemptStation,
        security: Esp32s31StaAttemptSecurity<'security>,
    ) -> Self {
        Self::JoinReady(Esp32s31StationInitialJoinPhase::new(
            runtime, hardware, receive, network, station, security,
        ))
    }

    pub const fn complete(outcome: StaAttemptOutcome<O, E, F>) -> Self {
        Self::Complete(outcome)
    }
}

/// Finite result of a disconnected candidate-refresh transaction.
///
/// A successful scan returns the exact next join frontier instead of invoking
/// Authentication/Association itself. The common engine consumes that value
/// and remains the only component which dispatches the reconnected phase.
pub enum Esp32s31StationRunningScanExit<'security, R, E, N, O, X, F = core::convert::Infallible> {
    JoinReady(Esp32s31StationReconnectedPhase<'security, R, E, N>),
    Complete(StaAttemptOutcome<O, X, F>),
}

/// Finite result of either initial or reconnected join.
///
/// Success returns a connected frontier instead of entering the data-plane
/// loop from the join future. Every other result is already a complete outer
/// lifecycle outcome.
pub enum Esp32s31StationJoinExit<'security, R, C, O, E, F = core::convert::Infallible> {
    ConnectedReady(Esp32s31StationConnectedPhase<'security, R, C>),
    Complete(StaAttemptOutcome<O, E, F>),
}

impl<'security, R, C, O, E, F> Esp32s31StationJoinExit<'security, R, C, O, E, F> {
    pub fn connected_ready(
        runtime: R,
        connected: C,
        security: Esp32s31StaAttemptSecurity<'security>,
    ) -> Self {
        Self::ConnectedReady(Esp32s31StationConnectedPhase::new(
            runtime, connected, security,
        ))
    }

    pub const fn complete(outcome: StaAttemptOutcome<O, E, F>) -> Self {
        Self::Complete(outcome)
    }
}

impl<'security, R, E, N, O, X, F> Esp32s31StationRunningScanExit<'security, R, E, N, O, X, F> {
    pub fn join_ready(
        runtime: R,
        epoch: E,
        network: N,
        station: Esp32s31StaAttemptStation,
        security: Esp32s31StaAttemptSecurity<'security>,
    ) -> Self {
        Self::JoinReady(Esp32s31StationReconnectedPhase::new(
            runtime, epoch, network, station, security,
        ))
    }

    pub const fn complete(outcome: StaAttemptOutcome<O, X, F>) -> Self {
        Self::Complete(outcome)
    }
}

/// Value-only result after a disconnected scan has returned every owner.
///
/// Scan execution and diagnostics remain target policy, but the transition
/// into the reconnected join frontier is shared by every production consumer.
pub enum Esp32s31StationRunningScanCompletion<E> {
    Selected(ScanRecord),
    Stopped,
    Failed {
        disposition: StaFailureDisposition,
        error: E,
    },
}

/// Complete candidate refresh without allowing a composition root to invent
/// a different outer station transition.
///
/// `prepare_reconnect` consumes the exact disconnected epoch only after a real
/// candidate was selected. All other exits reconstruct the same running-scan
/// owner; no PAC, RX or network capability can be synthesized here.
pub fn complete_esp32s31_station_running_scan<'security, R, D, E, N, O, X, F>(
    runtime: R,
    disconnected: D,
    mut station: Esp32s31StaAttemptStation,
    security: Esp32s31StaAttemptSecurity<'security>,
    completion: Esp32s31StationRunningScanCompletion<X>,
    prepare_reconnect: impl FnOnce(D) -> (N, E),
    restore_owner: impl FnOnce(R, D, Esp32s31StaAttemptStation, Esp32s31StaAttemptSecurity) -> O,
) -> Esp32s31StationRunningScanExit<'security, R, E, N, O, X, F> {
    match completion {
        Esp32s31StationRunningScanCompletion::Selected(candidate) => {
            station.access_point = candidate;
            let (network, epoch) = prepare_reconnect(disconnected);
            Esp32s31StationRunningScanExit::join_ready(runtime, epoch, network, station, security)
        }
        Esp32s31StationRunningScanCompletion::Stopped => {
            Esp32s31StationRunningScanExit::complete(StaAttemptOutcome::Stopped {
                owner: restore_owner(runtime, disconnected, station, security),
            })
        }
        Esp32s31StationRunningScanCompletion::Failed { disposition, error } => {
            Esp32s31StationRunningScanExit::complete(StaAttemptOutcome::Failed {
                owner: restore_owner(runtime, disconnected, station, security),
                failure: StaAttemptFailure::new(
                    StaLifecycleStage::CandidateSelection,
                    disposition,
                    error,
                ),
            })
        }
    }
}

/// Hardware-owning phase transactions required by the common station engine.
///
/// Implementations provide board/network bindings only. Observability is a
/// separate engine concern, so HIL telemetry cannot alter which hardware
/// transaction is selected. A port must return the exact owner frontier it
/// received after every finite transaction.
pub trait Esp32s31StationEnginePort<'security, M: RawMutex> {
    type Runtime;
    type InitialHardware;
    type InitialScanRx;
    type RxFrontier;
    type Network;
    type Disconnected;
    type Reconnected;
    type Connected;
    type Error;
    /// Exact non-reusable owner returned when a phase cannot prove the normal
    /// station frontier reusable. This is data, not a request to reset.
    type Fault;

    fn run_initial_scan<'a>(
        &'a mut self,
        phase: Esp32s31StationInitialScanPhase<
            'security,
            Self::Runtime,
            Self::InitialHardware,
            Self::InitialScanRx,
            Self::Network,
        >,
        discovery: StationDiscovery,
        context: StaAttemptContext,
        control: &'a mut Esp32s31StationCommandReceiver<'_, M>,
    ) -> impl Future<
        Output = Esp32s31StationInitialScanExit<
            'security,
            Self::Runtime,
            Self::InitialHardware,
            Self::RxFrontier,
            Self::Network,
            Esp32s31StationEngineOwner<'security, M, Self>,
            Self::Error,
            Self::Fault,
        >,
    > + 'a
    where
        Self: Sized + 'a,
        'security: 'a;

    fn run_initial_join<'a>(
        &'a mut self,
        phase: Esp32s31StationInitialJoinPhase<
            'security,
            Self::Runtime,
            Self::InitialHardware,
            Self::RxFrontier,
            Self::Network,
        >,
        context: StaAttemptContext,
        control: &'a mut Esp32s31StationCommandReceiver<'_, M>,
    ) -> impl Future<
        Output = Esp32s31StationJoinExit<
            'security,
            Self::Runtime,
            Self::Connected,
            Esp32s31StationEngineOwner<'security, M, Self>,
            Self::Error,
            Self::Fault,
        >,
    > + 'a
    where
        Self: Sized + 'a,
        'security: 'a;

    fn run_running_scan<'a>(
        &'a mut self,
        phase: Esp32s31StationRunningScanPhase<'security, Self::Runtime, Self::Disconnected>,
        discovery: StationDiscovery,
        context: StaAttemptContext,
        control: &'a mut Esp32s31StationCommandReceiver<'_, M>,
    ) -> impl Future<
        Output = Esp32s31StationRunningScanExit<
            'security,
            Self::Runtime,
            Self::Reconnected,
            Self::Network,
            Esp32s31StationEngineOwner<'security, M, Self>,
            Self::Error,
            Self::Fault,
        >,
    > + 'a
    where
        Self: Sized + 'a,
        'security: 'a;

    fn run_reconnected<'a>(
        &'a mut self,
        phase: Esp32s31StationReconnectedPhase<
            'security,
            Self::Runtime,
            Self::Reconnected,
            Self::Network,
        >,
        context: StaAttemptContext,
        control: &'a mut Esp32s31StationCommandReceiver<'_, M>,
    ) -> impl Future<
        Output = Esp32s31StationJoinExit<
            'security,
            Self::Runtime,
            Self::Connected,
            Esp32s31StationEngineOwner<'security, M, Self>,
            Self::Error,
            Self::Fault,
        >,
    > + 'a
    where
        Self: Sized + 'a,
        'security: 'a;

    fn run_connected<'a>(
        &'a mut self,
        phase: Esp32s31StationConnectedPhase<'security, Self::Runtime, Self::Connected>,
        context: StaAttemptContext,
        control: &'a mut Esp32s31StationCommandReceiver<'_, M>,
    ) -> impl Future<
        Output = StaAttemptOutcome<
            Esp32s31StationEngineOwner<'security, M, Self>,
            Self::Error,
            Self::Fault,
        >,
    > + 'a
    where
        Self: Sized + 'a,
        'security: 'a;

    fn candidate_refresh_contract_error(&mut self) -> Self::Error;
}

/// Side-effect-only observations emitted by the common station engine.
///
/// An observer never receives mutable access to the hardware port or an owned
/// phase frontier. It may record lifecycle progress, but cannot select or
/// bypass a station transition.
pub trait Esp32s31StationEngineObserver<'security, M, P>
where
    M: RawMutex,
    P: Esp32s31StationEnginePort<'security, M>,
{
    fn attempt_started(
        &mut self,
        _context: StaAttemptContext,
        _phase: Esp32s31StationServicePhaseKind,
    ) {
    }

    fn attempt_finished<'a>(
        &'a mut self,
        _context: StaAttemptContext,
        _outcome: &'a StaAttemptOutcome<
            Esp32s31StationEngineOwner<'security, M, P>,
            P::Error,
            P::Fault,
        >,
    ) -> impl Future<Output = ()> + 'a
    where
        Self: Sized + 'a,
        P: 'a,
        'security: 'a,
    {
        async {}
    }

    fn command_deferred(&mut self, _command: Esp32s31StationCommand, _accepted: bool) {}

    fn backoff_started(&mut self, _delay_millis: u32, _reason: StaBackoffReason) {}
}

/// Zero-cost observer for compositions that do not request lifecycle
/// instrumentation.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopEsp32s31StationEngineObserver;

impl<'security, M, P> Esp32s31StationEngineObserver<'security, M, P>
    for NoopEsp32s31StationEngineObserver
where
    M: RawMutex,
    P: Esp32s31StationEnginePort<'security, M>,
{
}

pub type Esp32s31StationEngineOwner<'security, M, E> = Esp32s31StationServiceOwner<
    'security,
    <E as Esp32s31StationEnginePort<'security, M>>::Runtime,
    Esp32s31StationServicePhase<
        <E as Esp32s31StationEnginePort<'security, M>>::InitialHardware,
        <E as Esp32s31StationEnginePort<'security, M>>::InitialScanRx,
        <E as Esp32s31StationEnginePort<'security, M>>::RxFrontier,
        <E as Esp32s31StationEnginePort<'security, M>>::Network,
        <E as Esp32s31StationEnginePort<'security, M>>::Disconnected,
        <E as Esp32s31StationEnginePort<'security, M>>::Reconnected,
        <E as Esp32s31StationEnginePort<'security, M>>::Connected,
    >,
>;

/// Common outer STA engine shared by firmware and HIL.
///
/// Scan/join/connected transactions remain finite port operations supplied
/// by `E`; phase selection, the refresh precondition and lifecycle callbacks
/// are not reimplemented by each consumer.
pub struct Esp32s31StationEngine<'security, P, O = NoopEsp32s31StationEngineObserver> {
    port: P,
    discovery: StationDiscovery,
    observer: O,
    _security: PhantomData<&'security mut ()>,
}

impl<P> Esp32s31StationEngine<'_, P, NoopEsp32s31StationEngineObserver> {
    pub const fn new(port: P, discovery: StationDiscovery) -> Self {
        Self {
            port,
            discovery,
            observer: NoopEsp32s31StationEngineObserver,
            _security: PhantomData,
        }
    }
}

impl<P, O> Esp32s31StationEngine<'_, P, O> {
    pub const fn port(&self) -> &P {
        &self.port
    }

    pub const fn with_observer(port: P, discovery: StationDiscovery, observer: O) -> Self {
        Self {
            port,
            discovery,
            observer,
            _security: PhantomData,
        }
    }

    pub fn into_port(self) -> P {
        self.port
    }

    pub fn into_parts(self) -> (P, StationDiscovery, O) {
        (self.port, self.discovery, self.observer)
    }
}

impl<'security, M, P, O> Esp32s31StationAttemptRunner<M> for Esp32s31StationEngine<'security, P, O>
where
    M: RawMutex,
    P: Esp32s31StationEnginePort<'security, M>,
    O: Esp32s31StationEngineObserver<'security, M, P>,
{
    type Owner = Esp32s31StationEngineOwner<'security, M, P>;
    type Error = P::Error;
    type Fault = P::Fault;

    fn run_attempt<'a>(
        &'a mut self,
        owner: Self::Owner,
        context: StaAttemptContext,
        control: &'a mut Esp32s31StationCommandReceiver<'_, M>,
    ) -> impl Future<Output = StaAttemptOutcome<Self::Owner, Self::Error, Self::Fault>> + 'a {
        async move {
            let phase_kind = match &owner.phase {
                Esp32s31StationServicePhase::InitialScan { .. } => {
                    Esp32s31StationServicePhaseKind::InitialScan
                }
                Esp32s31StationServicePhase::InitialJoin { .. } => {
                    Esp32s31StationServicePhaseKind::InitialJoin
                }
                Esp32s31StationServicePhase::RunningScan { .. } => {
                    Esp32s31StationServicePhaseKind::RunningScan
                }
                Esp32s31StationServicePhase::Reconnected { .. } => {
                    Esp32s31StationServicePhaseKind::Reconnected
                }
                Esp32s31StationServicePhase::Connected { .. } => {
                    Esp32s31StationServicePhaseKind::Connected
                }
            };
            self.observer.attempt_started(context, phase_kind);
            let (runtime, phase, security) = owner.into_parts();
            let outcome = match phase {
                Esp32s31StationServicePhase::InitialScan {
                    hardware,
                    receive,
                    network,
                    identity,
                } => {
                    match await_stack_boundary!(self.port.run_initial_scan(
                        Esp32s31StationInitialScanPhase::new(
                            runtime, hardware, receive, network, identity, security,
                        ),
                        self.discovery,
                        context,
                        control,
                    )) {
                        Esp32s31StationInitialScanExit::JoinReady(phase) => {
                            let (runtime, hardware, receive, network, station, security) =
                                phase.into_parts();
                            StaAttemptOutcome::Advanced {
                                owner: Esp32s31StationServiceOwner::new(
                                    runtime,
                                    Esp32s31StationServicePhase::InitialJoin {
                                        hardware,
                                        receive,
                                        network,
                                        station,
                                    },
                                    security,
                                ),
                            }
                        }
                        Esp32s31StationInitialScanExit::Complete(outcome) => outcome,
                    }
                }
                Esp32s31StationServicePhase::InitialJoin {
                    hardware,
                    receive,
                    network,
                    station,
                } => {
                    match await_stack_boundary!(self.port.run_initial_join(
                        Esp32s31StationInitialJoinPhase::new(
                            runtime, hardware, receive, network, station, security,
                        ),
                        context,
                        control,
                    )) {
                        Esp32s31StationJoinExit::ConnectedReady(phase) => {
                            let (runtime, connected, security) = phase.into_parts();
                            StaAttemptOutcome::Advanced {
                                owner: Esp32s31StationServiceOwner::new(
                                    runtime,
                                    Esp32s31StationServicePhase::Connected { connected },
                                    security,
                                ),
                            }
                        }
                        Esp32s31StationJoinExit::Complete(outcome) => outcome,
                    }
                }
                Esp32s31StationServicePhase::RunningScan {
                    disconnected,
                    station,
                } => {
                    if context.refresh_candidate {
                        match await_stack_boundary!(self.port.run_running_scan(
                            Esp32s31StationRunningScanPhase::new(
                                runtime,
                                disconnected,
                                station,
                                security,
                            ),
                            self.discovery,
                            context,
                            control,
                        )) {
                            Esp32s31StationRunningScanExit::JoinReady(phase) => {
                                let (runtime, epoch, network, station, security) =
                                    phase.into_parts();
                                StaAttemptOutcome::Advanced {
                                    owner: Esp32s31StationServiceOwner::new(
                                        runtime,
                                        Esp32s31StationServicePhase::Reconnected {
                                            epoch,
                                            network,
                                            station,
                                        },
                                        security,
                                    ),
                                }
                            }
                            Esp32s31StationRunningScanExit::Complete(outcome) => outcome,
                        }
                    } else {
                        let error = self.port.candidate_refresh_contract_error();
                        StaAttemptOutcome::Failed {
                            owner: Esp32s31StationServiceOwner::new(
                                runtime,
                                Esp32s31StationServicePhase::RunningScan {
                                    disconnected,
                                    station,
                                },
                                security,
                            ),
                            failure: StaAttemptFailure::new(
                                StaLifecycleStage::CandidateSelection,
                                StaFailureDisposition::Terminal,
                                error,
                            ),
                        }
                    }
                }
                Esp32s31StationServicePhase::Reconnected {
                    epoch,
                    network,
                    station,
                } => {
                    match await_stack_boundary!(self.port.run_reconnected(
                        Esp32s31StationReconnectedPhase::new(
                            runtime, epoch, network, station, security,
                        ),
                        context,
                        control,
                    )) {
                        Esp32s31StationJoinExit::ConnectedReady(phase) => {
                            let (runtime, connected, security) = phase.into_parts();
                            StaAttemptOutcome::Advanced {
                                owner: Esp32s31StationServiceOwner::new(
                                    runtime,
                                    Esp32s31StationServicePhase::Connected { connected },
                                    security,
                                ),
                            }
                        }
                        Esp32s31StationJoinExit::Complete(outcome) => outcome,
                    }
                }
                Esp32s31StationServicePhase::Connected { connected } => {
                    await_stack_boundary!(self.port.run_connected(
                        Esp32s31StationConnectedPhase::new(runtime, connected, security),
                        context,
                        control,
                    ))
                }
            };
            self.observer.attempt_finished(context, &outcome).await;
            outcome
        }
    }

    fn command_deferred(&mut self, command: Esp32s31StationCommand, accepted: bool) {
        self.observer.command_deferred(command, accepted);
    }

    fn backoff_started(&mut self, delay_millis: u32, reason: StaBackoffReason) {
        self.observer.backoff_started(delay_millis, reason);
    }
}

#[cfg(test)]
mod tests;
