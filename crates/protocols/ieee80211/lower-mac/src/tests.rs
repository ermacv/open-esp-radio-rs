//! An in-memory backend proving the port and its extensions are
//! implementable from portable values alone, and the contract behaviour a
//! caller relies on.

use core::{
    cell::RefCell,
    future::Future,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};
use std::{collections::VecDeque, vec, vec::Vec};

use oer_ieee80211_mac::{
    phy::{
        DsssPreamble, FecCoding, HeGiLtf, HeMcs, HeRate, HtMcs, LegacyRate, PpduBandwidth,
        SpatialStreams,
    },
    qos::WmmAccessCategory,
    sequence::SequenceNumber,
};
use oer_time::RadioInstant;

use crate::*;

const EVENT_CAPACITY: usize = 4;
const BUFFERS: usize = 3;
const AMPDU_BUFFERS: usize = 1;
const MAX_MPDU: usize = 2304;

enum ModelEvent {
    Received(Vec<u8>, RxMeta),
    Tx(TxCompletion),
    Tbtt(TbttEvent),
    Lifecycle(LifecycleEvent),
}

/// A buffer of the model's bounded pool.
#[derive(Debug, Eq, PartialEq)]
struct ModelBuffer(Vec<u8>);

impl TxBuffer for ModelBuffer {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn frame_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}

#[derive(Debug, Default, Eq, PartialEq)]
struct ModelAmpdu(Vec<Vec<u8>>);

impl AmpduBuffer for ModelAmpdu {
    fn push_mpdu(&mut self, len: usize) -> Option<&mut [u8]> {
        if len > MAX_MPDU || self.0.len() == usize::from(AMPDU.max_subframes) {
            return None;
        }
        self.0.push(vec![0; len]);
        self.0.last_mut().map(Vec::as_mut_slice)
    }

    fn subframes(&self) -> usize {
        self.0.len()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    /// The gate is closed: admitted, not published.
    Held,
    Published,
}

struct InFlight {
    completion: TxCompletion,
    queue: u8,
    phase: Phase,
    ampdu: bool,
}

#[derive(Default)]
struct State {
    enabled: bool,
    channel: Option<Channel>,
    vifs: [Option<VifConfig>; 2],
    tsf: [Tsf; 2],
    keys: [bool; 4],
    rx_block_ack: Vec<RxBlockAckAgreement>,
    gate_closed: bool,
    monitor: bool,
    in_flight: Vec<InFlight>,
    buffers_lent: usize,
    ampdu_lent: usize,
    events: VecDeque<ModelEvent>,
    lost: bool,
    now: u64,
}

impl State {
    fn push(&mut self, event: ModelEvent) {
        if self.events.len() == EVENT_CAPACITY {
            self.lost = true;
        } else {
            self.events.push_back(event);
        }
    }

    fn vif(&self, vif: VifId) -> Option<VifConfig> {
        self.vifs.get(usize::from(vif.0)).copied().flatten()
    }

    /// End an attempt and release its buffer.
    fn finish(&mut self, index: usize, status: TxStatus) {
        let attempt = self.in_flight.remove(index);
        let mut completion = attempt.completion;
        if status != TxStatus::Success {
            completion = TxCompletion {
                status,
                ack_snr_db: None,
                block_ack: None,
                ..completion
            };
        }
        if attempt.ampdu {
            self.ampdu_lent -= 1;
        } else {
            self.buffers_lent -= 1;
        }
        self.push(ModelEvent::Tx(completion));
    }
}

/// Keeps every admitted attempt in flight until the test completes it.
#[derive(Default)]
struct Model {
    state: RefCell<State>,
}

#[derive(Debug, Eq, PartialEq)]
enum Poisoned {}

const CAPABILITIES: LowerMacCapabilities = LowerMacCapabilities {
    bands: BandSet::GHZ2_4.union(BandSet::GHZ5),
    widths: WidthSet::MHZ20.union(WidthSet::MHZ40),
    rates: RateSupport {
        dsss_cck: true,
        ofdm: true,
        ht_max_mcs: HtMcs::new(7),
        he_max_mcs: HeMcs::new(9),
        he_max_bandwidth_mhz: 20,
        he_dcm: true,
        he_ldpc: true,
        spatial_streams: 1,
    },
    services: HardwareServices::FCS
        .union(HardwareServices::IMMEDIATE_ACK)
        .union(HardwareServices::BACKOFF_COUNTDOWN)
        .union(HardwareServices::CIPHER_TRANSFORM)
        .union(HardwareServices::RX_BLOCK_ACK_MATCHING)
        .union(HardwareServices::TX_BLOCK_ACK_CAPTURE),
    vifs: 2,
    tx_queues: 4,
    max_mpdu_length: MAX_MPDU as u16,
    max_backoff_slots: 1023,
    tx_power_ceiling_min_dbm: Some(0),
    coex_priorities: CoexPrioritySet::only(CoexPriority::Normal)
        .union(CoexPrioritySet::only(CoexPriority::Elevated)),
    individual_no_ack: PhyFormatSet::NON_HT,
    station_receive_filters: ReceiveFilter::BSS_MEMBER.union(ReceiveFilter::OTHER_BSS_MANAGEMENT),
    access_point_receive_filters: ReceiveFilter::BSS_MEMBER,
    key_slots: 4,
    rx_block_ack_agreements: 2,
    rx_block_ack_max_tid: 7,
    rx_block_ack_max_window: 64,
};

const AMPDU: AmpduCapabilities = AmpduCapabilities {
    max_subframes: 32,
    formats: PhyFormatSet::HT.union(PhyFormatSet::HE),
    max_length: 65_535,
};

impl Model {
    /// Deliver `frame` when an interface's filter or monitor reception
    /// admits it, as a backend narrowing a hardware superset does.
    fn receive(&self, frame: &[u8], meta: RxMeta) {
        let mut state = self.state.borrow_mut();
        let admitted = state.monitor || state.vifs.iter().flatten().any(|vif| vif.admits(frame));
        if state.enabled && admitted {
            state.push(ModelEvent::Received(frame.to_vec(), meta));
        }
    }

