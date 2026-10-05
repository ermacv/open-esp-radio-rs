//! One station over the lower-MAC port: its attempt phases, its connection
//! and its lifecycle backend.

use core::{convert::Infallible, marker::PhantomData};
use oer_ieee80211_upper_mac_service::client::{PortClientEnv, PortError, PortMsdu, PortRxBuffer};
use oer_ieee80211_upper_mac_service::queue::PORT_FRAME_CAPACITY;

use oer_ieee80211_lower_mac::{
    Channel, ChannelWidth, Ieee80211LowerMacPort, ReceiveFilter, RxEvidence, RxMeta,
};
use oer_ieee80211_mac::{
    ccmp::{CcmpPacketNumberStep, CcmpTxPacketNumber},
    scan::{HtSecondaryChannel, ScanRecord, ScanTable},
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
    association::{Preference, StaAssociatedPeer, StaAssociatedPeerError, select_association_phy},
    attempt::{
        AssociationAttemptOutcome, StaAttemptPort, StaAttemptSecurity, StaAttemptStateError,
        StaAttemptStepError, StaConnectedEntryFailure,
    },
    join::{
        StaAssociationSuccess, StaAuthenticationSuccess, StaJoinError, sae::StaSaeAuthentication,
    },
    modem_sleep::SleepType,
    rate_control::StaRateControl,
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
        ConnectionContext, PortConnection, PortConnectionBuffers, PortConnectionConfig,
        PortConnectionSecurity, PortDisconnect, PortLinkProbe, PortLinkSupervisor, PortSend,
    },
    join::{PortAssociation, PortHePower, PortJoin},
    link::{PortLink, PortLinkError, PortStationEnv},
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
    /// The station's HE power elements; an HE association needs them.
    pub he_power: Option<PortHePower>,
    /// The nominal packet padding of an access point's HE Capabilities
    /// element, an integrator's policy (the Espressif stack's is
    /// `oer-espressif-ieee80211-policy::he_txop::packet_padding`).
    pub he_packet_padding: fn(&[u8]) -> oer_ieee80211_upper_mac::HePacketPadding,
    /// The TX Block Ack agreements the station originates; `None`
    /// originates none.
    pub tx_block_ack: Option<PortTxBlockAck>,
    /// How the station supervises its link once connected.
    pub link: PortLinkSupervision<'a>,
    /// The power manager's sleep type for each association: the Espressif
    /// station's default is `SleepType::None`, which sleeps only for
    /// coexistence.
    pub sleep_type: SleepType,
    /// How long a receive reorder window holds a buffered run behind a
    /// missing MPDU, from the first MPDU it retains: an integrator's policy
    /// (the Espressif stack's is
    /// `oer-espressif-ieee80211-policy::block_ack::RX_REORDER_GAP_TIMEOUT_MICROS`).
    pub rx_reorder_gap: Duration,
    /// Which mode the station prefers among those it and each access point
    /// admit (`oer_ieee80211_sta::association::select_association_phy`):
    /// HT40 needs a port that tunes 40 MHz.
    pub preference: Preference,
    pub listen_interval: u16,
    /// The step between the station's CCMP packet numbers: one in the
    /// standard, the integrator's choice otherwise (the Espressif stack's
    /// is `oer-espressif-ieee80211-policy`'s).
    pub ccmp_step: CcmpPacketNumberStep,
    /// The random source of the first SA Query transaction identifier.
    pub sa_query_random: fn() -> u32,
}

/// How a connected station supervises its link: beacons keep it; after
/// `timeout` without one it probes the access point under `probe`
/// (`oer_ieee80211_sta::link_monitor`), and it leaves after the last
/// probe goes unanswered. The Espressif station's values are
/// `oer-espressif-ieee80211-policy::station_link`'s `STATION_INACTIVE_TIME`
/// and `STATION_LINK_PROBE`, with a miss limit of 10.
#[derive(Clone, Copy, Debug)]
pub struct PortLinkSupervision<'a> {
    pub timeout: Duration,
    /// The consecutive misses a hardware beacon monitor would allow.
    pub miss_limit: u8,
    pub probe: oer_ieee80211_sta::link_monitor::StaLinkProbePolicy,
    /// The supported rates of the station's Probe Requests.
    pub supported_rates: &'a [u8],
}

