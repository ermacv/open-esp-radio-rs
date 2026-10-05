//! The connected station over the lower-MAC port: its data plane, Block Ack
//! agreements, SA Query, disconnection and power save.

use oer_ieee80211_datapath::{DestinationTxQueues, SoftwareTxFrame};
use oer_ieee80211_lower_mac::{
    Cipher, KeyInstall, KeyScope, KeySelector, LowerMacBeaconTiming, LowerMacSetting, MacAddress,
    RxBlockAckAgreement, RxCryptoStatus, RxEvidence, RxMeta,
};
use oer_ieee80211_mac::block_ack::TX_BLOCK_ACK_MAX_TIDS;
use oer_ieee80211_mac::block_ack::{TxBlockAckOriginator, TxBlockAckResponseDisposition};
use oer_ieee80211_mac::qos::classify_ethernet_wmm;
use oer_ieee80211_mac::{
    block_ack::{
        ADDBA_ACTION_BODY_LEN, BLOCK_ACK_CATEGORY, BlockAckAction, parse_block_ack_action,
        write_declined_addba_response, write_successful_addba_response,
    },
    ccmp::{
        CCMP_HEADER_LEN, CcmpHeader, CcmpKeyId, CcmpReplayLane, CcmpRxReplayState,
        CcmpTxPacketNumber,
    },
    data::{
        DataDecapError, DataInterfaceRole, ETHERNET_HEADER_LEN, RxDuplicateFilter,
        decapsulate_data_frames, plan_data_decapsulation, plan_data_encapsulation,
    },
    management::{BROADCAST_ADDRESS, ProbeRequest},
    management_protection::{SA_QUERY_CATEGORY, SaQuery, is_robust_action_category},
    qos::{WmmAccessCategory, WmmUserPriority},
    sequence::SequenceNumber,
    station::{
        StaDisconnect, StaDisconnectKind, StaManagementFrame, StaManagementSubtype,
        StaProtectedDataFrame, StaProtectedManagementFrame, StaTxSequenceCounters,
        StationFrameError, parse_sta_disconnect,
    },
    station_beacon::parse_sta_beacon,
    station_power_save::StaAssociationId,
};
use oer_ieee80211_rsn::{
    OwnedEapolFrame, RsnInterface, bip::BipReceiver, keys::RsnKeyKind,
    runner::RSN_HANDSHAKE_EAPOL_CAPACITY, supplicant::RsnConnectedSupplicant,
};
use oer_ieee80211_rsn_service::supplicant::{RsnGroupMessage1Step, process_group_message1};
use oer_ieee80211_sta::{
    link_monitor::{StaLinkAction, StaLinkMonitor},
    modem_sleep::{PmBeacon, PmTraffic, SleepType},
    sa_query::{SaQueryStep, StationSaQuery},
};
use oer_ieee80211_upper_mac::{
    TxBody, TxReceiver, TxReport, TxRequest,
    aggregate::{AmpduLimits, AmpduRun},
    rate_control::RateControl,
};
use oer_ieee80211_upper_mac_service::UpperMacTxError;
use oer_ieee80211_upper_mac_service::aggregate::{AmpduSubframes, PORT_AMPDU_SUBFRAMES};
use oer_ieee80211_upper_mac_service::client::{PortError, PortFrame, PortInput, PortMsdu};
use oer_ieee80211_upper_mac_service::frame::PORT_MPDU_CAPACITY;
use oer_ieee80211_upper_mac_service::reorder::{
    CURRENT_SLOT, Offer, PORT_REORDER_WINDOW, ReorderRelease, RxReorder,
};
use oer_time::{Clock, Duration, Instant};

use super::{
    link::{PortConnectionFrame, PortLink, PortLinkError, PortStationEnv, PortStationFrame},
    power::{PortPowerSave, PowerContext, acknowledged},
    rsn::{EAPOL_ETHER_TYPE, PortKeys, send_protected_eapol},
    wire,
};

const TIDS: usize = 8;
const MANAGEMENT_HEADER_LEN: usize = 24;
const PROBE_RESPONSE_SUBTYPE: u8 = 5;
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
    /// The access point as the association left it: its PHY, HT and HE
    /// facts and the protection the planner applies.
    pub peer: oer_ieee80211_sta::association::StaAssociatedPeer,
    pub association_id: StaAssociationId,
    /// The access point takes QoS data frames.
    pub peer_qos: bool,
    /// The EDCA parameters the station contends with: the Association
    /// Response's set, else the one the access point advertised.
    pub edca: Option<oer_ieee80211_mac::extensions::wmm::WmmParameterSet>,
    /// The association protects its robust management frames.
    pub management_protection: bool,
    /// The random source of the first SA Query transaction identifier.
    pub sa_query_random: fn() -> u32,
    /// How long a receive reorder window holds a buffered run behind a
    /// missing MPDU.
    pub rx_reorder_gap: Duration,
    /// The access point's beacon interval, in time units.
    pub beacon_interval_tu: u16,
    /// The access point's TSF when the station joined.
    pub join_timestamp_tsf: oer_ieee80211_mac::tsf::TsfInstant,
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
    /// Neither beacons nor answers to the station's probes came back.
    BeaconLoss,
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
    /// Group keys the access point replaced through a Group Key Handshake.
    pub group_rekeys: u32,
    /// EAPOL frames after the handshake that were not an authentic Group
    /// Message 1 of the access point, or whose group key did not install.
    pub eapol_rejected: u32,
    /// ADDBA Responses that named no live negotiation.
    pub stale_addba_responses: u32,
    /// Group-addressed robust management frames that did not verify under
    /// the IGTK, or arrived without one.
    pub bip_rejected: u32,
    /// Buffered runs a reorder window released past a missing MPDU after
    /// the gap timeout.
    pub reorder_gap_timeouts: u32,
    /// Out-of-order MPDUs longer than [`PORT_FRAME_CAPACITY`](oer_ieee80211_upper_mac_service::frame::PORT_FRAME_CAPACITY), which the
    /// reorder storage cannot hold.
    pub unbuffered: u32,
}