    fn fire_tbtt(&self, vif: VifId) {
        let mut state = self.state.borrow_mut();
        let tsf = state.tsf[usize::from(vif.0)];
        state.push(ModelEvent::Tbtt(TbttEvent { vif, tsf }));
    }

    /// Complete the published attempt of `queue` with `status`.
    fn complete(&self, queue: u8, status: TxStatus) {
        let mut state = self.state.borrow_mut();
        let index = state
            .in_flight
            .iter()
            .position(|attempt| attempt.queue == queue && attempt.phase == Phase::Published)
            .expect("a published attempt on the queue");
        state.finish(index, status);
    }

    fn admit<P>(
        &self,
        attempt: &TxAttempt<P>,
        block_ack: Option<BlockAckReport>,
    ) -> Result<(), SubmitError> {
        let mut state = self.state.borrow_mut();
        if !state.enabled {
            return Err(SubmitError::Disabled);
        }
        if state.vif(attempt.vif).is_none() {
            return Err(SubmitError::UnknownVif);
        }
        if let KeySelector::Key(KeyHandle(slot)) = attempt.key
            && !state.keys.get(usize::from(slot)).copied().unwrap_or(false)
        {
            return Err(SubmitError::UnknownKey);
        }
        let Some(channel) = state.channel else {
            return Err(SubmitError::Disabled);
        };
        if !CAPABILITIES.supports_rate(attempt.rate, channel) {
            return Err(SubmitError::UnsupportedRate);
        }
        let power_ok = match attempt.power {
            TxPower::Calibrated => true,
            TxPower::MaxDbm(dbm) => CAPABILITIES
                .tx_power_ceiling_min_dbm
                .is_some_and(|floor| dbm >= floor),
        };
        if !CAPABILITIES.supports_backoff(attempt.backoff)
            || !power_ok
            || !CAPABILITIES.coex_priorities.contains(attempt.coex)
        {
            return Err(SubmitError::Unsupported);
        }
        if state
            .in_flight
            .iter()
            .any(|other| other.completion.id == attempt.id)
        {
            return Err(SubmitError::DuplicateId);
        }
        let queue = CAPABILITIES.tx_queue(attempt.access_category);
        if state.in_flight.iter().any(|other| other.queue == queue) {
            return Err(SubmitError::Busy);
        }
        let phase = if state.gate_closed {
            Phase::Held
        } else {
            Phase::Published
        };
        state.in_flight.push(InFlight {
            completion: TxCompletion {
                id: attempt.id,
                status: TxStatus::Success,
                ack_rssi_dbm: None,
                ack_snr_db: Some(20),
                block_ack,
            },
            queue,
            phase,
            ampdu: block_ack.is_some(),
        });
        Ok(())
    }
}

struct NextEvent<'a>(&'a Model);

impl Future for NextEvent<'_> {
    type Output = Result<ModelEvent, EventsLost>;

    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.0.state.borrow_mut();
        if state.lost {
            state.lost = false;
            return Poll::Ready(Err(EventsLost));
        }
        match state.events.pop_front() {
            Some(event) => Poll::Ready(Ok(event)),
            None => Poll::Pending,
        }
    }
}

impl Ieee80211LowerMacPort for Model {
    type Event = ModelEvent;
    type Error = Poisoned;
    type TxBuffer = ModelBuffer;

