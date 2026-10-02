//! The connected station over the lower-MAC port: its data plane, Block Ack
//! agreements, SA Query, disconnection and power save.

use oer_ieee80211_lower_mac::{
    Ieee80211LowerMacPort, KeySelector, LowerMacSetting, MacAddress, RxBlockAckAgreement,
    RxCryptoStatus, RxEvidence,
};
use oer_ieee80211_mac::{
    block_ack::{
        ADDBA_ACTION_BODY_LEN, BLOCK_ACK_CATEGORY, BlockAckAction, RxReorderBuffer, RxReorderError,
        RxReorderMpdu, RxReorderRelease, parse_block_ack_action, write_declined_addba_response,
        write_successful_addba_response,
    },
    ccmp::{
        CCMP_HEADER_LEN, CcmpHeader, CcmpKeyId, CcmpReplayLane, CcmpRxReplayState,
        CcmpTxPacketNumber,
    },
    data::{
        DataInterfaceRole, ETHERNET_HEADER_LEN, RxDuplicateFilter, decapsulate_data_frames,
        plan_data_encapsulation,
    },
    management_protection::{SA_QUERY_CATEGORY, SaQuery, is_robust_action_category},
    qos::WmmUserPriority,
    sequence::SequenceNumber,
    station::{
        StaDisconnect, StaDisconnectKind, StaManagementFrame, StaManagementSubtype,
        StaProtectedDataFrame, StaProtectedManagementFrame, StaTxSequenceCounters,
        StationFrameError, parse_sta_disconnect,
    },
    station_beacon::parse_sta_beacon,
    station_power_save::StaAssociationId,
};
use oer_ieee80211_sta::{
    modem_sleep::{PmBeacon, PmTraffic, SleepType},
    sa_query::{SaQueryStep, StationSaQuery},
};
use oer_ieee80211_upper_mac::TxReport;
use oer_time::{Clock, Instant};

use super::{
    link::{
        PORT_FRAME_CAPACITY, PortError, PortFrame, PortInput, PortLink, PortLinkError,
        PortStationEnv,
    },
    power::{PortPowerSave, PowerContext, acknowledged},
    rsn::PortKeys,
    wire,
};

/// MPDUs the reorder buffers of all agreements hold together.
pub const PORT_REORDER_SLOTS: usize = 8;
/// The widest receive Block Ack window the station accepts.
pub const PORT_REORDER_WINDOW: usize = 64;
const TIDS: usize = 8;
const MANAGEMENT_HEADER_LEN: usize = 24;
const BEACON_SUBTYPE: u8 = 8;
const DISASSOCIATION_SUBTYPE: u8 = 10;
const DEAUTHENTICATION_SUBTYPE: u8 = 12;
const ACTION_SUBTYPE: u8 = 13;
const BLOCK_ACK_REQUEST_SUBTYPE: u8 = 8;
/// Reason code of a station that leaves its BSS.
const REASON_LEAVING: u16 = 3;

/// What the association negotiated, which the connection keeps.
#[derive(Clone, Copy, Debug)]
pub struct PortConnectionConfig {
    pub bssid: MacAddress,
    pub association_id: StaAssociationId,
    /// The access point takes QoS data frames.
    pub peer_qos: bool,
    /// The association protects its robust management frames.
    pub management_protection: bool,
    /// The random source of the first SA Query transaction identifier.
    pub sa_query_random: fn() -> u32,
    /// The access point's beacon interval, in time units.
    pub beacon_interval_tu: u16,
    /// The access point's TSF when the station joined.
    pub join_timestamp_tsf: u64,
}

/// Why the association ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortDisconnect {
    /// The access point sent a Deauthentication.
    Deauthenticated { reason_code: u16 },
    /// The access point sent a Disassociation.
    Disassociated { reason_code: u16 },
    /// The access point did not answer an SA Query in time.
    SaQueryTimeout,
    /// The station left.
    Local,
}

/// What the receive path dropped or delivered.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PortRxCounters {
    /// Ethernet frames handed to the caller.
    pub delivered: u32,
    /// Retransmissions of an MPDU already received.
    pub duplicates: u32,
    /// Protected MPDUs whose packet number did not advance.
    pub replayed: u32,
    /// Protected MPDUs the backend did not decrypt, or no key covers.
    pub undecrypted: u32,
    /// Unprotected data MPDUs after the keys were installed.
    pub unprotected: u32,
    /// MPDUs behind a Block Ack window.
    pub behind_window: u32,
    /// MPDUs that do not decapsulate.
    pub malformed: u32,
}