/// Where one checked data MPDU's payload lies, and whether it came under
/// the pairwise key.
#[derive(Clone, Copy)]
struct MpduPayload {
    offset: usize,
    length: usize,
    pairwise: bool,
}

/// What the station sent.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PortTxCounters {
    /// Frames sent alone.
    pub mpdus: u32,
    /// A-MPDUs sent.
    pub aggregates: u32,
    /// Frames sent inside an A-MPDU.
    pub subframes: u32,
    /// Frames the access point acknowledged, alone or in a BlockAck.
    pub acknowledged: u32,
    /// Frames whose exchange ended without a report.
    pub failed: u32,
}

/// The buffers of a connection: its receive reordering windows and the
/// MPDUs they hold, the frames waiting to be sent and the encoded subframes
/// of the A-MPDU being sent.
///
/// They are most of a station's memory, so the composition places them
/// (in [`PortStationStorage`](super::PortStationStorage)) and each
/// connection borrows them for its association; the connection itself
/// stays small and is built without staging them on the stack.
pub struct PortConnectionBuffers {
    /// The reorder window of each TID with a receive Block Ack agreement,
    /// and copies of the out-of-order MPDUs the windows keep.
    reorder: RxReorder<TIDS>,
    subframes: AmpduSubframes,
}

impl PortConnectionBuffers {
    pub const fn new() -> Self {
        Self {
            reorder: RxReorder::new(),
            subframes: AmpduSubframes::new(),
        }
    }

    /// Empty the buffers for a new association.
    fn reset(&mut self) {
        self.reorder.reset();
    }
}

impl Default for PortConnectionBuffers {
    fn default() -> Self {
        Self::new()
    }
}

/// The connected station's state over the port, with the frame buffers it
/// borrows for its association.
pub struct PortConnection<'b, P: LowerMacBeaconTiming, R> {
    config: PortConnectionConfig,
    keys: Option<PortKeys>,
    packet_number: CcmpTxPacketNumber,
    pairwise_replay: CcmpRxReplayState,
    group_replay: CcmpRxReplayState,
    duplicates: RxDuplicateFilter,
    sa_query: StationSaQuery,
    power: Option<PortPowerSave<P>>,
    /// Reorder windows and their MPDUs, and A-MPDU subframes.
    buffers: &'b mut PortConnectionBuffers,
    tx_counters: PortTxCounters,
    /// An EAPOL frame the access point sent under the pairwise key, awaiting
    /// the Group Key Handshake.
    eapol: Option<OwnedEapolFrame<RSN_HANDSHAKE_EAPOL_CAPACITY>>,
    /// The station's TX Block Ack agreements with the access point.
    tx_block_ack: Option<TxBlockAckOriginator<TX_BLOCK_ACK_MAX_TIDS>>,
    /// The BIP receive state under the association's IGTK.
    bip: Option<BipReceiver>,
    /// Beacons keep the link; when they stop the station probes its
    /// access point before it gives the association up.
    link: StaLinkMonitor,
    /// The SSID and rates of the station's Probe Requests.
    probe: PortLinkProbe,
    /// The association's rate control: the rate of each data frame, and
    /// what its exchanges teach it.
    rate: R,
    counters: PortRxCounters,
}

/// The keys a connection starts with: the pairwise and group keys, the
/// CCMP packet number the handshake left and the BIP receive state.
pub(crate) struct PortConnectionSecurity {
    pub keys: Option<PortKeys>,
    pub packet_number: CcmpTxPacketNumber,
    pub bip: Option<BipReceiver>,
}

/// A connected station's supervision of its link: the monitor of its
/// beacons and the Probe Request it sends when they stop.
pub(crate) struct PortLinkSupervisor {
    pub monitor: StaLinkMonitor,
    pub probe: PortLinkProbe,
}

/// The SSID and supported rates a connected station's Probe Request
/// carries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PortLinkProbe {
    ssid: [u8; 32],
    ssid_len: u8,
    rates: [u8; 12],
    rates_len: u8,
}

impl PortLinkProbe {
    /// `None` for an SSID or a rate set longer than a Probe Request
    /// carries.
    pub(crate) fn new(ssid: &[u8], rates: &[u8]) -> Option<Self> {
        let mut probe = Self {
            ssid: [0; 32],
            ssid_len: u8::try_from(ssid.len()).ok()?,
            rates: [0; 12],
            rates_len: u8::try_from(rates.len()).ok()?,
        };
        probe.ssid.get_mut(..ssid.len())?.copy_from_slice(ssid);
        probe.rates.get_mut(..rates.len())?.copy_from_slice(rates);
        Some(probe)
    }
}

/// What one connection operation borrows from its station.
pub(crate) struct ConnectionContext<'a, 'p, X: PortStationEnv> {
    pub link: &'a mut PortLink<'p, X>,
    pub timer: &'a X::Timer,
    pub sequences: &'a mut StaTxSequenceCounters,
    /// The supplicant of a WPA2 association, which answers the access
    /// point's group rekeys.
    pub supplicant: Option<&'a mut RsnConnectedSupplicant>,
    pub key_unwrap: &'a mut X::KeyUnwrap,
    /// The network's frames to send.
    pub frames: &'a X::Frames,
    /// The destination last served: the next turn goes to the one after it.
    pub cursor: &'a mut Option<[u8; 6]>,
    /// A frame taken from the network that waits for its own exchange: one
    /// of another user priority than the A-MPDU it followed.
    pub frontier: &'a mut Option<PortStationFrame<X>>,
}