    fn view(event: &ModelEvent) -> LowerMacEvent<'_> {
        match event {
            ModelEvent::Received(frame, meta) => LowerMacEvent::Received { frame, meta: *meta },
            ModelEvent::Tx(completion) => LowerMacEvent::TxCompleted(*completion),
            ModelEvent::Tbtt(_) => LowerMacEvent::Extension,
            ModelEvent::Lifecycle(event) => LowerMacEvent::Lifecycle(*event),
        }
    }

    fn capabilities(&self) -> LowerMacCapabilities {
        CAPABILITIES
    }

    fn tx_buffer(&self, len: usize) -> Option<ModelBuffer> {
        let mut state = self.state.borrow_mut();
        if len > usize::from(CAPABILITIES.max_mpdu_length) || state.buffers_lent == BUFFERS {
            return None;
        }
        state.buffers_lent += 1;
        Some(ModelBuffer(vec![0; len]))
    }

    fn release_tx_buffer(&self, _buffer: ModelBuffer) {
        self.state.borrow_mut().buffers_lent -= 1;
    }

    fn submit(
        &self,
        attempt: MpduAttempt<ModelBuffer>,
    ) -> SubmitResult<MpduAttempt<ModelBuffer>, Poisoned> {
        let frame = &attempt.payload.frame.0;
        let individual = frame.get(4).is_some_and(|byte| byte & 1 == 0);
        let refused = if frame.len() < 10 {
            Err(SubmitError::InvalidLength)
        } else if individual
            && attempt.payload.response == TxResponse::None
            && !CAPABILITIES.individual_no_ack.contains_rate(attempt.rate)
        {
            Err(SubmitError::Unsupported)
        } else {
            self.admit(&attempt, None)
        };
        Ok(refused.map_err(|error| Refused { error, attempt }))
    }

    fn next_event(&self) -> impl Future<Output = Result<ModelEvent, EventsLost>> + '_ {
        NextEvent(self)
    }

    fn apply(&self, setting: LowerMacSetting) -> Result<Result<(), SettingError>, Poisoned> {
        let mut state = self.state.borrow_mut();
        Ok(match setting {
            LowerMacSetting::Channel(channel) if CAPABILITIES.supports_channel(channel) => {
                state.channel = Some(channel);
                Ok(())
            }
            LowerMacSetting::Channel(_) => Err(SettingError::UnsupportedChannel),
            LowerMacSetting::Vif {
                config: Some(config),
                ..
            } if !CAPABILITIES
                .receive_filters(config.role)
                .contains(config.receive)
                || (config.bss().is_none()
                    && !ReceiveFilter::OTHER_BSS_MANAGEMENT.contains(config.receive)) =>
            {
                Err(SettingError::Unsupported)
            }
            LowerMacSetting::Vif { vif, config } => match state.vifs.get_mut(usize::from(vif.0)) {
                Some(slot) => {
                    *slot = config;
                    Ok(())
                }
                None => Err(SettingError::UnknownVif),
            },
            LowerMacSetting::RemoveKey(KeyHandle(slot)) => {
                match state.keys.get_mut(usize::from(slot)) {
                    Some(installed @ true) => {
                        *installed = false;
                        Ok(())
                    }
                    _ => Err(SettingError::UnknownKey),
                }
            }
            LowerMacSetting::AddRxBlockAck(agreement)
                if agreement.tid > CAPABILITIES.rx_block_ack_max_tid
                    || agreement.window > CAPABILITIES.rx_block_ack_max_window =>
            {
                Err(SettingError::InvalidBlockAck)
            }
            LowerMacSetting::AddRxBlockAck(agreement) => {
                if state.rx_block_ack.len() == usize::from(CAPABILITIES.rx_block_ack_agreements) {
                    Err(SettingError::NoBlockAckSlot)
                } else {
                    state.rx_block_ack.push(agreement);
                    Ok(())
                }
            }
            LowerMacSetting::RemoveRxBlockAck { vif, peer, tid } => {
                let before = state.rx_block_ack.len();
                state
                    .rx_block_ack
                    .retain(|entry| (entry.vif, entry.peer, entry.tid) != (vif, peer, tid));
                if state.rx_block_ack.len() == before {
                    Err(SettingError::InvalidBlockAck)
                } else {
                    Ok(())
                }
            }
            LowerMacSetting::TxGate { open: false }
                if state
                    .in_flight
                    .iter()
                    .any(|attempt| attempt.phase == Phase::Published) =>
            {
                Err(SettingError::Busy)
            }
            LowerMacSetting::TxGate { open } => {
                state.gate_closed = !open;
                if open {
                    for attempt in &mut state.in_flight {
                        attempt.phase = Phase::Published;
                    }
                }
                Ok(())
            }
        })
    }

    fn install_key(
        &self,
        key: KeyInstall<'_>,
    ) -> Result<Result<KeyHandle, SettingError>, Poisoned> {
        let mut state = self.state.borrow_mut();
        if state.vif(key.vif).is_none() {
            return Ok(Err(SettingError::UnknownVif));
        }
        if key.key.len() != 16 {
            return Ok(Err(SettingError::InvalidKey));
        }
        let Some(slot) = state.keys.iter().position(|installed| !installed) else {
            return Ok(Err(SettingError::NoKeySlot));
        };
        state.keys[slot] = true;
        Ok(Ok(KeyHandle(slot as u8)))
    }

    fn lifecycle(&self, command: LifecycleCommand) -> Result<Result<(), LifecycleError>, Poisoned> {
        let mut state = self.state.borrow_mut();
        Ok(match command {
            LifecycleCommand::Enable if state.enabled => Err(LifecycleError::AlreadyInState),
            LifecycleCommand::Enable if state.channel.is_none() => {
                // Nothing to tune to: the command fails and the port stays
                // disabled.
                state.push(ModelEvent::Lifecycle(LifecycleEvent::Failed {
                    command,
                    class: FailureClass::Recoverable,
                }));
                Ok(())
            }
            LifecycleCommand::Enable => {
                state.enabled = true;
                state.push(ModelEvent::Lifecycle(LifecycleEvent::Enabled));
                Ok(())
            }
            LifecycleCommand::Disable | LifecycleCommand::Quiesce if !state.enabled => {
                Err(LifecycleError::AlreadyInState)
            }
            LifecycleCommand::Disable | LifecycleCommand::Quiesce => {
                state.enabled = false;
                while !state.in_flight.is_empty() {
                    state.finish(0, TxStatus::Aborted);
                }
                state.push(ModelEvent::Lifecycle(
                    if matches!(command, LifecycleCommand::Disable) {
                        LifecycleEvent::Disabled
                    } else {
                        LifecycleEvent::Quiesced
                    },
                ));
                Ok(())
            }
            LifecycleCommand::Cancel(id) => {
                match state
                    .in_flight
                    .iter()
                    .position(|attempt| attempt.completion.id == id)
                {
                    // A published attempt ends with its own completion.
                    Some(index) if state.in_flight[index].phase == Phase::Published => Ok(()),
                    Some(index) => {
                        state.finish(index, TxStatus::Aborted);
                        Ok(())
                    }
                    None => Err(LifecycleError::UnknownAttempt),
                }
            }
        })
    }

    fn now(&self) -> Result<RadioInstant, Poisoned> {
        let mut state = self.state.borrow_mut();
        state.now += 1;
        Ok(RadioInstant::from_micros(state.now))
    }
}

impl LowerMacAmpdu for Model {
    type AmpduBuffer = ModelAmpdu;

    fn ampdu_capabilities(&self) -> AmpduCapabilities {
        AMPDU
    }

    fn ampdu_buffer(&self) -> Option<ModelAmpdu> {
        let mut state = self.state.borrow_mut();
        if state.ampdu_lent == AMPDU_BUFFERS {
            return None;
        }
        state.ampdu_lent += 1;
        Some(ModelAmpdu::default())
    }

    fn release_ampdu_buffer(&self, _buffer: ModelAmpdu) {
        self.state.borrow_mut().ampdu_lent -= 1;
    }

