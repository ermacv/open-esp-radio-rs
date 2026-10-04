//! An in-memory backend of the port and every extension, for host tests.
//!
//! [`LowerMacModel`] implements [`Ieee80211LowerMacPort`], [`LowerMacAmpdu`],
//! [`LowerMacBeaconTiming`], [`LowerMacMonitor`] and
//! [`LowerMacCancelPublished`] from portable values alone, with the limits of
//! [`MODEL_CAPABILITIES`] and [`MODEL_AMPDU`]. It keeps every admitted attempt
//! in flight until the test ends it: [`LowerMacModel::complete`] and
//! [`LowerMacModel::complete_with`] end the published attempt of a queue,
//! and outcomes queued with [`LowerMacModel::respond`] end each attempt as
//! soon as it is published, so a driver that awaits
//! [`Ieee80211LowerMacPort::next_event`] runs without the test in between.
//! [`LowerMacModel::submitted`] records what each admitted attempt carried.
//! [`LowerMacModel::poison`] makes the backend's state unknown: the port
//! reports its terminal event and refuses every later call.
//!
//! Built for the crate's own tests and, with the `model` feature, for the
//! tests of packages that drive the port.

use alloc::{collections::VecDeque, vec, vec::Vec};
use core::{
    cell::{Cell, RefCell},
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use crate::{Ieee80211ClockSample, Ieee80211Instant, Ieee80211Stamp};
use oer_ieee80211_mac::tsf::TsfInstant;
use oer_ieee80211_mac::{
    phy::{HeMcs, HtMcs},
    qos::WmmAccessCategory,
    sequence::SequenceNumber,
};

use crate::*;

/// Events the model's queue holds before it reports a loss.
pub const MODEL_EVENT_CAPACITY: usize = 4;
/// MPDU buffers the model lends at once.
pub const MODEL_TX_BUFFERS: usize = 3;
/// Aggregate buffers the model lends at once.
pub const MODEL_AMPDU_BUFFERS: usize = 1;
/// The longest MPDU of a model buffer.
pub const MODEL_MAX_MPDU: usize = 2304;

/// The model's parametric limits.
pub const MODEL_CAPABILITIES: LowerMacCapabilities = LowerMacCapabilities {
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
    max_mpdu_length: MODEL_MAX_MPDU as u16,
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

/// The model's A-MPDU limits.
pub const MODEL_AMPDU: AmpduCapabilities = AmpduCapabilities {
    max_subframes: 32,
    formats: PhyFormatSet::HT.union(PhyFormatSet::HE),
    max_length: 65_535,
};

/// One owned event of the model.
#[derive(Debug)]
pub enum ModelEvent {
    Received(Vec<u8>, RxMeta),
    Tx(TxCompletion),
    Tbtt(TbttEvent),
    Lifecycle(LifecycleEvent),
    /// The terminal event of a poisoned model.
    Poisoned,
}

/// A buffer of the model's bounded pool.
#[derive(Debug, Eq, PartialEq)]
pub struct ModelBuffer(pub Vec<u8>);

impl TxBuffer for ModelBuffer {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn frame_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}

/// An aggregate buffer of the model: its MPDUs in order.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct ModelAmpdu(pub Vec<Vec<u8>>);

impl AmpduBuffer for ModelAmpdu {
    fn push_mpdu(&mut self, len: usize) -> Option<&mut [u8]> {
        if len > MODEL_MAX_MPDU || self.0.len() == usize::from(MODEL_AMPDU.max_subframes) {
            return None;
        }
        self.0.push(vec![0; len]);
        self.0.last_mut().map(Vec::as_mut_slice)
    }

    fn subframes(&self) -> usize {
        self.0.len()
    }
}

/// How the model ends a published attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelOutcome {
    /// The solicited response arrived; an A-MPDU's BlockAck acknowledges
    /// every subframe.
    Success,
    /// A BlockAck with this report arrived.
    BlockAck(BlockAckReport),
    /// The attempt ended with this status and no response.
    Fail(TxStatus),
}

/// What one admitted attempt carried.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubmittedAttempt {
    pub id: TxId,
    pub vif: VifId,
    pub access_category: WmmAccessCategory,
    pub rate: PhyRate,
    pub protection: Protection,
    pub key: KeySelector,
    pub power: TxPower,
    pub backoff: Backoff,
    pub coex: CoexPriority,
    /// The MPDU of a single attempt, or the subframes of an aggregate, as
    /// the caller encoded them.
    pub frames: Vec<Vec<u8>>,
    /// Whether the attempt was an A-MPDU.
    pub ampdu: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    /// The gate is closed: admitted, not published.
    Held,
    Published,
}

