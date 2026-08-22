//! Executor-independent finite ESP32-S31 station attempt transaction.
//!
//! Scan owns candidate discovery. This transaction consumes one selected
//! candidate owner and orders every pre-connected driver phase through the
//! connected-entry frontier. Initial join and reconnect provide different
//! owner types, but execute this same production transaction. A failed phase
//! returns the exact owner supplied by the caller; it is never reconstructed
//! from static storage or retained in an abandoned async task.

use core::{future::Future, marker::PhantomData};

use open_esp_radio_esp32s31_wifi_mac::tx::TxCompletion;
use open_esp_radio_ieee80211::{
    channel::{WifiChannel, WifiChannelError, WifiChannelWidth},
    scan::ScanRecord,
    station::{StaAssociationPreference, StaTxSequenceCounters, select_sta_association},
};
use open_esp_radio_wifi_sta::{
    join::{StaAssociationSuccess, StaAuthenticationSuccess},
    station::{StaFailureDisposition, StaLifecycleStage},
};
use open_esp_radio_wpa2::{
    Pmk, runner::Wpa2KeyInstallMetadata, supplicant::Wpa2ConnectedSupplicant,
};

use crate::{
    peer::Esp32s31StaPeerProgrammingReport,
    wpa2::{Esp32s31Wpa2HandshakeTelemetry, Esp32s31Wpa2Message4Protection},
};

/// Immutable local/candidate policy for one attempt.
#[derive(Clone, Copy)]
pub struct Esp32s31StaAttemptStation {
    pub station_address: [u8; 6],
    pub access_point: ScanRecord,
    pub association_preference: StaAssociationPreference,
}

impl Esp32s31StaAttemptStation {
    /// Exact portable channel selected by the same policy that programs the
    /// ESP32-S31 PHY. Reclaim and paired-role composition must preserve this
    /// width instead of manufacturing a 20-MHz role context from the primary
    /// channel number alone.
    pub fn selected_channel(&self) -> Result<WifiChannel, WifiChannelError> {
        let selection = select_sta_association(&self.access_point, self.association_preference);
        let width = match selection.cbw {
            2 => WifiChannelWidth::Mhz40Above,
            3 => WifiChannelWidth::Mhz40Below,
            _ => WifiChannelWidth::Mhz20,
        };
        WifiChannel::new_2_4_ghz(selection.primary_channel, width)
    }
}

/// Station identity and association policy before candidate selection.
///
/// Keeping this distinct from [`Esp32s31StaAttemptStation`] makes it
/// impossible to enter Authentication/Association with a fabricated empty
/// scan record merely to satisfy an owner layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31StaIdentity {
    pub station_address: [u8; 6],
    pub association_preference: StaAssociationPreference,
}

impl Esp32s31StaIdentity {
    pub const fn select(self, access_point: ScanRecord) -> Esp32s31StaAttemptStation {
        Esp32s31StaAttemptStation {
            station_address: self.station_address,
            access_point,
            association_preference: self.association_preference,
        }
    }
}

/// Security and sequence ownership retained across every finite phase.
///
/// These values are owned, rather than borrowed from a composition root, so a
/// complete station owner can move into an executor task without becoming
/// self-referential. A supervisor can replace credentials only after this
/// value returns through the finite task's terminal edge.
pub struct Esp32s31StaAttemptSecurity<'role> {
    pub pmk: Pmk,
    pub supplicant_nonce: [u8; 32],
    pub sequences: StaTxSequenceCounters,
    pub message4_protection: Esp32s31Wpa2Message4Protection,
    pub connected: Option<Wpa2ConnectedSupplicant>,
    role: PhantomData<&'role mut ()>,
}