    fn submit_ampdu(
        &self,
        attempt: AmpduAttempt<ModelAmpdu>,
    ) -> SubmitResult<AmpduAttempt<ModelAmpdu>, Poisoned> {
        let subframes = &attempt.payload.subframes.0;
        let refused = match subframes.first().and_then(|first| first.get(22..24)) {
            None => Err(SubmitError::InvalidLength),
            Some(_)
                if subframes.len() > usize::from(AMPDU.max_subframes)
                    || !AMPDU.formats.contains_rate(attempt.rate) =>
            {
                Err(SubmitError::Unsupported)
            }
            Some(control) => {
                let control = u16::from_le_bytes([control[0], control[1]]);
                let report = BlockAckReport {
                    start_sequence: SequenceNumber::from_low_bits(control >> 4),
                    bitmap: (1_u64 << subframes.len()) - 1,
                };
                self.admit(&attempt, Some(report))
            }
        };
        Ok(refused.map_err(|error| Refused { error, attempt }))
    }
}

impl LowerMacBeaconTiming for Model {
    fn beacon_timing_capabilities(&self) -> BeaconTimingCapabilities {
        let both = VifRoleSet::STATION.union(VifRoleSet::ACCESS_POINT);
        BeaconTimingCapabilities {
            tsf_read: both,
            tsf_set: both,
            tsf_restart: both,
            tbtt: VifRoleSet::STATION,
        }
    }

    fn tsf(&self, vif: VifId) -> Result<Result<Tsf, SettingError>, Poisoned> {
        let state = self.state.borrow();
        Ok(match state.vif(vif) {
            Some(_) => Ok(state.tsf[usize::from(vif.0)]),
            None => Err(SettingError::UnknownVif),
        })
    }

    fn set_tsf(&self, vif: VifId, tsf: Tsf) -> Result<Result<(), SettingError>, Poisoned> {
        let mut state = self.state.borrow_mut();
        Ok(match state.vif(vif) {
            Some(_) => {
                state.tsf[usize::from(vif.0)] = tsf;
                Ok(())
            }
            None => Err(SettingError::UnknownVif),
        })
    }

    fn set_tbtt(
        &self,
        vif: VifId,
        _schedule: Option<TbttSchedule>,
    ) -> Result<Result<(), SettingError>, Poisoned> {
        let capabilities = self.beacon_timing_capabilities();
        Ok(match self.state.borrow().vif(vif) {
            Some(config) if capabilities.tbtt.contains(config.role) => Ok(()),
            Some(_) => Err(SettingError::Unsupported),
            None => Err(SettingError::UnknownVif),
        })
    }

    fn tbtt(event: &ModelEvent) -> Option<TbttEvent> {
        match event {
            ModelEvent::Tbtt(event) => Some(*event),
            _ => None,
        }
    }
}

impl LowerMacMonitor for Model {
    fn monitor_capabilities(&self) -> MonitorCapabilities {
        MonitorCapabilities {
            with_receiving_interfaces: true,
        }
    }

    fn set_monitor(&self, enabled: bool) -> Result<Result<(), SettingError>, Poisoned> {
        self.state.borrow_mut().monitor = enabled;
        Ok(Ok(()))
    }
}

impl LowerMacCancelPublished for Model {
    fn cancel_published(&self, id: TxId) -> Result<Result<(), LifecycleError>, Poisoned> {
        let mut state = self.state.borrow_mut();
        Ok(
            match state
                .in_flight
                .iter()
                .position(|attempt| attempt.completion.id == id)
            {
                Some(index) => {
                    state.finish(index, TxStatus::Aborted);
                    Ok(())
                }
                None => Err(LifecycleError::UnknownAttempt),
            },
        )
    }
}

/// Take the next event without an executor; `None` when none is ready.
fn poll_event<P: Ieee80211LowerMacPort>(port: &P) -> Option<Result<P::Event, EventsLost>> {
    let mut future = pin!(port.next_event());
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(event) => Some(event),
        Poll::Pending => None,
    }
}

fn next<P: Ieee80211LowerMacPort>(port: &P) -> P::Event {
    poll_event(port)
        .expect("an event is ready")
        .expect("no event was lost")
}

fn next_completion<P: Ieee80211LowerMacPort>(port: &P) -> TxCompletion {
    match P::view(&next(port)) {
        LowerMacEvent::TxCompleted(completion) => completion,
        other => panic!("expected a completion, got {other:?}"),
    }
}

const STATION: VifId = VifId(0);
const ACCESS_POINT: VifId = VifId(1);
const ADDRESS: MacAddress = [0x02, 0, 0, 0, 0, 1];
const PEER: MacAddress = [0x02, 0, 0, 0, 0, 2];
const OTHER_BSS: MacAddress = [0x02, 0, 0, 0, 0, 9];

fn channel_six() -> Channel {
    Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap()
}

fn station_config() -> VifConfig {
    VifConfig {
        address: ADDRESS,
        role: VifRole::Station,
        bssid: Some(PEER),
        receive: ReceiveFilter::BSS_MEMBER,
    }
}

fn enabled_station() -> Model {
    let model = Model::default();
    assert_eq!(
        model.apply(LowerMacSetting::Channel(channel_six())),
        Ok(Ok(()))
    );
    assert_eq!(model.lifecycle(LifecycleCommand::Enable), Ok(Ok(())));
    assert_eq!(
        Model::view(&next(&model)),
        LowerMacEvent::Lifecycle(LifecycleEvent::Enabled)
    );
    assert_eq!(
        model.apply(LowerMacSetting::Vif {
            vif: STATION,
            config: Some(station_config()),
        }),
        Ok(Ok(()))
    );
    model
}

/// A QoS Data header from the station to its BSS.
fn header(sequence: u16) -> [u8; 24] {
    let mut frame = [0_u8; 24];
    frame[0] = 0x08;
    frame[1] = 0x01;
    frame[4..10].copy_from_slice(&PEER);
    frame[10..16].copy_from_slice(&ADDRESS);
    frame[16..22].copy_from_slice(&PEER);
    frame[22..24].copy_from_slice(&(sequence << 4).to_le_bytes());
    frame
}

