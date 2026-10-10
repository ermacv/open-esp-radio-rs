//! The contract behaviour a caller relies on, shown against the in-memory
//! [`LowerMacModel`](crate::model::LowerMacModel), which implements the port
//! and its extensions from portable values alone.

use core::{
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};
use std::{vec, vec::Vec};

use crate::Ieee80211Instant;
use oer_ieee80211_mac::tsf::{TsfInstant, time_units};

use oer_ieee80211_mac::{
    phy::{
        DsssPreamble, FecCoding, HeGiLtf, HeMcs, HeRate, HtMcs, LegacyRate, PpduBandwidth,
        SpatialStreams,
    },
    qos::WmmAccessCategory,
    sequence::SequenceNumber,
};

use crate::{
    model::{
        LowerMacModel, MODEL_CAPABILITIES as CAPABILITIES, MODEL_EVENT_CAPACITY as EVENT_CAPACITY,
        MODEL_MAX_MPDU as MAX_MPDU, MODEL_TX_BUFFERS as BUFFERS, ModelBody, ModelBuffer, ready,
    },
    *,
};

/// The model with bodies a test owns.
type Model = LowerMacModel<ModelBody>;

/// Poll `future` once without an executor; `None` when it is pending.
fn poll_once<F: Future>(future: F) -> Option<F::Output> {
    match pin!(future)
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(output) => Some(output),
        Poll::Pending => None,
    }
}

/// Take the next event without an executor; `None` when none is ready.
fn poll_event<P: Ieee80211LowerMacPort>(
    port: &P,
) -> Option<PortResult<P::Event, EventsLost, P::Fault>> {
    poll_once(port.next_event())
}

fn next<P: Ieee80211LowerMacPort>(port: &P) -> P::Event {
    poll_event(port)
        .expect("an event is ready")
        .expect("the port is not poisoned")
        .expect("no event was lost")
}

fn next_completion<P: Ieee80211LowerMacPort>(port: &P) -> TxCompletion {
    match P::view(&next(port)) {
        LowerMacEvent::TxCompleted(completion) => completion,
        other => panic!("expected a completion, got {other:?}"),
    }
}