struct InFlight {
    id: TxId,
    queue: u8,
    phase: Phase,
    /// The BlockAck an A-MPDU's success reports: every subframe.
    full_block_ack: Option<BlockAckReport>,
}

/// An interface's TSF: it advances with the model's radio clock from the
/// value it was set to. The model's counters are exact, so its samples
/// carry no uncertainty.
#[derive(Clone, Copy)]
struct ModelTsf {
    set_to: u64,
    set_at: u64,
    relation: TsfRelation,
}

impl Default for ModelTsf {
    fn default() -> Self {
        Self {
            set_to: 0,
            set_at: 0,
            relation: TsfRelation::new(0, oer_time::Duration::ZERO),
        }
    }
}

impl ModelTsf {
    fn at(self, now: Ieee80211Instant) -> TsfInstant {
        TsfInstant::from_micros(
            self.set_to
                .wrapping_add(now.as_micros().saturating_sub(self.set_at)),
        )
    }
}

#[derive(Default)]
struct State {
    enabled: bool,
    channel: Option<Channel>,
    vifs: [Option<VifConfig>; 2],
    /// Each interface's TSF as the value it was last set to, the model's
    /// radio clock then and its relation.
    tsf: [ModelTsf; 2],
    keys: [bool; 4],
    rx_block_ack: Vec<RxBlockAckAgreement>,
    /// The EDCA parameter set last applied, if any.
    edca: Option<oer_ieee80211_mac::extensions::wmm::WmmParameterSet>,
    rx_beacon_priority: Option<crate::RxBeaconPriority>,
    he_bss_color: Option<(VifId, u8)>,
    gate_closed: bool,
    monitor: bool,
    in_flight: Vec<InFlight>,
    buffers_lent: usize,
    ampdu_lent: usize,
    /// Events in order, with a loss marker in place of the first event a
    /// full queue dropped.
    events: VecDeque<Result<ModelEvent, EventsLost>>,
    /// Events the queue holds, loss markers apart.
    queued: usize,
    poisoned: bool,
    responses: VecDeque<ModelOutcome>,
    submitted: Vec<SubmittedAttempt>,
}

impl State {
    fn push(&mut self, event: ModelEvent) {
        if self.queued < MODEL_EVENT_CAPACITY {
            self.queued += 1;
            self.events.push_back(Ok(event));
        } else if !matches!(self.events.back(), Some(Err(EventsLost))) {
            self.events.push_back(Err(EventsLost));
        }
    }

    fn pop(&mut self) -> Option<Result<ModelEvent, EventsLost>> {
        let event = self.events.pop_front()?;
        if event.is_ok() {
            self.queued -= 1;
        }
        Some(event)
    }

    fn vif(&self, vif: VifId) -> Option<VifConfig> {
        self.vifs.get(usize::from(vif.0)).copied().flatten()
    }

    /// End an attempt and release its buffer.
    fn finish(&mut self, index: usize, outcome: ModelOutcome) {
        let attempt = self.in_flight.remove(index);
        let (status, block_ack) = match outcome {
            ModelOutcome::Success => (TxStatus::Success, attempt.full_block_ack),
            ModelOutcome::BlockAck(report) => (TxStatus::Success, Some(report)),
            ModelOutcome::Fail(status) => (status, None),
        };
        let completion = TxCompletion {
            id: attempt.id,
            status,
            ack_rssi_dbm: None,
            ack_snr_db: (status == TxStatus::Success).then_some(20),
            block_ack,
        };
        if attempt.full_block_ack.is_some() {
            self.ampdu_lent -= 1;
        } else {
            self.buffers_lent -= 1;
        }
        self.push(ModelEvent::Tx(completion));
    }

    /// End every published attempt that a queued outcome answers.
    fn respond(&mut self) {
        while !self.responses.is_empty() {
            let Some(index) = self
                .in_flight
                .iter()
                .position(|attempt| attempt.phase == Phase::Published)
            else {
                return;
            };
            let outcome = self.responses.pop_front().expect("a queued outcome");
            self.finish(index, outcome);
        }
    }
}