/// Encode `frame` into a buffer of the port.
fn buffer<P: Ieee80211LowerMacPort>(port: &P, frame: &[u8]) -> P::TxBuffer {
    let mut buffer = port.tx_buffer(frame.len()).expect("a free buffer");
    buffer.frame_mut().copy_from_slice(frame);
    buffer
}

fn attempt<P>(id: u32, payload: P, rate: PhyRate) -> TxAttempt<P> {
    TxAttempt {
        id: TxId(id),
        vif: STATION,
        access_category: WmmAccessCategory::BestEffort,
        payload,
        rate,
        protection: Protection::None,
        key: KeySelector::Plaintext,
        power: TxPower::Calibrated,
        backoff: Backoff::Slots(3),
        coex: CoexPriority::Normal,
    }
}

fn mpdu(model: &Model, id: u32, sequence: u16) -> MpduAttempt<ModelBuffer> {
    attempt(
        id,
        TxPayload {
            frame: buffer(model, &header(sequence)),
            response: TxResponse::Ack,
        },
        OFDM24,
    )
}

const OFDM24: PhyRate = PhyRate::Legacy(LegacyRate::Ofdm24M);

#[test]
fn one_admitted_attempt_reports_exactly_one_completion_and_releases_its_buffer() {
    let model = enabled_station();
    assert_eq!(model.submit(mpdu(&model, 9, 7)), Ok(Ok(())));
    assert_eq!(model.state.borrow().buffers_lent, 1);
    assert!(poll_event(&model).is_none());

    model.complete(
        CAPABILITIES.tx_queue(WmmAccessCategory::BestEffort),
        TxStatus::Success,
    );
    let completion = next_completion(&model);
    assert_eq!(completion.id, TxId(9));
    assert_eq!(completion.status, TxStatus::Success);
    assert_eq!(completion.block_ack, None);
    assert_eq!(model.state.borrow().buffers_lent, 0);
    assert!(poll_event(&model).is_none());
}

#[test]
fn buffers_are_bounded_and_an_unsubmitted_one_is_released() {
    let model = enabled_station();
    assert!(model.tx_buffer(MAX_MPDU + 1).is_none());
    let held: Vec<_> = (0..BUFFERS).map(|_| model.tx_buffer(24).unwrap()).collect();
    assert!(model.tx_buffer(24).is_none());
    for buffer in held {
        model.release_tx_buffer(buffer);
    }
    assert!(model.tx_buffer(24).is_some());
}

#[test]
fn each_queue_holds_one_attempt_and_completions_correlate_by_identity() {
    let model = enabled_station();
    assert_eq!(model.submit(mpdu(&model, 1, 1)), Ok(Ok(())));
    // The same queue is busy; the refused attempt comes back with its
    // buffer.
    let Ok(Err(refused)) = model.submit(mpdu(&model, 2, 2)) else {
        panic!("the best-effort queue is busy");
    };
    assert_eq!(refused.error, SubmitError::Busy);
    assert_eq!(refused.attempt.id, TxId(2));
    // Another access category has its own queue.
    let voice = TxAttempt {
        access_category: WmmAccessCategory::Voice,
        ..refused.attempt
    };
    assert_eq!(model.submit(voice), Ok(Ok(())));
    let Ok(Err(duplicate)) = model.submit(TxAttempt {
        access_category: WmmAccessCategory::Video,
        ..mpdu(&model, 1, 3)
    }) else {
        panic!("identity 1 is in flight");
    };
    assert_eq!(duplicate.error, SubmitError::DuplicateId);
    model.release_tx_buffer(duplicate.attempt.payload.frame);

    // Voice completes first: order follows the queues, not submission.
    model.complete(
        CAPABILITIES.tx_queue(WmmAccessCategory::Voice),
        TxStatus::Success,
    );
    model.complete(
        CAPABILITIES.tx_queue(WmmAccessCategory::BestEffort),
        TxStatus::AckTimeout,
    );
    assert_eq!(next_completion(&model).id, TxId(2));
    let first = next_completion(&model);
    assert_eq!((first.id, first.status), (TxId(1), TxStatus::AckTimeout));
    assert_eq!(model.state.borrow().buffers_lent, 0);
}

#[test]
fn values_outside_the_limits_are_refused_as_unsupported() {
    let model = enabled_station();
    let refuse = |mutate: fn(&mut MpduAttempt<ModelBuffer>)| {
        let mut request = mpdu(&model, 1, 1);
        mutate(&mut request);
        let Ok(Err(refused)) = model.submit(request) else {
            panic!("the attempt is outside the limits");
        };
        model.release_tx_buffer(refused.attempt.payload.frame);
        refused.error
    };
    assert!(
        !CAPABILITIES
            .services
            .contains(HardwareServices::BACKOFF_DRAW)
    );
    for mutate in [
        (|request: &mut MpduAttempt<ModelBuffer>| {
            request.backoff = Backoff::HardwareDraw { cw_exponent: 4 };
        }) as fn(&mut MpduAttempt<ModelBuffer>),
        |request| request.backoff = Backoff::Slots(1024),
        |request| request.power = TxPower::MaxDbm(-1),
        |request| request.coex = CoexPriority::Critical,
        |request| {
            // Unicast without acknowledgement at an HT rate.
            request.payload.response = TxResponse::None;
            request.rate = PhyRate::Ht(
                oer_ieee80211_mac::phy::HtRate::new(
                    HtMcs::new(0).unwrap(),
                    PpduBandwidth::Mhz20,
                    false,
                )
                .unwrap(),
            );
        },
    ] {
        assert_eq!(refuse(mutate), SubmitError::Unsupported);
    }
    // Inside the limits: the lowest power ceiling, an elevated level and a
    // legacy unicast without acknowledgement.
    let mut request = mpdu(&model, 1, 1);
    request.power = TxPower::MaxDbm(0);
    request.coex = CoexPriority::Elevated;
    request.payload.response = TxResponse::None;
    assert_eq!(model.submit(request), Ok(Ok(())));
}

