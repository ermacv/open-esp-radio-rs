//! A trivial in-memory backend proving the port is implementable from
//! portable values alone, and the contract behaviour a caller relies on.

use core::{
    cell::RefCell,
    future::Future,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};
use std::{collections::VecDeque, vec::Vec};

use oer_ieee80211_mac::{
    phy::{
        DsssPreamble, FecCoding, HeGiLtf, HeMcs, HeRate, HtMcs, LegacyRate, PpduBandwidth,
        SpatialStreams,
    },
    qos::WmmAccessCategory,
    sequence::SequenceNumber,
};
use oer_radio_coex::CoexPriority;
use oer_time::RadioInstant;

use crate::*;

const EVENT_CAPACITY: usize = 4;

enum ModelEvent {
    Received(Vec<u8>, RxMeta),
    Tx(TxCompletion),
    Tbtt(VifId, Tsf),
    Lifecycle(LifecycleEvent),
}

struct Held {
    completion: TxCompletion,
    vif: VifId,
}

#[derive(Default)]
struct State {
    enabled: bool,
    channel: Option<Channel>,
    vifs: [Option<VifConfig>; 2],
    tsf: [Tsf; 2],
    keys: [bool; 4],
    rx_block_ack: Vec<RxBlockAckAgreement>,
    blocked: [bool; 2],
    held: Vec<Held>,
    coex: CoexPriority,
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
}

/// Completes every admitted attempt at once, successfully, unless power
/// save holds its interface.
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
    max_ampdu_subframes: 32,
    key_slots: 4,
    rx_block_ack_agreements: 2,
    rx_block_ack_max_tid: 7,
    rx_block_ack_max_window: 64,
};

impl Model {
    fn receive(&self, frame: &[u8], meta: RxMeta) {
        self.state
            .borrow_mut()
            .push(ModelEvent::Received(frame.to_vec(), meta));
    }

    fn tbtt(&self, vif: VifId) {
        let mut state = self.state.borrow_mut();
        let tsf = state.tsf[usize::from(vif.0)];
        state.push(ModelEvent::Tbtt(vif, tsf));
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

    fn view(event: &ModelEvent) -> LowerMacEvent<'_> {
        match event {
            ModelEvent::Received(frame, meta) => LowerMacEvent::Received { frame, meta: *meta },
            ModelEvent::Tx(completion) => LowerMacEvent::TxCompleted(*completion),
            ModelEvent::Tbtt(vif, tsf) => LowerMacEvent::Tbtt {
                vif: *vif,
                tsf: *tsf,
            },
            ModelEvent::Lifecycle(event) => LowerMacEvent::Lifecycle(*event),
        }
    }

    fn capabilities(&self) -> LowerMacCapabilities {
        CAPABILITIES
    }

    fn submit(&self, attempt: TxAttempt<'_>) -> Result<Result<(), SubmitError>, Poisoned> {
        let mut state = self.state.borrow_mut();
        if !state.enabled {
            return Ok(Err(SubmitError::Disabled));
        }
        if state.vif(attempt.vif).is_none() {
            return Ok(Err(SubmitError::UnknownVif));
        }
        if let KeySelector::Key(KeyHandle(slot)) = attempt.key
            && !state.keys.get(usize::from(slot)).copied().unwrap_or(false)
        {
            return Ok(Err(SubmitError::UnknownKey));
        }
        let Some(channel) = state.channel else {
            return Ok(Err(SubmitError::Disabled));
        };
        if !CAPABILITIES.supports_rate(attempt.rate, channel) {
            return Ok(Err(SubmitError::UnsupportedRate));
        }
        if state
            .held
            .iter()
            .any(|held| held.completion.id == attempt.id)
        {
            return Ok(Err(SubmitError::DuplicateId));
        }
        let block_ack = match attempt.payload {
            TxPayload::Mpdu { frame, .. } if frame.len() < 10 => {
                return Ok(Err(SubmitError::InvalidLength));
            }
            TxPayload::Mpdu { .. } => None,
            TxPayload::Ampdu(ampdu) => {
                let Some(first) = ampdu.subframes.first() else {
                    return Ok(Err(SubmitError::InvalidLength));
                };
                if ampdu.subframes.len() > usize::from(CAPABILITIES.max_ampdu_subframes) {
                    return Ok(Err(SubmitError::TooManySubframes));
                }
                let Some(control) = first.get(22..24) else {
                    return Ok(Err(SubmitError::InvalidLength));
                };
                let control = u16::from_le_bytes([control[0], control[1]]);
                Some(BlockAckReport {
                    start_sequence: SequenceNumber::from_low_bits(control >> 4),
                    bitmap: (1_u64 << ampdu.subframes.len()) - 1,
                })
            }
        };
        let completion = TxCompletion {
            id: attempt.id,
            status: TxStatus::Success,
            ack_rssi_dbm: None,
            ack_snr_db: Some(20),
            block_ack,
        };
        if state.blocked[usize::from(attempt.vif.0)] {
            state.held.push(Held {
                completion,
                vif: attempt.vif,
            });
        } else {
            state.push(ModelEvent::Tx(completion));
        }
        Ok(Ok(()))
    }