/// The in-memory backend. Every admitted attempt stays in flight until the
/// test or a queued outcome ends it. Time enters as a value: its radio
/// clock reads what the test last passed to [`LowerMacModel::set_now`].
pub struct LowerMacModel {
    state: RefCell<State>,
    /// The radio clock, as the test last set it.
    now: Cell<Ieee80211Instant>,
}

impl Default for LowerMacModel {
    fn default() -> Self {
        Self::new()
    }
}

/// The model was poisoned with [`LowerMacModel::poison`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelPoisoned;

impl PortError for ModelPoisoned {
    fn class(&self) -> FailureClass {
        FailureClass::Poisoned
    }
}

impl LowerMacModel {
    /// A model whose radio clock stands at the epoch.
    pub fn new() -> Self {
        Self {
            state: RefCell::default(),
            now: Cell::new(Ieee80211Instant::from_micros(0)),
        }
    }

    /// Set the radio clock to `now`.
    ///
    /// # Panics
    ///
    /// When `now` is earlier than the time already set: a radio clock never
    /// runs backwards, so the test has lost track of time.
    pub fn set_now(&self, now: Ieee80211Instant) {
        assert!(
            now >= self.now.get(),
            "the model's radio clock runs backwards"
        );
        self.now.set(now);
    }

    /// Make the backend's state unknown: the events queued so far are still
    /// reported, then the terminal [`LowerMacEvent::Poisoned`] at every
    /// [`Ieee80211LowerMacPort::next_event`], and every other call fails.
    pub fn poison(&self) {
        self.state.borrow_mut().poisoned = true;
    }