#[test]
fn an_ampdu_completion_carries_the_block_ack() {
    let model = enabled_station();
    let mut aggregate = model.ampdu_buffer().unwrap();
    assert!(model.ampdu_buffer().is_none());
    for sequence in [100, 101] {
        aggregate
            .push_mpdu(24)
            .unwrap()
            .copy_from_slice(&header(sequence));
    }
    let ht = PhyRate::Ht(
        oer_ieee80211_mac::phy::HtRate::new(HtMcs::new(7).unwrap(), PpduBandwidth::Mhz20, true)
            .unwrap(),
    );
    let payload = AmpduPayload {
        subframes: aggregate,
        tid: 0,
        min_mpdu_start_spacing: 0,
    };
    assert_eq!(model.submit_ampdu(attempt(1, payload, ht)), Ok(Ok(())));
    model.complete(0, TxStatus::Success);
    let completion = next_completion(&model);
    assert_eq!(
        completion.block_ack,
        Some(BlockAckReport {
            start_sequence: SequenceNumber::new(100).unwrap(),
            bitmap: 0b11,
        })
    );
    assert!(model.ampdu_buffer().is_some());
}

#[test]
fn an_empty_aggregate_is_refused() {
    let model = enabled_station();
    let payload = AmpduPayload {
        subframes: model.ampdu_buffer().unwrap(),
        tid: 0,
        min_mpdu_start_spacing: 0,
    };
    let Ok(Err(refused)) = model.submit_ampdu(attempt(1, payload, OFDM24)) else {
        panic!("an empty aggregate");
    };
    assert_eq!(refused.error, SubmitError::InvalidLength);

    // A non-empty aggregate at a rate outside the declared formats.
    let mut payload = refused.attempt.payload;
    payload
        .subframes
        .push_mpdu(24)
        .unwrap()
        .copy_from_slice(&header(100));
    let Ok(Err(refused)) = model.submit_ampdu(attempt(2, payload, OFDM24)) else {
        panic!("a non-HT aggregate");
    };
    assert_eq!(refused.error, SubmitError::Unsupported);
    model.release_ampdu_buffer(refused.attempt.payload.subframes);
}

#[test]
fn refusal_is_a_value_and_sends_nothing() {
    let model = Model::default();
    let Ok(Err(refused)) = model.submit(mpdu(&model, 1, 1)) else {
        panic!("the port is disabled");
    };
    assert_eq!(refused.error, SubmitError::Disabled);
    model.release_tx_buffer(refused.attempt.payload.frame);

    let model = enabled_station();
    let mut unknown_vif = mpdu(&model, 1, 1);
    unknown_vif.vif = VifId(1);
    assert_eq!(
        model.submit(unknown_vif).unwrap().unwrap_err().error,
        SubmitError::UnknownVif
    );
    let mut unknown_key = mpdu(&model, 1, 1);
    unknown_key.key = KeySelector::Key(KeyHandle(3));
    assert_eq!(
        model.submit(unknown_key).unwrap().unwrap_err().error,
        SubmitError::UnknownKey
    );
    let he40 = PhyRate::He(
        HeRate::new(
            HeMcs::new(0).unwrap(),
            SpatialStreams::ONE,
            PpduBandwidth::Mhz40,
            HeGiLtf::Ltf2xGi800Ns,
            FecCoding::Bcc,
            false,
        )
        .unwrap(),
    );
    let mut wide = mpdu(&model, 1, 1);
    wide.rate = he40;
    assert_eq!(
        model.submit(wide).unwrap().unwrap_err().error,
        SubmitError::UnsupportedRate
    );
    assert!(poll_event(&model).is_none());
}

#[test]
fn dsss_rates_are_not_sent_in_the_5_ghz_band() {
    let five = Channel::ghz5(36, ChannelWidth::Mhz20).unwrap();
    let cck = PhyRate::Legacy(LegacyRate::Cck11M(DsssPreamble::Short));
    assert!(CAPABILITIES.supports_rate(cck, channel_six()));
    assert!(!CAPABILITIES.supports_rate(cck, five));
    assert!(CAPABILITIES.supports_rate(OFDM24, five));
    assert!(CAPABILITIES.supports_channel(five));
}

#[test]
fn queues_follow_the_access_category_only_with_four_queues() {
    assert_eq!(CAPABILITIES.tx_queue(WmmAccessCategory::Voice), 3);
    assert_eq!(CAPABILITIES.tx_queue(WmmAccessCategory::Background), 1);
    let shared = LowerMacCapabilities {
        tx_queues: 1,
        ..CAPABILITIES
    };
    for category in [
        WmmAccessCategory::BestEffort,
        WmmAccessCategory::Background,
        WmmAccessCategory::Video,
        WmmAccessCategory::Voice,
    ] {
        assert_eq!(shared.tx_queue(category), 0);
    }
}

#[test]
fn keys_are_selected_by_the_handle_the_backend_returns() {
    let model = enabled_station();
    let key = [0x11; 16];
    let install = KeyInstall {
        vif: STATION,
        cipher: Cipher::Ccmp128,
        scope: KeyScope::Pairwise { peer: PEER },
        key: &key,
    };
    assert_eq!(
        model.install_key(KeyInstall {
            key: &key[..5],
            ..install
        }),
        Ok(Err(SettingError::InvalidKey))
    );
    let handle = model.install_key(install).unwrap().unwrap();
    let mut protected = mpdu(&model, 2, 1);
    protected.key = KeySelector::Key(handle);
    assert_eq!(model.submit(protected), Ok(Ok(())));
    assert_eq!(model.apply(LowerMacSetting::RemoveKey(handle)), Ok(Ok(())));
    let mut stale = mpdu(&model, 3, 2);
    stale.key = KeySelector::Key(handle);
    stale.access_category = WmmAccessCategory::Voice;
    assert_eq!(
        model.submit(stale).unwrap().unwrap_err().error,
        SubmitError::UnknownKey
    );
}

