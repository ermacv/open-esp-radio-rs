//! One station over the lower-MAC port: its attempt phases, its connection
//! and its lifecycle backend.

use core::{convert::Infallible, marker::PhantomData};

use oer_ieee80211_lower_mac::{Channel, ChannelWidth, ReceiveFilter};
use oer_ieee80211_mac::{
    ccmp::{CcmpPacketNumberStep, CcmpTxPacketNumber},
    scan::{ScanRecord, ScanTable},
    security::{AssociationAkm, AssociationSecurity, Pmkid, RsnAssociation},
    station::{
        AssociationCapabilities, AssociationResponse, SelectedRsn, StaSecurityError,
        association::PhyMode, select_association_rsn,
    },
    station_power_save::StaAssociationId,
};
use oer_ieee80211_rsn::{
    Pmk,
    aes::AsyncRsnKeyUnwrap,
    runner::{RsnHandshakeConfig, RsnHandshakeError, RsnKeyInstallError, RsnKeyInstallMetadata},
    sae::SaeError,
};
use oer_ieee80211_rsn_service::runner::{
    RsnHandshakeRunner, RsnKeyInstallRunner, RsnPendingKeyInstall,
};
use oer_ieee80211_sta::{
    attempt::{
        AssociationAttemptOutcome, StaAttemptPort, StaAttemptSecurity, StaAttemptStateError,
        StaAttemptStepError, StaConnectedEntryFailure,
    },
    join::{
        StaAssociationSuccess, StaAuthenticationSuccess, StaJoinError, sae::StaSaeAuthentication,
    },
    modem_sleep::SleepType,
    scan::{StaCandidateScanExit, StaScanConfig, StaScanError, StaScanPlanError},
    station::{
        StaAttemptContext, StaAttemptFailure, StaAttemptOutcome, StaBackoffOutcome,
        StaBackoffReason, StaFailureDisposition, StaLifecycleBackend, StaLifecycleStage,
        StaNextCandidate,
    },
};
use oer_time::{Clock, Duration, Timer};

use crate::{
    attempt::StaAttempt,
    join::StaJoinRunner,
    scan::{StaCandidateScanService, StaScanBackend},
};

use super::{
    connected::{
        ConnectionContext, PortConnection, PortConnectionConfig, PortDisconnect, PortSend,
    },
    join::{PortAssociation, PortJoin},
    link::{PORT_FRAME_CAPACITY, PortError, PortLink, PortLinkError, PortStationEnv},
    rsn::{BorrowedUnwrap, PortHandshake, PortKeyInstall, PortKeys},
    scan::{PortProbe, PortScan, PortScanTarget},
};

/// The key-data unwrap error type of an environment.
pub type PortUnwrapError<X> = <<X as PortStationEnv>::KeyUnwrap as AsyncRsnKeyUnwrap>::Error;

/// What the station is and looks for, fixed across its attempts.
#[derive(Clone, Copy, Debug)]
pub struct PortStationProfile<'a> {
    /// The SSID the station joins.
    pub ssid: &'a [u8],
    /// The channels a scan visits, in order.
    pub channels: &'a [Channel],
    /// Dwell ticks per channel.
    pub scan: StaScanConfig,
    /// The length of one dwell tick.
    pub dwell_tick: Duration,
    /// The Probe Request of an active scan; `None` scans passively.
    pub probe: Option<PortProbe<'a>>,
    /// The station's HT, HE and WMM elements.
    pub capabilities: &'a AssociationCapabilities,
    pub phy: PhyMode,
    pub listen_interval: u16,
    /// The step between the station's CCMP packet numbers: one in the
    /// standard, the integrator's choice otherwise (the Espressif stack's
    /// is `oer-espressif-ieee80211-policy`'s).
    pub ccmp_step: CcmpPacketNumberStep,
    /// The random source of the first SA Query transaction identifier.
    pub sa_query_random: fn() -> u32,
}