    /// Fail with [`ModelPoisoned`] once poisoned.
    fn serving(&self) -> Result<core::cell::RefMut<'_, State>, ModelPoisoned> {
        let state = self.state.borrow_mut();
        if state.poisoned {
            Err(ModelPoisoned)
        } else {
            Ok(state)
        }
    }

    /// Deliver `frame` when an interface's filter or monitor reception
    /// admits it, as a backend narrowing a hardware superset does.
    pub fn receive(&self, frame: &[u8], meta: RxMeta) {
        let mut state = self.state.borrow_mut();
        let admitted = state.monitor || state.vifs.iter().flatten().any(|vif| vif.admits(frame));
        if state.enabled && admitted {
            state.push(ModelEvent::Received(frame.to_vec(), meta));
        }
    }

    /// Whether `vif` is configured with a role whose TBTT schedule the
    /// model programs.
    fn tbtt_role(&self, vif: VifId) -> Result<Result<(), SettingError>, ModelPoisoned> {
        let capabilities = self.beacon_timing_capabilities();
        Ok(match self.serving()?.vif(vif) {
            Some(config) if capabilities.tbtt.contains(config.role) => Ok(()),
            Some(_) => Err(SettingError::Unsupported),
            None => Err(SettingError::UnknownVif),
        })
    }

    /// Report the TBTT of an interface's current TSF.
    pub fn fire_tbtt(&self, vif: VifId) {
        let mut state = self.state.borrow_mut();
        let at = state.tsf[usize::from(vif.0)].at(self.now.get());
        state.push(ModelEvent::Tbtt(TbttEvent {
            tbtt: VifTsf::new(vif, at),
        }));
    }

    /// End the published attempt of `queue` with `status`; a successful
    /// A-MPDU reports every subframe acknowledged.
    pub fn complete(&self, queue: u8, status: TxStatus) {
        self.complete_with(
            queue,
            if status == TxStatus::Success {
                ModelOutcome::Success
            } else {
                ModelOutcome::Fail(status)
            },
        );
    }

    /// End the published attempt of `queue` with `outcome`.
    pub fn complete_with(&self, queue: u8, outcome: ModelOutcome) {
        let mut state = self.state.borrow_mut();
        let index = state
            .in_flight
            .iter()
            .position(|attempt| attempt.queue == queue && attempt.phase == Phase::Published)
            .expect("a published attempt on the queue");
        state.finish(index, outcome);
    }

    /// End the next published attempts, in publication order, with these
    /// outcomes as soon as they are published.
    pub fn respond(&self, outcomes: impl IntoIterator<Item = ModelOutcome>) {
        let mut state = self.state.borrow_mut();
        state.responses.extend(outcomes);
        state.respond();
    }

    /// Outcomes queued by [`Self::respond`] that no attempt has used yet.
    pub fn pending_responses(&self) -> usize {
        self.state.borrow().responses.len()
    }

    /// Every admitted attempt so far, in admission order.
    pub fn submitted(&self) -> Vec<SubmittedAttempt> {
        self.state.borrow().submitted.clone()
    }

    /// MPDU buffers lent and not yet released.
    pub fn buffers_lent(&self) -> usize {
        self.state.borrow().buffers_lent
    }

    /// Aggregate buffers lent and not yet released.
    pub fn ampdu_buffers_lent(&self) -> usize {
        self.state.borrow().ampdu_lent
    }

    /// Attempts admitted and not yet completed.
    pub fn in_flight(&self) -> usize {
        self.state.borrow().in_flight.len()
    }

    /// Events queued and not yet taken, so a test can deliver frames
    /// without overflowing the queue.
    pub fn queued_events(&self) -> usize {
        self.state.borrow().events.len()
    }

    /// Whether the transmit gate is open.
    pub fn gate_open(&self) -> bool {
        !self.state.borrow().gate_closed
    }

    /// The configuration of an interface.
    pub fn vif_config(&self, vif: VifId) -> Option<VifConfig> {
        self.state.borrow().vif(vif)
    }

    /// The channel the model is tuned to.
    pub fn channel(&self) -> Option<Channel> {
        self.state.borrow().channel
    }

    /// The EDCA parameter set last applied; `None` keeps the defaults.
    pub fn edca(&self) -> Option<oer_ieee80211_mac::extensions::wmm::WmmParameterSet> {
        self.state.borrow().edca
    }

    /// The last beacon receive priority applied.
    pub fn rx_beacon_priority(&self) -> Option<crate::RxBeaconPriority> {
        self.state.borrow().rx_beacon_priority
    }

    /// The last HE BSS color applied, with its interface.
    pub fn he_bss_color(&self) -> Option<(VifId, u8)> {
        self.state.borrow().he_bss_color
    }

    /// Whether monitor reception runs.
    pub fn monitoring(&self) -> bool {
        self.state.borrow().monitor
    }

    fn admit<P>(
        &self,
        attempt: &TxAttempt<P>,
        frames: Vec<Vec<u8>>,
        full_block_ack: Option<BlockAckReport>,
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
        if !MODEL_CAPABILITIES.supports_rate(attempt.rate, channel) {
            return Err(SubmitError::UnsupportedRate);
        }
        let power_ok = match attempt.power {
            TxPower::Calibrated => true,
            TxPower::MaxDbm(dbm) => MODEL_CAPABILITIES
                .tx_power_ceiling_min_dbm
                .is_some_and(|floor| dbm >= floor),
        };
        if !MODEL_CAPABILITIES.supports_backoff(attempt.backoff)
            || !power_ok
            || !MODEL_CAPABILITIES.coex_priorities.contains(attempt.coex)
        {
            return Err(SubmitError::Unsupported);
        }
        if state.in_flight.iter().any(|other| other.id == attempt.id) {
            return Err(SubmitError::DuplicateId);
        }
        let queue = MODEL_CAPABILITIES.tx_queue(attempt.access_category);
        if state.in_flight.iter().any(|other| other.queue == queue) {
            return Err(SubmitError::Busy);
        }
        let phase = if state.gate_closed {
            Phase::Held
        } else {
            Phase::Published
        };
        state.in_flight.push(InFlight {
            id: attempt.id,
            queue,
            phase,
            full_block_ack,
        });
        state.submitted.push(SubmittedAttempt {
            id: attempt.id,
            vif: attempt.vif,
            access_category: attempt.access_category,
            rate: attempt.rate,
            protection: attempt.protection,
            key: attempt.key,
            power: attempt.power,
            backoff: attempt.backoff,
            coex: attempt.coex,
            frames,
            ampdu: full_block_ack.is_some(),
        });
        state.respond();
        Ok(())
    }
}

/// The future of [`LowerMacModel::next_event`]: ready while an event or a
/// loss is queued. It registers no waker; a driver polls it again after the
/// test changed the model.
pub struct NextModelEvent<'a>(&'a LowerMacModel);