/// The outcome of offering one frame for transmission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortSend {
    /// The exchange ended with this report.
    Sent(TxReport),
    /// The station dozes; the frame waits for it to wake.
    Held,
}

struct HeldFrame {
    ethernet: [u8; PORT_FRAME_CAPACITY],
    len: usize,
    user_priority: u8,
}

/// The connected station's state over the port.
pub struct PortConnection<P: Ieee80211LowerMacPort> {
    config: PortConnectionConfig,
    keys: Option<PortKeys>,
    packet_number: CcmpTxPacketNumber,
    pairwise_replay: CcmpRxReplayState,
    group_replay: CcmpRxReplayState,
    duplicates: RxDuplicateFilter,
    reorder: [Option<RxReorderBuffer<PORT_REORDER_WINDOW, PORT_REORDER_SLOTS>>; TIDS],
    slots: [Option<PortFrame>; PORT_REORDER_SLOTS],
    sa_query: StationSaQuery,
    power: Option<PortPowerSave<P>>,
    held: Option<HeldFrame>,
    counters: PortRxCounters,
}

/// What one connection operation borrows from its station.
pub(crate) struct ConnectionContext<'a, 'p, X: PortStationEnv> {
    pub link: &'a mut PortLink<'p, X>,
    pub timer: &'a X::Timer,
    pub sequences: &'a mut StaTxSequenceCounters,
}

impl<P: Ieee80211LowerMacPort> PortConnection<P> {
    pub(crate) fn new(
        config: PortConnectionConfig,
        keys: Option<PortKeys>,
        packet_number: CcmpTxPacketNumber,
    ) -> Self {
        let group_replay = keys
            .and_then(|keys| {
                CcmpRxReplayState::from_receive_sequence(keys.group_receive_sequence).ok()
            })
            .unwrap_or_default();
        Self {
            config,
            keys,
            packet_number,
            pairwise_replay: CcmpRxReplayState::default(),
            group_replay,
            duplicates: RxDuplicateFilter::new(),
            reorder: [const { None }; TIDS],
            slots: [const { None }; PORT_REORDER_SLOTS],
            sa_query: StationSaQuery::new(),
            power: None,
            held: None,
            counters: PortRxCounters::default(),
        }
    }

    pub const fn config(&self) -> &PortConnectionConfig {
        &self.config
    }

    pub const fn keys(&self) -> Option<PortKeys> {
        self.keys
    }

    pub const fn counters(&self) -> PortRxCounters {
        self.counters
    }

    pub const fn power_save(&self) -> Option<&PortPowerSave<P>> {
        self.power.as_ref()
    }

    /// Whether a receive Block Ack agreement of `tid` runs.
    pub fn block_ack(&self, tid: u8) -> bool {
        self.reorder
            .get(usize::from(tid))
            .is_some_and(Option::is_some)
    }

    /// The earliest instant the connection needs its owner without input.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        let sa_query = self.sa_query.deadline_micros().map(Instant::from_micros);
        let power = self.power.as_ref().and_then(PortPowerSave::next_deadline);
        match (sa_query, power) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn traffic(&self) -> PmTraffic {
        PmTraffic {
            tx_pending: self.held.is_some(),
            connection_pending: false,
        }
    }

    fn power_context<'a, 'p, X: PortStationEnv<Port = P>>(
        context: &'a mut ConnectionContext<'_, 'p, X>,
        bssid: MacAddress,
        traffic: PmTraffic,
    ) -> PowerContext<'a, 'p, X> {
        PowerContext {
            link: &mut *context.link,
            timer: context.timer,
            sequence: context.sequences.non_qos_mut(),
            bssid,
            traffic,
        }
    }