/// Why a phase of the station failed.
#[derive(Debug, Eq, PartialEq)]
pub enum PortStationError<E, U> {
    Link(PortLinkError<E>),
    Scan(StaScanError<PortLinkError<E>>),
    ScanPlan(StaScanPlanError),
    /// No access point of the SSID admits the security policy.
    NoCandidate,
    Security(StaSecurityError),
    SaeCommit(SaeError),
    Join(StaJoinError<PortLinkError<E>>),
    Handshake(RsnHandshakeError<PortLinkError<E>, U>),
    KeyInstall(RsnKeyInstallError<PortLinkError<E>>),
    State(StaAttemptStateError),
}

/// The error type of an environment's station.
pub type PortAttemptError<X> = PortStationError<PortError<X>, PortUnwrapError<X>>;

/// What the phases of the last attempt reported.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PortAttemptReport {
    pub authentication: Option<StaAuthenticationSuccess>,
    /// The attempt authenticated with SAE.
    pub sae: bool,
    /// The attempt resumed a cached PMKSA instead of running SAE.
    pub pmksa_resumed: bool,
    pub association: Option<StaAssociationSuccess>,
    pub keys: Option<RsnKeyInstallMetadata>,
}

type StepResult<X> = Result<(), StaAttemptStepError<PortAttemptError<X>>>;

/// One station over the lower-MAC port.
///
/// It owns the port's single consumer ([`PortLink`]), its timer, its
/// security material and the state each phase hands the next. Its attempt
/// phases are the [`StaAttemptPort`] of [`PortAttemptPort`], which the
/// `StaAttempt` transaction runs in order: scan when the candidate must be
/// refreshed, tune to its channel, authenticate (Open System, SAE, or Open
/// System resuming a cached SAE PMKSA), associate, program the BSS filter,
/// run the WPA2 four-way handshake and install its keys. A connected
/// station sends and receives Ethernet frames, keeps its Block Ack
/// agreements and its SA Query, dozes under power save and reports how its
/// association ended.
pub struct PortStation<'p, X: PortStationEnv> {
    link: PortLink<'p, X>,
    timer: X::Timer,
    key_unwrap: X::KeyUnwrap,
    profile: PortStationProfile<'p>,
    security: StaAttemptSecurity<'p>,
    table: ScanTable,
    refresh: bool,
    candidate: Option<ScanRecord>,
    selected: Option<SelectedRsn>,
    association: Option<AssociationResponse>,
    pending: Option<RsnPendingKeyInstall>,
    keys: Option<PortKeys>,
    packet_number: Option<CcmpTxPacketNumber>,
    connection: Option<PortConnection<X::Port>>,
    report: PortAttemptReport,
}

/// A connected station's connection and the context its phases run in.
type ConnectionParts<'a, 'p, X> = (
    &'a mut PortConnection<<X as PortStationEnv>::Port>,
    ConnectionContext<'a, 'p, X>,
);

impl<'p, X: PortStationEnv> PortStation<'p, X> {
    pub fn new(
        link: PortLink<'p, X>,
        timer: X::Timer,
        key_unwrap: X::KeyUnwrap,
        profile: PortStationProfile<'p>,
        security: StaAttemptSecurity<'p>,
    ) -> Self {
        Self {
            link,
            timer,
            key_unwrap,
            profile,
            security,
            table: ScanTable::new(),
            refresh: true,
            candidate: None,
            selected: None,
            association: None,
            pending: None,
            keys: None,
            packet_number: None,
            connection: None,
            report: PortAttemptReport::default(),
        }
    }