/// Why a station could not supervise its link.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkSupervisionError {
    /// The beacon interval, miss limit or timeout is not one a monitor
    /// keeps.
    BeaconLoss(oer_ieee80211_sta::link_monitor::StaBeaconLossConfigError),
    /// The SSID or rate set is longer than a Probe Request carries.
    Probe,
}

/// The TX Block Ack agreements a station originates once connected.
#[derive(Clone, Copy, Debug)]
pub struct PortTxBlockAck {
    /// Which TIDs and Dialog Tokens: an integrator's policy (the Espressif
    /// station's is `oer-espressif-ieee80211-policy::block_ack`).
    pub policy: oer_ieee80211_mac::block_ack::TxBlockAckOriginatorPolicy,
    pub config: oer_ieee80211_mac::block_ack::TxBlockAckOriginatorConfig,
    /// Negotiations of each TID before the station gives it up, and how
    /// long a failed one waits before the next.
    pub retry: oer_ieee80211_mac::block_ack::TxBlockAckRetry,
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
    /// The access point's elements give no associated peer.
    Peer(StaAssociatedPeerError),
    /// The profile's link supervision does not fit the association.
    LinkSupervision(LinkSupervisionError),
    /// The profile's TX Block Ack policy is not one an originator takes.
    TxBlockAck(oer_ieee80211_mac::block_ack::TxBlockAckOriginatorError),
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
/// The memory of one port station that the composition places: the scan
/// table and the frame buffers its connections borrow. Most of a
/// station's size is here, so a `static` (or a placement the target
/// chooses, such as external RAM) keeps it off the stack; the station
/// itself holds only protocol state.
pub struct PortStationStorage {
    table: ScanTable,
    connection: PortConnectionBuffers,
}

impl PortStationStorage {
    pub const fn new() -> Self {
        Self {
            table: ScanTable::new(),
            connection: PortConnectionBuffers::new(),
        }
    }
}

impl Default for PortStationStorage {
    fn default() -> Self {
        Self::new()
    }
}

pub struct PortStation<'p, X: PortStationEnv> {
    link: PortLink<'p, X>,
    timer: X::Timer,
    key_unwrap: X::KeyUnwrap,
    profile: PortStationProfile<'p>,
    security: StaAttemptSecurity<'p>,
    table: &'p mut ScanTable,
    refresh: bool,
    candidate: Option<ScanRecord>,
    selected: Option<SelectedRsn>,
    association: Option<AssociationResponse>,
    /// The receive metadata of the access point's Association Response.
    response_meta: Option<RxMeta>,
    /// The access point as the association left it.
    peer: Option<StaAssociatedPeer>,
    pending: Option<RsnPendingKeyInstall>,
    keys: Option<PortKeys>,
    /// The BIP receive state of an association that protects its
    /// management frames.
    bip: Option<oer_ieee80211_rsn::bip::BipReceiver>,
    packet_number: Option<CcmpTxPacketNumber>,
    /// The frame buffers a connection borrows; `None` while one holds them.
    buffers: Option<&'p mut PortConnectionBuffers>,
    connection: Option<PortConnection<'p, X::Port, X::RateControl>>,
    report: PortAttemptReport,
}

/// A connected station's connection and the context its phases run in.
type ConnectionParts<'a, 'p, X> = (
    &'a mut PortConnection<'p, <X as PortClientEnv>::Port, <X as PortStationEnv>::RateControl>,
    ConnectionContext<'a, 'p, X>,
);