impl Esp32s31StaAttemptSecurity<'_> {
    pub const fn new(
        pmk: Pmk,
        supplicant_nonce: [u8; 32],
        sequences: StaTxSequenceCounters,
        message4_protection: Esp32s31Wpa2Message4Protection,
    ) -> Self {
        Self {
            pmk,
            supplicant_nonce,
            sequences,
            message4_protection,
            connected: None,
            role: PhantomData,
        }
    }

    /// Retag the owned security state for the next finite role scope.
    ///
    /// No borrow is extended: PMK and sequence counters move by value. The
    /// marker only prevents a composition from accidentally mixing two live
    /// role scopes while the wider station API still carries that lifetime.
    pub fn into_role<'next>(self) -> Esp32s31StaAttemptSecurity<'next> {
        Esp32s31StaAttemptSecurity::new(
            self.pmk,
            self.supplicant_nonce,
            self.sequences,
            self.message4_protection,
        )
    }
}

/// Internal state invariant failure. The complete outer owner is still
/// returned, but retrying without resetting the transaction would be unsafe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StaAttemptStateError {
    MissingReceive,
    MissingPreparedPeer,
    MissingAssociation,
    MissingConnectedPeer,
    MissingHandshake,
    MissingKeys,
    MissingConnectedSecurity,
}

/// Value-only reports produced by the real driver phases.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31StaAttemptReport {
    pub authentication: Option<StaAuthenticationSuccess>,
    pub association: Option<StaAssociationSuccess>,
    pub peer: Option<Esp32s31StaPeerProgrammingReport>,
    /// Message-2 progress is retained even when the handshake later fails.
    pub wpa2_handshake: Option<Esp32s31Wpa2HandshakeTelemetry>,
    pub wpa2: Option<Wpa2KeyInstallMetadata>,
    /// A failed Message 4 status is still useful attempt evidence.
    pub message4: Option<TxCompletion>,
}

/// Connected-entry proof returned only after all preceding phases.
pub struct Esp32s31StaAttemptConnected<O> {
    owner: O,
}

impl<O> Esp32s31StaAttemptConnected<O> {
    pub const fn new(owner: O) -> Self {
        Self { owner }
    }

    pub fn into_owner(self) -> O {
        self.owner
    }
}

/// Exact finite phase currently owned by a station attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Esp32s31StaAttemptStage {
    Candidate = 0,
    Channel = 1,
    Authentication = 2,
    Association = 3,
    PeerProgramming = 4,
    Wpa2Handshake = 5,
    Wpa2KeyInstall = 6,
    ConnectedEntry = 7,
}

impl Esp32s31StaAttemptStage {
    pub const COUNT: u8 = 8;

    /// Coarser stage consumed by the chip-independent reconnect policy.
    pub const fn lifecycle_stage(self) -> StaLifecycleStage {
        match self {
            Self::Candidate => StaLifecycleStage::CandidateSelection,
            Self::Channel | Self::PeerProgramming => StaLifecycleStage::Hardware,
            Self::Authentication => StaLifecycleStage::Authentication,
            Self::Association => StaLifecycleStage::Association,
            Self::Wpa2Handshake | Self::Wpa2KeyInstall => StaLifecycleStage::Security,
            Self::ConnectedEntry => StaLifecycleStage::Connected,
        }
    }

    const fn bit(self) -> u16 {
        1_u16 << self as u8
    }
}

/// Bounded evidence returned at both success and failure edges.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31StaAttemptProgress {
    completed: u16,
}

impl Esp32s31StaAttemptProgress {
    pub const fn completed(self, stage: Esp32s31StaAttemptStage) -> bool {
        self.completed & stage.bit() != 0
    }

    pub const fn completed_count(self) -> u8 {
        self.completed.count_ones() as u8
    }

    fn mark_completed(&mut self, stage: Esp32s31StaAttemptStage) {
        self.completed |= stage.bit();
    }
}

/// Port-classified error from a phase which only borrowed the attempt owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31StaAttemptStepError<E> {
    pub disposition: StaFailureDisposition,
    pub error: E,
}

impl<E> Esp32s31StaAttemptStepError<E> {
    pub const fn new(disposition: StaFailureDisposition, error: E) -> Self {
        Self { disposition, error }
    }

    pub const fn retry_current(error: E) -> Self {
        Self::new(StaFailureDisposition::RetryCurrentCandidate, error)
    }