/// The next event, a completion, with the bodies it carries.
fn next_completed<P: Ieee80211LowerMacPort>(port: &P) -> (TxCompletion, P::TxBodies) {
    match P::into_completed(next(port)) {
        Ok(completed) => completed,
        Err(other) => panic!("expected a completion, got {:?}", P::view(&other)),
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
    let model = Model::new();
    assert_eq!(
        model.apply(LowerMacSetting::Channel(channel_six())),
        Ok(Ok(()))
    );
    assert_eq!(ready(model.lifecycle(LifecycleCommand::Enable)), Ok(Ok(())));
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
    let mut buffer = port
        .tx_buffer(frame.len())
        .ok()
        .and_then(Result::ok)
        .flatten()
        .expect("a free buffer");
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

fn mpdu(model: &Model, id: u32, sequence: u16) -> MpduAttempt<ModelBuffer, ModelBody> {
    attempt(
        id,
        TxPayload {
            frame: buffer(model, &header(sequence)),
            body: None,
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
    assert_eq!(model.buffers_lent(), 1);
    assert!(poll_event(&model).is_none());

    model.complete(
        CAPABILITIES.tx_queue(WmmAccessCategory::BestEffort),
        TxStatus::Success,
    );
    let completion = next_completion(&model);
    assert_eq!(completion.id, TxId(9));
    assert_eq!(completion.status, TxStatus::Success);
    assert_eq!(completion.block_ack, None);
    assert_eq!(model.buffers_lent(), 0);
    assert!(poll_event(&model).is_none());
}

#[test]
fn buffers_are_bounded_and_an_unsubmitted_one_is_released() {
    let model = enabled_station();
    assert_eq!(model.tx_buffer(MAX_MPDU + 1), Ok(Ok(None)));
    let held: Vec<_> = (0..BUFFERS)
        .map(|_| model.tx_buffer(24).unwrap().unwrap().unwrap())
        .collect();
    assert_eq!(model.tx_buffer(24), Ok(Ok(None)));
    for buffer in held {
        model.release_tx_buffer(buffer);
    }
    assert!(model.tx_buffer(24).unwrap().unwrap().is_some());
}

#[test]
fn a_port_without_its_backend_refuses_and_hands_the_attempt_and_body_back() {
    let model = enabled_station();
    model.uninstall();
    let mut with_body = mpdu(&model, 1, 1);
    with_body.payload.body = Some(ModelBody(b"kept by the caller".to_vec()));
    let Ok(Err(refused)) = model.submit(with_body) else {
        panic!("a port without its backend refuses the attempt");
    };
    assert_eq!(refused.error, SubmitError::NotInstalled);
    assert_eq!(
        refused.attempt.payload.body,
        Some(ModelBody(b"kept by the caller".to_vec()))
    );
    assert_eq!(model.bodies_held(), 0);
    model.release_tx_buffer(refused.attempt.payload.frame);
    // Installed again, the port admits.
    model.install();
    assert_eq!(model.submit(mpdu(&model, 1, 1)), Ok(Ok(())));
    // Only a poisoned port fails outside the refusal.
    let late = TxAttempt {
        access_category: WmmAccessCategory::Voice,
        ..mpdu(&model, 2, 2)
    };
    model.poison();
    assert_eq!(
        model.submit(late),
        Err(Poisoned {
            cause: model::ModelFault
        })
    );
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
    assert_eq!(model.buffers_lent(), 0);
}

#[test]
fn values_outside_the_limits_are_refused_as_unsupported() {
    let model = enabled_station();
    let refuse = |mutate: fn(&mut MpduAttempt<ModelBuffer, ModelBody>)| {
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
        (|request: &mut MpduAttempt<ModelBuffer, ModelBody>| {
            request.backoff = Backoff::HardwareDraw { cw_exponent: 4 };
        }) as fn(&mut MpduAttempt<ModelBuffer, ModelBody>),
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
    let mut aggregate = model.ampdu_buffer().unwrap().unwrap().unwrap();
    assert_eq!(model.ampdu_buffer(), Ok(Ok(None)));
    for sequence in [100, 101] {
        aggregate
            .push_mpdu(24, None)
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
    assert!(model.ampdu_buffer().unwrap().unwrap().is_some());
}

#[test]
fn an_empty_aggregate_is_refused() {
    let model = enabled_station();
    let payload = AmpduPayload {
        subframes: model.ampdu_buffer().unwrap().unwrap().unwrap(),
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
        .push_mpdu(24, None)
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
    let model = Model::new();
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

    assert_eq!(ready(model.cancel(TxId(1))), Ok(Ok(())));
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
        ready(model.cancel(TxId(2))),
        Ok(Err(CancelError::NotRunning))
    );
}

#[test]
fn cancel_of_a_published_attempt_ends_with_its_own_completion() {
    let model = enabled_station();
    assert_eq!(model.submit(mpdu(&model, 1, 1)), Ok(Ok(())));
    assert_eq!(ready(model.cancel(TxId(1))), Ok(Ok(())));
    assert!(poll_event(&model).is_none());
    model.complete(0, TxStatus::Success);
    assert_eq!(next_completion(&model).status, TxStatus::Success);

    // The extension withdraws it from the air.
    assert_eq!(model.submit(mpdu(&model, 2, 2)), Ok(Ok(())));
    assert_eq!(model.cancel_published(TxId(2)), Ok(Ok(())));
    assert_eq!(next_completion(&model).status, TxStatus::Aborted);
    assert_eq!(
        model.cancel_published(TxId(2)),
        Ok(Err(CancelError::NotRunning))
    );
}

#[test]
fn quiesce_ends_with_its_terminal_event_and_stops_admission() {
    let model = enabled_station();
    assert_eq!(
        ready(model.lifecycle(LifecycleCommand::Quiesce)),
        Ok(Ok(()))
    );
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
    let model = Model::new();
    assert_eq!(ready(model.lifecycle(LifecycleCommand::Enable)), Ok(Ok(())));
    assert_eq!(
        Model::view(&next(&model)),
        LowerMacEvent::Lifecycle(LifecycleEvent::Failed {
            command: LifecycleCommand::Enable,
        })
    );
    // The port stayed disabled and can be enabled once it can tune.
    model
        .apply(LowerMacSetting::Channel(channel_six()))
        .unwrap()
        .unwrap();
    assert_eq!(ready(model.lifecycle(LifecycleCommand::Enable)), Ok(Ok(())));
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
fn loss_is_reported_once_in_place_of_the_first_dropped_event() {
    let model = enabled_station();
    for _ in 0..EVENT_CAPACITY + 2 {
        model.fire_tbtt(STATION);
    }
    // The events before the gap come first, then one marker.
    for _ in 0..EVENT_CAPACITY {
        let event = next(&model);
        assert_eq!(Model::view(&event), LowerMacEvent::Extension);
        assert_eq!(
            Model::tbtt(&event),
            Some(TbttEvent {
                tbtt: VifTsf::new(STATION, TsfInstant::from_micros(0))
            })
        );
    }
    assert!(matches!(poll_event(&model), Some(Ok(Err(EventsLost)))));
    assert!(poll_event(&model).is_none());
    // Events after the gap follow the marker.
    model.fire_tbtt(STATION);
    assert_eq!(Model::view(&next(&model)), LowerMacEvent::Extension);
}

#[test]
fn a_completion_holds_the_slot_its_attempt_reserved_and_is_never_lost() {
    let model = enabled_station();
    assert_eq!(model.submit(mpdu(&model, 1, 1)), Ok(Ok(())));
    // Non-terminal events fill the queue and overflow it.
    for _ in 0..EVENT_CAPACITY + 1 {
        model.fire_tbtt(STATION);
    }
    model.complete(0, TxStatus::Success);
    for _ in 0..EVENT_CAPACITY {
        assert_eq!(Model::view(&next(&model)), LowerMacEvent::Extension);
    }
    // The loss covers only the TBTT; the completion follows it.
    assert!(matches!(poll_event(&model), Some(Ok(Err(EventsLost)))));
    assert_eq!(next_completion(&model).id, TxId(1));
    assert!(poll_event(&model).is_none());
}

#[test]
fn a_poisoned_port_reports_its_terminal_event_after_the_earlier_ones() {
    let model = enabled_station();
    assert_eq!(model.submit(mpdu(&model, 1, 1)), Ok(Ok(())));
    model.complete(0, TxStatus::Success);
    model.poison();
    // The completion produced before the fault is still reported.
    assert_eq!(next_completion(&model).id, TxId(1));
    // Then the poisoning with its cause, at every call.
    let poisoned = Poisoned {
        cause: model::ModelFault,
    };
    for _ in 0..2 {
        assert!(matches!(poll_event(&model), Some(Err(cause)) if cause == poisoned));
    }
    assert_eq!(model.tx_buffer(24).unwrap_err(), poisoned);
    assert_eq!(
        ready(model.lifecycle(LifecycleCommand::Disable)),
        Err(poisoned)
    );
    assert_eq!(ready(model.cancel(TxId(1))), Err(poisoned));
    assert_eq!(ready(model.now()), Err(poisoned));
}

#[test]
fn the_model_clock_is_monotonic_and_samples_one_reading() {
    let model = Model::new();
    assert_eq!(model.clock_info().epoch, RadioEpoch::Monotonic);
    model.set_now(Ieee80211Instant::from_micros(1_234));
    let sample = model.clock_sample().unwrap().unwrap();
    assert_eq!(sample.radio, Ieee80211Instant::from_micros(1_234));
    assert_eq!(sample.monotonic.as_micros(), 1_234);
    assert_eq!(
        model
            .clock_info()
            .to_monotonic_with(sample.stamp(), &sample)
            .map(|projected| projected.at.as_micros()),
        Ok(1_234)
    );
}

#[test]
fn beacon_timing_addresses_configured_interfaces_within_its_roles() {
    let model = enabled_station();
    let at = VifTsf::new(STATION, TsfInstant::from_micros(1_024_000));
    assert_eq!(model.set_tsf(at), Ok(Ok(())));
    assert_eq!(model.tsf(STATION), Ok(Ok(at)));
    assert_eq!(model.tsf(VifId(1)), Ok(Err(SettingError::UnknownVif)));
    let schedule = TbttSchedule {
        next: at,
        beacon_interval: time_units(100),
        lead: oer_time::RadioDuration::from_micros(3_000),
    };
    assert_eq!(model.set_tbtt(schedule), Ok(Ok(())));
    assert_eq!(model.stop_tbtt(STATION), Ok(Ok(())));
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
        model.set_tbtt(TbttSchedule {
            next: VifTsf::new(ACCESS_POINT, at.at),
            ..schedule
        }),
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
}

#[test]
fn the_radio_clock_reads_the_time_the_test_sets() {
    let model = Model::new();
    assert_eq!(ready(model.now()), Ok(Ok(Ieee80211Instant::from_micros(0))));
    model.set_now(Ieee80211Instant::from_micros(250));
    assert_eq!(
        ready(model.now()),
        Ok(Ok(Ieee80211Instant::from_micros(250)))
    );
    assert_eq!(
        ready(model.now()),
        Ok(Ok(Ieee80211Instant::from_micros(250))),
        "time stands still until the test moves it"
    );
}

#[test]
#[should_panic(expected = "runs backwards")]
fn the_radio_clock_never_runs_backwards() {
    let model = Model::new();
    model.set_now(Ieee80211Instant::from_micros(250));
    model.set_now(Ieee80211Instant::from_micros(249));
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
    requires_every_extension(&Model::new());
}

#[test]
fn a_block_ack_report_acknowledges_its_bitmap_and_a_bounded_predecessor() {
    let report = BlockAckReport {
        start_sequence: SequenceNumber::new(4094).unwrap(),
        bitmap: 0b101,
    };
    let acknowledged = |sequence| report.acknowledges(SequenceNumber::new(sequence).unwrap());
    assert!(acknowledged(4094));
    assert!(!acknowledged(4095));
    // Bit two wraps to sequence zero.
    assert!(acknowledged(0));
    // Before the start: an advanced starting sequence already covered it.
    assert!(acknowledged(4093));
    assert!(acknowledged(4094 - 64));
    assert!(!acknowledged(4094 - 65));
    // Beyond the bitmap: not yet reported.
    assert!(!acknowledged(62));
}

#[test]
fn tsf_values_of_different_interfaces_do_not_combine() {
    let station = VifTsf::new(STATION, TsfInstant::from_micros(5_000));
    let access_point = VifTsf::new(ACCESS_POINT, TsfInstant::from_micros(1_000));
    assert_eq!(
        station.checked_duration_since(access_point),
        Err(TsfArithmeticError::VifMismatch(TsfVifMismatch {
            left: STATION,
            right: ACCESS_POINT
        }))
    );
    let earlier = VifTsf::new(STATION, TsfInstant::from_micros(1_000));
    assert_eq!(
        station.checked_duration_since(earlier),
        Ok(oer_time::RadioDuration::from_micros(4_000))
    );
    assert_eq!(
        earlier.checked_add(oer_time::RadioDuration::from_micros(4_000)),
        Some(station)
    );
    assert_eq!(time_units(100).as_micros(), 102_400);
}

#[test]
fn a_tsf_sample_projects_both_ways_with_the_drift_bound() {
    const CURRENT: TsfGeneration = TsfGeneration { epoch: 2, jump: 7 };
    let sample = TsfSample {
        tsf: VifTsf::new(STATION, TsfInstant::from_micros(10_000_000)),
        local: Ieee80211Stamp {
            at: Ieee80211Instant::from_micros(500_000),
            generation: 3,
        },
        uncertainty: oer_time::Duration::from_micros(2),
        generation: CURRENT,
    };
    let distance = 1_000_000;
    let drift = (distance * u64::from(TSF_DRIFT_PPM)).div_ceil(1_000_000);
    let later = Ieee80211Stamp {
        at: Ieee80211Instant::from_micros(500_000 + distance),
        generation: 3,
    };
    let projected = sample.tsf_at(later, CURRENT).unwrap();
    assert_eq!(projected.at.at.as_micros(), 10_000_000 + distance);
    assert_eq!(projected.at.vif, STATION);
    assert_eq!(projected.uncertainty.as_micros(), 2 + drift);
    // The inverse lands on the same radio stamp.
    let back = sample.local_at(projected.at, CURRENT).unwrap().unwrap();
    assert_eq!(back.at, later);
    assert_eq!(back.uncertainty.as_micros(), 2 + drift);
    // A stamp of another radio-clock generation, a TSF of another
    // interface, and a value before the TSF's start do not project.
    let stale = Ieee80211Stamp {
        generation: 4,
        ..later
    };
    assert_eq!(
        sample.tsf_at(stale, CURRENT),
        Err(TsfProjectionError::StaleStamp)
    );
    // A sample of an earlier generation of the relation does not convert.
    let later_generation = TsfGeneration {
        jump: CURRENT.jump + 1,
        ..CURRENT
    };
    assert_eq!(
        sample.tsf_at(later, later_generation),
        Err(TsfProjectionError::StaleSample)
    );
    assert_eq!(
        sample.local_at(projected.at, later_generation),
        Ok(Err(TsfProjectionError::StaleSample))
    );
    assert!(
        sample
            .local_at(VifTsf::new(ACCESS_POINT, projected.at.at), CURRENT)
            .is_err()
    );
    let before_start = Ieee80211Stamp {
        at: Ieee80211Instant::from_micros(0),
        generation: 3,
    };
    assert_eq!(
        TsfSample {
            tsf: VifTsf::new(STATION, TsfInstant::from_micros(10)),
            ..sample
        }
        .tsf_at(before_start, CURRENT),
        Err(TsfProjectionError::OutOfRange)
    );
}

#[test]
fn the_model_tsf_advances_with_its_clock_and_a_jump_starts_a_generation() {
    let model = enabled_station();
    model.set_now(Ieee80211Instant::from_micros(100));
    let first = model.tsf_sample(STATION).unwrap().unwrap();
    model
        .set_tsf(VifTsf::new(STATION, TsfInstant::from_micros(1_000_000)))
        .unwrap()
        .unwrap();
    model.set_now(Ieee80211Instant::from_micros(600));
    let sample = model.tsf_sample(STATION).unwrap().unwrap();
    assert_eq!(sample.tsf.at.as_micros(), 1_000_500);
    assert_eq!(sample.local.at, Ieee80211Instant::from_micros(600));
    assert_ne!(sample.generation, first.generation);
    model.fire_tbtt(STATION);
    assert_eq!(
        Model::tbtt(&next(&model)).map(|event| event.tbtt),
        Some(sample.tsf)
    );
}

#[test]
fn live_retuning_preserves_both_tsfs_configuration_and_completed_tx_ownership() {
    let model = enabled_station();
    let ap = VifConfig {
        address: OTHER_BSS,
        role: VifRole::AccessPoint,
        bssid: Some(OTHER_BSS),
        receive: ReceiveFilter::BSS_MEMBER,
    };
    model
        .apply(LowerMacSetting::Vif {
            vif: ACCESS_POINT,
            config: Some(ap),
        })
        .unwrap()
        .unwrap();
    model.set_now(Ieee80211Instant::from_micros(100));
    model
        .set_tsf(VifTsf::new(STATION, TsfInstant::from_micros(1_000_000)))
        .unwrap()
        .unwrap();
    model
        .set_tsf(VifTsf::new(
            ACCESS_POINT,
            TsfInstant::from_micros(2_000_000),
        ))
        .unwrap()
        .unwrap();
    let samples = [
        model.tsf_sample(STATION).unwrap().unwrap(),
        model.tsf_sample(ACCESS_POINT).unwrap().unwrap(),
    ];
    let clock = model.clock_sample().unwrap().unwrap();
    let agreement = RxBlockAckAgreement {
        vif: STATION,
        peer: PEER,
        tid: 0,
        start_sequence: SequenceNumber::ZERO,
        window: 64,
    };
    model
        .apply(LowerMacSetting::AddRxBlockAck(agreement))
        .unwrap()
        .unwrap();
    let handle = model
        .install_key(KeyInstall {
            vif: STATION,
            cipher: Cipher::Ccmp128,
            scope: KeyScope::Pairwise { peer: PEER },
            key: &[0x11; 16],
        })
        .unwrap()
        .unwrap();
    let body = ModelBody(b"retained across the visit".to_vec());
    let mut outgoing = mpdu(&model, 1, 1);
    outgoing.key = KeySelector::Key(handle);
    let mut frame = model
        .tx_buffer(24 + body.0.len())
        .unwrap()
        .unwrap()
        .unwrap();
    frame.frame_mut()[..24].copy_from_slice(&header(1));
    model.release_tx_buffer(outgoing.payload.frame);
    outgoing.payload.frame = frame;
    outgoing.payload.body = Some(body.clone());
    model.submit(outgoing).unwrap().unwrap();
    model.complete(0, TxStatus::Success);
    // Keep the completion, which carries the body, queued during the visit.
    model
        .apply(LowerMacSetting::TxGate { open: false })
        .unwrap()
        .unwrap();
    let lifecycle = model.lifecycle_requests();
    let away = Channel::ghz2_4(11, ChannelWidth::Mhz20).unwrap();
    model.retune_live(away).unwrap().unwrap();
    model.set_now(Ieee80211Instant::from_micros(600));
    model.retune_live(channel_six()).unwrap().unwrap();
    for (vif, before) in [STATION, ACCESS_POINT].into_iter().zip(samples) {
        let after = model.tsf_sample(vif).unwrap().unwrap();
        assert_eq!(after.generation, before.generation);
        assert_eq!(after.tsf.at.as_micros(), before.tsf.at.as_micros() + 500);
    }
    assert_eq!(
        model.clock_sample().unwrap().unwrap().generation,
        clock.generation
    );
    assert_eq!(model.vif_config(STATION), Some(station_config()));
    assert_eq!(model.vif_config(ACCESS_POINT), Some(ap));
    assert_eq!(model.installed_keys(), [KeyScope::Pairwise { peer: PEER }]);
    assert_eq!(model.rx_block_acks(), [agreement]);
    assert!(!model.gate_open());
    assert_eq!(model.lifecycle_requests(), lifecycle);
    let (completion, bodies) = next_completed(&model);
    assert_eq!(completion.id, TxId(1));
    assert_eq!(bodies, [(0, body)]);
    assert_eq!(model.bodies_held(), 0);
}

#[test]
fn live_retuning_refuses_an_admitted_tx_or_a_disabled_port_without_changing_channel() {
    let model = enabled_station();
    let away = Channel::ghz2_4(11, ChannelWidth::Mhz20).unwrap();
    model.submit(mpdu(&model, 1, 1)).unwrap().unwrap();
    assert_eq!(model.retune_live(away), Ok(Err(SettingError::Busy)));
    assert_eq!(model.channel(), Some(channel_six()));
    assert_eq!(model.in_flight(), 1);
    model.complete(0, TxStatus::Success);
    assert_eq!(next_completion(&model).status, TxStatus::Success);
    ready(model.lifecycle(LifecycleCommand::Disable))
        .unwrap()
        .unwrap();
    assert_eq!(model.retune_live(away), Ok(Err(SettingError::Busy)));
    assert_eq!(model.channel(), Some(channel_six()));
}

#[test]
fn a_tsf_relation_keeps_its_generation_through_drift_after_missed_beacons() {
    let mut relation = TsfRelation::new(4, oer_time::Duration::from_micros(1));
    let tsf = TsfInstant::from_micros;
    let interval = 102_400;
    assert_eq!(
        relation.set(tsf(0), tsf(1_000_000)).unwrap(),
        TsfSetKind::Jump
    );
    let generation = relation.generation();
    // Ten missed beacons later the follow corrects ten intervals of drift:
    // more than one interval allows, within what ten allow.
    let ten = relation
        .tolerance(oer_time::RadioDuration::from_micros(10 * interval))
        .unwrap()
        .as_micros();
    let one = relation
        .tolerance(oer_time::RadioDuration::from_micros(interval))
        .unwrap()
        .as_micros();
    assert!(ten > one);
    let reading = 1_000_000 + 10 * interval;
    assert_eq!(
        relation.set(tsf(reading), tsf(reading + ten)).unwrap(),
        TsfSetKind::Drift
    );
    assert_eq!(relation.generation(), generation);
    // Each bound is the drift of its span plus the sample's microsecond.
    assert_eq!(
        one,
        (interval * u64::from(TSF_DRIFT_PPM)).div_ceil(1_000_000) + 1
    );
}

#[test]
fn a_tsf_relation_starts_a_generation_at_a_jump_between_consecutive_beacons() {
    let mut relation = TsfRelation::new(4, oer_time::Duration::from_micros(1));
    let tsf = TsfInstant::from_micros;
    let interval = 102_400;
    relation.set(tsf(0), tsf(1_000_000)).unwrap();
    let generation = relation.generation();
    let one = relation
        .tolerance(oer_time::RadioDuration::from_micros(interval))
        .unwrap()
        .as_micros();
    let reading = 1_000_000 + interval;
    assert_eq!(
        relation.set(tsf(reading), tsf(reading + one + 1)).unwrap(),
        TsfSetKind::Jump
    );
    assert_ne!(relation.generation(), generation);
    // A break forgets the sample: the next set is a jump again.
    let after_jump = relation.generation();
    relation.break_relation();
    assert_ne!(relation.generation(), after_jump);
    assert_eq!(
        relation.set(tsf(reading), tsf(reading)).unwrap(),
        TsfSetKind::Jump
    );
}

#[test]
fn a_tsf_crossing_2_pow_64_starts_a_generation() {
    let mut relation = TsfRelation::new(5, oer_time::Duration::from_micros(1));
    let tsf = TsfInstant::from_micros;
    relation.set(tsf(0), tsf(u64::MAX - 100)).unwrap();
    // A set across 2^64: a few microseconds on the air, a jump in order.
    let before = relation.generation();
    assert_eq!(
        relation.set(tsf(u64::MAX - 10), tsf(20)).unwrap(),
        TsfSetKind::Jump
    );
    assert_ne!(relation.generation(), before);
    // The counter itself passed 2^64 since the last sample: even a set
    // within the sample uncertainty of the reading is a jump.
    relation.set(tsf(30), tsf(u64::MAX - 100)).unwrap();
    let before = relation.generation();
    assert_eq!(relation.set(tsf(5), tsf(6)).unwrap(), TsfSetKind::Jump);
    assert_ne!(relation.generation(), before);
}

#[test]
fn tsf_relations_of_different_epochs_never_share_a_generation() {
    let first = TsfRelation::new(1, oer_time::Duration::ZERO);
    let mut second = TsfRelation::new(2, oer_time::Duration::ZERO);
    assert_ne!(first.generation(), second.generation());
    // Jumps count within an epoch: the second relation's first jump is not
    // the first relation's.
    let mut first_jumped = first;
    first_jumped.break_relation();
    second.break_relation();
    assert_eq!(first_jumped.generation().jump, second.generation().jump);
    assert_ne!(first_jumped.generation(), second.generation());
}

#[test]
fn an_edca_set_applies_whole_or_not_at_all() {
    use oer_ieee80211_mac::extensions::wmm::{WmmAcParameters, WmmParameterSet};
    let model = enabled_station();
    assert_eq!(model.edca(), None);
    let record = |aifsn| WmmAcParameters {
        admission_control_mandatory: false,
        aifsn,
        ecw_min: 4,
        ecw_max: 10,
        txop_limit_units_32_us: 94,
    };
    let set = WmmParameterSet::new(1, false, [record(3), record(7), record(2), record(2)]);
    assert_eq!(model.apply(LowerMacSetting::Edca(set)), Ok(Ok(())));
    assert_eq!(model.edca(), Some(set));
    // An AIFSN below two keeps the set before it.
    let refused = WmmParameterSet::new(2, false, [record(3), record(1), record(2), record(2)]);
    assert_eq!(
        model.apply(LowerMacSetting::Edca(refused)),
        Ok(Err(SettingError::Unsupported))
    );
    assert_eq!(model.edca(), Some(set));
}

#[test]
fn a_received_frame_is_the_backend_s_buffer_until_it_is_dropped() {
    let model = enabled_station();
    assert_eq!(model.set_monitor(true), Ok(Ok(())));
    let meta = RxMeta::unavailable(channel_six());
    let beacon = management(8, [0xff; 6], OTHER_BSS);
    model.receive(&beacon, meta);
    assert_eq!(model.rx_buffers_lent(), 1);

    let Ok((buffer, received)) = Model::into_received(next(&model)) else {
        panic!("a received event lends its frame");
    };
    assert_eq!((buffer.bytes(), received), (beacon.as_slice(), meta));
    assert_eq!(model.rx_buffers_lent(), 1);
    drop(buffer);
    assert_eq!(model.rx_buffers_lent(), 0);

    // Any other event comes back unchanged.
    assert_eq!(
        ready(model.lifecycle(LifecycleCommand::Disable)),
        Ok(Ok(()))
    );
    let Err(event) = Model::into_received(next(&model)) else {
        panic!("a lifecycle event lends no frame");
    };
    assert_eq!(
        Model::view(&event),
        LowerMacEvent::Lifecycle(LifecycleEvent::Disabled)
    );
}

#[test]
fn an_access_point_receives_the_probe_requests_it_answers() {
    let access_point = VifConfig {
        address: OTHER_BSS,
        role: VifRole::AccessPoint,
        bssid: None,
        receive: ReceiveFilter::BSS_MEMBER.union(ReceiveFilter::PROBE_REQUESTS),
    };
    // Broadcast with the wildcard BSSID, and directed with its own.
    assert!(access_point.admits(&management(4, [0xff; 6], [0xff; 6])));
    assert!(access_point.admits(&management(4, OTHER_BSS, OTHER_BSS)));
    // Another BSS's.
    assert!(!access_point.admits(&management(4, [0xff; 6], [0x02, 0, 0, 0, 0, 0x55])));
    // Without the rule, a wildcard request is another BSS's management.
    let member = VifConfig {
        receive: ReceiveFilter::BSS_MEMBER,
        ..access_point
    };
    assert!(!member.admits(&management(4, [0xff; 6], [0xff; 6])));
}

#[test]
fn a_body_travels_by_ownership_and_comes_back_once_its_attempt_ended() {
    let model = enabled_station();
    let body = ModelBody(b"payload".to_vec());
    let mut frame = model.tx_buffer(24 + 7).unwrap().unwrap().unwrap();
    frame.frame_mut()[..24].copy_from_slice(&header(7));
    let payload = TxPayload {
        frame,
        body: Some(body.clone()),
        response: TxResponse::Ack,
    };
    assert_eq!(payload.header_len(), Some(24));
    assert_eq!(model.submit(attempt(1, payload, OFDM24)), Ok(Ok(())));
    // The MPDU went whole: the header the caller wrote, then the body.
    let sent = &model.submitted()[0].frames[0];
    assert_eq!(
        (&sent[..24], &sent[24..]),
        (&header(7)[..], &b"payload"[..])
    );
    // The port holds the body while the attempt runs, and while its
    // completion waits to be taken.
    assert_eq!(model.bodies_held(), 1);
    model.complete(0, TxStatus::Success);
    assert_eq!(model.bodies_held(), 1);
    // The completion event owns the body: taking the event takes it, once.
    let event = next(&model);
    assert_eq!(model.bodies_held(), 0);
    let Ok((completion, bodies)) = Model::into_completed(event) else {
        panic!("a completion");
    };
    assert_eq!(completion.id, TxId(1));
    assert_eq!(bodies, [(0, body)]);
}

#[test]
fn a_completion_without_a_body_carries_none_and_other_events_come_back() {
    let model = enabled_station();
    model.submit(mpdu(&model, 1, 1)).unwrap().unwrap();
    model.complete(0, TxStatus::Success);
    let (completion, bodies) = next_completed(&model);
    assert_eq!(completion.id, TxId(1));
    assert!(bodies.is_empty());
    model.fire_tbtt(STATION);
    let Err(event) = Model::into_completed(next(&model)) else {
        panic!("a TBTT is no completion");
    };
    assert_eq!(Model::view(&event), LowerMacEvent::Extension);
}

/// An attempt `id` whose MPDU carries `body`.
fn with_body(model: &Model, id: u32, body: &ModelBody) -> MpduAttempt<ModelBuffer, ModelBody> {
    let len = body.0.len();
    let mut frame = model.tx_buffer(24 + len).unwrap().unwrap().unwrap();
    frame.frame_mut()[..24].copy_from_slice(&header(7));
    attempt(
        id,
        TxPayload {
            frame,
            body: Some(body.clone()),
            response: TxResponse::Ack,
        },
        OFDM24,
    )
}

#[test]
fn a_failed_or_cancelled_attempt_s_completion_carries_its_body_back() {
    let model = enabled_station();
    let failed = ModelBody(b"failed".to_vec());
    assert_eq!(model.submit(with_body(&model, 1, &failed)), Ok(Ok(())));
    model.complete(0, TxStatus::AckTimeout);
    let (completion, bodies) = next_completed(&model);
    assert_eq!(
        (completion.id, completion.status),
        (TxId(1), TxStatus::AckTimeout)
    );
    assert_eq!(bodies, [(0, failed)]);

    // A cancelled attempt that was never published.
    assert_eq!(
        model.apply(LowerMacSetting::TxGate { open: false }),
        Ok(Ok(()))
    );
    let cancelled = ModelBody(b"cancelled".to_vec());
    assert_eq!(model.submit(with_body(&model, 2, &cancelled)), Ok(Ok(())));
    assert_eq!(ready(model.cancel(TxId(2))), Ok(Ok(())));
    let (completion, bodies) = next_completed(&model);
    assert_eq!(
        (completion.id, completion.status),
        (TxId(2), TxStatus::Aborted)
    );
    assert_eq!(bodies, [(0, cancelled)]);
    assert_eq!(model.bodies_held(), 0);
}

#[test]
fn a_refused_attempt_keeps_its_body() {
    let model = enabled_station();
    let mut frame = model.tx_buffer(30).unwrap().unwrap().unwrap();
    frame.frame_mut()[..24].copy_from_slice(&header(7));
    // A body longer than the MPDU leaves no header.
    let payload = TxPayload {
        frame,
        body: Some(ModelBody(vec![0; 31])),
        response: TxResponse::Ack,
    };
    let Ok(Err(refused)) = model.submit(attempt(1, payload, OFDM24)) else {
        panic!("a body longer than its MPDU");
    };
    assert_eq!(refused.error, SubmitError::InvalidLength);
    assert_eq!(refused.attempt.payload.body, Some(ModelBody(vec![0; 31])));
    assert_eq!(model.bodies_held(), 0);
    model.release_tx_buffer(refused.attempt.payload.frame);
}

#[test]
fn an_aggregate_s_bodies_come_back_by_subframe() {
    let model = enabled_station();
    let mut aggregate = model.ampdu_buffer().unwrap().unwrap().unwrap();
    // A subframe without a body, then two with one.
    aggregate
        .push_mpdu(24, None)
        .unwrap()
        .copy_from_slice(&header(100));
    for (sequence, body) in [(101, &b"one"[..]), (102, b"two")] {
        let written = aggregate
            .push_mpdu(24 + body.len(), Some(ModelBody(body.to_vec())))
            .unwrap();
        assert_eq!(written.len(), 24);
        written.copy_from_slice(&header(sequence));
    }
    // A body longer than its MPDU comes back.
    assert_eq!(
        aggregate.push_mpdu(2, Some(ModelBody(vec![0; 3]))),
        Err(Some(ModelBody(vec![0; 3])))
    );
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
    assert!(model.submitted()[0].frames[2].ends_with(b"two"));
    model.complete(0, TxStatus::Success);
    let (_, bodies) = next_completed(&model);
    let back: Vec<_> = bodies
        .into_iter()
        .map(|(index, body)| (index, body.0))
        .collect();
    assert_eq!(back, [(1, b"one".to_vec()), (2, b"two".to_vec())]);
    assert_eq!(model.bodies_held(), 0);
}

/// A radio on `channel` whose access point interface is `PEER`.
fn access_point_on(channel: Channel) -> Model {
    let model = Model::new();
    assert_eq!(model.apply(LowerMacSetting::Channel(channel)), Ok(Ok(())));
    assert_eq!(ready(model.lifecycle(LifecycleCommand::Enable)), Ok(Ok(())));
    assert_eq!(
        Model::view(&next(&model)),
        LowerMacEvent::Lifecycle(LifecycleEvent::Enabled)
    );
    assert_eq!(
        model.apply(LowerMacSetting::Vif {
            vif: ACCESS_POINT,
            config: Some(VifConfig {
                address: PEER,
                role: VifRole::AccessPoint,
                bssid: Some(PEER),
                receive: ReceiveFilter::BSS_MEMBER,
            }),
        }),
        Ok(Ok(()))
    );
    model
}

#[test]
fn the_air_carries_a_frame_to_the_radio_on_its_channel_which_acknowledges_it() {
    use crate::model::ModelAir;

    let station = enabled_station();
    let access_point = access_point_on(channel_six());
    let air = ModelAir::new([&station, &access_point]);
    assert!(!air.step());

    assert_eq!(station.submit(mpdu(&station, 1, 7)), Ok(Ok(())));
    assert!(air.step());
    assert_eq!(next_completion(&station).status, TxStatus::Success);
    let LowerMacEvent::Received { .. } = Model::view(&next(&access_point)) else {
        panic!("the access point received nothing");
    };
    assert_eq!(station.in_flight(), 0);

    // To an address nobody has, the frame goes unacknowledged.
    let mut frame = header(8);
    frame[4..10].copy_from_slice(&OTHER_BSS);
    let lost = attempt(
        2,
        TxPayload {
            frame: buffer(&station, &frame),
            body: None,
            response: TxResponse::Ack,
        },
        OFDM24,
    );
    assert_eq!(station.submit(lost), Ok(Ok(())));
    assert!(air.step());
    assert_eq!(next_completion(&station).status, TxStatus::AckTimeout);

    // A radio on another channel hears nothing.
    let elsewhere = access_point_on(Channel::ghz2_4(11, ChannelWidth::Mhz20).unwrap());
    let apart = ModelAir::new([&station, &elsewhere]);
    assert_eq!(station.submit(mpdu(&station, 3, 9)), Ok(Ok(())));
    assert!(apart.step());
    assert_eq!(next_completion(&station).status, TxStatus::AckTimeout);
    assert_eq!(elsewhere.queued_events(), 0);
}

#[test]
fn tsf_elapsed_preserves_the_full_range_and_refuses_reversal() {
    let first = VifTsf::new(STATION, TsfInstant::from_micros(0));
    let last = VifTsf::new(STATION, TsfInstant::from_micros(u64::MAX));
    assert_eq!(
        last.checked_duration_since(first),
        Ok(oer_time::RadioDuration::from_micros(u64::MAX))
    );
    assert_eq!(
        first.checked_duration_since(last),
        Err(TsfArithmeticError::ReversedTime)
    );
    assert_eq!(
        first.checked_add(oer_time::RadioDuration::from_micros(u64::MAX)),
        Some(last)
    );
    assert_eq!(
        last.checked_add(oer_time::RadioDuration::from_micros(1)),
        None
    );
}

#[test]
fn tsf_tolerance_failure_preserves_the_relation() {
    let mut relation = TsfRelation::new(7, oer_time::Duration::from_micros(u64::MAX));
    relation
        .set(TsfInstant::from_micros(0), TsfInstant::from_micros(0))
        .unwrap();
    let before = relation;
    assert_eq!(
        relation.set(TsfInstant::from_micros(1), TsfInstant::from_micros(2)),
        Err(TsfTimingError::ToleranceOverflow)
    );
    assert_eq!(relation, before);
    assert_eq!(
        TsfRelation::new(8, oer_time::Duration::ZERO)
            .tolerance(oer_time::RadioDuration::from_micros(u64::MAX)),
        Ok(oer_time::RadioDuration::from_micros(
            (u128::from(u64::MAX) * u128::from(TSF_DRIFT_PPM)).div_ceil(1_000_000) as u64
        ))
    );
}