    /// Start modem sleep of `sleep_type` over the port's beacon timing.
    pub(crate) async fn enable_power_save<X: PortStationEnv<Port = P>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
        sleep_type: SleepType,
    ) -> Result<(), PortLinkError<PortError<X>>> {
        let ops = context
            .link
            .beacon_timing()
            .ok_or(PortLinkError::MissingState)?;
        let mut power = PortPowerSave::new(sleep_type, ops);
        let join_beacon = PmBeacon {
            timestamp_tsf: self.config.join_timestamp_tsf,
            interval_tu: self.config.beacon_interval_tu,
            tim: None,
        };
        let traffic = self.traffic();
        power
            .start(
                &mut Self::power_context(context, self.config.bssid, traffic),
                join_beacon,
            )
            .await?;
        self.power = Some(power);
        Ok(())
    }

    /// Send one Ethernet-II frame to the access point's BSS.
    pub(crate) async fn send<X: PortStationEnv<Port = P>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
        ethernet: &[u8],
        user_priority: u8,
    ) -> Result<PortSend, PortLinkError<PortError<X>>> {
        if ethernet.len() < ETHERNET_HEADER_LEN || ethernet.len() > PORT_FRAME_CAPACITY {
            return Err(PortLinkError::Frame(
                StationFrameError::EthernetFrameTooShort,
            ));
        }
        let priority = WmmUserPriority::new(user_priority).ok_or(PortLinkError::Frame(
            StationFrameError::UserPriorityOutOfRange,
        ))?;
        let traffic = self.traffic();
        if let Some(power) = &mut self.power {
            let wait = power
                .tx_data(&mut Self::power_context(
                    context,
                    self.config.bssid,
                    traffic,
                ))
                .await?;
            if wait {
                let mut held = HeldFrame {
                    ethernet: [0; PORT_FRAME_CAPACITY],
                    len: ethernet.len(),
                    user_priority,
                };
                held.ethernet[..ethernet.len()].copy_from_slice(ethernet);
                self.held = Some(held);
                return Ok(PortSend::Held);
            }
        }
        let report = self.transmit(context, ethernet, priority).await?;
        let traffic = self.traffic();
        if let Some(power) = &mut self.power {
            power
                .tx_data_done(
                    &mut Self::power_context(context, self.config.bssid, traffic),
                    acknowledged(&report),
                )
                .await?;
        }
        Ok(PortSend::Sent(report))
    }

    async fn transmit<X: PortStationEnv<Port = P>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
        ethernet: &[u8],
        priority: WmmUserPriority,
    ) -> Result<TxReport, PortLinkError<PortError<X>>> {
        let config = *context.link.config();
        let destination: [u8; 6] = ethernet[..6].try_into().expect("six octets");
        let ether_type = u16::from_be_bytes([ethernet[12], ethernet[13]]);
        let payload = &ethernet[ETHERNET_HEADER_LEN..];
        let sequence_number = if self.config.peer_qos {
            context
                .sequences
                .take_qos(priority.value())
                .ok_or(PortLinkError::Frame(
                    StationFrameError::UserPriorityOutOfRange,
                ))?
        } else {
            context.sequences.take_non_qos()
        };
        let mut frame = [0_u8; PORT_FRAME_CAPACITY + 64];
        let (length, key) = match self.keys {
            Some(keys) => {
                let ccmp_header = self
                    .packet_number
                    .next_header(CcmpKeyId::new(0).expect("key identifier zero"))
                    .map_err(PortLinkError::PacketNumber)?;
                let length = StaProtectedDataFrame {
                    source: config.address,
                    bssid: self.config.bssid,
                    destination,
                    sequence_number,
                    user_priority: priority.value(),
                    peer_qos: self.config.peer_qos,
                    ccmp_header,
                    ether_type,
                    payload,
                }
                .encode(&mut frame)
                .map_err(PortLinkError::Frame)?;
                (length, KeySelector::Key(keys.pairwise))
            }
            None => (
                encode_open_data(
                    &mut frame,
                    config.address,
                    self.config.bssid,
                    ethernet,
                    priority.value(),
                    self.config.peer_qos,
                    sequence_number,
                )?,
                KeySelector::Plaintext,
            ),
        };
        context
            .link
            .transmit(
                &frame[..length],
                key,
                priority.access_category(),
                config.data_rate,
            )
            .await
    }

    /// Process inputs until `deadline`: received frames, TBTTs, the SA
    /// Query and power-save timers. Every delivered Ethernet frame goes to
    /// `deliver`. `Some` when the association ended.
    pub(crate) async fn run_until<X: PortStationEnv<Port = P>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
        deadline: Instant,
        deliver: &mut impl FnMut(&[u8]),
    ) -> Result<Option<PortDisconnect>, PortLinkError<PortError<X>>> {
        loop {
            if let Some(disconnect) = self.expire(context).await? {
                return Ok(Some(disconnect));
            }
            if self.power.as_mut().is_some_and(PortPowerSave::take_release)
                && let Some(held) = self.held.take()
            {
                self.send(context, &held.ethernet[..held.len], held.user_priority)
                    .await?;
            }
            let now = context.timer.now();
            if now >= deadline {
                return Ok(None);
            }
            let wake = self
                .next_deadline()
                .map_or(deadline, |due| due.min(deadline));
            let Some(input) = context.link.next_input(context.timer, wake).await else {
                continue;
            };
            if let Some(disconnect) = self.input(context, input, deliver).await? {
                return Ok(Some(disconnect));
            }
        }
    }

    /// Run the SA Query and power-save timers that are due.
    async fn expire<X: PortStationEnv<Port = P>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
    ) -> Result<Option<PortDisconnect>, PortLinkError<PortError<X>>> {
        let now = context.timer.now().as_micros();
        match self.sa_query.step(now) {
            SaQueryStep::Idle => {}
            SaQueryStep::Request(transaction) => {
                self.send_sa_query(context, SaQuery::Request { transaction })
                    .await?;
            }
            SaQueryStep::TimedOut => return Ok(Some(PortDisconnect::SaQueryTimeout)),
        }
        let traffic = self.traffic();
        if let Some(power) = &mut self.power {
            power
                .expire(&mut Self::power_context(
                    context,
                    self.config.bssid,
                    traffic,
                ))
                .await?;
        }
        Ok(None)
    }

    async fn input<X: PortStationEnv<Port = P>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
        input: PortInput,
        deliver: &mut impl FnMut(&[u8]),
    ) -> Result<Option<PortDisconnect>, PortLinkError<PortError<X>>> {
        let frame = match input {
            PortInput::Frame(frame) => frame,
            PortInput::Tbtt(_) => {
                let traffic = self.traffic();
                if let Some(power) = &mut self.power {
                    power
                        .tbtt(&mut Self::power_context(
                            context,
                            self.config.bssid,
                            traffic,
                        ))
                        .await?;
                }
                return Ok(None);
            }
            // Frames in the gap are gone; a waiting exchange recovers its
            // own completion through the router.
            PortInput::EventsLost => return Ok(None),
            PortInput::Poisoned => return Err(PortLinkError::Poisoned),
        };
        let bytes = frame.bytes();
        if wire::address2(bytes) != Some(self.config.bssid) {
            return Ok(None);
        }
        if wire::is_management(bytes) {
            return self.management(context, &frame).await;
        }
        if wire::is_control(bytes) {
            self.control(bytes, deliver);
            return Ok(None);
        }
        if wire::is_data(bytes) && wire::from_access_point(bytes) {
            self.data(context, frame, deliver).await?;
        }
        Ok(None)
    }

    async fn management<X: PortStationEnv<Port = P>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
        frame: &PortFrame,
    ) -> Result<Option<PortDisconnect>, PortLinkError<PortError<X>>> {
        let bytes = frame.bytes();
        let address = context.link.config().address;
        let protected = wire::is_protected(bytes);
        if protected && !decrypted(frame) {
            self.counters.undecrypted = self.counters.undecrypted.saturating_add(1);
            return Ok(None);
        }
        match wire::subtype(bytes) {
            BEACON_SUBTYPE => {
                if let Ok(beacon) =
                    parse_sta_beacon(bytes, self.config.bssid, self.config.association_id.get())
                {
                    let traffic = self.traffic();
                    if let Some(power) = &mut self.power {
                        power
                            .beacon(
                                &mut Self::power_context(context, self.config.bssid, traffic),
                                &beacon,
                            )
                            .await?;
                    }
                }
                Ok(None)
            }
            DISASSOCIATION_SUBTYPE | DEAUTHENTICATION_SUBTYPE => {
                let Some(disconnect) = parse_disconnect(bytes, address, self.config.bssid) else {
                    return Ok(None);
                };
                if self.config.management_protection && !protected {
                    // An unprotected disconnect may be forged: ask the access
                    // point whether the association holds.
                    let now = context.timer.now().as_micros();
                    if let Some(transaction) =
                        self.sa_query.start(now, (self.config.sa_query_random)())
                    {
                        self.send_sa_query(context, SaQuery::Request { transaction })
                            .await?;
                    }
                    return Ok(None);
                }
                Ok(Some(match disconnect.kind {
                    StaDisconnectKind::Deauthentication => PortDisconnect::Deauthenticated {
                        reason_code: disconnect.reason_code,
                    },
                    StaDisconnectKind::Disassociation => PortDisconnect::Disassociated {
                        reason_code: disconnect.reason_code,
                    },
                }))
            }
            ACTION_SUBTYPE if wire::address1(bytes) == Some(address) => {
                let Some(body) = wire::management_body(bytes) else {
                    return Ok(None);
                };
                let Some(&category) = body.first() else {
                    return Ok(None);
                };
                if self.config.management_protection
                    && is_robust_action_category(category)
                    && !protected
                {
                    return Ok(None);
                }
                if category == BLOCK_ACK_CATEGORY {
                    self.block_ack_action(context, body).await?;
                } else if category == SA_QUERY_CATEGORY {
                    match SaQuery::parse(body) {
                        Some(SaQuery::Request { transaction }) => {
                            self.send_sa_query(context, SaQuery::Response { transaction })
                                .await?;
                        }
                        Some(SaQuery::Response { transaction }) => {
                            self.sa_query.response(transaction);
                        }
                        None => {}
                    }
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    async fn block_ack_action<X: PortStationEnv<Port = P>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
        body: &[u8],
    ) -> Result<(), PortLinkError<PortError<X>>> {
        match parse_block_ack_action(body) {
            Some(BlockAckAction::AddbaRequest {
                dialog_token,
                tid,
                window,
                starting_sequence,
                ..
            }) => {
                let capabilities = context.link.port().capabilities();
                let accepted = usize::from(tid) < TIDS
                    && window != 0
                    && tid <= capabilities.rx_block_ack_max_tid
                    && self.reorder[usize::from(tid)].is_none();
                let window = window
                    .min(capabilities.rx_block_ack_max_window)
                    .min(PORT_REORDER_WINDOW as u16);
                let agreement = RxBlockAckAgreement {
                    vif: context.link.config().vif,
                    peer: self.config.bssid,
                    tid,
                    start_sequence: starting_sequence,
                    window,
                };
                let buffer = RxReorderBuffer::new(starting_sequence, window).ok();
                let installed = accepted
                    && buffer.is_some()
                    && context
                        .link
                        .apply(LowerMacSetting::AddRxBlockAck(agreement))
                        .is_ok();
                let mut response = [0_u8; ADDBA_ACTION_BODY_LEN];
                if installed {
                    self.reorder[usize::from(tid)] = buffer;
                    write_successful_addba_response(&mut response, dialog_token, tid, window)
                } else {
                    write_declined_addba_response(&mut response, dialog_token, tid & 0x0f, window)
                }
                .map_err(|_| PortLinkError::MissingState)?;
                self.send_action(context, &response).await
            }
            Some(BlockAckAction::Delba { tid, .. }) => {
                if let Some(mut buffer) = self
                    .reorder
                    .get_mut(usize::from(tid))
                    .and_then(Option::take)
                {
                    let release = buffer.stop();
                    self.recycle(&release);
                    context.link.apply(LowerMacSetting::RemoveRxBlockAck {
                        vif: context.link.config().vif,
                        peer: self.config.bssid,
                        tid,
                    })?;
                }
                Ok(())
            }
            Some(BlockAckAction::AddbaResponse { .. }) | None => Ok(()),
        }
    }

    /// A BlockAckReq moves its agreement's window.
    fn control(&mut self, bytes: &[u8], deliver: &mut impl FnMut(&[u8])) {
        if wire::subtype(bytes) != BLOCK_ACK_REQUEST_SUBTYPE || bytes.len() < 20 {
            return;
        }
        let tid = bytes[17] >> 4;
        let starting_sequence =
            SequenceNumber::from_sequence_control(u16::from_le_bytes([bytes[18], bytes[19]]));
        let Some(buffer) = self
            .reorder
            .get_mut(usize::from(tid))
            .and_then(Option::as_mut)
        else {
            return;
        };
        if let Some(release) = buffer.move_window_to(starting_sequence) {
            self.release(&release, deliver);
        }
    }

    async fn data<X: PortStationEnv<Port = P>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
        frame: PortFrame,
        deliver: &mut impl FnMut(&[u8]),
    ) -> Result<(), PortLinkError<PortError<X>>> {
        let bytes = frame.bytes();
        let Some(receiver) = wire::address1(bytes) else {
            return Ok(());
        };
        let group = wire::is_group(receiver);
        if !group && receiver != context.link.config().address {
            return Ok(());
        }
        let more_data = wire::more_data(bytes);
        if wire::is_protected(bytes) {
            if self.keys.is_none() || !decrypted(&frame) {
                self.counters.undecrypted = self.counters.undecrypted.saturating_add(1);
                return Ok(());
            }
        } else if self.keys.is_some() {
            self.counters.unprotected = self.counters.unprotected.saturating_add(1);
            return Ok(());
        }
        let tid = wire::tid(bytes);
        if self
            .duplicates
            .is_duplicate(wire::is_retry(bytes), wire::sequence_control(bytes), tid)
        {
            self.counters.duplicates = self.counters.duplicates.saturating_add(1);
            return Ok(());
        }
        let traffic = self.traffic();
        if let Some(power) = &mut self.power {
            power
                .rx_data(
                    &mut Self::power_context(context, self.config.bssid, traffic),
                    group,
                    more_data,
                )
                .await?;
        }
        if wire::is_null_data(bytes) {
            return Ok(());
        }
        match tid.filter(|_| !group) {
            Some(tid) if self.block_ack(tid) => self.reorder_mpdu(tid, frame, deliver),
            _ => self.deliver(&frame, deliver),
        }
        Ok(())
    }

    /// Buffer one MPDU of an agreement and deliver what its window releases.
    fn reorder_mpdu(&mut self, tid: u8, frame: PortFrame, deliver: &mut impl FnMut(&[u8])) {
        let sequence = SequenceNumber::from_sequence_control(wire::sequence_control(frame.bytes()));
        let slot = match self.slots.iter().position(Option::is_none) {
            Some(slot) => slot,
            None => {
                // Every slot is taken: release the oldest run to make room.
                let Some(buffer) = self.reorder[usize::from(tid)].as_mut() else {
                    return;
                };
                let release = buffer.expire_gap();
                self.release(&release, deliver);
                match self.slots.iter().position(Option::is_none) {
                    Some(slot) => slot,
                    None => return,
                }
            }
        };
        self.slots[slot] = Some(frame);
        let Some(buffer) = self.reorder[usize::from(tid)].as_mut() else {
            self.slots[slot] = None;
            return;
        };
        match buffer.ingest(RxReorderMpdu {
            sequence,
            slot: slot as u8,
        }) {
            Ok(release) => {
                if release.rejected.is_some() {
                    self.slots[slot] = None;
                    self.counters.behind_window = self.counters.behind_window.saturating_add(1);
                }
                self.release(&release, deliver);
            }
            Err(RxReorderError::DuplicateSequence(_)) => {
                self.slots[slot] = None;
                self.counters.duplicates = self.counters.duplicates.saturating_add(1);
            }
            Err(_) => self.slots[slot] = None,
        }
    }

    fn release(
        &mut self,
        release: &RxReorderRelease<PORT_REORDER_WINDOW>,
        deliver: &mut impl FnMut(&[u8]),
    ) {
        for mpdu in release.iter() {
            if let Some(frame) = self
                .slots
                .get_mut(usize::from(mpdu.slot))
                .and_then(Option::take)
            {
                self.deliver(&frame, deliver);
            }
        }
    }

    /// Drop the MPDUs an ended agreement released.
    fn recycle(&mut self, release: &RxReorderRelease<PORT_REORDER_WINDOW>) {
        for mpdu in release.iter() {
            if let Some(slot) = self.slots.get_mut(usize::from(mpdu.slot)) {
                *slot = None;
            }
        }
    }

    /// Check one in-order MPDU's packet number and hand its MSDUs on.
    fn deliver(&mut self, frame: &PortFrame, deliver: &mut impl FnMut(&[u8])) {
        let bytes = frame.bytes();
        let header = wire::data_header_len(bytes);
        let protected = wire::is_protected(bytes);
        if protected {
            let Some(ccmp) =
                wire::ccmp_header(bytes, header).and_then(|ccmp| CcmpHeader::parse(ccmp).ok())
            else {
                self.counters.malformed = self.counters.malformed.saturating_add(1);
                return;
            };
            let lane = match wire::tid(bytes) {
                Some(tid) => CcmpReplayLane::Tid(tid),
                None => CcmpReplayLane::NonQos,
            };
            let group = wire::address1(bytes).is_some_and(wire::is_group);
            let replay = if group {
                &mut self.group_replay
            } else {
                &mut self.pairwise_replay
            };
            if replay.commit_immediate(lane, ccmp.packet_number()).is_err() {
                self.counters.replayed = self.counters.replayed.saturating_add(1);
                return;
            }
        }
        let offset = header + if protected { CCMP_HEADER_LEN } else { 0 };
        let Some(length) = bytes.len().checked_sub(offset) else {
            self.counters.malformed = self.counters.malformed.saturating_add(1);
            return;
        };
        let Ok(frames) = decapsulate_data_frames(DataInterfaceRole::Station, bytes, offset, length)
        else {
            self.counters.malformed = self.counters.malformed.saturating_add(1);
            return;
        };
        let mut ethernet = [0_u8; PORT_FRAME_CAPACITY];
        for parts in frames {
            match parts.and_then(|parts| parts.copy_to(&mut ethernet)) {
                Ok(length) => {
                    self.counters.delivered = self.counters.delivered.saturating_add(1);
                    deliver(&ethernet[..length]);
                }
                Err(_) => {
                    self.counters.malformed = self.counters.malformed.saturating_add(1);
                    return;
                }
            }
        }
    }

    async fn send_sa_query<X: PortStationEnv<Port = P>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
        query: SaQuery,
    ) -> Result<(), PortLinkError<PortError<X>>> {
        self.send_action(context, &query.encode()).await
    }

    /// Send one Action frame, protected under management frame protection.
    async fn send_action<X: PortStationEnv<Port = P>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
        body: &[u8],
    ) -> Result<(), PortLinkError<PortError<X>>> {
        self.send_management(context, StaManagementSubtype::Action, body)
            .await
    }

    async fn send_management<X: PortStationEnv<Port = P>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
        subtype: StaManagementSubtype,
        body: &[u8],
    ) -> Result<(), PortLinkError<PortError<X>>> {
        let config = *context.link.config();
        let sequence_number = context.sequences.take_non_qos();
        let mut frame = [0_u8; MANAGEMENT_HEADER_LEN + CCMP_HEADER_LEN + 64];
        let robust = match subtype {
            StaManagementSubtype::Action => {
                body.first().copied().is_some_and(is_robust_action_category)
            }
            StaManagementSubtype::Deauthentication => true,
        };
        let (length, key) = match self.keys {
            Some(keys) if self.config.management_protection && robust => {
                let ccmp_header = self
                    .packet_number
                    .next_header(CcmpKeyId::new(0).expect("key identifier zero"))
                    .map_err(PortLinkError::PacketNumber)?;
                let length = StaProtectedManagementFrame {
                    subtype,
                    source: config.address,
                    bssid: self.config.bssid,
                    sequence_number,
                    ccmp_header,
                    body,
                }
                .encode(&mut frame)
                .map_err(PortLinkError::Frame)?;
                (length, KeySelector::Key(keys.pairwise))
            }
            _ => (
                StaManagementFrame {
                    subtype,
                    source: config.address,
                    bssid: self.config.bssid,
                    sequence_number,
                    body,
                }
                .encode(&mut frame)
                .map_err(PortLinkError::Frame)?,
                KeySelector::Plaintext,
            ),
        };
        context
            .link
            .transmit(
                &frame[..length],
                key,
                oer_ieee80211_mac::qos::WmmAccessCategory::Voice,
                config.management_rate,
            )
            .await
            .map(|_| ())
    }

    /// Leave the association: send a Deauthentication while the station
    /// still owns it, stop power save, end the Block Ack agreements and
    /// remove the keys.
    pub(crate) async fn leave<X: PortStationEnv<Port = P>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
        send_deauthentication: bool,
    ) -> Result<(), PortLinkError<PortError<X>>> {
        let traffic = self.traffic();
        if let Some(mut power) = self.power.take() {
            power
                .stop(&mut Self::power_context(
                    context,
                    self.config.bssid,
                    traffic,
                ))
                .await?;
        }
        if send_deauthentication {
            self.send_management(
                context,
                StaManagementSubtype::Deauthentication,
                &REASON_LEAVING.to_le_bytes(),
            )
            .await?;
        }
        self.held = None;
        for tid in 0..TIDS as u8 {
            if let Some(mut buffer) = self.reorder[usize::from(tid)].take() {
                let release = buffer.stop();
                self.recycle(&release);
                context.link.apply(LowerMacSetting::RemoveRxBlockAck {
                    vif: context.link.config().vif,
                    peer: self.config.bssid,
                    tid,
                })?;
            }
        }
        if let Some(keys) = self.keys.take() {
            context.link.apply(LowerMacSetting::RemoveKey(keys.group))?;
            context
                .link
                .apply(LowerMacSetting::RemoveKey(keys.pairwise))?;
        }
        context
            .link
            .configure(None, oer_ieee80211_lower_mac::ReceiveFilter::NONE)
    }
}