impl<'b, P: LowerMacBeaconTiming, R: RateControl> PortConnection<'b, P, R> {
    pub(crate) fn new(
        config: PortConnectionConfig,
        security: PortConnectionSecurity,
        tx_block_ack: Option<TxBlockAckOriginator<TX_BLOCK_ACK_MAX_TIDS>>,
        supervision: PortLinkSupervisor,
        rate: R,
        buffers: &'b mut PortConnectionBuffers,
    ) -> Self {
        let PortConnectionSecurity {
            keys,
            packet_number,
            bip,
        } = security;
        buffers.reset();
        let PortLinkSupervisor {
            monitor: link,
            probe,
        } = supervision;
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
            sa_query: StationSaQuery::new(),
            power: None,
            buffers,
            tx_counters: PortTxCounters::default(),
            eapol: None,
            tx_block_ack,
            bip,
            link,
            probe,
            rate,
            counters: PortRxCounters::default(),
        }
    }

    /// End the connection and return the buffers it borrowed.
    pub(crate) fn into_buffers(self) -> &'b mut PortConnectionBuffers {
        self.buffers
    }

    /// Start the beacon window at the association-complete edge.
    pub(crate) fn arm_link(&mut self, now: Instant) {
        // A window past the representable time is never reached.
        let _ = self.link.arm(now);
    }

    /// The station's supervision of its link.
    pub const fn link(&self) -> &StaLinkMonitor {
        &self.link
    }

    /// The association's rate control.
    pub const fn rate_control(&self) -> &R {
        &self.rate
    }

    /// What the station sent.
    pub const fn tx_counters(&self) -> PortTxCounters {
        self.tx_counters
    }

    /// The station's TX Block Ack agreements, when it originates any.
    pub const fn tx_block_ack(&self) -> Option<&TxBlockAckOriginator<TX_BLOCK_ACK_MAX_TIDS>> {
        self.tx_block_ack.as_ref()
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
        self.buffers.reorder.is_active(self.config.bssid, tid)
    }

    /// The earliest instant the connection needs its owner without input.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        [
            self.sa_query.next_deadline(),
            self.power.as_ref().and_then(PortPowerSave::next_deadline),
            self.tx_block_ack
                .as_ref()
                .and_then(TxBlockAckOriginator::earliest_alarm_deadline),
            self.tx_block_ack
                .as_ref()
                .and_then(TxBlockAckOriginator::next_pending_deadline),
            self.buffers.reorder.next_gap_deadline(),
            self.link.deadline(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// The traffic the power manager weighs: whether the network has a
    /// frame for the station to send.
    fn traffic<X: PortStationEnv>(context: &ConnectionContext<'_, '_, X>) -> PmTraffic {
        PmTraffic {
            tx_pending: context.frontier.is_some()
                || context.frames.next_head_after(None).is_some(),
            connection_pending: false,
        }
    }

    fn power_context<'a, 'p, X: PortStationEnv<Port = P, RateControl = R>>(
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

    /// Start the power manager of the association with `sleep_type`, or
    /// restart it with a new one.
    pub(crate) async fn start_power<X: PortStationEnv<Port = P, RateControl = R>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
        sleep_type: SleepType,
    ) -> Result<(), PortLinkError<PortError<X>>> {
        if let Some(mut power) = self.power.take() {
            let traffic = Self::traffic(context);
            power
                .stop(&mut Self::power_context(
                    context,
                    self.config.bssid,
                    traffic,
                ))
                .await?;
        }
        let mut power = PortPowerSave::new(sleep_type);
        let join_beacon = PmBeacon {
            timestamp_tsf: self.config.join_timestamp_tsf,
            interval_tu: self.config.beacon_interval_tu,
            tim: None,
        };
        let traffic = Self::traffic(context);
        power
            .start(
                &mut Self::power_context(context, self.config.bssid, traffic),
                join_beacon,
            )
            .await?;
        self.power = Some(power);
        Ok(())
    }

    /// Send the network's frames while the station is awake: a run of one
    /// user priority to one destination as an A-MPDU where its TX Block Ack
    /// agreement is operational and the port aggregates at the data rate,
    /// every other frame alone. An exchange that ended without a report
    /// counts as failed; any other error ends the loop.
    async fn drain<X: PortStationEnv<Port = P, RateControl = R>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
    ) -> Result<(), PortLinkError<PortError<X>>> {
        if let Some(power) = &mut self.power {
            power.take_release();
        }
        loop {
            let traffic = Self::traffic(context);
            if !traffic.tx_pending {
                return Ok(());
            }
            if let Some(power) = &mut self.power
                && power
                    .tx_data(&mut Self::power_context(
                        context,
                        self.config.bssid,
                        traffic,
                    ))
                    .await?
            {
                return Ok(());
            }
            let Some(frame) = Self::next_frame(context) else {
                return Ok(());
            };
            let priority = self.priority(&frame);
            let sent = match self.aggregate_run(context, priority, &frame) {
                Some(run) => self.transmit_aggregate(context, priority, frame, run).await,
                None => self
                    .transmit(context, frame.ethernet(), priority)
                    .await
                    .map(|report| (report, 1)),
            };
            let acknowledged_now = match sent {
                Ok((report, frames)) => {
                    // The rate control learns from every data exchange.
                    match report {
                        TxReport::Mpdu(status) => self.rate.observe_mpdu(
                            status.attempts,
                            status.acknowledged == Some(true),
                            status.ack_snr_db,
                        ),
                        TxReport::Ampdu(status) => self.rate.observe_ampdu(
                            context.timer.now(),
                            status.original_subframes,
                            status.block_acknowledged_subframes,
                            status.ack_snr_db,
                        ),
                    }
                    let delivered = match report {
                        TxReport::Mpdu(status) => u32::from(status.acknowledged == Some(true)),
                        TxReport::Ampdu(status) => u32::from(status.block_acknowledged_subframes),
                    };
                    if frames == 1 {
                        self.tx_counters.mpdus = self.tx_counters.mpdus.saturating_add(1);
                    } else {
                        self.tx_counters.aggregates = self.tx_counters.aggregates.saturating_add(1);
                        self.tx_counters.subframes =
                            self.tx_counters.subframes.saturating_add(frames as u32);
                    }
                    self.tx_counters.acknowledged =
                        self.tx_counters.acknowledged.saturating_add(delivered);
                    acknowledged(&report)
                }
                Err(PortLinkError::Tx(UpperMacTxError::CompletionLost { .. })) => {
                    self.tx_counters.failed = self.tx_counters.failed.saturating_add(1);
                    false
                }
                Err(error) => return Err(error),
            };
            let traffic = Self::traffic(context);
            if let Some(power) = &mut self.power {
                power
                    .tx_data_done(
                        &mut Self::power_context(context, self.config.bssid, traffic),
                        acknowledged_now,
                    )
                    .await?;
            }
        }
    }

    /// The next frame to send: one an A-MPDU left, then the head of the
    /// destination after the last one served.
    fn next_frame<X: PortStationEnv>(
        context: &mut ConnectionContext<'_, '_, X>,
    ) -> Option<PortStationFrame<X>> {
        if let Some(frame) = context.frontier.take() {
            return Some(frame);
        }
        let (destination, _) = context.frames.next_head_after(*context.cursor)?;
        *context.cursor = Some(destination);
        context.frames.try_take_for(destination)
    }

    /// The user priority of `ethernet`, as its DSCP or VLAN tag gives it,
    /// where the access point serves QoS; zero otherwise.
    fn priority(&self, frame: &impl SoftwareTxFrame) -> WmmUserPriority {
        if self.config.peer_qos {
            classify_ethernet_wmm(frame.ethernet()).user_priority
        } else {
            WmmUserPriority::UP0
        }
    }

    /// The TXOP limit of `priority`'s access category in microseconds;
    /// `None` without one.
    fn txop_limit_micros(&self, priority: WmmUserPriority) -> Option<u32> {
        let units = self
            .config
            .edca?
            .access_category(priority.access_category())
            .txop_limit_units_32_us;
        (units != 0).then_some(u32::from(units) * 32)
    }

    /// The A-MPDU run that `head` of `priority` begins, its head and the
    /// next frame the network queued for its destination admitted: where its
    /// operational agreement's window, the port's and the peer's limits and
    /// the TXOP limit admit two frames. `None` sends `head` alone.
    fn aggregate_run<X: PortStationEnv<Port = P, RateControl = R>>(
        &self,
        context: &ConnectionContext<'_, '_, X>,
        priority: WmmUserPriority,
        head: &PortStationFrame<X>,
    ) -> Option<AmpduRun> {
        let agreement = self.tx_block_ack.as_ref()?.operational(priority.value())?;
        let port = context.link.ampdu_capabilities()?;
        self.keys?;
        if !self.config.peer_qos {
            return None;
        }
        let mut run = AmpduLimits {
            window: agreement.window,
            port,
            peer_ampdu_parameters: self.config.peer.ht_ampdu_parameters,
            txop_limit_micros: self.txop_limit_micros(priority),
            rate: self.rate.ampdu_rate(),
        }
        .begin()?;
        let destination = destination(head.ethernet())?;
        let next = context.frames.head_for(destination)?;
        (run.admit(head.ethernet().len()) && run.admit(next.ethernet_bytes)).then_some(run)
    }

    /// Send `head` and the frames of `priority` the network queued for its
    /// destination that `run` admits after it, taken one at a time, as one
    /// A-MPDU under the pairwise key; the report and the frames it carried.
    /// A frame of another priority waits as the frontier for its own
    /// exchange; one the run does not admit stays in the network's queue.
    async fn transmit_aggregate<X: PortStationEnv<Port = P, RateControl = R>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
        priority: WmmUserPriority,
        head: PortStationFrame<X>,
        mut run: AmpduRun,
    ) -> Result<(TxReport, usize), PortLinkError<PortError<X>>> {
        let tid = priority.value();
        let destination = destination(head.ethernet()).ok_or(PortLinkError::MissingState)?;
        let mut first_sequence = None;
        self.buffers.subframes.clear();
        // The run admitted the head and the next frame already.
        let mut admitted = 1_usize;
        let mut frame = Some(head);
        while let Some(owner) = frame.take() {
            let sequence = context.sequences.take_qos(tid).ok_or(PortLinkError::Frame(
                StationFrameError::UserPriorityOutOfRange,
            ))?;
            first_sequence.get_or_insert(sequence);
            let mut out = [0_u8; PORT_MPDU_CAPACITY];
            let (length, selector) =
                self.encode_data(context, owner.ethernet(), priority, sequence, &mut out)?;
            // The subframe is encoded: the network's owner goes back.
            drop(owner);
            self.buffers.subframes.push(|buffer| {
                buffer[..length].copy_from_slice(&out[..length]);
                Ok::<_, PortLinkError<PortError<X>>>((length, selector))
            })?;
            if self.buffers.subframes.len() == PORT_AMPDU_SUBFRAMES {
                break;
            }
            let admitted_next = admitted == 1
                || context
                    .frames
                    .head_for(destination)
                    .is_some_and(|next| run.admit(next.ethernet_bytes));
            if !admitted_next {
                break;
            }
            admitted += 1;
            let Some(next) = context.frames.try_take_for(destination) else {
                break;
            };
            if self.priority(&next) == priority {
                frame = Some(next);
            } else {
                *context.frontier = Some(next);
            }
        }
        let run = self.buffers.subframes.len();
        let first_sequence = first_sequence.ok_or(PortLinkError::MissingState)?;
        let committed_at = context.link.port().now().map_err(PortLinkError::Port)?;
        let ampdu = self
            .buffers
            .subframes
            .request(tid, first_sequence, committed_at)
            .ok_or(PortLinkError::MissingState)?;
        let config = *context.link.config();
        let request = TxRequest {
            access_category: priority.access_category(),
            initial_rate: self.rate.ampdu_rate(),
            receiver: TxReceiver::Individual,
            power: config.power,
            coex: config.coex,
            mpdu_retry_limit: config.retry_limit,
            body: TxBody::Ampdu(ampdu),
        };
        let mut slices = [&[][..]; PORT_AMPDU_SUBFRAMES];
        let frames = self.buffers.subframes.frames(
            &mut slices,
            (self.config.peer.ht_ampdu_parameters >> 2) & 0x07,
        );
        context
            .link
            .transmit_ampdu(frames, request)
            .await
            .map(|report| (report, run))
    }

    async fn transmit<X: PortStationEnv<Port = P, RateControl = R>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
        ethernet: &[u8],
        priority: WmmUserPriority,
    ) -> Result<TxReport, PortLinkError<PortError<X>>> {
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
        let mut frame = [0_u8; PORT_MPDU_CAPACITY];
        let (length, key) =
            self.encode_data(context, ethernet, priority, sequence_number, &mut frame)?;
        context
            .link
            .transmit(
                &frame[..length],
                key,
                priority.access_category(),
                self.rate.mpdu_rate(),
            )
            .await
    }

    /// Encode `ethernet` as a data MPDU to the access point with
    /// `sequence_number`: under the pairwise key once the keys are
    /// installed, with the next CCMP packet number.
    fn encode_data<X: PortStationEnv<Port = P, RateControl = R>>(
        &mut self,
        context: &ConnectionContext<'_, '_, X>,
        ethernet: &[u8],
        priority: WmmUserPriority,
        sequence_number: SequenceNumber,
        frame: &mut [u8],
    ) -> Result<(usize, KeySelector), PortLinkError<PortError<X>>> {
        let config = *context.link.config();
        let destination: [u8; 6] = ethernet[..6].try_into().expect("six octets");
        let ether_type = u16::from_be_bytes([ethernet[12], ethernet[13]]);
        let payload = &ethernet[ETHERNET_HEADER_LEN..];
        Ok(match self.keys {
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
                .encode(frame)
                .map_err(PortLinkError::Frame)?;
                (length, KeySelector::Key(keys.pairwise))
            }
            None => (
                encode_open_data(
                    frame,
                    config.address,
                    self.config.bssid,
                    ethernet,
                    priority.value(),
                    self.config.peer_qos,
                    sequence_number,
                )?,
                KeySelector::Plaintext,
            ),
        })
    }

    /// Process inputs until `deadline`: received frames, TBTTs, the SA
    /// Query, power-save and reorder gap timers. Every delivered Ethernet frame goes to
    /// `deliver`. `Some` when the association ended.
    pub(crate) async fn run_until<X: PortStationEnv<Port = P, RateControl = R>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
        deadline: Instant,
        deliver: &mut impl FnMut(PortMsdu<'_, P::RxBuffer>),
    ) -> Result<Option<PortDisconnect>, PortLinkError<PortError<X>>> {
        loop {
            if let Some(disconnect) = self.expire(context).await? {
                return Ok(Some(disconnect));
            }
            self.expire_reorder_gaps(context.timer.now(), deliver);
            self.drain(context).await?;
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
    async fn expire<X: PortStationEnv<Port = P, RateControl = R>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
    ) -> Result<Option<PortDisconnect>, PortLinkError<PortError<X>>> {
        let now = context.timer.now();
        match self.sa_query.step(now) {
            SaQueryStep::Idle => {}
            SaQueryStep::Request(transaction) => {
                self.send_sa_query(context, SaQuery::Request { transaction })
                    .await?;
            }
            SaQueryStep::TimedOut => return Ok(Some(PortDisconnect::SaQueryTimeout)),
        }
        match self.link.due(now) {
            Some(StaLinkAction::Probe { directed }) => {
                self.send_link_probe(context, directed).await?;
                // A window past the representable time is never reached.
                let _ = self.link.probe_sent(context.timer.now());
            }
            Some(StaLinkAction::Lost) => return Ok(Some(PortDisconnect::BeaconLoss)),
            None => {}
        }
        let traffic = Self::traffic(context);
        if let Some(power) = &mut self.power {
            power
                .expire(&mut Self::power_context(
                    context,
                    self.config.bssid,
                    traffic,
                ))
                .await?;
        }
        self.negotiate_tx_block_ack(context, now).await?;
        Ok(None)
    }

    /// Expire overdue ADDBA negotiations and send the next one that waits,
    /// unless the station dozes.
    async fn negotiate_tx_block_ack<X: PortStationEnv<Port = P, RateControl = R>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
        now: Instant,
    ) -> Result<(), PortLinkError<PortError<X>>> {
        let Some(originator) = self.tx_block_ack.as_mut() else {
            return Ok(());
        };
        while originator.expire_next(now).is_some() {}
        if self.power.as_ref().is_some_and(|power| !power.awake()) {
            return Ok(());
        }
        let Some(tid) = originator.take_pending(now) else {
            return Ok(());
        };
        let starting_sequence = context
            .sequences
            .peek_qos(tid)
            .ok_or(PortLinkError::MissingState)?;
        let request = originator
            .begin(tid, starting_sequence, now)
            .map_err(|_| PortLinkError::MissingState)?;
        let report = self
            .send_management(context, StaManagementSubtype::Action, &request.body)
            .await?;
        if !acknowledged(&report)
            && let Some(originator) = self.tx_block_ack.as_mut()
        {
            originator.transmit_failed(tid, now);
        }
        Ok(())
    }

    async fn input<X: PortStationEnv<Port = P, RateControl = R>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
        input: PortInput<P::RxBuffer>,
        deliver: &mut impl FnMut(PortMsdu<'_, P::RxBuffer>),
    ) -> Result<Option<PortDisconnect>, PortLinkError<PortError<X>>> {
        let frame = match input {
            PortInput::Frame(frame) => frame,
            PortInput::Tbtt(_) => {
                let traffic = Self::traffic(context);
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

    async fn management<X: PortStationEnv<Port = P, RateControl = R>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
        frame: &PortFrame<P::RxBuffer>,
    ) -> Result<Option<PortDisconnect>, PortLinkError<PortError<X>>> {
        let bytes = frame.bytes();
        let address = context.link.config().address;
        let protected = wire::is_protected(bytes);
        if protected && !decrypted(frame.meta()) {
            self.counters.undecrypted = self.counters.undecrypted.saturating_add(1);
            return Ok(None);
        }
        if self.config.management_protection
            && wire::address1(bytes).is_some_and(wire::is_group)
            && robust_management(bytes)
        {
            return Ok(self.group_management(bytes));
        }
        match wire::subtype(bytes) {
            BEACON_SUBTYPE => {
                if let Ok(beacon) =
                    parse_sta_beacon(bytes, self.config.bssid, self.config.association_id.get())
                {
                    let _ = self.link.observe_beacon(context.timer.now(), beacon);
                    let traffic = Self::traffic(context);
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
            // The access point answered a probe: the link is reachable.
            PROBE_RESPONSE_SUBTYPE
                if wire::address1(bytes) == Some(address)
                    && wire::address2(bytes) == Some(self.config.bssid) =>
            {
                let _ = self.link.observe_probe_response(context.timer.now());
                Ok(None)
            }
            DISASSOCIATION_SUBTYPE | DEAUTHENTICATION_SUBTYPE => {
                let Some(disconnect) = parse_disconnect(bytes, address, self.config.bssid) else {
                    return Ok(None);
                };
                if self.config.management_protection && !protected {
                    // An unprotected disconnect may be forged: ask the access
                    // point whether the association holds.
                    let now = context.timer.now();
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

    /// A group-addressed robust management frame of an association that
    /// protects its management frames: it counts only when it verifies
    /// under the IGTK, and then a Deauthentication or Disassociation ends the
    /// association; a group Action carries nothing the station answers.
    fn group_management(&mut self, bytes: &[u8]) -> Option<PortDisconnect> {
        if !self
            .bip
            .as_mut()
            .is_some_and(|bip| bip.verify(bytes).is_ok())
        {
            self.counters.bip_rejected = self.counters.bip_rejected.saturating_add(1);
            return None;
        }
        let disconnect = parse_sta_disconnect(bytes, [0xff; 6], self.config.bssid)?;
        Some(match disconnect.kind {
            StaDisconnectKind::Deauthentication => PortDisconnect::Deauthenticated {
                reason_code: disconnect.reason_code,
            },
            StaDisconnectKind::Disassociation => PortDisconnect::Disassociated {
                reason_code: disconnect.reason_code,
            },
        })
    }

    async fn block_ack_action<X: PortStationEnv<Port = P, RateControl = R>>(
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
                    && !self.buffers.reorder.is_active(self.config.bssid, tid);
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
                let installed = accepted
                    && self.buffers.reorder.accept(
                        self.config.bssid,
                        tid,
                        starting_sequence,
                        window,
                    )
                    && if context
                        .link
                        .apply(LowerMacSetting::AddRxBlockAck(agreement))
                        .is_ok()
                    {
                        true
                    } else {
                        self.buffers.reorder.stop(self.config.bssid, tid);
                        false
                    };
                let mut response = [0_u8; ADDBA_ACTION_BODY_LEN];
                if installed {
                    write_successful_addba_response(&mut response, dialog_token, tid, window)
                } else {
                    write_declined_addba_response(&mut response, dialog_token, tid & 0x0f, window)
                }
                .map_err(|_| PortLinkError::MissingState)?;
                self.send_action(context, &response).await
            }
            // The access point ends one of the station's TX agreements.
            Some(BlockAckAction::Delba {
                tid,
                initiator: false,
                ..
            }) => {
                if let Some(originator) = self.tx_block_ack.as_mut() {
                    originator.stop(tid);
                }
                Ok(())
            }
            Some(BlockAckAction::Delba { tid, .. }) => {
                if self.buffers.reorder.stop(self.config.bssid, tid) {
                    context.link.apply(LowerMacSetting::RemoveRxBlockAck {
                        vif: context.link.config().vif,
                        peer: self.config.bssid,
                        tid,
                    })?;
                }
                Ok(())
            }
            Some(action @ BlockAckAction::AddbaResponse { .. }) => {
                if let Some(originator) = self.tx_block_ack.as_mut()
                    && let Ok(TxBlockAckResponseDisposition::StaleDialogToken(_)) =
                        originator.on_response_action(action)
                {
                    self.counters.stale_addba_responses =
                        self.counters.stale_addba_responses.saturating_add(1);
                }
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// A BlockAckReq moves its agreement's window.
    fn control(&mut self, bytes: &[u8], deliver: &mut impl FnMut(PortMsdu<'_, P::RxBuffer>)) {
        if wire::subtype(bytes) != BLOCK_ACK_REQUEST_SUBTYPE || bytes.len() < 20 {
            return;
        }
        let tid = bytes[17] >> 4;
        let starting_sequence =
            SequenceNumber::from_sequence_control(u16::from_le_bytes([bytes[18], bytes[19]]));
        if let Some(release) =
            self.buffers
                .reorder
                .move_window(self.config.bssid, tid, starting_sequence)
        {
            self.release(&release, None, deliver);
        }
    }

    async fn data<X: PortStationEnv<Port = P, RateControl = R>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
        frame: PortFrame<P::RxBuffer>,
        deliver: &mut impl FnMut(PortMsdu<'_, P::RxBuffer>),
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
            if self.keys.is_none() || !decrypted(frame.meta()) {
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
        let traffic = Self::traffic(context);
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
            _ => self.deliver_mpdu(frame, deliver),
        }
        if let Some(eapol) = self.eapol.take() {
            self.group_rekey(context, eapol).await?;
        }
        Ok(())
    }

    /// Answer one EAPOL frame of the access point after the handshake: a
    /// Group Message 1 replaces the group key through the port and is
    /// answered with Group Message 2 under the pairwise key; a repeat of the
    /// last one is answered again without a key change. Anything else, and a
    /// group key that does not install, counts as rejected.
    async fn group_rekey<X: PortStationEnv<Port = P, RateControl = R>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
        eapol: OwnedEapolFrame<RSN_HANDSHAKE_EAPOL_CAPACITY>,
    ) -> Result<(), PortLinkError<PortError<X>>> {
        let (Some(supplicant), Some(keys)) = (context.supplicant.as_deref_mut(), self.keys) else {
            self.counters.eapol_rejected = self.counters.eapol_rejected.saturating_add(1);
            return Ok(());
        };
        let Ok(step) = process_group_message1(supplicant, eapol, context.key_unwrap).await else {
            self.counters.eapol_rejected = self.counters.eapol_rejected.saturating_add(1);
            return Ok(());
        };
        let response = match step {
            RsnGroupMessage1Step::Retransmit(response) => response,
            RsnGroupMessage1Step::Install(request) => {
                let RsnKeyKind::Group { key_id, .. } = request.group().kind() else {
                    return Err(PortLinkError::MissingState);
                };
                let receive_sequence = *request.group().receive_sequence();
                if let Some(igtk) = request.igtk() {
                    match &mut self.bip {
                        Some(bip) => bip.rekey(igtk),
                        None => self.bip = Some(BipReceiver::new(igtk)),
                    }
                }
                let installed = context.link.port().install_key(KeyInstall {
                    vif: context.link.config().vif,
                    cipher: Cipher::Ccmp128,
                    scope: KeyScope::Group { key_id },
                    key: request.group().key().as_bytes(),
                });
                let group = match &installed {
                    Ok(Ok(group)) => Some(*group),
                    _ => None,
                };
                let completed = supplicant.complete_group_key_install(request, group.is_some());
                let group = match (installed, completed) {
                    (Err(error), _) => return Err(PortLinkError::Port(error)),
                    (Ok(Ok(group)), Ok(response)) => (group, response),
                    _ => {
                        self.counters.eapol_rejected =
                            self.counters.eapol_rejected.saturating_add(1);
                        return Ok(());
                    }
                };
                let (group, response) = group;
                if group != keys.group {
                    context.link.apply(LowerMacSetting::RemoveKey(keys.group))?;
                }
                self.keys = Some(PortKeys {
                    group,
                    group_key_id: key_id,
                    group_receive_sequence: receive_sequence,
                    ..keys
                });
                self.group_replay =
                    CcmpRxReplayState::from_receive_sequence(receive_sequence).unwrap_or_default();
                self.counters.group_rekeys = self.counters.group_rekeys.saturating_add(1);
                response
            }
        };
        send_protected_eapol(
            context.link,
            self.config.bssid,
            context.sequences.take_non_qos(),
            &mut self.packet_number,
            keys.pairwise,
            response.as_bytes(),
        )
        .await
    }

    /// Release the buffered run of every window whose gap timed out, then
    /// time the gap of every window that buffers an MPDU from the first one
    /// it retained (a window that buffers nothing has no gap).
    fn expire_reorder_gaps(
        &mut self,
        now: Instant,
        deliver: &mut impl FnMut(PortMsdu<'_, P::RxBuffer>),
    ) {
        while let Some((_, release)) = self.buffers.reorder.expire_due_gap(now) {
            self.counters.reorder_gap_timeouts =
                self.counters.reorder_gap_timeouts.saturating_add(1);
            self.release(&release, None, deliver);
        }
        self.buffers
            .reorder
            .arm_gaps(now, self.config.rx_reorder_gap);
    }

    /// Offer one MPDU of a Block Ack agreement to its reorder window. An
    /// MPDU the window releases at once is delivered from the port's
    /// buffer; only one it keeps is copied into the storage, whose oldest
    /// run of the same window goes first when every slot is taken.
    fn reorder_mpdu(
        &mut self,
        tid: u8,
        frame: PortFrame<P::RxBuffer>,
        deliver: &mut impl FnMut(PortMsdu<'_, P::RxBuffer>),
    ) {
        let bssid = self.config.bssid;
        let mut make_room = true;
        loop {
            match self
                .buffers
                .reorder
                .offer(bssid, tid, frame.bytes(), make_room)
            {
                Offer::MakeRoom(release) => {
                    self.release(&release, None, deliver);
                    make_room = false;
                }
                Offer::Released { release, current } => {
                    self.release(&release, current.then_some(frame), deliver);
                    return;
                }
                Offer::Duplicate => {
                    self.counters.duplicates = self.counters.duplicates.saturating_add(1);
                    return;
                }
                Offer::Behind => {
                    self.counters.behind_window = self.counters.behind_window.saturating_add(1);
                    return;
                }
                Offer::Unbuffered => {
                    self.counters.unbuffered = self.counters.unbuffered.saturating_add(1);
                    return;
                }
                Offer::NoAgreement | Offer::Dropped => return,
            }
        }
    }

    /// Deliver a released run in order: stored MPDUs from the storage, the
    /// current one from the port's buffer.
    fn release(
        &mut self,
        release: &ReorderRelease,
        mut current: Option<PortFrame<P::RxBuffer>>,
        deliver: &mut impl FnMut(PortMsdu<'_, P::RxBuffer>),
    ) {
        for mpdu in release.iter() {
            if mpdu.slot == CURRENT_SLOT {
                if let Some(frame) = current.take() {
                    self.deliver_mpdu(frame, deliver);
                }
            } else if let Some(stored) = self.buffers.reorder.take(mpdu.slot) {
                self.deliver_stored(stored.bytes(), deliver);
            }
        }
    }

    /// Check one in-order MPDU's packet number and hand its MSDUs on: the
    /// only MSDU of an MPDU in the port's buffer, the MSDUs of an A-MSDU
    /// as parts.
    fn deliver_mpdu(
        &mut self,
        frame: PortFrame<P::RxBuffer>,
        deliver: &mut impl FnMut(PortMsdu<'_, P::RxBuffer>),
    ) {
        let bytes = frame.bytes();
        let Some(payload) = self.checked_payload(bytes) else {
            return;
        };
        match plan_data_decapsulation(
            DataInterfaceRole::Station,
            bytes,
            payload.offset,
            payload.length,
        ) {
            Ok(plan) => {
                let range = plan.payload_offset..plan.payload_offset + plan.payload_length;
                let Some(body) = bytes.get(range.clone()) else {
                    self.counters.malformed = self.counters.malformed.saturating_add(1);
                    return;
                };
                if payload.pairwise && plan.ether_type == EAPOL_ETHER_TYPE {
                    self.keep_eapol(body);
                    return;
                }
                self.counters.delivered = self.counters.delivered.saturating_add(1);
                deliver(PortMsdu::Buffer {
                    buffer: frame.into_buffer(),
                    destination: plan.destination,
                    source: plan.source,
                    ether_type: plan.ether_type,
                    payload: range,
                });
            }
            Err(DataDecapError::AmsduUnsupported) => self.deliver_parts(bytes, payload, deliver),
            Err(_) => self.counters.malformed = self.counters.malformed.saturating_add(1),
        }
    }

    /// Check one MPDU a reorder window kept and hand its MSDUs on as
    /// parts.
    fn deliver_stored(
        &mut self,
        bytes: &[u8],
        deliver: &mut impl FnMut(PortMsdu<'_, P::RxBuffer>),
    ) {
        if let Some(payload) = self.checked_payload(bytes) {
            self.deliver_parts(bytes, payload, deliver);
        }
    }

    /// The payload of one in-order data MPDU after its CCMP replay check;
    /// `None` for a replay or a malformed frame, which it counts.
    fn checked_payload(&mut self, bytes: &[u8]) -> Option<MpduPayload> {
        let header = wire::data_header_len(bytes);
        let protected = wire::is_protected(bytes);
        if protected {
            let Some(ccmp) =
                wire::ccmp_header(bytes, header).and_then(|ccmp| CcmpHeader::parse(ccmp).ok())
            else {
                self.counters.malformed = self.counters.malformed.saturating_add(1);
                return None;
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
                return None;
            }
        }
        let offset = header + if protected { CCMP_HEADER_LEN } else { 0 };
        let Some(length) = bytes.len().checked_sub(offset) else {
            self.counters.malformed = self.counters.malformed.saturating_add(1);
            return None;
        };
        Some(MpduPayload {
            offset,
            length,
            pairwise: protected && !wire::address1(bytes).is_some_and(wire::is_group),
        })
    }

    /// Hand every MSDU of one checked MPDU on as parts.
    fn deliver_parts(
        &mut self,
        bytes: &[u8],
        payload: MpduPayload,
        deliver: &mut impl FnMut(PortMsdu<'_, P::RxBuffer>),
    ) {
        let Ok(frames) = decapsulate_data_frames(
            DataInterfaceRole::Station,
            bytes,
            payload.offset,
            payload.length,
        ) else {
            self.counters.malformed = self.counters.malformed.saturating_add(1);
            return;
        };
        for parts in frames {
            match parts {
                Ok(parts) if payload.pairwise && parts.ether_type == EAPOL_ETHER_TYPE => {
                    self.keep_eapol(parts.payload);
                }
                Ok(parts) => {
                    self.counters.delivered = self.counters.delivered.saturating_add(1);
                    deliver(PortMsdu::Parts(parts));
                }
                Err(_) => {
                    self.counters.malformed = self.counters.malformed.saturating_add(1);
                    return;
                }
            }
        }
    }

    /// The access point's EAPOL after the handshake belongs to the Group Key
    /// Handshake, never to the caller.
    fn keep_eapol(&mut self, body: &[u8]) {
        match OwnedEapolFrame::try_copy(RsnInterface::Station, self.config.bssid, body) {
            Ok(eapol) => self.eapol = Some(eapol),
            Err(_) => {
                self.counters.eapol_rejected = self.counters.eapol_rejected.saturating_add(1);
            }
        }
    }

    /// A Probe Request to the access point, or broadcast, carrying the
    /// station's SSID and rates, as the vendor's `send_ap_probe` sends.
    async fn send_link_probe<X: PortStationEnv<Port = P, RateControl = R>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
        directed: bool,
    ) -> Result<(), PortLinkError<PortError<X>>> {
        let config = *context.link.config();
        let destination = if directed {
            self.config.bssid
        } else {
            BROADCAST_ADDRESS
        };
        let probe = self.probe;
        let mut frame = [0_u8; 128];
        let length = ProbeRequest {
            destination,
            source: config.address,
            bssid: destination,
            sequence_number: context.sequences.take_non_qos(),
            ssid: &probe.ssid[..usize::from(probe.ssid_len)],
            supported_rates: &probe.rates[..usize::from(probe.rates_len)],
        }
        .encode(&mut frame)
        .map_err(|_| PortLinkError::MissingState)?;
        context
            .link
            .transmit_connection_frame(
                PortConnectionFrame::ProbeRequest,
                &frame[..length],
                KeySelector::Plaintext,
                WmmAccessCategory::Voice,
                config.management_rate,
            )
            .await
            .map(|_| ())
    }

    async fn send_sa_query<X: PortStationEnv<Port = P, RateControl = R>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
        query: SaQuery,
    ) -> Result<(), PortLinkError<PortError<X>>> {
        self.send_action(context, &query.encode()).await
    }

    /// Send one Action frame, protected under management frame protection.
    async fn send_action<X: PortStationEnv<Port = P, RateControl = R>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
        body: &[u8],
    ) -> Result<(), PortLinkError<PortError<X>>> {
        self.send_management(context, StaManagementSubtype::Action, body)
            .await
            .map(|_| ())
    }

    async fn send_management<X: PortStationEnv<Port = P, RateControl = R>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
        subtype: StaManagementSubtype,
        body: &[u8],
    ) -> Result<TxReport, PortLinkError<PortError<X>>> {
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
    }

    /// Leave the association: send a Deauthentication while the station
    /// still owns it, stop power save, end the Block Ack agreements and
    /// remove the keys.
    pub(crate) async fn leave<X: PortStationEnv<Port = P, RateControl = R>>(
        &mut self,
        context: &mut ConnectionContext<'_, '_, X>,
        send_deauthentication: bool,
    ) -> Result<(), PortLinkError<PortError<X>>> {
        let traffic = Self::traffic(context);
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
        // A frame taken from the network goes back with the association.
        *context.frontier = None;
        for tid in 0..TIDS as u8 {
            if self.buffers.reorder.stop(self.config.bssid, tid) {
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
fn decrypted(meta: RxMeta) -> bool {
    matches!(
        meta.crypto,
        RxEvidence::HardwareObserved(RxCryptoStatus::DecryptedAndIntegrityVerified)
            | RxEvidence::ProtocolValidated(RxCryptoStatus::DecryptedAndIntegrityVerified)
    )
}

/// A Deauthentication or Disassociation of `bssid` to `local`, its CCMP
/// header skipped when protected.
/// A robust management frame: a Deauthentication, a Disassociation or an
/// Action of a robust category.
fn robust_management(frame: &[u8]) -> bool {
    match wire::subtype(frame) {
        DISASSOCIATION_SUBTYPE | DEAUTHENTICATION_SUBTYPE => true,
        ACTION_SUBTYPE => wire::management_body(frame)
            .and_then(|body| body.first().copied())
            .is_some_and(is_robust_action_category),
        _ => false,
    }
}

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

/// The Ethernet destination of `ethernet`.
fn destination(ethernet: &[u8]) -> Option<[u8; 6]> {
    ethernet.get(..6)?.try_into().ok()
}
