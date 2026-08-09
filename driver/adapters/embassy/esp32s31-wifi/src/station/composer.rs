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
pub enum Esp32s31StationServicePhase<H, S, R, N, D, E> {
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
    type PreconnectedRx;
    type Network;
    type Disconnected;
    type Reconnected;
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
            Self::PreconnectedRx,
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
            Self::PreconnectedRx,
            Self::Network,
        >,
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
        <E as Esp32s31StationEnginePort<'security, M>>::PreconnectedRx,
        <E as Esp32s31StationEnginePort<'security, M>>::Network,
        <E as Esp32s31StationEnginePort<'security, M>>::Disconnected,
        <E as Esp32s31StationEnginePort<'security, M>>::Reconnected,
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
                    match self
                        .port
                        .run_initial_scan(
                            Esp32s31StationInitialScanPhase::new(
                                runtime, hardware, receive, network, identity, security,
                            ),
                            self.discovery,
                            context,
                            control,
                        )
                        .await
                    {
                        Esp32s31StationInitialScanExit::JoinReady(phase) => {
                            self.port.run_initial_join(phase, context, control).await
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
                    self.port
                        .run_initial_join(
                            Esp32s31StationInitialJoinPhase::new(
                                runtime, hardware, receive, network, station, security,
                            ),
                            context,
                            control,
                        )
                        .await
                }
                Esp32s31StationServicePhase::RunningScan {
                    disconnected,
                    station,
                } => {
                    if context.refresh_candidate {
                        match self
                            .port
                            .run_running_scan(
                                Esp32s31StationRunningScanPhase::new(
                                    runtime,
                                    disconnected,
                                    station,
                                    security,
                                ),
                                self.discovery,
                                context,
                                control,
                            )
                            .await
                        {
                            Esp32s31StationRunningScanExit::JoinReady(phase) => {
                                self.port.run_reconnected(phase, context, control).await
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
                    self.port
                        .run_reconnected(
                            Esp32s31StationReconnectedPhase::new(
                                runtime, epoch, network, station, security,
                            ),
                            context,
                            control,
                        )
                        .await
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
mod tests {
    use core::num::NonZeroU16;

    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use open_esp_radio_ieee80211::{
        scan::ScanRecord,
        station::{StaAssociationPreference, StaTxSequenceCounters},
    };
    use open_esp_radio_wifi_sta::request::{StationScanChannels, StationScanPolicy, WifiSsid};
    use open_esp_radio_wpa2::Pmk;

    use super::*;
    use crate::station::Esp32s31StationControlResources;

    fn discovery() -> StationDiscovery {
        StationDiscovery::new(
            WifiSsid::new(b"ssid").expect("test SSID is valid"),
            StationScanPolicy::new(
                StationScanChannels::CHANNELS_1_TO_13,
                NonZeroU16::new(20).expect("test scan dwell is nonzero"),
                StaAssociationPreference::Automatic,
            ),
        )
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum FakeError {
        RefreshContract,
    }

    struct FakePort;

    #[derive(Default)]
    struct FakeObserver {
        started: Option<Esp32s31StationServicePhaseKind>,
        finished: bool,
    }

    impl<'security> Esp32s31StationEnginePort<'security, NoopRawMutex> for FakePort {
        type Runtime = u8;
        type InitialHardware = u16;
        type InitialScanRx = i16;
        type PreconnectedRx = u32;
        type Network = u64;
        type Disconnected = u128;
        type Reconnected = usize;
        type Error = FakeError;
        type Fault = core::convert::Infallible;

        fn run_initial_scan<'a>(
            &'a mut self,
            _phase: Esp32s31StationInitialScanPhase<
                'security,
                Self::Runtime,
                Self::InitialHardware,
                Self::InitialScanRx,
                Self::Network,
            >,
            _discovery: StationDiscovery,
            _context: StaAttemptContext,
            _control: &'a mut Esp32s31StationCommandReceiver<'_, NoopRawMutex>,
        ) -> impl Future<
            Output = Esp32s31StationInitialScanExit<
                'security,
                Self::Runtime,
                Self::InitialHardware,
                Self::PreconnectedRx,
                Self::Network,
                Esp32s31StationEngineOwner<'security, NoopRawMutex, Self>,
                Self::Error,
            >,
        > + 'a
        where
            'security: 'a,
        {
            async { panic!("refresh-contract test must not enter initial scan") }
        }

        fn run_initial_join<'a>(
            &'a mut self,
            _phase: Esp32s31StationInitialJoinPhase<
                'security,
                Self::Runtime,
                Self::InitialHardware,
                Self::PreconnectedRx,
                Self::Network,
            >,
            _context: StaAttemptContext,
            _control: &'a mut Esp32s31StationCommandReceiver<'_, NoopRawMutex>,
        ) -> impl Future<
            Output = StaAttemptOutcome<
                Esp32s31StationEngineOwner<'security, NoopRawMutex, Self>,
                Self::Error,
            >,
        > + 'a
        where
            'security: 'a,
        {
            async { panic!("refresh-contract test must not enter initial") }
        }

        fn run_running_scan<'a>(
            &'a mut self,
            _phase: Esp32s31StationRunningScanPhase<'security, Self::Runtime, Self::Disconnected>,
            _discovery: StationDiscovery,
            _context: StaAttemptContext,
            _control: &'a mut Esp32s31StationCommandReceiver<'_, NoopRawMutex>,
        ) -> impl Future<
            Output = Esp32s31StationRunningScanExit<
                'security,
                Self::Runtime,
                Self::Reconnected,
                Self::Network,
                Esp32s31StationEngineOwner<'security, NoopRawMutex, Self>,
                Self::Error,
            >,
        > + 'a
        where
            'security: 'a,
        {
            async { panic!("missing refresh must be rejected before scan") }
        }

        fn run_reconnected<'a>(
            &'a mut self,
            _phase: Esp32s31StationReconnectedPhase<
                'security,
                Self::Runtime,
                Self::Reconnected,
                Self::Network,
            >,
            _context: StaAttemptContext,
            _control: &'a mut Esp32s31StationCommandReceiver<'_, NoopRawMutex>,
        ) -> impl Future<
            Output = StaAttemptOutcome<
                Esp32s31StationEngineOwner<'security, NoopRawMutex, Self>,
                Self::Error,
            >,
        > + 'a
        where
            'security: 'a,
        {
            async { panic!("refresh-contract test must not enter reconnect") }
        }

        fn candidate_refresh_contract_error(&mut self) -> Self::Error {
            FakeError::RefreshContract
        }
    }

    impl<'security> Esp32s31StationEngineObserver<'security, NoopRawMutex, FakePort> for FakeObserver {
        fn attempt_started(
            &mut self,
            _context: StaAttemptContext,
            phase: Esp32s31StationServicePhaseKind,
        ) {
            self.started = Some(phase);
        }

        fn attempt_finished<'a>(
            &'a mut self,
            _context: StaAttemptContext,
            _outcome: &'a StaAttemptOutcome<
                Esp32s31StationEngineOwner<'security, NoopRawMutex, FakePort>,
                FakeError,
            >,
        ) -> impl Future<Output = ()> + 'a
        where
            'security: 'a,
        {
            async move {
                self.finished = true;
            }
        }
    }

    #[derive(Default)]
    struct ScanTransitionPort {
        initial_joined: bool,
        reconnected: bool,
    }

    impl<'security> Esp32s31StationEnginePort<'security, NoopRawMutex> for ScanTransitionPort {
        type Runtime = u8;
        type InitialHardware = u16;
        type InitialScanRx = i16;
        type PreconnectedRx = u32;
        type Network = u64;
        type Disconnected = u128;
        type Reconnected = usize;
        type Error = FakeError;
        type Fault = core::convert::Infallible;

        fn run_initial_scan<'a>(
            &'a mut self,
            phase: Esp32s31StationInitialScanPhase<
                'security,
                Self::Runtime,
                Self::InitialHardware,
                Self::InitialScanRx,
                Self::Network,
            >,
            _discovery: StationDiscovery,
            _context: StaAttemptContext,
            _control: &'a mut Esp32s31StationCommandReceiver<'_, NoopRawMutex>,
        ) -> impl Future<
            Output = Esp32s31StationInitialScanExit<
                'security,
                Self::Runtime,
                Self::InitialHardware,
                Self::PreconnectedRx,
                Self::Network,
                Esp32s31StationEngineOwner<'security, NoopRawMutex, Self>,
                Self::Error,
            >,
        > + 'a
        where
            'security: 'a,
        {
            async move {
                let (runtime, hardware, scan_rx, network, identity, security) = phase.into_parts();
                assert_eq!(scan_rx, -17);
                let candidate = ScanRecord {
                    bssid: [0x02, 0, 0, 0, 0, 6],
                    channel: 6,
                    rssi: -42,
                    ..ScanRecord::EMPTY
                };
                Esp32s31StationInitialScanExit::join_ready(
                    runtime,
                    hardware,
                    41,
                    network,
                    identity.select(candidate),
                    security,
                )
            }
        }

        fn run_initial_join<'a>(
            &'a mut self,
            phase: Esp32s31StationInitialJoinPhase<
                'security,
                Self::Runtime,
                Self::InitialHardware,
                Self::PreconnectedRx,
                Self::Network,
            >,
            _context: StaAttemptContext,
            _control: &'a mut Esp32s31StationCommandReceiver<'_, NoopRawMutex>,
        ) -> impl Future<
            Output = StaAttemptOutcome<
                Esp32s31StationEngineOwner<'security, NoopRawMutex, Self>,
                Self::Error,
            >,
        > + 'a
        where
            'security: 'a,
        {
            async move {
                let (runtime, hardware, receive, network, station, security) = phase.into_parts();
                assert_eq!((hardware, receive, network), (11, 41, 13));
                assert_eq!(station.access_point.channel, 6);
                self.initial_joined = true;
                StaAttemptOutcome::Stopped {
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
        }

        fn run_running_scan<'a>(
            &'a mut self,
            phase: Esp32s31StationRunningScanPhase<'security, Self::Runtime, Self::Disconnected>,
            _discovery: StationDiscovery,
            _context: StaAttemptContext,
            _control: &'a mut Esp32s31StationCommandReceiver<'_, NoopRawMutex>,
        ) -> impl Future<
            Output = Esp32s31StationRunningScanExit<
                'security,
                Self::Runtime,
                Self::Reconnected,
                Self::Network,
                Esp32s31StationEngineOwner<'security, NoopRawMutex, Self>,
                Self::Error,
            >,
        > + 'a
        where
            'security: 'a,
        {
            async move {
                let (runtime, disconnected, station, security) = phase.into_parts();
                assert_eq!(disconnected, 19);
                Esp32s31StationRunningScanExit::join_ready(runtime, 23, 29, station, security)
            }
        }

        fn run_reconnected<'a>(
            &'a mut self,
            phase: Esp32s31StationReconnectedPhase<
                'security,
                Self::Runtime,
                Self::Reconnected,
                Self::Network,
            >,
            _context: StaAttemptContext,
            _control: &'a mut Esp32s31StationCommandReceiver<'_, NoopRawMutex>,
        ) -> impl Future<
            Output = StaAttemptOutcome<
                Esp32s31StationEngineOwner<'security, NoopRawMutex, Self>,
                Self::Error,
            >,
        > + 'a
        where
            'security: 'a,
        {
            async move {
                let (runtime, epoch, network, station, security) = phase.into_parts();
                assert_eq!((epoch, network), (23, 29));
                self.reconnected = true;
                StaAttemptOutcome::Stopped {
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
        }

        fn candidate_refresh_contract_error(&mut self) -> Self::Error {
            FakeError::RefreshContract
        }
    }

    #[test]
    fn phase_owner_returns_runtime_target_and_security_without_reconstruction() {
        let pmk = Pmk::derive(b"password", b"ssid").expect("test WPA2 input is valid");
        let sequences = StaTxSequenceCounters::new(9);
        let station = Esp32s31StaAttemptStation {
            station_address: [2, 0, 0, 0, 0, 1],
            access_point: ScanRecord::EMPTY,
            association_preference: StaAssociationPreference::Automatic,
        };
        let owner = Esp32s31StationServiceOwner::new(
            7_u8,
            Esp32s31StationServicePhase::<u16, i16, u32, u64, u128, usize>::InitialJoin {
                hardware: 11,
                receive: 12,
                network: 13,
                station,
            },
            Esp32s31StaAttemptSecurity::new(
                pmk,
                [0x5a; 32],
                sequences,
                open_esp_radio_esp32s31_wifi_sta::wpa2::Esp32s31Wpa2Message4Protection::PairwiseCcmp,
            ),
        );
        let (runtime, phase, security) = owner.into_parts();
        assert_eq!(runtime, 7);
        let Esp32s31StationServicePhase::InitialJoin {
            hardware,
            receive,
            network,
            station: returned_station,
        } = phase
        else {
            panic!("initial owner must remain initial");
        };
        assert_eq!((hardware, receive, network), (11, 12, 13));
        assert_eq!(returned_station.station_address, station.station_address);
        assert_eq!(security.supplicant_nonce, [0x5a; 32]);
        assert_eq!(security.sequences.peek_non_qos(), 9);
    }

    #[test]
    fn common_engine_rejects_running_scan_without_refresh_before_port_entry() {
        let pmk = Pmk::derive(b"password", b"ssid").expect("test WPA2 input is valid");
        let sequences = StaTxSequenceCounters::new(3);
        let station = Esp32s31StaAttemptStation {
            station_address: [2, 0, 0, 0, 0, 2],
            access_point: ScanRecord::EMPTY,
            association_preference: StaAssociationPreference::Automatic,
        };
        let owner = Esp32s31StationServiceOwner::new(
            7_u8,
            Esp32s31StationServicePhase::<u16, i16, u32, u64, u128, usize>::RunningScan {
                disconnected: 19,
                station,
            },
            Esp32s31StaAttemptSecurity::new(
                pmk,
                [0x33; 32],
                sequences,
                open_esp_radio_esp32s31_wifi_sta::wpa2::Esp32s31Wpa2Message4Protection::PairwiseCcmp,
            ),
        );
        let control = Esp32s31StationControlResources::<NoopRawMutex>::new();
        let (_controller, mut receiver) = control.split().expect("fresh control domain splits");
        let mut runner =
            Esp32s31StationEngine::with_observer(FakePort, discovery(), FakeObserver::default());
        let outcome = block_on(runner.run_attempt(
            owner,
            StaAttemptContext {
                generation: 1,
                attempt: 2,
                refresh_candidate: false,
            },
            &mut receiver,
        ));
        let StaAttemptOutcome::Failed { owner, failure } = outcome else {
            panic!("missing refresh must be a finite contract failure");
        };
        assert_eq!(failure.stage, StaLifecycleStage::CandidateSelection);
        assert_eq!(failure.disposition, StaFailureDisposition::Terminal);
        assert_eq!(failure.error, FakeError::RefreshContract);
        assert!(matches!(
            owner.phase,
            Esp32s31StationServicePhase::RunningScan {
                disconnected: 19,
                ..
            }
        ));
        let (_port, _discovery, observer) = runner.into_parts();
        assert_eq!(
            observer.started,
            Some(Esp32s31StationServicePhaseKind::RunningScan)
        );
        assert!(observer.finished);
    }

    #[test]
    fn common_engine_selects_candidate_before_dispatching_initial_join() {
        let pmk = Pmk::derive(b"password", b"ssid").expect("test WPA2 input is valid");
        let sequences = StaTxSequenceCounters::new(5);
        let identity = Esp32s31StaIdentity {
            station_address: [2, 0, 0, 0, 0, 4],
            association_preference: StaAssociationPreference::Automatic,
        };
        let owner = Esp32s31StationServiceOwner::new(
            7_u8,
            Esp32s31StationServicePhase::<u16, i16, u32, u64, u128, usize>::InitialScan {
                hardware: 11,
                receive: -17,
                network: 13,
                identity,
            },
            Esp32s31StaAttemptSecurity::new(
                pmk,
                [0x55; 32],
                sequences,
                open_esp_radio_esp32s31_wifi_sta::wpa2::Esp32s31Wpa2Message4Protection::PairwiseCcmp,
            ),
        );
        let control = Esp32s31StationControlResources::<NoopRawMutex>::new();
        let (_controller, mut receiver) = control.split().expect("fresh control domain splits");
        let mut runner = Esp32s31StationEngine::new(ScanTransitionPort::default(), discovery());
        let outcome = block_on(runner.run_attempt(
            owner,
            StaAttemptContext {
                generation: 0,
                attempt: 1,
                refresh_candidate: true,
            },
            &mut receiver,
        ));
        let StaAttemptOutcome::Stopped { owner } = outcome else {
            panic!("initial join test must return its terminal owner");
        };
        let Esp32s31StationServicePhase::InitialJoin { station, .. } = owner.phase else {
            panic!("candidate selection must precede initial join");
        };
        assert_eq!(station.station_address, identity.station_address);
        assert_eq!(station.access_point.channel, 6);
        assert!(runner.into_port().initial_joined);
    }

    #[test]
    fn common_engine_dispatches_join_ready_scan_owner_to_reconnected_phase() {
        let pmk = Pmk::derive(b"password", b"ssid").expect("test WPA2 input is valid");
        let sequences = StaTxSequenceCounters::new(4);
        let station = Esp32s31StaAttemptStation {
            station_address: [2, 0, 0, 0, 0, 3],
            access_point: ScanRecord::EMPTY,
            association_preference: StaAssociationPreference::Automatic,
        };
        let owner = Esp32s31StationServiceOwner::new(
            7_u8,
            Esp32s31StationServicePhase::<u16, i16, u32, u64, u128, usize>::RunningScan {
                disconnected: 19,
                station,
            },
            Esp32s31StaAttemptSecurity::new(
                pmk,
                [0x44; 32],
                sequences,
                open_esp_radio_esp32s31_wifi_sta::wpa2::Esp32s31Wpa2Message4Protection::PairwiseCcmp,
            ),
        );
        let control = Esp32s31StationControlResources::<NoopRawMutex>::new();
        let (_controller, mut receiver) = control.split().expect("fresh control domain splits");
        let mut runner = Esp32s31StationEngine::new(ScanTransitionPort::default(), discovery());
        let outcome = block_on(runner.run_attempt(
            owner,
            StaAttemptContext {
                generation: 2,
                attempt: 1,
                refresh_candidate: true,
            },
            &mut receiver,
        ));
        let StaAttemptOutcome::Stopped { owner } = outcome else {
            panic!("reconnected test phase must return its terminal owner");
        };
        assert!(matches!(
            owner.phase,
            Esp32s31StationServicePhase::Reconnected {
                epoch: 23,
                network: 29,
                ..
            }
        ));
        assert!(runner.into_port().reconnected);
    }

    #[test]
    fn running_scan_completion_prepares_reconnect_only_for_a_selected_candidate() {
        let sequences = StaTxSequenceCounters::new(0);
        let pmk = Pmk::derive(b"password", b"ssid").expect("test WPA2 input is valid");
        let security = Esp32s31StaAttemptSecurity::new(
            pmk,
            [0; 32],
            sequences,
            open_esp_radio_esp32s31_wifi_sta::wpa2::Esp32s31Wpa2Message4Protection::PairwiseCcmp,
        );
        let original = Esp32s31StaAttemptStation {
            station_address: [2, 0, 0, 0, 0, 8],
            access_point: ScanRecord::EMPTY,
            association_preference: StaAssociationPreference::Automatic,
        };
        let candidate = ScanRecord {
            bssid: [0x02, 1, 2, 3, 4, 9],
            ..original.access_point
        };
        let exit =
            complete_esp32s31_station_running_scan::<_, _, _, _, _, _, core::convert::Infallible>(
                7_u8,
                11_u16,
                original,
                security,
                Esp32s31StationRunningScanCompletion::<FakeError>::Selected(candidate),
                |disconnected| (u32::from(disconnected), usize::from(disconnected) + 1),
                |_runtime, _disconnected, _station, _security| -> u64 {
                    panic!("selected candidate must not restore the running-scan owner")
                },
            );
        match exit {
            Esp32s31StationRunningScanExit::JoinReady(phase) => {
                let (runtime, epoch, network, station, _security) = phase.into_parts();
                assert_eq!(runtime, 7);
                assert_eq!(epoch, 12);
                assert_eq!(network, 11);
                assert_eq!(station.access_point.bssid, candidate.bssid);
            }
            Esp32s31StationRunningScanExit::Complete(_) => panic!("expected reconnect frontier"),
        }
    }
}