#[test]
fn a_closed_gate_holds_attempts_and_cancel_ends_a_held_one() {
    let model = enabled_station();
    let gate = |open| LowerMacSetting::TxGate { open };
    assert_eq!(model.apply(gate(false)), Ok(Ok(())));
    assert_eq!(model.submit(mpdu(&model, 1, 1)), Ok(Ok(())));
    let mut voice = mpdu(&model, 2, 2);
    voice.access_category = WmmAccessCategory::Voice;
    assert_eq!(model.submit(voice), Ok(Ok(())));
    assert!(poll_event(&model).is_none());

    assert_eq!(
        model.lifecycle(LifecycleCommand::Cancel(TxId(1))),
        Ok(Ok(()))
    );
    let cancelled = next_completion(&model);
    assert_eq!(
        (cancelled.id, cancelled.status),
        (TxId(1), TxStatus::Aborted)
    );

    assert_eq!(model.apply(gate(true)), Ok(Ok(())));
    // A published attempt keeps the gate open.
    assert_eq!(model.apply(gate(false)), Ok(Err(SettingError::Busy)));
    model.complete(
        CAPABILITIES.tx_queue(WmmAccessCategory::Voice),
        TxStatus::Success,
    );
    let released = next_completion(&model);
    assert_eq!((released.id, released.status), (TxId(2), TxStatus::Success));
    assert_eq!(
        model.lifecycle(LifecycleCommand::Cancel(TxId(2))),
        Ok(Err(LifecycleError::UnknownAttempt))
    );
}

#[test]
fn cancel_of_a_published_attempt_ends_with_its_own_completion() {
    let model = enabled_station();
    assert_eq!(model.submit(mpdu(&model, 1, 1)), Ok(Ok(())));
    assert_eq!(
        model.lifecycle(LifecycleCommand::Cancel(TxId(1))),
        Ok(Ok(()))
    );
    assert!(poll_event(&model).is_none());
    model.complete(0, TxStatus::Success);
    assert_eq!(next_completion(&model).status, TxStatus::Success);

    // The extension withdraws it from the air.
    assert_eq!(model.submit(mpdu(&model, 2, 2)), Ok(Ok(())));
    assert_eq!(model.cancel_published(TxId(2)), Ok(Ok(())));
    assert_eq!(next_completion(&model).status, TxStatus::Aborted);
    assert_eq!(
        model.cancel_published(TxId(2)),
        Ok(Err(LifecycleError::UnknownAttempt))
    );
}

#[test]
fn quiesce_ends_with_its_terminal_event_and_stops_admission() {
    let model = enabled_station();
    assert_eq!(model.lifecycle(LifecycleCommand::Quiesce), Ok(Ok(())));
    assert_eq!(
        Model::view(&next(&model)),
        LowerMacEvent::Lifecycle(LifecycleEvent::Quiesced)
    );
    assert_eq!(
        model.submit(mpdu(&model, 1, 1)).unwrap().unwrap_err().error,
        SubmitError::Disabled
    );
}

#[test]
fn a_failed_enable_is_a_recoverable_terminal_event() {
    let model = Model::default();
    assert_eq!(model.lifecycle(LifecycleCommand::Enable), Ok(Ok(())));
    assert_eq!(
        Model::view(&next(&model)),
        LowerMacEvent::Lifecycle(LifecycleEvent::Failed {
            command: LifecycleCommand::Enable,
            class: FailureClass::Recoverable,
        })
    );
    // The port stayed disabled and can be enabled once it can tune.
    model
        .apply(LowerMacSetting::Channel(channel_six()))
        .unwrap()
        .unwrap();
    assert_eq!(model.lifecycle(LifecycleCommand::Enable), Ok(Ok(())));
    assert_eq!(
        Model::view(&next(&model)),
        LowerMacEvent::Lifecycle(LifecycleEvent::Enabled)
    );
}

/// A frame of `frame_control` from `transmitter` in `bssid` to `receiver`.
fn management(subtype: u8, receiver: MacAddress, bssid: MacAddress) -> [u8; 24] {
    let mut frame = [0_u8; 24];
    frame[0] = subtype << 4;
    frame[4..10].copy_from_slice(&receiver);
    frame[10..16].copy_from_slice(&bssid);
    frame[16..22].copy_from_slice(&bssid);
    frame
}

