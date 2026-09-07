//! Executor-independent finite ESP32-S31 station attempt transaction.
//!
//! Scan owns candidate discovery. This transaction consumes one selected
//! candidate owner and orders every pre-connected driver phase through the
//! connected-entry frontier. Initial join and reconnect provide different
//! owner types, but execute this same production transaction. A failed phase
//! returns the exact owner supplied by the caller; it is never reconstructed
//! from static storage or retained in an abandoned async task.

use core::{future::Future, marker::PhantomData};

use crate::{
    connected_rx::StaCcmpRxReplayEpoch,
    peer::StaPeerProgrammingReport,
    wpa2::{Wpa2HandshakeTelemetry, Wpa2Message4Protection},
};

use oer_esp32s31_wifi_mac::{
    crypto::{StaGroupCcmpKeyMaterial, StaGroupCcmpSlot, StaPairwiseCcmpSlot},
    tx::TxCompletion,
};

use {
    oer_ieee80211::channel::WifiChannel, oer_ieee80211::channel::WifiChannelError,
    oer_ieee80211::channel::WifiChannelWidth, oer_ieee80211::scan::ScanRecord,
    oer_ieee80211::security::WifiSecurityMode, oer_ieee80211::station::StaTxSequenceCounters,
    oer_ieee80211::station::association::Preference,
};

use oer_wifi_sta::{
    join::{StaAssociationSuccess, StaAuthenticationSuccess},
    station::{StaFailureDisposition, StaLifecycleStage},
};

use oer_wpa2::{Pmk, runner::Wpa2KeyInstallMetadata, supplicant::Wpa2ConnectedSupplicant};

/// Immutable local/candidate policy for one attempt.
#[derive(Clone, Copy)]
pub struct StaAttemptStation {
    pub station_address: [u8; 6],
    pub access_point: ScanRecord,
    pub association_preference: Preference,
    pub security: WifiSecurityMode,
}

impl StaAttemptStation {
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
/// Keeping this distinct from [`StaAttemptStation`] makes it
/// impossible to enter Authentication/Association with a fabricated empty
/// scan record merely to satisfy an owner layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaIdentity {
    pub station_address: [u8; 6],
    pub association_preference: Preference,
    pub security: WifiSecurityMode,
}