    pub const fn link(&self) -> &PortLink<'p, X> {
        &self.link
    }

    pub fn link_mut(&mut self) -> &mut PortLink<'p, X> {
        &mut self.link
    }

    pub const fn timer(&self) -> &X::Timer {
        &self.timer
    }

    pub const fn security(&self) -> &StaAttemptSecurity<'p> {
        &self.security
    }

    /// The access point the next attempt joins.
    pub const fn candidate(&self) -> Option<&ScanRecord> {
        self.candidate.as_ref()
    }

    /// The scan table of the last scan.
    pub const fn scan_table(&self) -> &ScanTable {
        &self.table
    }

    /// Scan again before the next attempt, or join the current candidate.
    pub fn set_refresh(&mut self, refresh: bool) {
        self.refresh = refresh;
    }

    pub const fn report(&self) -> &PortAttemptReport {
        &self.report
    }

    /// The connection of a connected station.
    pub const fn connection(&self) -> Option<&PortConnection<X::Port>> {
        self.connection.as_ref()
    }

    /// Run one attempt: every phase up to the connected frontier, in order.
    pub async fn connect(self) -> AssociationAttemptOutcome<Self, Self, PortAttemptError<X>> {
        StaAttempt::new(PortAttemptPort::new()).run(self).await
    }

    fn context(&mut self) -> Result<ConnectionParts<'_, 'p, X>, PortLinkError<PortError<X>>> {
        let Self {
            link,
            timer,
            key_unwrap,
            security,
            connection,
            ..
        } = self;
        let connection = connection.as_mut().ok_or(PortLinkError::MissingState)?;
        let (sequences, supplicant) = security.connected_parts();
        Ok((
            connection,
            ConnectionContext {
                link,
                timer,
                sequences,
                supplicant,
                key_unwrap,
            },
        ))
    }

    /// Send one Ethernet-II frame with `user_priority`.
    pub async fn send(
        &mut self,
        ethernet: &[u8],
        user_priority: u8,
    ) -> Result<PortSend, PortLinkError<PortError<X>>> {
        let (connection, mut context) = self.context()?;
        connection.send(&mut context, ethernet, user_priority).await
    }

    /// Receive until `deadline`, handing every Ethernet frame to `deliver`.
    /// `Some` when the association ended; the station has then left it.
    pub async fn run_until(
        &mut self,
        deadline: oer_time::Instant,
        deliver: &mut impl FnMut(&[u8]),
    ) -> Result<Option<PortDisconnect>, PortLinkError<PortError<X>>> {
        let (connection, mut context) = self.context()?;
        let ended = connection
            .run_until(&mut context, deadline, deliver)
            .await?;
        if ended.is_some() {
            self.end_connection(false).await?;
        }
        Ok(ended)
    }

    /// Start modem sleep; the port must report TBTTs
    /// ([`PortLink::with_beacon_timing`]).
    pub async fn enable_power_save(
        &mut self,
        sleep_type: SleepType,
    ) -> Result<(), PortLinkError<PortError<X>>> {
        let (connection, mut context) = self.context()?;
        connection.enable_power_save(&mut context, sleep_type).await
    }

    /// Leave the association with a Deauthentication.
    pub async fn disconnect(&mut self) -> Result<(), PortLinkError<PortError<X>>> {
        self.end_connection(true).await
    }

    async fn end_connection(
        &mut self,
        send_deauthentication: bool,
    ) -> Result<(), PortLinkError<PortError<X>>> {
        let result = match self.context() {
            Ok((connection, mut context)) => {
                connection.leave(&mut context, send_deauthentication).await
            }
            Err(_) => Ok(()),
        };
        self.connection = None;
        self.keys = None;
        self.association = None;
        self.selected = None;
        self.pending = None;
        self.packet_number = None;
        result
    }

    fn bssid(&self) -> Result<[u8; 6], StaAttemptStepError<PortAttemptError<X>>> {
        self.candidate
            .map(|candidate| candidate.bssid)
            .ok_or(StaAttemptStepError::terminal(PortStationError::State(
                StaAttemptStateError::MissingPreparedPeer,
            )))
    }

    async fn prepare_candidate(&mut self) -> StepResult<X> {
        self.report = PortAttemptReport::default();
        if !self.refresh && self.candidate.is_some() {
            return Ok(());
        }
        self.candidate = None;
        let Self {
            link,
            timer,
            profile,
            security,
            table,
            ..
        } = self;
        let target = PortScanTarget {
            ssid: profile.ssid,
            policy: security.policy(),
            probe: profile.probe,
        };
        let scan = PortScan::new(
            link,
            timer,
            security.sequences.non_qos_mut(),
            table,
            target,
            profile.dwell_tick,
        );
        let mut service = StaCandidateScanService::new(StaScanBackend::new(profile.scan));
        match service.run(scan, profile.channels).await {
            StaCandidateScanExit::Selected { candidate, .. } => {
                self.candidate = Some(candidate);
                self.refresh = false;
                Ok(())
            }
            StaCandidateScanExit::NoCandidate { .. } | StaCandidateScanExit::Stopped { .. } => Err(
                StaAttemptStepError::refresh_candidate(PortStationError::NoCandidate),
            ),
            StaCandidateScanExit::Failed { error, .. } => Err(
                StaAttemptStepError::refresh_candidate(PortStationError::Scan(error)),
            ),
            StaCandidateScanExit::InvalidPlan { error, .. } => Err(StaAttemptStepError::terminal(
                PortStationError::ScanPlan(error),
            )),
        }
    }

    async fn select_channel(&mut self) -> StepResult<X> {
        let candidate =
            self.candidate
                .ok_or(StaAttemptStepError::terminal(PortStationError::State(
                    StaAttemptStateError::MissingPreparedPeer,
                )))?;
        let channel = if candidate.channel <= 14 {
            Channel::ghz2_4(candidate.channel, ChannelWidth::Mhz20)
        } else {
            Channel::ghz5(candidate.channel, ChannelWidth::Mhz20)
        }
        .map_err(|_| StaAttemptStepError::refresh_candidate(PortStationError::NoCandidate))?;
        self.link
            .retune(channel)
            .await
            .map_err(|error| StaAttemptStepError::retry_current(PortStationError::Link(error)))
    }

    async fn authenticate(&mut self) -> StepResult<X> {
        let bssid = self.bssid()?;
        let candidate = self.candidate.expect("a prepared candidate");
        let address = self.link.config().address;
        let mut selected = select_association_rsn(&candidate, self.security.policy())
            .map_err(|error| StaAttemptStepError::terminal(PortStationError::Security(error)))?;
        let Self {
            link,
            timer,
            security,
            report,
            ..
        } = self;
        let sae = match selected.security() {
            AssociationSecurity::Rsn(RsnAssociation {
                akm: AssociationAkm::Sae(pwe),
                ..
            }) => {
                let credentials = security.credentials().ok_or(StaAttemptStepError::terminal(
                    PortStationError::State(StaAttemptStateError::MissingConnectedSecurity),
                ))?;
                match credentials.pmksa().resume(&candidate) {
                    Some((pmk, pmkid)) => {
                        selected = selected.with_pmkid(Pmkid(pmkid));
                        security.set_sae_pmk(Some(pmk));
                        report.pmksa_resumed = true;
                        None
                    }
                    None => {
                        let commit =
                            credentials
                                .sae_commit(address, &candidate, pwe)
                                .map_err(|error| {
                                    StaAttemptStepError::retry_current(PortStationError::SaeCommit(
                                        error,
                                    ))
                                })?;
                        Some(StaSaeAuthentication::new(address, bssid, commit, pwe))
                    }
                }
            }
            AssociationSecurity::Open | AssociationSecurity::Rsn(_) => {
                security.set_sae_pmk(None);
                None
            }
        };
        let mut runner = StaJoinRunner::new(PortJoin::new(link, bssid), &*timer);
        match sae {
            Some(exchange) => {
                let pmk = runner
                    .authenticate_sae(exchange, security.sequences.non_qos_mut())
                    .await
                    .map_err(|error| {
                        StaAttemptStepError::retry_current(PortStationError::Join(error))
                    })?;
                if let Some(credentials) = security.credentials() {
                    credentials
                        .pmksa()
                        .insert(&candidate, &Pmk::from_bytes(pmk.pmk), pmk.pmkid);
                }
                security.set_sae_pmk(Some(Pmk::from_bytes(pmk.pmk)));
                report.sae = true;
            }
            None => {
                report.authentication = Some(
                    runner
                        .authenticate(address, bssid, security.sequences.non_qos_mut())
                        .await
                        .map_err(|error| {
                            StaAttemptStepError::retry_current(PortStationError::Join(error))
                        })?,
                );
            }
        }
        self.selected = Some(selected);
        Ok(())
    }

    async fn associate(&mut self) -> StepResult<X> {
        let bssid = self.bssid()?;
        let candidate = self.candidate.expect("a prepared candidate");
        let selected =
            self.selected
                .ok_or(StaAttemptStepError::terminal(PortStationError::State(
                    StaAttemptStateError::MissingSelectedRsn,
                )))?;
        let address = self.link.config().address;
        let Self {
            link,
            timer,
            profile,
            security,
            ..
        } = self;
        let join = PortJoin::new(link, bssid).with_association(PortAssociation {
            access_point: &candidate,
            security: &selected,
            phy: profile.phy,
            capabilities: profile.capabilities,
            listen_interval: profile.listen_interval,
        });
        let success = StaJoinRunner::new(join, &*timer)
            .associate(
                address,
                bssid,
                selected.security().link_protection(),
                security.sequences.non_qos_mut(),
            )
            .await
            .map_err(|error| StaAttemptStepError::retry_current(PortStationError::Join(error)))?;
        self.association = Some(success.response);
        self.report.association = Some(success);
        Ok(())
    }

    async fn program_peer(&mut self) -> StepResult<X> {
        let bssid = self.bssid()?;
        self.link
            .configure(Some(bssid), ReceiveFilter::BSS_MEMBER)
            .map_err(|error| StaAttemptStepError::retry_current(PortStationError::Link(error)))
    }

    fn protected(&self) -> bool {
        self.selected
            .is_some_and(|selected| !matches!(selected.security(), AssociationSecurity::Open))
    }

    async fn run_wpa2_handshake(&mut self) -> StepResult<X> {
        if !self.protected() {
            return Ok(());
        }
        let bssid = self.bssid()?;
        let candidate = self.candidate.expect("a prepared candidate");
        let selected = self.selected.expect("a selected security");
        let address = self.link.config().address;
        let Self {
            link,
            timer,
            key_unwrap,
            security,
            ..
        } = self;
        let (pmk, supplicant_nonce, sequences) =
            security
                .wpa2_handshake_parts()
                .ok_or(StaAttemptStepError::terminal(PortStationError::State(
                    StaAttemptStateError::MissingHandshake,
                )))?;
        let config = RsnHandshakeConfig {
            local: address,
            authenticator: bssid,
            supplicant_nonce,
            association_security_ies: selected.as_bytes(),
            authenticator_rsn_ie: candidate.rsn_ie_bytes(),
            authenticator_rsnxe: candidate.rsnxe_bytes(),
            pmk,
        };
        let mut runner = RsnHandshakeRunner::new(
            PortHandshake::new(link, bssid),
            &*timer,
            BorrowedUnwrap(key_unwrap),
        );
        let pending = runner
            .run(config, &mut || sequences.take_non_qos())
            .await
            .map_err(|error| {
                StaAttemptStepError::retry_current(PortStationError::Handshake(error))
            })?;
        self.pending = Some(pending);
        Ok(())
    }

    async fn install_wpa2_keys(&mut self) -> StepResult<X> {
        if !self.protected() {
            return Ok(());
        }
        let bssid = self.bssid()?;
        let pending =
            self.pending
                .take()
                .ok_or(StaAttemptStepError::terminal(PortStationError::State(
                    StaAttemptStateError::MissingKeys,
                )))?;
        let protection = self
            .security
            .wpa2_material()
            .map(|(_, _, protection)| protection)
            .ok_or(StaAttemptStepError::terminal(PortStationError::State(
                StaAttemptStateError::MissingKeys,
            )))?;
        let mut packet_number = CcmpTxPacketNumber::new(self.profile.ccmp_step);
        let established = RsnKeyInstallRunner::new(PortKeyInstall::new(
            &mut self.link,
            bssid,
            &mut self.security.sequences,
            &mut packet_number,
            protection,
        ))
        .run(pending)
        .await
        .map_err(|error| StaAttemptStepError::retry_current(PortStationError::KeyInstall(error)))?;
        self.report.keys = Some(established.metadata());
        let (keys, connected) = established.into_parts();
        self.security.set_connected(connected);
        self.keys = Some(keys);
        self.packet_number = Some(packet_number);
        Ok(())
    }

    fn connection_config(&self) -> Option<PortConnectionConfig> {
        let candidate = self.candidate?;
        let association = self.association?;
        let selected = self.selected?;
        Some(PortConnectionConfig {
            bssid: candidate.bssid,
            association_id: StaAssociationId::new(association.association_id & 0x3fff)?,
            peer_qos: association.wmm,
            management_protection: selected.security().protects_management(),
            sa_query_random: self.profile.sa_query_random,
            beacon_interval_tu: candidate.beacon_interval_tu,
            join_timestamp_tsf: candidate.timestamp,
        })
    }
}