impl<'p, X: PortStationEnv> PortStation<'p, X> {
    pub fn new(
        link: PortLink<'p, X>,
        timer: X::Timer,
        key_unwrap: X::KeyUnwrap,
        profile: PortStationProfile<'p>,
        security: StaAttemptSecurity<'p>,
        storage: &'p mut PortStationStorage,
    ) -> Self {
        let PortStationStorage { table, connection } = storage;
        Self {
            link,
            timer,
            key_unwrap,
            profile,
            security,
            table,
            refresh: true,
            candidate: None,
            selected: None,
            association: None,
            response_meta: None,
            peer: None,
            pending: None,
            keys: None,
            bip: None,
            packet_number: None,
            buffers: Some(connection),
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
        self.table
    }

    /// Scan again before the next attempt, or join the current candidate.
    pub fn set_refresh(&mut self, refresh: bool) {
        self.refresh = refresh;
    }

    pub const fn report(&self) -> &PortAttemptReport {
        &self.report
    }

    /// The connection of a connected station.
    pub const fn connection(&self) -> Option<&PortConnection<'p, X::Port, X::RateControl>> {
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

    /// Queue one Ethernet-II frame with `user_priority` for the access
    /// point; [`Self::run_until`] sends it.
    pub fn send(
        &mut self,
        ethernet: &[u8],
        user_priority: u8,
    ) -> Result<PortSend, PortLinkError<PortError<X>>> {
        let (connection, _) = self.context()?;
        connection.send(ethernet, user_priority)
    }

    /// Receive until `deadline`, handing every Ethernet frame to `deliver`.
    /// `Some` when the association ended; the station has then left it.
    pub async fn run_until(
        &mut self,
        deadline: oer_time::Instant,
        deliver: &mut impl FnMut(PortMsdu<'_, PortRxBuffer<X>>),
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

    /// Restart the association's power manager with `sleep_type`.
    pub async fn set_sleep_type(
        &mut self,
        sleep_type: SleepType,
    ) -> Result<(), PortLinkError<PortError<X>>> {
        let (connection, mut context) = self.context()?;
        connection.start_power(&mut context, sleep_type).await
    }

    /// Leave the association with a Deauthentication.
    pub async fn disconnect(&mut self) -> Result<(), PortLinkError<PortError<X>>> {
        self.end_connection(true).await
    }

    /// Drop the connection and take its frame buffers back.
    fn release_connection(&mut self) {
        if let Some(connection) = self.connection.take() {
            self.buffers = Some(connection.into_buffers());
        }
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
        self.release_connection();
        self.keys = None;
        self.association = None;
        self.response_meta = None;
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

    /// The mode the station associates with `candidate` in.
    fn phy(&self, candidate: &ScanRecord) -> PhyMode {
        let ht40_capable = self
            .link
            .port()
            .capabilities()
            .widths
            .contains(ChannelWidth::Mhz40Above);
        select_association_phy(candidate, self.profile.preference, ht40_capable)
    }

    async fn select_channel(&mut self) -> StepResult<X> {
        let candidate =
            self.candidate
                .ok_or(StaAttemptStepError::terminal(PortStationError::State(
                    StaAttemptStateError::MissingPreparedPeer,
                )))?;
        // An HT40 association tunes its access point's secondary channel too.
        let width = match (self.phy(&candidate), candidate.ht40_secondary_channel()) {
            (PhyMode::Ht40, Some(HtSecondaryChannel::Above)) => ChannelWidth::Mhz40Above,
            (PhyMode::Ht40, Some(HtSecondaryChannel::Below)) => ChannelWidth::Mhz40Below,
            _ => ChannelWidth::Mhz20,
        };
        let channel = if candidate.channel <= 14 {
            Channel::ghz2_4(candidate.channel, width)
        } else {
            Channel::ghz5(candidate.channel, width)
        }
        .map_err(|_| StaAttemptStepError::refresh_candidate(PortStationError::NoCandidate))?;
        self.link
            .retune(channel)
            .await
            .map_err(|error| StaAttemptStepError::retry_current(PortStationError::Link(error)))?;
        // Authentication and Association already contend with the access
        // point's advertised parameters, as the vendor station does.
        if let Some(parameters) = candidate.wmm_parameters() {
            self.link.install_edca(parameters).map_err(|error| {
                StaAttemptStepError::refresh_candidate(PortStationError::Link(error))
            })?;
        }
        Ok(())
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
        let phy = self.phy(&candidate);
        let Self {
            link,
            timer,
            profile,
            security,
            response_meta,
            ..
        } = self;
        *response_meta = None;
        let join = PortJoin::new(link, bssid)
            .with_response_meta(response_meta)
            .with_association(PortAssociation {
                access_point: &candidate,
                security: &selected,
                phy,
                capabilities: profile.capabilities,
                listen_interval: profile.listen_interval,
                he_power: profile.he_power,
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
        // The Association Response's set replaces the advertised one.
        if let Some(parameters) = self
            .association
            .as_ref()
            .and_then(|response| response.wmm_parameters)
        {
            self.link.install_edca(parameters).map_err(|error| {
                StaAttemptStepError::retry_current(PortStationError::Link(error))
            })?;
        }
        let candidate =
            self.candidate
                .ok_or(StaAttemptStepError::terminal(PortStationError::State(
                    StaAttemptStateError::MissingPreparedPeer,
                )))?;
        let association =
            self.association
                .ok_or(StaAttemptStepError::terminal(PortStationError::State(
                    StaAttemptStateError::MissingPreparedPeer,
                )))?;
        let peer = StaAssociatedPeer::derive(
            &candidate,
            &association,
            self.phy(&candidate),
            self.profile.he_packet_padding,
        )
        .map_err(|error| StaAttemptStepError::refresh_candidate(PortStationError::Peer(error)))?;
        // The planner protects every exchange as the BSS requires.
        self.link
            .tx_mut()
            .planner_mut()
            .protection_mut()
            .install_bss(peer.protection);
        // An HE association's PPDUs carry the BSS color of its HE Operation.
        if peer.phy == PhyMode::He20 {
            let vif = self.link.config().vif;
            self.link
                .apply(oer_ieee80211_lower_mac::LowerMacSetting::HeBssColor {
                    vif,
                    color: peer.he_bss_color,
                })
                .map_err(|error| {
                    StaAttemptStepError::retry_current(PortStationError::Link(error))
                })?;
        }
        self.peer = Some(peer);
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
        let (installed, connected) = established.into_parts();
        self.security.set_connected(connected);
        self.keys = Some(installed.keys);
        self.bip = installed.bip;
        self.packet_number = Some(packet_number);
        Ok(())
    }

    fn connection_config(&self) -> Option<PortConnectionConfig> {
        let candidate = self.candidate?;
        let association = self.association?;
        let selected = self.selected?;
        let peer = self.peer?;
        Some(PortConnectionConfig {
            peer,
            bssid: candidate.bssid,
            association_id: StaAssociationId::new(association.association_id & 0x3fff)?,
            peer_qos: association.wmm,
            edca: association
                .wmm_parameters
                .or_else(|| candidate.wmm_parameters()),
            management_protection: selected.security().protects_management(),
            sa_query_random: self.profile.sa_query_random,
            rx_reorder_gap: self.profile.rx_reorder_gap,
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
        // As the vendor station: agreements for a QoS HT or HE association
        // under CCMP.
        let tx_block_ack = match owner.profile.tx_block_ack.filter(|_| {
            config.peer_qos && owner.keys.is_some() && config.peer.phy != PhyMode::Legacy
        }) {
            Some(block_ack) => {
                match oer_ieee80211_mac::block_ack::TxBlockAckOriginator::new(
                    block_ack.policy,
                    block_ack.config,
                ) {
                    Ok(mut originator) => {
                        originator.queue_initial(block_ack.retry);
                        Some(originator)
                    }
                    Err(error) => {
                        return Err(StaConnectedEntryFailure::new(
                            owner,
                            StaFailureDisposition::Terminal,
                            PortStationError::TxBlockAck(error),
                        ));
                    }
                }
            }
            None => None,
        };
        let supervision = owner.profile.link;
        let link = match oer_ieee80211_sta::link_monitor::StaBeaconLossConfig::new(
            config.beacon_interval_tu,
            supervision.miss_limit,
            supervision.timeout,
        ) {
            Ok(loss) => {
                oer_ieee80211_sta::link_monitor::StaLinkMonitor::new(loss, supervision.probe)
            }
            Err(error) => {
                return Err(StaConnectedEntryFailure::new(
                    owner,
                    StaFailureDisposition::Terminal,
                    PortStationError::LinkSupervision(LinkSupervisionError::BeaconLoss(error)),
                ));
            }
        };
        let Some(probe) = PortLinkProbe::new(owner.profile.ssid, supervision.supported_rates)
        else {
            return Err(StaConnectedEntryFailure::new(
                owner,
                StaFailureDisposition::Terminal,
                PortStationError::LinkSupervision(LinkSupervisionError::Probe),
            ));
        };
        let packet_number = owner
            .packet_number
            .take()
            .unwrap_or(CcmpTxPacketNumber::new(owner.profile.ccmp_step));
        let Some(buffers) = owner.buffers.take() else {
            return Err(StaConnectedEntryFailure::new(
                owner,
                StaFailureDisposition::Terminal,
                PortStationError::Link(PortLinkError::MissingState),
            ));
        };
        let bip = owner.bip.take();
        let rate = <X::RateControl as StaRateControl>::associate(
            owner.link.rate_config(),
            &config.peer,
            owner.response_meta.and_then(link_metric),
        );
        owner.connection = Some(PortConnection::new(
            config,
            PortConnectionSecurity {
                keys: owner.keys,
                packet_number,
                bip,
            },
            tx_block_ack,
            PortLinkSupervisor {
                monitor: link,
                probe,
            },
            rate,
            buffers,
        ));
        if let Some(connection) = owner.connection.as_mut() {
            connection.arm_link(owner.timer.now());
        }
        // The power manager runs for every association: it decides when the
        // station dozes and when it asks its radio system for the air.
        let sleep_type = owner.profile.sleep_type;
        let started = match owner.context() {
            Ok((connection, mut context)) => connection.start_power(&mut context, sleep_type).await,
            Err(error) => Err(error),
        };
        if let Err(error) = started {
            owner.release_connection();
            return Err(StaConnectedEntryFailure::new(
                owner,
                StaFailureDisposition::RetryCurrentCandidate,
                PortStationError::Link(error),
            ));
        }
        Ok(owner)
    }
}

/// The access point's signal over the noise floor in a frame's receive
/// metadata, narrowed to a signed byte as the vendor's `ic_set_trc` does;
/// `None` when the port reported either value unavailable.
fn link_metric(meta: RxMeta) -> Option<i8> {
    let value = |evidence: RxEvidence<i8>| match evidence {
        RxEvidence::HardwareObserved(value) | RxEvidence::ProtocolValidated(value) => Some(value),
        RxEvidence::Unavailable => None,
    };
    Some(value(meta.rssi_dbm)?.wrapping_sub(value(meta.noise_floor_dbm)?))
}

/// The application a lifecycle-driven station serves.
pub trait PortStationApplication<B> {
    /// Whether the station should leave and stop.
    fn stop_requested(&mut self) -> bool;

    /// The next Ethernet-II frame to send, written into `ethernet`: its
    /// length and user priority.
    fn next_transmit(&mut self, _ethernet: &mut [u8]) -> Option<(usize, u8)> {
        None
    }

    /// One received Ethernet-II frame: the port's buffer to hand on where
    /// the network stack adopts it, or parts to copy.
    fn deliver(&mut self, msdu: PortMsdu<'_, B>);

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

impl<X: PortStationEnv, A: PortStationApplication<PortRxBuffer<X>>> PortStationLifecycle<'_, X, A> {
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

impl<'p, X: PortStationEnv, A: PortStationApplication<PortRxBuffer<X>>> StaLifecycleBackend
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
            while station.connection().is_some_and(PortConnection::can_queue)
                && let Some((length, priority)) = self.application.next_transmit(&mut frame)
            {
                if let Err(error) = station.send(&frame[..length], priority) {
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
                .run_until(deadline, &mut |msdu| application.deliver(msdu))
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
