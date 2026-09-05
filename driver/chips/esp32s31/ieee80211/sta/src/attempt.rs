//! Executor-independent finite ESP32-S31 station attempt transaction.
//!
//! Scan owns candidate discovery. This transaction consumes one selected
//! candidate owner and orders every pre-connected driver phase through the
//! connected-entry frontier. Initial join and reconnect provide different
//! owner types, but execute this same production transaction. A failed phase
//! returns the exact owner supplied by the caller; it is never reconstructed
//! from static storage or retained in an abandoned async task.

use core::{future::Future, marker::PhantomData};

use open_esp_radio_esp32s31_wifi_mac::crypto::{
    StaGroupCcmpKeyMaterial, StaGroupCcmpSlot, StaPairwiseCcmpSlot,
};
use open_esp_radio_esp32s31_wifi_mac::tx::TxCompletion;
use open_esp_radio_ieee80211::{
    channel::{WifiChannel, WifiChannelError, WifiChannelWidth},
    scan::ScanRecord,
    security::WifiSecurityMode,
    station::{StaAssociationPreference, StaTxSequenceCounters},
};
use open_esp_radio_wifi_sta::{
    join::{StaAssociationSuccess, StaAuthenticationSuccess},
    station::{StaFailureDisposition, StaLifecycleStage},
};
use open_esp_radio_wpa2::{
    Pmk, runner::Wpa2KeyInstallMetadata, supplicant::Wpa2ConnectedSupplicant,
};

use crate::{
    connected_rx::StaCcmpRxReplayEpoch,
    peer::Esp32s31StaPeerProgrammingReport,
    wpa2::{Esp32s31Wpa2HandshakeTelemetry, Esp32s31Wpa2Message4Protection},
};

/// Immutable local/candidate policy for one attempt.
#[derive(Clone, Copy)]
pub struct Esp32s31StaAttemptStation {
    pub station_address: [u8; 6],
    pub access_point: ScanRecord,
    pub association_preference: StaAssociationPreference,
    pub security: WifiSecurityMode,
}