/// The [`StaAttemptPort`] of a [`PortStation`]: every phase runs on the
/// station it is handed.
pub struct PortAttemptPort<'p, X>(PhantomData<fn() -> PortStation<'p, X>>)
where
    X: PortStationEnv;

impl<X: PortStationEnv> PortAttemptPort<'_, X> {
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<X: PortStationEnv> Default for PortAttemptPort<'_, X> {
    fn default() -> Self {
        Self::new()
    }
}

impl<X: PortStationEnv> Clone for PortAttemptPort<'_, X> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<X: PortStationEnv> Copy for PortAttemptPort<'_, X> {}

impl<'p, X: PortStationEnv> StaAttemptPort for PortAttemptPort<'p, X> {
    type Owner = PortStation<'p, X>;
    type Connected = PortStation<'p, X>;
    type Error = PortAttemptError<X>;

    async fn prepare_candidate<'a>(&'a mut self, owner: &'a mut Self::Owner) -> StepResult<X> {
        owner.prepare_candidate().await
    }

    async fn select_channel<'a>(&'a mut self, owner: &'a mut Self::Owner) -> StepResult<X> {
        owner.select_channel().await
    }

    async fn authenticate<'a>(&'a mut self, owner: &'a mut Self::Owner) -> StepResult<X> {
        owner.authenticate().await
    }

    async fn associate<'a>(&'a mut self, owner: &'a mut Self::Owner) -> StepResult<X> {
        owner.associate().await
    }

    async fn program_peer<'a>(&'a mut self, owner: &'a mut Self::Owner) -> StepResult<X> {
        owner.program_peer().await
    }

    async fn run_wpa2_handshake<'a>(&'a mut self, owner: &'a mut Self::Owner) -> StepResult<X> {
        owner.run_wpa2_handshake().await
    }

    async fn install_wpa2_keys<'a>(&'a mut self, owner: &'a mut Self::Owner) -> StepResult<X> {
        owner.install_wpa2_keys().await
    }

    async fn enter_connected(
        &mut self,
        mut owner: Self::Owner,
    ) -> Result<Self::Connected, StaConnectedEntryFailure<Self::Owner, Self::Error>> {
        let Some(config) = owner.connection_config() else {
            return Err(StaConnectedEntryFailure::new(
                owner,
                StaFailureDisposition::RetryCurrentCandidate,
                PortStationError::State(StaAttemptStateError::MissingAssociation),
            ));
        };
        let packet_number = owner
            .packet_number
            .take()
            .unwrap_or(CcmpTxPacketNumber::new(owner.profile.ccmp_step));
        owner.connection = Some(PortConnection::new(config, owner.keys, packet_number));
        Ok(owner)
    }
}