    fn next_event(&self) -> impl Future<Output = Result<ModelEvent, EventsLost>> + '_ {
        NextEvent(self)
    }

    fn apply(&self, setting: LowerMacSetting) -> Result<Result<(), SettingError>, Poisoned> {
        let mut state = self.state.borrow_mut();
        let known = |state: &State, vif: VifId| state.vif(vif).is_some();
        Ok(match setting {
            LowerMacSetting::Channel(channel) if CAPABILITIES.supports_channel(channel) => {
                state.channel = Some(channel);
                Ok(())
            }
            LowerMacSetting::Channel(_) => Err(SettingError::UnsupportedChannel),
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
            LowerMacSetting::SetTsf { vif, tsf } if known(&state, vif) => {
                state.tsf[usize::from(vif.0)] = tsf;
                Ok(())
            }
            LowerMacSetting::Tbtt { vif, .. } if known(&state, vif) => Ok(()),
            LowerMacSetting::PowerSaveTxBlock {
                vif,
                peer: None,
                blocked,
            } if known(&state, vif) => {
                state.blocked[usize::from(vif.0)] = blocked;
                if !blocked {
                    let (released, held) = core::mem::take(&mut state.held)
                        .into_iter()
                        .partition::<Vec<_>, _>(|held| held.vif == vif);
                    state.held = held;
                    for held in released {
                        state.push(ModelEvent::Tx(held.completion));
                    }
                }
                Ok(())
            }
            LowerMacSetting::PowerSaveTxBlock { peer: Some(_), .. } => {
                Err(SettingError::Unsupported)
            }
            LowerMacSetting::CoexPriority(priority) => {
                state.coex = priority;
                Ok(())
            }
            LowerMacSetting::SetTsf { .. }
            | LowerMacSetting::Tbtt { .. }
            | LowerMacSetting::PowerSaveTxBlock { .. } => Err(SettingError::UnknownVif),
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
                for held in core::mem::take(&mut state.held) {
                    state.push(ModelEvent::Tx(TxCompletion {
                        status: TxStatus::Aborted,
                        ack_snr_db: None,
                        block_ack: None,
                        ..held.completion
                    }));
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
                match state.held.iter().position(|held| held.completion.id == id) {
                    Some(index) => {
                        let held = state.held.remove(index);
                        state.push(ModelEvent::Tx(TxCompletion {
                            status: TxStatus::Aborted,
                            ack_snr_db: None,
                            block_ack: None,
                            ..held.completion
                        }));
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

    fn tsf(&self, vif: VifId) -> Result<Result<Tsf, SettingError>, Poisoned> {
        let state = self.state.borrow();
        Ok(match state.vif(vif) {
            Some(_) => Ok(state.tsf[usize::from(vif.0)]),
            None => Err(SettingError::UnknownVif),
        })
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

const STATION: VifId = VifId(0);
const ADDRESS: MacAddress = [0x02, 0, 0, 0, 0, 1];
const PEER: MacAddress = [0x02, 0, 0, 0, 0, 2];

fn channel_six() -> Channel {
    Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap()
}

fn enabled_station() -> Model {
    let model = Model::default();
    assert_eq!(model.lifecycle(LifecycleCommand::Enable), Ok(Ok(())));
    assert_eq!(
        Model::view(&next(&model)),
        LowerMacEvent::Lifecycle(LifecycleEvent::Enabled)
    );
    assert_eq!(
        model.apply(LowerMacSetting::Channel(channel_six())),
        Ok(Ok(()))
    );
    assert_eq!(
        model.apply(LowerMacSetting::Vif {
            vif: STATION,
            config: Some(VifConfig {
                address: ADDRESS,
                role: VifRole::Station,
                bssid: Some(PEER),
                receive: ReceiveFilter::BSS_MEMBER,
            }),
        }),
        Ok(Ok(()))
    );
    model
}

fn header(sequence: u16) -> [u8; 24] {
    let mut frame = [0_u8; 24];
    frame[0] = 0x08;
    frame[4..10].copy_from_slice(&PEER);
    frame[10..16].copy_from_slice(&ADDRESS);
    frame[22..24].copy_from_slice(&(sequence << 4).to_le_bytes());
    frame
}

fn attempt<'a>(id: u32, payload: TxPayload<'a>, rate: PhyRate) -> TxAttempt<'a> {
    TxAttempt {
        id: TxId(id),
        vif: STATION,
        access_category: WmmAccessCategory::BestEffort,
        payload,
        rate,
        protection: Protection::None,
        key: KeySelector::Plaintext,
        power: TxPower::Calibrated,
    }
}

const OFDM24: PhyRate = PhyRate::Legacy(LegacyRate::Ofdm24M);

#[test]
fn one_admitted_attempt_reports_exactly_one_completion() {
    let model = enabled_station();
    let frame = header(7);
    let payload = TxPayload::Mpdu {
        frame: &frame,
        response: TxResponse::Ack,
    };
    assert_eq!(model.submit(attempt(9, payload, OFDM24)), Ok(Ok(())));
    let event = next(&model);
    let LowerMacEvent::TxCompleted(completion) = Model::view(&event) else {
        panic!("expected a completion");
    };
    assert_eq!(completion.id, TxId(9));
    assert_eq!(completion.status, TxStatus::Success);
    assert_eq!(completion.block_ack, None);
    assert!(poll_event(&model).is_none());
}

#[test]
fn an_ampdu_completion_carries_the_block_ack() {
    let model = enabled_station();
    let (first, second) = (header(100), header(101));
    let subframes: [&[u8]; 2] = [&first, &second];
    let ht = PhyRate::Ht(
        oer_ieee80211_mac::phy::HtRate::new(HtMcs::new(7).unwrap(), PpduBandwidth::Mhz20, true)
            .unwrap(),
    );
    let payload = TxPayload::Ampdu(AmpduSubmission {
        subframes: &subframes,
        tid: 0,
        min_mpdu_start_spacing: 0,
    });
    assert_eq!(model.submit(attempt(1, payload, ht)), Ok(Ok(())));
    let event = next(&model);
    let LowerMacEvent::TxCompleted(completion) = Model::view(&event) else {
        panic!("expected a completion");
    };
    assert_eq!(
        completion.block_ack,
        Some(BlockAckReport {
            start_sequence: SequenceNumber::new(100).unwrap(),
            bitmap: 0b11,
        })
    );
}

#[test]
fn refusal_is_a_value_and_sends_nothing() {
    let model = Model::default();
    let frame = header(1);
    let payload = TxPayload::Mpdu {
        frame: &frame,
        response: TxResponse::Ack,
    };
    assert_eq!(
        model.submit(attempt(1, payload, OFDM24)),
        Ok(Err(SubmitError::Disabled))
    );

    let model = enabled_station();
    let mut unknown_vif = attempt(1, payload, OFDM24);
    unknown_vif.vif = VifId(1);
    assert_eq!(model.submit(unknown_vif), Ok(Err(SubmitError::UnknownVif)));
    let mut unknown_key = attempt(1, payload, OFDM24);
    unknown_key.key = KeySelector::Key(KeyHandle(3));
    assert_eq!(model.submit(unknown_key), Ok(Err(SubmitError::UnknownKey)));
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
    assert_eq!(
        model.submit(attempt(1, payload, he40)),
        Ok(Err(SubmitError::UnsupportedRate))
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
    let frame = header(1);
    let mut protected = attempt(
        2,
        TxPayload::Mpdu {
            frame: &frame,
            response: TxResponse::Ack,
        },
        OFDM24,
    );
    protected.key = KeySelector::Key(handle);
    assert_eq!(model.submit(protected), Ok(Ok(())));
    assert_eq!(model.apply(LowerMacSetting::RemoveKey(handle)), Ok(Ok(())));
    assert_eq!(
        model.submit(TxAttempt {
            id: TxId(3),
            ..protected
        }),
        Ok(Err(SubmitError::UnknownKey))
    );
}

#[test]
fn power_save_holds_attempts_and_cancel_ends_them_with_their_completion() {
    let model = enabled_station();
    let block = |blocked| LowerMacSetting::PowerSaveTxBlock {
        vif: STATION,
        peer: None,
        blocked,
    };
    assert_eq!(model.apply(block(true)), Ok(Ok(())));
    let frame = header(1);
    let payload = TxPayload::Mpdu {
        frame: &frame,
        response: TxResponse::Ack,
    };
    assert_eq!(model.submit(attempt(1, payload, OFDM24)), Ok(Ok(())));
    assert_eq!(model.submit(attempt(2, payload, OFDM24)), Ok(Ok(())));
    assert!(poll_event(&model).is_none());

    assert_eq!(
        model.lifecycle(LifecycleCommand::Cancel(TxId(1))),
        Ok(Ok(()))
    );
    let LowerMacEvent::TxCompleted(cancelled) = Model::view(&next(&model)) else {
        panic!("expected the cancelled completion");
    };
    assert_eq!(
        (cancelled.id, cancelled.status),
        (TxId(1), TxStatus::Aborted)
    );

    assert_eq!(model.apply(block(false)), Ok(Ok(())));
    let LowerMacEvent::TxCompleted(released) = Model::view(&next(&model)) else {
        panic!("expected the released completion");
    };
    assert_eq!((released.id, released.status), (TxId(2), TxStatus::Success));
    assert_eq!(
        model.lifecycle(LifecycleCommand::Cancel(TxId(2))),
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
    let frame = header(1);
    let payload = TxPayload::Mpdu {
        frame: &frame,
        response: TxResponse::None,
    };
    assert_eq!(
        model.submit(attempt(1, payload, OFDM24)),
        Ok(Err(SubmitError::Disabled))
    );
}

#[test]
fn received_frames_are_viewed_with_portable_metadata_and_loss_is_reported_once() {
    let model = enabled_station();
    let frame = header(3);
    let meta = RxMeta {
        rate: RxEvidence::HardwareObserved(OFDM24),
        rssi_dbm: RxEvidence::HardwareObserved(-40),
        ..RxMeta::unavailable(channel_six())
    };
    model.receive(&frame, meta);
    let event = next(&model);
    assert_eq!(
        Model::view(&event),
        LowerMacEvent::Received {
            frame: &frame,
            meta
        }
    );

    for _ in 0..=EVENT_CAPACITY {
        model.tbtt(STATION);
    }
    assert!(matches!(poll_event(&model), Some(Err(EventsLost))));
    for _ in 0..EVENT_CAPACITY {
        assert!(matches!(
            Model::view(&next(&model)),
            LowerMacEvent::Tbtt { vif: STATION, .. }
        ));
    }
    assert!(poll_event(&model).is_none());
}

#[test]
fn tsf_and_settings_address_configured_interfaces_only() {
    let model = enabled_station();
    assert_eq!(
        model.apply(LowerMacSetting::SetTsf {
            vif: STATION,
            tsf: Tsf(1_024_000),
        }),
        Ok(Ok(()))
    );
    assert_eq!(model.tsf(STATION), Ok(Ok(Tsf(1_024_000))));
    assert_eq!(model.tsf(VifId(1)), Ok(Err(SettingError::UnknownVif)));
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
    assert_eq!(
        model.apply(LowerMacSetting::CoexPriority(CoexPriority::Elevated)),
        Ok(Ok(()))
    );
    assert!(model.now().unwrap() < model.now().unwrap());
}