#[test]
fn receive_filters_admit_exactly_their_rules() {
    const BROADCAST: MacAddress = [0xff; 6];
    let beacon = management(8, BROADCAST, PEER);
    let other_beacon = management(8, BROADCAST, OTHER_BSS);
    let probe_response = management(5, OTHER_BSS, OTHER_BSS);
    let to_us = management(13, ADDRESS, PEER);
    let mut group_data = [0_u8; 24];
    group_data[0] = 0x08;
    group_data[1] = 0x02;
    group_data[4..10].copy_from_slice(&BROADCAST);
    group_data[10..16].copy_from_slice(&PEER);
    let mut block_ack_request = [0_u8; 16];
    block_ack_request[0] = 0x84;
    block_ack_request[4..10].copy_from_slice(&ADDRESS);

    let admits = |receive, frame: &[u8]| {
        VifConfig {
            receive,
            ..station_config()
        }
        .admits(frame)
    };
    assert!(admits(ReceiveFilter::OWN_BSS_BEACONS, &beacon));
    assert!(!admits(ReceiveFilter::OWN_BSS_BEACONS, &other_beacon));
    assert!(admits(ReceiveFilter::OTHER_BSS_MANAGEMENT, &other_beacon));
    assert!(admits(ReceiveFilter::OTHER_BSS_MANAGEMENT, &probe_response));
    assert!(!admits(ReceiveFilter::OTHER_BSS_MANAGEMENT, &to_us));
    assert!(admits(ReceiveFilter::OWN_UNICAST, &to_us));
    assert!(!admits(ReceiveFilter::OWN_UNICAST, &beacon));
    assert!(admits(ReceiveFilter::OWN_BSS_GROUP, &group_data));
    assert!(!admits(ReceiveFilter::OWN_BSS_BEACONS, &group_data));
    assert!(admits(ReceiveFilter::OWN_CONTROL, &block_ack_request));
    assert!(!admits(ReceiveFilter::OWN_UNICAST, &block_ack_request));
    assert!(!admits(ReceiveFilter::BSS_MEMBER, &other_beacon));
    assert!(!admits(ReceiveFilter::BSS_MEMBER, &[0_u8; 9]));

    // A station outside a BSS has no own-BSS frames.
    let unjoined = VifConfig {
        bssid: None,
        receive: ReceiveFilter::BSS_MEMBER,
        ..station_config()
    };
    assert!(!unjoined.admits(&beacon));
    // An access point's BSS is its own address.
    let access_point = VifConfig {
        address: PEER,
        role: VifRole::AccessPoint,
        bssid: None,
        receive: ReceiveFilter::OWN_BSS_BEACONS,
    };
    assert!(access_point.admits(&beacon));
}

#[test]
fn received_frames_are_narrowed_to_the_filters_and_monitor_widens_them() {
    let model = enabled_station();
    let meta = RxMeta {
        rate: RxEvidence::HardwareObserved(OFDM24),
        rssi_dbm: RxEvidence::HardwareObserved(-40),
        ..RxMeta::unavailable(channel_six())
    };
    let other_beacon = management(8, [0xff; 6], OTHER_BSS);
    model.receive(&other_beacon, meta);
    assert!(poll_event(&model).is_none());

    assert_eq!(model.set_monitor(true), Ok(Ok(())));
    model.receive(&other_beacon, meta);
    assert_eq!(
        Model::view(&next(&model)),
        LowerMacEvent::Received {
            frame: &other_beacon,
            meta
        }
    );
}

#[test]
fn filters_outside_the_role_limits_are_refused() {
    let model = enabled_station();
    let configure = |config| {
        model.apply(LowerMacSetting::Vif {
            vif: ACCESS_POINT,
            config: Some(config),
        })
    };
    let access_point = VifConfig {
        address: PEER,
        role: VifRole::AccessPoint,
        bssid: None,
        receive: ReceiveFilter::BSS_MEMBER,
    };
    assert_eq!(configure(access_point), Ok(Ok(())));
    assert_eq!(
        configure(VifConfig {
            receive: ReceiveFilter::OTHER_BSS_MANAGEMENT,
            ..access_point
        }),
        Ok(Err(SettingError::Unsupported))
    );
    // Own-BSS rules need a BSS.
    assert_eq!(
        model.apply(LowerMacSetting::Vif {
            vif: STATION,
            config: Some(VifConfig {
                bssid: None,
                ..station_config()
            }),
        }),
        Ok(Err(SettingError::Unsupported))
    );
}

#[test]
fn loss_is_reported_once_and_tbtts_are_extension_events() {
    let model = enabled_station();
    for _ in 0..=EVENT_CAPACITY {
        model.fire_tbtt(STATION);
    }
    assert!(matches!(poll_event(&model), Some(Err(EventsLost))));
    for _ in 0..EVENT_CAPACITY {
        let event = next(&model);
        assert_eq!(Model::view(&event), LowerMacEvent::Extension);
        assert_eq!(
            Model::tbtt(&event),
            Some(TbttEvent {
                vif: STATION,
                tsf: Tsf(0)
            })
        );
    }
    assert!(poll_event(&model).is_none());
}

#[test]
fn beacon_timing_addresses_configured_interfaces_within_its_roles() {
    let model = enabled_station();
    assert_eq!(model.set_tsf(STATION, Tsf(1_024_000)), Ok(Ok(())));
    assert_eq!(model.tsf(STATION), Ok(Ok(Tsf(1_024_000))));
    assert_eq!(model.tsf(VifId(1)), Ok(Err(SettingError::UnknownVif)));
    let schedule = TbttSchedule {
        beacon_interval_tu: 100,
        next: Tsf(1_024_000),
        lead_micros: 3_000,
    };
    assert_eq!(model.set_tbtt(STATION, Some(schedule)), Ok(Ok(())));
    model
        .apply(LowerMacSetting::Vif {
            vif: ACCESS_POINT,
            config: Some(VifConfig {
                address: PEER,
                role: VifRole::AccessPoint,
                bssid: None,
                receive: ReceiveFilter::BSS_MEMBER,
            }),
        })
        .unwrap()
        .unwrap();
    assert_eq!(
        model.set_tbtt(ACCESS_POINT, Some(schedule)),
        Ok(Err(SettingError::Unsupported))
    );
    assert_eq!(
        model.apply(LowerMacSetting::AddRxBlockAck(RxBlockAckAgreement {
            vif: STATION,
            peer: PEER,
            tid: 8,
            start_sequence: SequenceNumber::ZERO,
            window: 64,
        })),
        Ok(Err(SettingError::InvalidBlockAck))
    );
    assert!(model.now().unwrap() < model.now().unwrap());
}

/// An upper layer that needs a feature names its trait: this compiles only
/// for backends that implement every extension.
fn requires_every_extension<P>(_: &P)
where
    P: LowerMacAmpdu + LowerMacBeaconTiming + LowerMacMonitor + LowerMacCancelPublished,
{
}

#[test]
fn features_are_required_through_their_traits() {
    requires_every_extension(&Model::default());
}