/// Whether the backend decrypted and verified a protected frame.
fn decrypted(frame: &PortFrame) -> bool {
    matches!(
        frame.meta().crypto,
        RxEvidence::HardwareObserved(RxCryptoStatus::DecryptedAndIntegrityVerified)
            | RxEvidence::ProtocolValidated(RxCryptoStatus::DecryptedAndIntegrityVerified)
    )
}

/// A Deauthentication or Disassociation of `bssid` to `local`, its CCMP
/// header skipped when protected.
fn parse_disconnect(frame: &[u8], local: MacAddress, bssid: MacAddress) -> Option<StaDisconnect> {
    if !wire::is_protected(frame) {
        return parse_sta_disconnect(frame, local, bssid);
    }
    let body = wire::management_body(frame)?;
    let mut unprotected = [0_u8; MANAGEMENT_HEADER_LEN + 2];
    unprotected[..MANAGEMENT_HEADER_LEN].copy_from_slice(frame.get(..MANAGEMENT_HEADER_LEN)?);
    unprotected[1] &= !0x40;
    unprotected[MANAGEMENT_HEADER_LEN..].copy_from_slice(body.get(..2)?);
    parse_sta_disconnect(&unprotected, local, bssid)
}

/// Encode an unprotected data MPDU of one Ethernet-II frame.
fn encode_open_data<E>(
    output: &mut [u8],
    address: MacAddress,
    bssid: MacAddress,
    ethernet: &[u8],
    user_priority: u8,
    peer_qos: bool,
    sequence_number: SequenceNumber,
) -> Result<usize, PortLinkError<E>> {
    let header: [u8; ETHERNET_HEADER_LEN] = ethernet[..ETHERNET_HEADER_LEN]
        .try_into()
        .expect("an Ethernet header");
    let plan = plan_data_encapsulation(
        DataInterfaceRole::Station,
        bssid,
        address,
        header,
        user_priority,
        peer_qos,
        false,
    )
    .ok_or(PortLinkError::Frame(
        StationFrameError::UserPriorityOutOfRange,
    ))?;
    let header_len = usize::from(plan.header_len);
    let payload = &ethernet[ETHERNET_HEADER_LEN..];
    let required = header_len + plan.llc_snap.len() + payload.len();
    let frame = output.get_mut(..required).ok_or(PortLinkError::Frame(
        StationFrameError::OutputTooSmall { required },
    ))?;
    frame[..header_len].copy_from_slice(&plan.header[..header_len]);
    frame[22..24].copy_from_slice(&sequence_number.sequence_control().to_le_bytes());
    let llc_end = header_len + plan.llc_snap.len();
    frame[header_len..llc_end].copy_from_slice(&plan.llc_snap);
    frame[llc_end..].copy_from_slice(payload);
    Ok(required)
}