    pub const fn refresh_candidate(error: E) -> Self {
        Self::new(StaFailureDisposition::RefreshCandidate, error)
    }

    pub const fn terminal(error: E) -> Self {
        Self::new(StaFailureDisposition::Terminal, error)
    }
}

/// Failed consuming connected-entry edge with the exact input owner restored.
#[derive(Debug, Eq, PartialEq)]
pub struct Esp32s31StaConnectedEntryFailure<O, E> {
    pub owner: O,
    pub disposition: StaFailureDisposition,
    pub error: E,
}

impl<O, E> Esp32s31StaConnectedEntryFailure<O, E> {
    pub const fn new(owner: O, disposition: StaFailureDisposition, error: E) -> Self {
        Self {
            owner,
            disposition,
            error,
        }
    }
}

/// One finite attempt failure with complete retry or terminal ownership.
#[derive(Debug, Eq, PartialEq)]
pub struct Esp32s31StaAttemptFailure<O, E> {
    pub owner: O,
    pub stage: Esp32s31StaAttemptStage,
    pub disposition: StaFailureDisposition,
    pub error: E,
    pub progress: Esp32s31StaAttemptProgress,
}

impl<O, E> Esp32s31StaAttemptFailure<O, E> {
    pub const fn lifecycle_stage(&self) -> StaLifecycleStage {
        self.stage.lifecycle_stage()
    }

    pub fn into_parts(
        self,
    ) -> (
        O,
        Esp32s31StaAttemptStage,
        StaFailureDisposition,
        E,
        Esp32s31StaAttemptProgress,
    ) {
        (
            self.owner,
            self.stage,
            self.disposition,
            self.error,
            self.progress,
        )
    }
}

/// Successful connected frontier or a fully owned finite failure.
#[derive(Debug, Eq, PartialEq)]
pub enum Esp32s31StaAttemptOutcome<O, C, E> {
    Connected {
        connected: C,
        progress: Esp32s31StaAttemptProgress,
    },
    Failed(Esp32s31StaAttemptFailure<O, E>),
}

/// Value-only observation hooks around the production transaction.
///
/// Observers cannot touch the owner or alter driver policy. The normal driver
/// uses `()`; HIL may map these boundaries to UART timing evidence.
pub trait Esp32s31StaAttemptObserver {
    fn stage_started(&mut self, _stage: Esp32s31StaAttemptStage) {}

    fn stage_completed(&mut self, _stage: Esp32s31StaAttemptStage) {}

    fn stage_failed(
        &mut self,
        _stage: Esp32s31StaAttemptStage,
        _disposition: StaFailureDisposition,
    ) {
    }
}

impl Esp32s31StaAttemptObserver for () {}

/// Concrete finite operations needed by [`Esp32s31StaAttempt`].
///
/// Every non-consuming operation must leave `Owner` at a valid retry or
/// terminal frontier before returning `Err`. Only connected entry consumes
/// the owner, so that edge must return it explicitly on failure.
pub trait Esp32s31StaAttemptPort: Copy {
    type Owner;
    type Connected;
    type Error;

    fn prepare_candidate<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), Esp32s31StaAttemptStepError<Self::Error>>> + 'a;

    fn select_channel<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), Esp32s31StaAttemptStepError<Self::Error>>> + 'a;

    fn authenticate<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), Esp32s31StaAttemptStepError<Self::Error>>> + 'a;

    fn associate<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), Esp32s31StaAttemptStepError<Self::Error>>> + 'a;

    fn program_peer<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), Esp32s31StaAttemptStepError<Self::Error>>> + 'a;

    fn run_wpa2_handshake<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), Esp32s31StaAttemptStepError<Self::Error>>> + 'a;

    fn install_wpa2_keys<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), Esp32s31StaAttemptStepError<Self::Error>>> + 'a;

    fn enter_connected(
        &mut self,
        owner: Self::Owner,
    ) -> impl Future<
        Output = Result<
            Self::Connected,
            Esp32s31StaConnectedEntryFailure<Self::Owner, Self::Error>,
        >,
    > + '_;
}