impl StaIdentity {
    pub const fn select(self, access_point: ScanRecord) -> StaAttemptStation {
        StaAttemptStation {
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
pub enum StaAttemptSecurityMaterial {
    Open,
    Wpa2Personal {
        pmk: Pmk,
        supplicant_nonce: [u8; 32],
        message4_protection: Wpa2Message4Protection,
        connected: Option<Wpa2ConnectedSupplicant>,
    },
}

pub struct StaAttemptSecurity<'role> {
    pub sequences: StaTxSequenceCounters,
    material: StaAttemptSecurityMaterial,
    role: PhantomData<&'role mut ()>,
}

impl StaAttemptSecurity<'_> {
    pub const fn new(
        pmk: Pmk,
        supplicant_nonce: [u8; 32],
        sequences: StaTxSequenceCounters,
        message4_protection: Wpa2Message4Protection,
    ) -> Self {
        Self {
            sequences,
            material: StaAttemptSecurityMaterial::Wpa2Personal {
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
            material: StaAttemptSecurityMaterial::Open,
            role: PhantomData,
        }
    }

    pub const fn mode(&self) -> WifiSecurityMode {
        match self.material {
            StaAttemptSecurityMaterial::Open => WifiSecurityMode::Open,
            StaAttemptSecurityMaterial::Wpa2Personal { .. } => WifiSecurityMode::Wpa2Personal,
        }
    }

    pub const fn wpa2_material(&self) -> Option<(&Pmk, [u8; 32], Wpa2Message4Protection)> {
        match &self.material {
            StaAttemptSecurityMaterial::Open => None,
            StaAttemptSecurityMaterial::Wpa2Personal {
                pmk,
                supplicant_nonce,
                message4_protection,
                ..
            } => Some((pmk, *supplicant_nonce, *message4_protection)),
        }
    }

    pub fn wpa2_handshake_parts(&mut self) -> Option<(&Pmk, [u8; 32], &mut StaTxSequenceCounters)> {
        match &self.material {
            StaAttemptSecurityMaterial::Open => None,
            StaAttemptSecurityMaterial::Wpa2Personal {
                pmk,
                supplicant_nonce,
                ..
            } => Some((pmk, *supplicant_nonce, &mut self.sequences)),
        }
    }

    pub fn set_connected(&mut self, value: Wpa2ConnectedSupplicant) -> bool {
        match &mut self.material {
            StaAttemptSecurityMaterial::Open => false,
            StaAttemptSecurityMaterial::Wpa2Personal { connected, .. } => {
                *connected = Some(value);
                true
            }
        }
    }

    pub const fn has_connected_wpa2(&self) -> bool {
        matches!(
            &self.material,
            StaAttemptSecurityMaterial::Wpa2Personal {
                connected: Some(_),
                ..
            }
        )
    }

    pub fn into_parts(self) -> (StaTxSequenceCounters, StaAttemptSecurityMaterial) {
        (self.sequences, self.material)
    }

    /// Retag the owned security state for the next finite role scope.
    ///
    /// No borrow is extended: PMK and sequence counters move by value. The
    /// marker only prevents a composition from accidentally mixing two live
    /// role scopes while the wider station API still carries that lifetime.
    pub fn into_role<'next>(self) -> StaAttemptSecurity<'next> {
        StaAttemptSecurity {
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
pub enum StaInstalledSecurity {
    Open,
    Wpa2Personal {
        pairwise: StaPairwiseCcmpSlot,
        group: StaGroupCcmpSlot,
        group_material: StaGroupCcmpKeyMaterial,
        replay: StaCcmpRxReplayEpoch,
    },
}

impl StaInstalledSecurity {
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
pub enum StaAttemptStateError {
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
pub struct StaAttemptReport {
    /// Exact security execution selected before association. Open makes the
    /// legacy WPA2-named transaction stages explicit no-ops; it never means
    /// that a handshake or key installation succeeded.
    pub security: Option<StaAttemptSecurityExecution>,
    pub authentication: Option<StaAuthenticationSuccess>,
    pub association: Option<StaAssociationSuccess>,
    pub peer: Option<StaPeerProgrammingReport>,
    /// Message-2 progress is retained even when the handshake later fails.
    pub wpa2_handshake: Option<Wpa2HandshakeTelemetry>,
    pub wpa2: Option<Wpa2KeyInstallMetadata>,
    /// A failed Message 4 status is still useful attempt evidence.
    pub message4: Option<TxCompletion>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaAttemptSecurityExecution {
    OpenHandshakeAndKeyInstallSkipped,
    Wpa2Personal,
}

/// Connected-entry proof returned only after all preceding phases.
pub struct StaAttemptConnected<O> {
    owner: O,
}

impl<O> StaAttemptConnected<O> {
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
pub enum StaAttemptStage {
    Candidate = 0,
    Channel = 1,
    Authentication = 2,
    Association = 3,
    PeerProgramming = 4,
    Wpa2Handshake = 5,
    Wpa2KeyInstall = 6,
    ConnectedEntry = 7,
}

impl StaAttemptStage {
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
pub struct StaAttemptProgress {
    completed: u16,
}

impl StaAttemptProgress {
    pub const fn completed(self, stage: StaAttemptStage) -> bool {
        self.completed & stage.bit() != 0
    }

    pub const fn completed_count(self) -> u8 {
        self.completed.count_ones() as u8
    }

    fn mark_completed(&mut self, stage: StaAttemptStage) {
        self.completed |= stage.bit();
    }
}

/// Port-classified error from a phase which only borrowed the attempt owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaAttemptStepError<E> {
    pub disposition: StaFailureDisposition,
    pub error: E,
}

impl<E> StaAttemptStepError<E> {
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
pub struct StaConnectedEntryFailure<O, E> {
    pub owner: O,
    pub disposition: StaFailureDisposition,
    pub error: E,
}

impl<O, E> StaConnectedEntryFailure<O, E> {
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
pub struct AssociationAttemptFailure<O, E> {
    pub owner: O,
    pub stage: StaAttemptStage,
    pub disposition: StaFailureDisposition,
    pub error: E,
    pub progress: StaAttemptProgress,
}

impl<O, E> AssociationAttemptFailure<O, E> {
    pub const fn lifecycle_stage(&self) -> StaLifecycleStage {
        self.stage.lifecycle_stage()
    }

    pub fn into_parts(
        self,
    ) -> (
        O,
        StaAttemptStage,
        StaFailureDisposition,
        E,
        StaAttemptProgress,
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
pub enum AssociationAttemptOutcome<O, C, E> {
    Connected {
        connected: C,
        progress: StaAttemptProgress,
    },
    Failed(AssociationAttemptFailure<O, E>),
}

/// Value-only observation hooks around the production transaction.
///
/// Observers cannot touch the owner or alter driver policy. The normal driver
/// uses `()`; HIL may map these boundaries to UART timing evidence.
pub trait StaAttemptObserver {
    fn stage_started(&mut self, _stage: StaAttemptStage) {}

    fn stage_completed(&mut self, _stage: StaAttemptStage) {}

    fn stage_failed(&mut self, _stage: StaAttemptStage, _disposition: StaFailureDisposition) {}
}

impl StaAttemptObserver for () {}

/// Concrete finite operations needed by [`StaAttempt`].
///
/// Every non-consuming operation must leave `Owner` at a valid retry or
/// terminal frontier before returning `Err`. Only connected entry consumes
/// the owner, so that edge must return it explicitly on failure.
pub trait StaAttemptPort: Copy {
    type Owner;
    type Connected;
    type Error;

    fn prepare_candidate<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), StaAttemptStepError<Self::Error>>> + 'a;

    fn select_channel<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), StaAttemptStepError<Self::Error>>> + 'a;

    fn authenticate<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), StaAttemptStepError<Self::Error>>> + 'a;

    fn associate<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), StaAttemptStepError<Self::Error>>> + 'a;

    fn program_peer<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), StaAttemptStepError<Self::Error>>> + 'a;

    fn run_wpa2_handshake<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), StaAttemptStepError<Self::Error>>> + 'a;

    fn install_wpa2_keys<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), StaAttemptStepError<Self::Error>>> + 'a;

    fn enter_connected(
        &mut self,
        owner: Self::Owner,
    ) -> impl Future<
        Output = Result<Self::Connected, StaConnectedEntryFailure<Self::Owner, Self::Error>>,
    > + '_;
}

/// Shared production transaction for initial join and reconnect.
pub struct StaAttempt<P, O = ()> {
    port: P,
    observer: O,
}

impl<P> StaAttempt<P, ()> {
    pub const fn new(port: P) -> Self {
        Self { port, observer: () }
    }
}

impl<P, O> StaAttempt<P, O> {
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

impl<P, O> StaAttempt<P, O>
where
    P: StaAttemptPort,
    O: StaAttemptObserver,
{
    /// Execute every finite pre-connected phase exactly once and in order.
    pub async fn run(
        &mut self,
        mut owner: P::Owner,
    ) -> AssociationAttemptOutcome<P::Owner, P::Connected, P::Error> {
        let mut progress = StaAttemptProgress::default();

        self.observer.stage_started(StaAttemptStage::Candidate);
        if let Err(failure) = self.port.prepare_candidate(&mut owner).await {
            return self.failed(owner, StaAttemptStage::Candidate, failure, progress);
        }
        self.completed(StaAttemptStage::Candidate, &mut progress);

        self.observer.stage_started(StaAttemptStage::Channel);
        if let Err(failure) = self.port.select_channel(&mut owner).await {
            return self.failed(owner, StaAttemptStage::Channel, failure, progress);
        }
        self.completed(StaAttemptStage::Channel, &mut progress);

        self.observer.stage_started(StaAttemptStage::Authentication);
        if let Err(failure) = self.port.authenticate(&mut owner).await {
            return self.failed(owner, StaAttemptStage::Authentication, failure, progress);
        }
        self.completed(StaAttemptStage::Authentication, &mut progress);

        self.observer.stage_started(StaAttemptStage::Association);
        if let Err(failure) = self.port.associate(&mut owner).await {
            return self.failed(owner, StaAttemptStage::Association, failure, progress);
        }
        self.completed(StaAttemptStage::Association, &mut progress);

        self.observer
            .stage_started(StaAttemptStage::PeerProgramming);
        if let Err(failure) = self.port.program_peer(&mut owner).await {
            return self.failed(owner, StaAttemptStage::PeerProgramming, failure, progress);
        }
        self.completed(StaAttemptStage::PeerProgramming, &mut progress);

        self.observer.stage_started(StaAttemptStage::Wpa2Handshake);
        if let Err(failure) = self.port.run_wpa2_handshake(&mut owner).await {
            return self.failed(owner, StaAttemptStage::Wpa2Handshake, failure, progress);
        }
        self.completed(StaAttemptStage::Wpa2Handshake, &mut progress);

        self.observer.stage_started(StaAttemptStage::Wpa2KeyInstall);
        if let Err(failure) = self.port.install_wpa2_keys(&mut owner).await {
            return self.failed(owner, StaAttemptStage::Wpa2KeyInstall, failure, progress);
        }
        self.completed(StaAttemptStage::Wpa2KeyInstall, &mut progress);

        self.observer.stage_started(StaAttemptStage::ConnectedEntry);
        match self.port.enter_connected(owner).await {
            Ok(connected) => {
                self.completed(StaAttemptStage::ConnectedEntry, &mut progress);
                AssociationAttemptOutcome::Connected {
                    connected,
                    progress,
                }
            }
            Err(failure) => {
                self.observer
                    .stage_failed(StaAttemptStage::ConnectedEntry, failure.disposition);
                AssociationAttemptOutcome::Failed(AssociationAttemptFailure {
                    owner: failure.owner,
                    stage: StaAttemptStage::ConnectedEntry,
                    disposition: failure.disposition,
                    error: failure.error,
                    progress,
                })
            }
        }
    }

    fn completed(&mut self, stage: StaAttemptStage, progress: &mut StaAttemptProgress) {
        progress.mark_completed(stage);
        self.observer.stage_completed(stage);
    }

    fn failed(
        &mut self,
        owner: P::Owner,
        stage: StaAttemptStage,
        failure: StaAttemptStepError<P::Error>,
        progress: StaAttemptProgress,
    ) -> AssociationAttemptOutcome<P::Owner, P::Connected, P::Error> {
        self.observer.stage_failed(stage, failure.disposition);
        AssociationAttemptOutcome::Failed(AssociationAttemptFailure {
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