impl Future for NextModelEvent<'_> {
    type Output = Result<ModelEvent, EventsLost>;

    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.0.state.borrow_mut();
        match state.pop() {
            Some(event) => Poll::Ready(event),
            None if state.poisoned => Poll::Ready(Ok(ModelEvent::Poisoned)),
            None => Poll::Pending,
        }
    }
}

impl Ieee80211LowerMacPort for LowerMacModel {
    type Event = ModelEvent;
    type Error = ModelPoisoned;
    type TxBuffer = ModelBuffer;

    fn view(event: &ModelEvent) -> LowerMacEvent<'_> {
        match event {
            ModelEvent::Received(frame, meta) => LowerMacEvent::Received { frame, meta: *meta },
            ModelEvent::Tx(completion) => LowerMacEvent::TxCompleted(*completion),
            ModelEvent::Tbtt(_) => LowerMacEvent::Extension,
            ModelEvent::Lifecycle(event) => LowerMacEvent::Lifecycle(*event),
            ModelEvent::Poisoned => LowerMacEvent::Poisoned(Poisoned),
        }
    }

    fn capabilities(&self) -> LowerMacCapabilities {
        MODEL_CAPABILITIES
    }

    /// The model's clock is the time the test sets, which a test sets to
    /// its own monotonic time.
    fn clock_info(&self) -> ClockInfo {
        ClockInfo::MONOTONIC_MICROS
    }

    fn tx_buffer(&self, len: usize) -> Result<Option<ModelBuffer>, ModelPoisoned> {
        let mut state = self.serving()?;
        if len > usize::from(MODEL_CAPABILITIES.max_mpdu_length)
            || state.buffers_lent == MODEL_TX_BUFFERS
        {
            return Ok(None);
        }
        state.buffers_lent += 1;
        Ok(Some(ModelBuffer(vec![0; len])))
    }

    fn release_tx_buffer(&self, _buffer: ModelBuffer) {
        self.state.borrow_mut().buffers_lent -= 1;
    }

    fn submit(
        &self,
        attempt: MpduAttempt<ModelBuffer>,
    ) -> SubmitResult<MpduAttempt<ModelBuffer>, ModelPoisoned> {
        drop(self.serving()?);
        let frame = &attempt.payload.frame.0;
        let individual = frame.get(4).is_some_and(|byte| byte & 1 == 0);
        let refused = if frame.len() < 10 {
            Err(SubmitError::InvalidLength)
        } else if individual
            && attempt.payload.response == TxResponse::None
            && !MODEL_CAPABILITIES
                .individual_no_ack
                .contains_rate(attempt.rate)
        {
            Err(SubmitError::Unsupported)
        } else {
            self.admit(&attempt, vec![frame.clone()], None)
        };
        Ok(refused.map_err(|error| Refused { error, attempt }))
    }

    fn next_event(&self) -> impl Future<Output = Result<ModelEvent, EventsLost>> + '_ {
        NextModelEvent(self)
    }

    fn apply(&self, setting: LowerMacSetting) -> Result<Result<(), SettingError>, ModelPoisoned> {
        let mut state = self.serving()?;
        Ok(match setting {
            LowerMacSetting::Channel(channel) if MODEL_CAPABILITIES.supports_channel(channel) => {
                state.channel = Some(channel);
                Ok(())
            }
            LowerMacSetting::Channel(_) => Err(SettingError::UnsupportedChannel),
            LowerMacSetting::Vif {
                config: Some(config),
                ..
            } if !MODEL_CAPABILITIES
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
                if agreement.tid > MODEL_CAPABILITIES.rx_block_ack_max_tid
                    || agreement.window > MODEL_CAPABILITIES.rx_block_ack_max_window =>
            {
                Err(SettingError::InvalidBlockAck)
            }
            LowerMacSetting::AddRxBlockAck(agreement) => {
                if state.rx_block_ack.len()
                    == usize::from(MODEL_CAPABILITIES.rx_block_ack_agreements)
                {
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
            LowerMacSetting::Edca(parameters) => {
                use oer_ieee80211_mac::qos::WmmAccessCategory;
                // AIFSN and the four-bit ECW fields as the element encodes them.
                let valid = [
                    WmmAccessCategory::BestEffort,
                    WmmAccessCategory::Background,
                    WmmAccessCategory::Video,
                    WmmAccessCategory::Voice,
                ]
                .into_iter()
                .map(|category| parameters.access_category(category))
                .all(|record| {
                    (2..=15).contains(&record.aifsn)
                        && record.ecw_min <= 15
                        && record.ecw_max <= 15
                        && record.ecw_min <= record.ecw_max
                });
                if valid {
                    state.edca = Some(parameters);
                    Ok(())
                } else {
                    Err(SettingError::Unsupported)
                }
            }
            LowerMacSetting::RxBeaconPriority(priority) => {
                state.rx_beacon_priority = Some(priority);
                Ok(())
            }
            LowerMacSetting::HeBssColor { color, .. } if color > 63 => {
                Err(SettingError::Unsupported)
            }
            LowerMacSetting::HeBssColor { vif, color } => {
                state.he_bss_color = Some((vif, color));
                Ok(())
            }
            LowerMacSetting::TxGate { open } => {
                state.gate_closed = !open;
                if open {
                    for attempt in &mut state.in_flight {
                        attempt.phase = Phase::Published;
                    }
                    state.respond();
                }
                Ok(())
            }
        })
    }

    fn install_key(
        &self,
        key: KeyInstall<'_>,
    ) -> Result<Result<KeyHandle, SettingError>, ModelPoisoned> {
        let mut state = self.serving()?;
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

    fn lifecycle(
        &self,
        command: LifecycleCommand,
    ) -> Result<Result<(), LifecycleError>, ModelPoisoned> {
        let mut state = self.serving()?;
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
                    state.finish(0, ModelOutcome::Fail(TxStatus::Aborted));
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
        })
    }

    fn cancel(&self, id: TxId) -> Result<Result<(), CancelError>, ModelPoisoned> {
        let mut state = self.serving()?;
        Ok(
            match state.in_flight.iter().position(|attempt| attempt.id == id) {
                // A published attempt ends with its own completion.
                Some(index) if state.in_flight[index].phase == Phase::Published => Ok(()),
                Some(index) => {
                    state.finish(index, ModelOutcome::Fail(TxStatus::Aborted));
                    Ok(())
                }
                None => Err(CancelError::NotRunning),
            },
        )
    }

    fn now(&self) -> Result<Ieee80211Instant, ModelPoisoned> {
        self.serving()?;
        Ok(self.now.get())
    }

    /// The model's clock is the monotonic clock: one reading is both, in
    /// the one generation the model has.
    fn clock_sample(&self) -> Result<Ieee80211ClockSample, ModelPoisoned> {
        let radio = self.now()?;
        Ok(Ieee80211ClockSample {
            radio,
            monotonic: oer_time::Instant::from_micros(radio.as_micros()),
            uncertainty: oer_time::Duration::ZERO,
            generation: 0,
        })
    }
}