/// Shared production transaction for initial join and reconnect.
pub struct Esp32s31StaAttempt<P, O = ()> {
    port: P,
    observer: O,
}

impl<P> Esp32s31StaAttempt<P, ()> {
    pub const fn new(port: P) -> Self {
        Self { port, observer: () }
    }
}

impl<P, O> Esp32s31StaAttempt<P, O> {
    pub const fn with_observer(port: P, observer: O) -> Self {
        Self { port, observer }
    }

    pub fn port(&self) -> &P {
        &self.port
    }

    pub fn port_mut(&mut self) -> &mut P {
        &mut self.port
    }

    pub fn into_parts(self) -> (P, O) {
        (self.port, self.observer)
    }
}

impl<P, O> Esp32s31StaAttempt<P, O>
where
    P: Esp32s31StaAttemptPort,
    O: Esp32s31StaAttemptObserver,
{
    /// Execute every finite pre-connected phase exactly once and in order.
    pub async fn run(
        &mut self,
        mut owner: P::Owner,
    ) -> Esp32s31StaAttemptOutcome<P::Owner, P::Connected, P::Error> {
        let mut progress = Esp32s31StaAttemptProgress::default();

        self.observer
            .stage_started(Esp32s31StaAttemptStage::Candidate);
        if let Err(failure) = self.port.prepare_candidate(&mut owner).await {
            return self.failed(owner, Esp32s31StaAttemptStage::Candidate, failure, progress);
        }
        self.completed(Esp32s31StaAttemptStage::Candidate, &mut progress);

        self.observer
            .stage_started(Esp32s31StaAttemptStage::Channel);
        if let Err(failure) = self.port.select_channel(&mut owner).await {
            return self.failed(owner, Esp32s31StaAttemptStage::Channel, failure, progress);
        }
        self.completed(Esp32s31StaAttemptStage::Channel, &mut progress);

        self.observer
            .stage_started(Esp32s31StaAttemptStage::Authentication);
        if let Err(failure) = self.port.authenticate(&mut owner).await {
            return self.failed(
                owner,
                Esp32s31StaAttemptStage::Authentication,
                failure,
                progress,
            );
        }
        self.completed(Esp32s31StaAttemptStage::Authentication, &mut progress);

        self.observer
            .stage_started(Esp32s31StaAttemptStage::Association);
        if let Err(failure) = self.port.associate(&mut owner).await {
            return self.failed(
                owner,
                Esp32s31StaAttemptStage::Association,
                failure,
                progress,
            );
        }
        self.completed(Esp32s31StaAttemptStage::Association, &mut progress);

        self.observer
            .stage_started(Esp32s31StaAttemptStage::PeerProgramming);
        if let Err(failure) = self.port.program_peer(&mut owner).await {
            return self.failed(
                owner,
                Esp32s31StaAttemptStage::PeerProgramming,
                failure,
                progress,
            );
        }
        self.completed(Esp32s31StaAttemptStage::PeerProgramming, &mut progress);

        self.observer
            .stage_started(Esp32s31StaAttemptStage::Wpa2Handshake);
        if let Err(failure) = self.port.run_wpa2_handshake(&mut owner).await {
            return self.failed(
                owner,
                Esp32s31StaAttemptStage::Wpa2Handshake,
                failure,
                progress,
            );
        }
        self.completed(Esp32s31StaAttemptStage::Wpa2Handshake, &mut progress);

        self.observer
            .stage_started(Esp32s31StaAttemptStage::Wpa2KeyInstall);
        if let Err(failure) = self.port.install_wpa2_keys(&mut owner).await {
            return self.failed(
                owner,
                Esp32s31StaAttemptStage::Wpa2KeyInstall,
                failure,
                progress,
            );
        }
        self.completed(Esp32s31StaAttemptStage::Wpa2KeyInstall, &mut progress);

        self.observer
            .stage_started(Esp32s31StaAttemptStage::ConnectedEntry);
        match self.port.enter_connected(owner).await {
            Ok(connected) => {
                self.completed(Esp32s31StaAttemptStage::ConnectedEntry, &mut progress);
                Esp32s31StaAttemptOutcome::Connected {
                    connected,
                    progress,
                }
            }
            Err(failure) => {
                self.observer
                    .stage_failed(Esp32s31StaAttemptStage::ConnectedEntry, failure.disposition);
                Esp32s31StaAttemptOutcome::Failed(Esp32s31StaAttemptFailure {
                    owner: failure.owner,
                    stage: Esp32s31StaAttemptStage::ConnectedEntry,
                    disposition: failure.disposition,
                    error: failure.error,
                    progress,
                })
            }
        }
    }

    fn completed(
        &mut self,
        stage: Esp32s31StaAttemptStage,
        progress: &mut Esp32s31StaAttemptProgress,
    ) {
        progress.mark_completed(stage);
        self.observer.stage_completed(stage);
    }

    fn failed(
        &mut self,
        owner: P::Owner,
        stage: Esp32s31StaAttemptStage,
        failure: Esp32s31StaAttemptStepError<P::Error>,
        progress: Esp32s31StaAttemptProgress,
    ) -> Esp32s31StaAttemptOutcome<P::Owner, P::Connected, P::Error> {
        self.observer.stage_failed(stage, failure.disposition);
        Esp32s31StaAttemptOutcome::Failed(Esp32s31StaAttemptFailure {
            owner,
            stage,
            disposition: failure.disposition,
            error: failure.error,
            progress,
        })
    }
}

