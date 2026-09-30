//! The contract behaviour a caller relies on, shown against the in-memory
//! [`LowerMacModel`](crate::model::LowerMacModel), which implements the port
//! and its extensions from portable values alone.

use core::{
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};
use std::vec::Vec;

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
        LowerMacModel as Model, MODEL_CAPABILITIES as CAPABILITIES,
        MODEL_EVENT_CAPACITY as EVENT_CAPACITY, MODEL_MAX_MPDU as MAX_MPDU,
        MODEL_TX_BUFFERS as BUFFERS, ModelBuffer,
    },
    *,
};

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
    assert_eq!(model.buffers_lent(), 0);
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