impl LowerMacAmpdu for LowerMacModel {
    type AmpduBuffer = ModelAmpdu;

    fn ampdu_capabilities(&self) -> AmpduCapabilities {
        MODEL_AMPDU
    }

    fn ampdu_buffer(&self) -> Result<Option<ModelAmpdu>, ModelPoisoned> {
        let mut state = self.serving()?;
        if state.ampdu_lent == MODEL_AMPDU_BUFFERS {
            return Ok(None);
        }
        state.ampdu_lent += 1;
        Ok(Some(ModelAmpdu::default()))
    }

    fn release_ampdu_buffer(&self, _buffer: ModelAmpdu) {
        self.state.borrow_mut().ampdu_lent -= 1;
    }

    fn submit_ampdu(
        &self,
        attempt: AmpduAttempt<ModelAmpdu>,
    ) -> SubmitResult<AmpduAttempt<ModelAmpdu>, ModelPoisoned> {
        drop(self.serving()?);
        let subframes = &attempt.payload.subframes.0;
        let refused = match subframes.first().and_then(|first| first.get(22..24)) {
            None => Err(SubmitError::InvalidLength),
            Some(_)
                if subframes.len() > usize::from(MODEL_AMPDU.max_subframes)
                    || !MODEL_AMPDU.formats.contains_rate(attempt.rate) =>
            {
                Err(SubmitError::Unsupported)
            }
            Some(control) => {
                // A BlockAck that acknowledges exactly the subframes sent.
                let sequence = |mpdu: &[u8]| {
                    mpdu.get(22..24).map(|control| {
                        SequenceNumber::from_sequence_control(u16::from_le_bytes([
                            control[0], control[1],
                        ]))
                    })
                };
                let start_sequence = SequenceNumber::from_sequence_control(u16::from_le_bytes([
                    control[0], control[1],
                ]));
                let bitmap = subframes
                    .iter()
                    .filter_map(|mpdu| sequence(mpdu))
                    .map(|sequence| start_sequence.forward_distance(sequence))
                    .filter(|distance| *distance < BlockAckReport::BITMAP_BITS)
                    .fold(0_u64, |bitmap, distance| bitmap | 1 << distance);
                let report = BlockAckReport {
                    start_sequence,
                    bitmap,
                };
                self.admit(&attempt, subframes.clone(), Some(report))
            }
        };
        Ok(refused.map_err(|error| Refused { error, attempt }))
    }
}