/// The application a lifecycle-driven station serves.
pub trait PortStationApplication {
    /// Whether the station should leave and stop.
    fn stop_requested(&mut self) -> bool;

    /// The next Ethernet-II frame to send, written into `ethernet`: its
    /// length and user priority.
    fn next_transmit(&mut self, _ethernet: &mut [u8]) -> Option<(usize, u8)> {
        None
    }

    /// One received Ethernet-II frame.
    fn deliver(&mut self, ethernet: &[u8]);

    /// The station connected.
    fn connected(&mut self, _config: &PortConnectionConfig) {}

    /// The association ended.
    fn disconnected(&mut self, _reason: PortDisconnect) {}
}

/// The [`StaLifecycleBackend`] of a [`PortStation`].
///
/// Each attempt runs the `StaAttempt` transaction over [`PortAttemptPort`]
/// and then serves the connection until the association ends or the
/// application asks the station to stop: it sends the application's
/// frames, receives for one `poll` interval at a time and hands every frame
/// to the application. A connection the access point ended returns the
/// station for the next attempt with the same candidate.
pub struct PortStationLifecycle<'p, X: PortStationEnv, A> {
    application: A,
    poll: Duration,
    _station: PhantomData<fn() -> PortStation<'p, X>>,
}

impl<X: PortStationEnv, A: PortStationApplication> PortStationLifecycle<'_, X, A> {
    pub const fn new(application: A, poll: Duration) -> Self {
        Self {
            application,
            poll,
            _station: PhantomData,
        }
    }

    pub const fn application(&self) -> &A {
        &self.application
    }

    pub fn application_mut(&mut self) -> &mut A {
        &mut self.application
    }
}