#[cfg(test)]
mod tests {
    use core::future::ready;

    use super::*;
    use crate::test_support::block_on;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct Owner {
        identity: u32,
        completed: u8,
        calls: [Option<Esp32s31StaAttemptStage>; Esp32s31StaAttemptStage::COUNT as usize],
        call_count: usize,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct Connected {
        identity: u32,
        calls: [Option<Esp32s31StaAttemptStage>; Esp32s31StaAttemptStage::COUNT as usize],
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct Error(Esp32s31StaAttemptStage);

    #[derive(Clone, Copy)]
    struct MockPort {
        fail_at: Option<Esp32s31StaAttemptStage>,
    }

    impl MockPort {
        fn new(fail_at: Option<Esp32s31StaAttemptStage>) -> Self {
            Self { fail_at }
        }

        fn step(
            &mut self,
            owner: &mut Owner,
            stage: Esp32s31StaAttemptStage,
        ) -> Result<(), Esp32s31StaAttemptStepError<Error>> {
            owner.calls[owner.call_count] = Some(stage);
            owner.call_count += 1;
            if self.fail_at == Some(stage) {
                return Err(Esp32s31StaAttemptStepError::retry_current(Error(stage)));
            }
            owner.completed += 1;
            Ok(())
        }
    }

    impl Esp32s31StaAttemptPort for MockPort {
        type Owner = Owner;
        type Connected = Connected;
        type Error = Error;

        fn prepare_candidate<'a>(
            &'a mut self,
            owner: &'a mut Self::Owner,
        ) -> impl Future<Output = Result<(), Esp32s31StaAttemptStepError<Self::Error>>> + 'a
        {
            ready(self.step(owner, Esp32s31StaAttemptStage::Candidate))
        }

        fn select_channel<'a>(
            &'a mut self,
            owner: &'a mut Self::Owner,
        ) -> impl Future<Output = Result<(), Esp32s31StaAttemptStepError<Self::Error>>> + 'a
        {
            ready(self.step(owner, Esp32s31StaAttemptStage::Channel))
        }

        fn authenticate<'a>(
            &'a mut self,
            owner: &'a mut Self::Owner,
        ) -> impl Future<Output = Result<(), Esp32s31StaAttemptStepError<Self::Error>>> + 'a
        {
            ready(self.step(owner, Esp32s31StaAttemptStage::Authentication))
        }