impl LowerMacBeaconTiming for LowerMacModel {
    fn beacon_timing_capabilities(&self) -> BeaconTimingCapabilities {
        let both = VifRoleSet::STATION.union(VifRoleSet::ACCESS_POINT);
        BeaconTimingCapabilities {
            tsf_read: both,
            tsf_set: both,
            tsf_restart: both,
            tbtt: VifRoleSet::STATION,
        }
    }

    fn tsf(&self, vif: VifId) -> Result<Result<VifTsf, SettingError>, ModelPoisoned> {
        let now = self.now.get();
        let state = self.serving()?;
        Ok(match state.vif(vif) {
            Some(_) => Ok(VifTsf::new(vif, state.tsf[usize::from(vif.0)].at(now))),
            None => Err(SettingError::UnknownVif),
        })
    }

    /// The model's radio clock is the monotonic clock of one generation;
    /// the TSF relation's generation advances with every jump
    /// ([`TsfRelation`]).
    fn tsf_sample(&self, vif: VifId) -> Result<Result<TsfSample, SettingError>, ModelPoisoned> {
        let now = self.now.get();
        let state = self.serving()?;
        Ok(match state.vif(vif) {
            Some(_) => {
                let tsf = state.tsf[usize::from(vif.0)];
                Ok(TsfSample {
                    tsf: VifTsf::new(vif, tsf.at(now)),
                    local: Ieee80211Stamp {
                        at: now,
                        generation: 0,
                    },
                    uncertainty: oer_time::Duration::ZERO,
                    generation: tsf.relation.generation(),
                })
            }
            None => Err(SettingError::UnknownVif),
        })
    }

    fn set_tsf(&self, tsf: VifTsf) -> Result<Result<(), SettingError>, ModelPoisoned> {
        let now = self.now.get();
        let mut state = self.serving()?;
        Ok(match state.vif(tsf.vif) {
            Some(_) => {
                let current = &mut state.tsf[usize::from(tsf.vif.0)];
                let reading = current.at(now);
                current.relation.set(reading, tsf.at);
                current.set_to = tsf.at.as_micros();
                current.set_at = now.as_micros();
                Ok(())
            }
            None => Err(SettingError::UnknownVif),
        })
    }

    fn set_tbtt(&self, schedule: TbttSchedule) -> Result<Result<(), SettingError>, ModelPoisoned> {
        self.tbtt_role(schedule.next.vif)
    }

    fn stop_tbtt(&self, vif: VifId) -> Result<Result<(), SettingError>, ModelPoisoned> {
        self.tbtt_role(vif)
    }

    fn tbtt(event: &ModelEvent) -> Option<TbttEvent> {
        match event {
            ModelEvent::Tbtt(event) => Some(*event),
            _ => None,
        }
    }
}

impl LowerMacMonitor for LowerMacModel {
    fn monitor_capabilities(&self) -> MonitorCapabilities {
        MonitorCapabilities {
            with_receiving_interfaces: true,
        }
    }

    fn set_monitor(&self, enabled: bool) -> Result<Result<(), SettingError>, ModelPoisoned> {
        self.serving()?.monitor = enabled;
        Ok(Ok(()))
    }
}

impl LowerMacCancelPublished for LowerMacModel {
    fn cancel_published(&self, id: TxId) -> Result<Result<(), CancelError>, ModelPoisoned> {
        let mut state = self.serving()?;
        Ok(
            match state.in_flight.iter().position(|attempt| attempt.id == id) {
                Some(index) => {
                    state.finish(index, ModelOutcome::Fail(TxStatus::Aborted));
                    Ok(())
                }
                None => Err(CancelError::NotRunning),
            },
        )
    }
}