impl Esp32s31StaAttemptStation {
    /// Exact portable channel selected by the same policy that programs the
    /// ESP32-S31 PHY. Reclaim and paired-role composition must preserve this
    /// width instead of manufacturing a 20-MHz role context from the primary
    /// channel number alone.
    pub fn selected_channel(&self) -> Result<WifiChannel, WifiChannelError> {
        let selection =
            crate::profile::select_association(&self.access_point, self.association_preference);
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
    pub security: WifiSecurityMode,
}

impl Esp32s31StaIdentity {
    pub const fn select(self, access_point: ScanRecord) -> Esp32s31StaAttemptStation {
        Esp32s31StaAttemptStation {
            station_address: self.station_address,
            access_point,
            association_preference: self.association_preference,
            security: self.security,
        }
    }
}

/// Security and sequence ownership retained across every finite phase.
///
/// These values are owned, rather than borrowed from a composition root, so a
/// complete station owner can move into an executor task without becoming
/// self-referential. A supervisor can replace credentials only after this
/// value returns through the finite task's terminal edge.
pub enum Esp32s31StaAttemptSecurityMaterial {
    Open,
    Wpa2Personal {
        pmk: Pmk,
        supplicant_nonce: [u8; 32],
        message4_protection: Esp32s31Wpa2Message4Protection,
        connected: Option<Wpa2ConnectedSupplicant>,
    },
}

pub struct Esp32s31StaAttemptSecurity<'role> {
    pub sequences: StaTxSequenceCounters,
    material: Esp32s31StaAttemptSecurityMaterial,
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
            sequences,
            material: Esp32s31StaAttemptSecurityMaterial::Wpa2Personal {
                pmk,
                supplicant_nonce,
                message4_protection,
                connected: None,
            },
            role: PhantomData,
        }
    }

    pub const fn open(sequences: StaTxSequenceCounters) -> Self {
        Self {
            sequences,
            material: Esp32s31StaAttemptSecurityMaterial::Open,
            role: PhantomData,
        }
    }

    pub const fn mode(&self) -> WifiSecurityMode {
        match self.material {
            Esp32s31StaAttemptSecurityMaterial::Open => WifiSecurityMode::Open,
            Esp32s31StaAttemptSecurityMaterial::Wpa2Personal { .. } => {
                WifiSecurityMode::Wpa2Personal
            }
        }
    }

    pub const fn wpa2_material(&self) -> Option<(&Pmk, [u8; 32], Esp32s31Wpa2Message4Protection)> {
        match &self.material {
            Esp32s31StaAttemptSecurityMaterial::Open => None,
            Esp32s31StaAttemptSecurityMaterial::Wpa2Personal {
                pmk,
                supplicant_nonce,
                message4_protection,
                ..
            } => Some((pmk, *supplicant_nonce, *message4_protection)),
        }
    }

    pub fn wpa2_handshake_parts(&mut self) -> Option<(&Pmk, [u8; 32], &mut StaTxSequenceCounters)> {
        match &self.material {
            Esp32s31StaAttemptSecurityMaterial::Open => None,
            Esp32s31StaAttemptSecurityMaterial::Wpa2Personal {
                pmk,
                supplicant_nonce,
                ..
            } => Some((pmk, *supplicant_nonce, &mut self.sequences)),
        }
    }

    pub fn set_connected(&mut self, value: Wpa2ConnectedSupplicant) -> bool {
        match &mut self.material {
            Esp32s31StaAttemptSecurityMaterial::Open => false,
            Esp32s31StaAttemptSecurityMaterial::Wpa2Personal { connected, .. } => {
                *connected = Some(value);
                true
            }
        }
    }

    pub const fn has_connected_wpa2(&self) -> bool {
        matches!(
            &self.material,
            Esp32s31StaAttemptSecurityMaterial::Wpa2Personal {
                connected: Some(_),
                ..
            }
        )
    }

    pub fn into_parts(self) -> (StaTxSequenceCounters, Esp32s31StaAttemptSecurityMaterial) {
        (self.sequences, self.material)
    }

    /// Retag the owned security state for the next finite role scope.
    ///
    /// No borrow is extended: PMK and sequence counters move by value. The
    /// marker only prevents a composition from accidentally mixing two live
    /// role scopes while the wider station API still carries that lifetime.
    pub fn into_role<'next>(self) -> Esp32s31StaAttemptSecurity<'next> {
        Esp32s31StaAttemptSecurity {
            sequences: self.sequences,
            material: self.material,
            role: PhantomData,
        }
    }
}

/// Hardware key ownership created only by a completed WPA2 attempt.
// Keep the installed slots and replay epoch inline: this no-alloc owner must
// return every hardware capability by value when the connected role ends.
#[allow(clippy::large_enum_variant)]
pub enum Esp32s31StaInstalledSecurity {
    Open,
    Wpa2Personal {
        pairwise: StaPairwiseCcmpSlot,
        group: StaGroupCcmpSlot,
        group_material: StaGroupCcmpKeyMaterial,
        replay: StaCcmpRxReplayEpoch,
    },
}

impl Esp32s31StaInstalledSecurity {
    pub const fn mode(&self) -> WifiSecurityMode {
        match self {
            Self::Open => WifiSecurityMode::Open,
            Self::Wpa2Personal { .. } => WifiSecurityMode::Wpa2Personal,
        }
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
    /// Exact security execution selected before association. Open makes the
    /// legacy WPA2-named transaction stages explicit no-ops; it never means
    /// that a handshake or key installation succeeded.
    pub security: Option<Esp32s31StaAttemptSecurityExecution>,
    pub authentication: Option<StaAuthenticationSuccess>,
    pub association: Option<StaAssociationSuccess>,
    pub peer: Option<Esp32s31StaPeerProgrammingReport>,
    /// Message-2 progress is retained even when the handshake later fails.
    pub wpa2_handshake: Option<Esp32s31Wpa2HandshakeTelemetry>,
    pub wpa2: Option<Wpa2KeyInstallMetadata>,
    /// A failed Message 4 status is still useful attempt evidence.
    pub message4: Option<TxCompletion>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31StaAttemptSecurityExecution {
    OpenHandshakeAndKeyInstallSkipped,
    Wpa2Personal,
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
mod tests;