impl<'p, X: PortStationEnv, A: PortStationApplication> StaLifecycleBackend
    for PortStationLifecycle<'p, X, A>
{
    type Owner = PortStation<'p, X>;
    type Error = PortAttemptError<X>;
    type Fault = Infallible;

    async fn run_attempt(
        &mut self,
        mut owner: Self::Owner,
        context: StaAttemptContext,
    ) -> StaAttemptOutcome<Self::Owner, Self::Error, Self::Fault> {
        if self.application.stop_requested() {
            return StaAttemptOutcome::Stopped { owner };
        }
        if context.refresh_candidate {
            owner.set_refresh(true);
        }
        let mut station = match owner.connect().await {
            AssociationAttemptOutcome::Connected { connected, .. } => connected,
            AssociationAttemptOutcome::Failed(failure) => {
                let lifecycle_stage = failure.lifecycle_stage();
                let (owner, _, disposition, error, _) = failure.into_parts();
                return StaAttemptOutcome::Failed {
                    owner,
                    failure: StaAttemptFailure::new(lifecycle_stage, disposition, error),
                };
            }
        };
        if let Some(connection) = station.connection() {
            self.application.connected(connection.config());
        }
        let mut frame = [0_u8; PORT_FRAME_CAPACITY];
        loop {
            if self.application.stop_requested() {
                let _ = station.disconnect().await;
                return StaAttemptOutcome::Stopped { owner: station };
            }
            while let Some((length, priority)) = self.application.next_transmit(&mut frame) {
                if let Err(error) = station.send(&frame[..length], priority).await {
                    let _ = station.end_connection(false).await;
                    return connected_failure(station, error);
                }
            }
            let deadline = station.timer.now().checked_add(self.poll);
            let Some(deadline) = deadline else {
                return connected_failure(station, PortLinkError::MissingState);
            };
            let application = &mut self.application;
            match station
                .run_until(deadline, &mut |ethernet| application.deliver(ethernet))
                .await
            {
                Ok(None) => {}
                Ok(Some(reason)) => {
                    self.application.disconnected(reason);
                    return StaAttemptOutcome::Disconnected {
                        owner: station,
                        next_candidate: StaNextCandidate::Reuse,
                    };
                }
                Err(error) => {
                    let _ = station.end_connection(false).await;
                    return connected_failure(station, error);
                }
            }
        }
    }

    async fn wait_backoff(
        &mut self,
        owner: Self::Owner,
        delay_millis: u32,
        _reason: StaBackoffReason,
    ) -> StaBackoffOutcome<Self::Owner> {
        let Some(deadline) = owner
            .timer
            .now()
            .checked_add(Duration::from_millis(delay_millis))
        else {
            return StaBackoffOutcome::Stopped { owner };
        };
        loop {
            if self.application.stop_requested() {
                return StaBackoffOutcome::Stopped { owner };
            }
            let now = owner.timer.now();
            if now >= deadline {
                return StaBackoffOutcome::Elapsed { owner };
            }
            let next = now
                .checked_add(self.poll)
                .map_or(deadline, |next| next.min(deadline));
            owner.timer.wait_until(next).await;
        }
    }
}

fn connected_failure<'p, X: PortStationEnv>(
    owner: PortStation<'p, X>,
    error: PortLinkError<PortError<X>>,
) -> StaAttemptOutcome<PortStation<'p, X>, PortAttemptError<X>, Infallible> {
    StaAttemptOutcome::Failed {
        owner,
        failure: StaAttemptFailure::new(
            StaLifecycleStage::Connected,
            StaFailureDisposition::RefreshCandidate,
            PortStationError::Link(error),
        ),
    }
}