        fn associate<'a>(
            &'a mut self,
            owner: &'a mut Self::Owner,
        ) -> impl Future<Output = Result<(), Esp32s31StaAttemptStepError<Self::Error>>> + 'a
        {
            ready(self.step(owner, Esp32s31StaAttemptStage::Association))
        }

        fn program_peer<'a>(
            &'a mut self,
            owner: &'a mut Self::Owner,
        ) -> impl Future<Output = Result<(), Esp32s31StaAttemptStepError<Self::Error>>> + 'a
        {
            ready(self.step(owner, Esp32s31StaAttemptStage::PeerProgramming))
        }

        fn run_wpa2_handshake<'a>(
            &'a mut self,
            owner: &'a mut Self::Owner,
        ) -> impl Future<Output = Result<(), Esp32s31StaAttemptStepError<Self::Error>>> + 'a
        {
            ready(self.step(owner, Esp32s31StaAttemptStage::Wpa2Handshake))
        }

        fn install_wpa2_keys<'a>(
            &'a mut self,
            owner: &'a mut Self::Owner,
        ) -> impl Future<Output = Result<(), Esp32s31StaAttemptStepError<Self::Error>>> + 'a
        {
            ready(self.step(owner, Esp32s31StaAttemptStage::Wpa2KeyInstall))
        }

        fn enter_connected(
            &mut self,
            mut owner: Self::Owner,
        ) -> impl Future<
            Output = Result<
                Self::Connected,
                Esp32s31StaConnectedEntryFailure<Self::Owner, Self::Error>,
            >,
        > + '_ {
            owner.calls[owner.call_count] = Some(Esp32s31StaAttemptStage::ConnectedEntry);
            owner.call_count += 1;
            if self.fail_at == Some(Esp32s31StaAttemptStage::ConnectedEntry) {
                return ready(Err(Esp32s31StaConnectedEntryFailure::new(
                    owner,
                    StaFailureDisposition::Terminal,
                    Error(Esp32s31StaAttemptStage::ConnectedEntry),
                )));
            }
            owner.completed += 1;
            ready(Ok(Connected {
                identity: owner.identity,
                calls: owner.calls,
            }))
        }
    }

    #[derive(Default)]
    struct Observer {
        started: [Option<Esp32s31StaAttemptStage>; Esp32s31StaAttemptStage::COUNT as usize],
        completed: [Option<Esp32s31StaAttemptStage>; Esp32s31StaAttemptStage::COUNT as usize],
        failed: Option<(Esp32s31StaAttemptStage, StaFailureDisposition)>,
        started_count: usize,
        completed_count: usize,
    }

    impl Esp32s31StaAttemptObserver for Observer {
        fn stage_started(&mut self, stage: Esp32s31StaAttemptStage) {
            self.started[self.started_count] = Some(stage);
            self.started_count += 1;
        }

        fn stage_completed(&mut self, stage: Esp32s31StaAttemptStage) {
            self.completed[self.completed_count] = Some(stage);
            self.completed_count += 1;
        }

        fn stage_failed(
            &mut self,
            stage: Esp32s31StaAttemptStage,
            disposition: StaFailureDisposition,
        ) {
            self.failed = Some((stage, disposition));
        }
    }

    const STAGES: [Esp32s31StaAttemptStage; Esp32s31StaAttemptStage::COUNT as usize] = [
        Esp32s31StaAttemptStage::Candidate,
        Esp32s31StaAttemptStage::Channel,
        Esp32s31StaAttemptStage::Authentication,
        Esp32s31StaAttemptStage::Association,
        Esp32s31StaAttemptStage::PeerProgramming,
        Esp32s31StaAttemptStage::Wpa2Handshake,
        Esp32s31StaAttemptStage::Wpa2KeyInstall,
        Esp32s31StaAttemptStage::ConnectedEntry,
    ];

    #[test]
    fn selected_channel_preserves_negotiated_ht40_geometry() {
        let mut access_point = ScanRecord {
            channel: 6,
            ht_capability_ie_present: true,
            ht_operation_ie_present: true,
            ..ScanRecord::EMPTY
        };
        access_point.ht_capability_ie[0..4].copy_from_slice(&[45, 26, 0x02, 0]);
        access_point.ht_operation_ie[0..4].copy_from_slice(&[61, 22, 6, 0x05]);
        let station = Esp32s31StaAttemptStation {
            station_address: [0; 6],
            access_point,
            association_preference: StaAssociationPreference::Automatic,
        };
        assert_eq!(
            station.selected_channel(),
            WifiChannel::new_2_4_ghz(6, WifiChannelWidth::Mhz40Above)
        );
    }

    #[test]
    fn complete_attempt_orders_every_phase_once() {
        let mut attempt =
            Esp32s31StaAttempt::with_observer(MockPort::new(None), Observer::default());
        let outcome = block_on(attempt.run(Owner {
            identity: 41,
            completed: 0,
            calls: [None; Esp32s31StaAttemptStage::COUNT as usize],
            call_count: 0,
        }));
        let Esp32s31StaAttemptOutcome::Connected {
            connected,
            progress,
        } = outcome
        else {
            panic!("complete attempt failed");
        };
        assert_eq!(
            connected,
            Connected {
                identity: 41,
                calls: STAGES.map(Some),
            }
        );
        assert_eq!(progress.completed_count(), Esp32s31StaAttemptStage::COUNT);
        for stage in STAGES {
            assert!(progress.completed(stage));
        }
        let (_, observer) = attempt.into_parts();
        assert_eq!(observer.started, STAGES.map(Some));
        assert_eq!(observer.completed, STAGES.map(Some));
        assert_eq!(observer.failed, None);
    }

    #[test]
    fn every_borrowed_phase_failure_returns_the_exact_owner_and_frontier() {
        for (failed_index, failed_stage) in STAGES[..STAGES.len() - 1].iter().enumerate() {
            let mut attempt = Esp32s31StaAttempt::new(MockPort::new(Some(*failed_stage)));
            let outcome = block_on(attempt.run(Owner {
                identity: 77,
                completed: 0,
                calls: [None; Esp32s31StaAttemptStage::COUNT as usize],
                call_count: 0,
            }));
            let Esp32s31StaAttemptOutcome::Failed(failure) = outcome else {
                panic!("failed phase reached connected frontier");
            };
            assert_eq!(failure.owner.identity, 77);
            assert_eq!(failure.owner.completed, failed_index as u8);
            assert_eq!(failure.stage, *failed_stage);
            assert_eq!(
                failure.disposition,
                StaFailureDisposition::RetryCurrentCandidate
            );
            assert_eq!(failure.error, Error(*failed_stage));
            assert_eq!(failure.progress.completed_count(), failed_index as u8);
            for (call, expected) in failure.owner.calls[..=failed_index]
                .iter()
                .zip(&STAGES[..=failed_index])
            {
                assert_eq!(*call, Some(*expected));
            }
        }
    }

    #[test]
    fn connected_entry_failure_must_return_the_consumed_owner() {
        let mut attempt =
            Esp32s31StaAttempt::new(MockPort::new(Some(Esp32s31StaAttemptStage::ConnectedEntry)));
        let outcome = block_on(attempt.run(Owner {
            identity: 91,
            completed: 0,
            calls: [None; Esp32s31StaAttemptStage::COUNT as usize],
            call_count: 0,
        }));
        let Esp32s31StaAttemptOutcome::Failed(failure) = outcome else {
            panic!("connected entry unexpectedly passed");
        };
        assert_eq!(failure.owner.identity, 91);
        assert_eq!(failure.owner.completed, 7);
        assert_eq!(failure.stage, Esp32s31StaAttemptStage::ConnectedEntry);
        assert_eq!(failure.disposition, StaFailureDisposition::Terminal);
        assert_eq!(failure.progress.completed_count(), 7);
        assert!(
            !failure
                .progress
                .completed(Esp32s31StaAttemptStage::ConnectedEntry)
        );
    }
}
