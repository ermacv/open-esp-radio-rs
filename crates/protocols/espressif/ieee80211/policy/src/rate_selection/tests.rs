//! The Espressif rate selection against the vendor's records.

extern crate std;

use std::vec::Vec;

use oer_ieee80211_lower_mac::{
    BandSet, CoexPriority, CoexPrioritySet, HardwareServices, LowerMacCapabilities,
    PhyFormatSet, RateSupport, ReceiveFilter, WidthSet,
};
use oer_ieee80211_mac::{
    ht::ht_peer_capabilities,
    phy::{HeMcs, HtMcs},
    station::association::PhyMode,
};

use super::*;
use oer_ieee80211_lower_mac::{Channel, ChannelWidth, LinkMetric, RxMeta, RxMetadata};
use crate::rate_schedule::{RateScheduleKind, schedule_state};

struct Caps;

impl Ieee80211Capabilities for Caps {
    const CAPS: LowerMacCapabilities = LowerMacCapabilities {
        bands: BandSet::GHZ2_4,
        widths: WidthSet::MHZ20,
        rates: RateSupport {
            dsss_cck: true,
            ofdm: true,
            ht_max_mcs: HtMcs::new(7),
            he_max_mcs: HeMcs::new(9),
            he_max_bandwidth_mhz: 20,
            he_dcm: false,
            he_ldpc: true,
            spatial_streams: 1,
        },
        services: HardwareServices::FCS,
        vifs: 1,
        tx_queues: 4,
        max_mpdu_length: 2304,
        max_backoff_slots: 1023,
        tx_power_ceiling_min_dbm: None,
        coex_priorities: CoexPrioritySet::only(CoexPriority::Normal),
        individual_no_ack: PhyFormatSet::NON_HT,
        station_receive_filters: ReceiveFilter::BSS_MEMBER,
        access_point_receive_filters: ReceiveFilter::BSS_MEMBER,
        key_slots: 4,
        rx_block_ack_agreements: 0,
        rx_block_ack_max_tid: 0,
        rx_block_ack_max_window: 0,
        edca_max_aifsn: 15,
        edca_max_cw_exponent: 10,
    };
}

/// The metadata of a frame received `db` dB over the noise floor.
#[derive(Clone, Copy)]
struct Measured(i8, RxMeta);

impl RxMetadata for Measured {
    fn common(&self) -> &RxMeta {
        &self.1
    }
}

impl SignalMetadata for Measured {
    fn link_metric(&self) -> LinkMetric {
        LinkMetric::new(self.0, 0)
    }
}

/// The metadata of an Association Response `db` dB over the noise floor.
fn metric(db: i8) -> Measured {
    Measured(
        db,
        RxMeta::unavailable(Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap()),
    )
}


fn legacy_peer() -> RatePeer {
    RatePeer {
        phy: PhyMode::Legacy,
        ht_capabilities: None,
        he_capabilities: None,
        he_peer_state: None,
    }
}

fn ht20_peer() -> RatePeer {
    let mut element = [0_u8; 28];
    element[..6].copy_from_slice(&[45, 26, 0x20, 0, 0x17, 0xff]);
    RatePeer {
        phy: PhyMode::Ht20,
        ht_capabilities: ht_peer_capabilities(&element),
        he_capabilities: None,
        he_peer_state: None,
    }
}

fn stages(plan: &RetryPlan<PhyRate>) -> Vec<(PhyRate, u8)> {
    plan.stages()
        .map(|stage| (stage.rate, stage.transmissions.get()))
        .collect()
}

/// The rate of every transmission of a control record, by the vendor's
/// walk: the last rate once the record has no further one.
fn record_walk(schedule: RateScheduleRef) -> Vec<PhyRate> {
    let mut rates = Vec::new();
    let mut last = None;
    for failures in 0..schedule_publication_limit(schedule) {
        let rate = schedule_rate_after_failures(schedule, failures)
            .and_then(legacy_rate)
            .map(PhyRate::Legacy)
            .or(last)
            .unwrap();
        last = Some(rate);
        rates.push(rate);
    }
    rates
}

fn plan_walk(plan: &RetryPlan<PhyRate>) -> Vec<PhyRate> {
    stages(plan)
        .into_iter()
        .flat_map(|(rate, transmissions)| core::iter::repeat_n(rate, usize::from(transmissions)))
        .collect()
}

fn completion(status: TxStatus, transmissions: u8) -> TxCompletion {
    TxCompletion {
        status,
        ack_rssi_dbm: None,
        ack_snr_db: Some(30),
        block_ack: None,
        transmissions,
        rate: None,
    }
}

#[test]
fn a_probe_goes_once_at_one_megabit() {
    let selection = EspressifRateSelection::<Caps>::new();
    assert_eq!(
        stages(&RateSelection::<Measured>::plan(&selection, FrameRates::Probe)),
        [(PhyRate::Legacy(LegacyRate::Dsss1M), 1)]
    );
}

#[test]
fn until_associated_control_frames_walk_the_default_record() {
    let selection = EspressifRateSelection::<Caps>::new();
    let plan = RateSelection::<Measured>::plan(&selection, FrameRates::Control);
    assert_eq!(plan_walk(&plan), record_walk(DEFAULT_CONTROL_SCHEDULE));
    assert_eq!(plan.stages().next().unwrap().rate, PhyRate::Legacy(LegacyRate::Dsss1M));
    assert_eq!(plan.transmissions(), u16::from(schedule_publication_limit(DEFAULT_CONTROL_SCHEDULE)));
}

#[test]
fn associated_control_frames_walk_the_association_record_and_leaving_restores_the_default() {
    let mut selection = EspressifRateSelection::<Caps>::new();
    RateSelection::<Measured>::associated(&mut selection, &legacy_peer(), &metric(40));
    let schedule = selection.control().unwrap().association().control_schedule();
    // An 802.11g association: the vendor's BasicOFDMSched, from 6 Mb/s.
    assert_eq!(schedule, RateScheduleRef::new(RateScheduleKind::BasicOfdm, 0).unwrap());
    let plan = RateSelection::<Measured>::plan(&selection, FrameRates::Control);
    assert_eq!(plan_walk(&plan), record_walk(schedule));
    assert_eq!(plan.stages().next().unwrap().rate, PhyRate::Legacy(LegacyRate::Ofdm6M));
    RateSelection::<Measured>::disconnected(&mut selection);
    assert_eq!(
        plan_walk(&RateSelection::<Measured>::plan(&selection, FrameRates::Control)),
        record_walk(DEFAULT_CONTROL_SCHEDULE)
    );
}

#[test]
fn data_starts_at_the_controls_rate_and_steps_down_the_vendor_ladder_within_its_budget() {
    let mut selection = EspressifRateSelection::<Caps>::new();
    RateSelection::<Measured>::associated(&mut selection, &ht20_peer(), &metric(60));
    let control = selection.control().unwrap();
    let initial = control.mpdu_rate();
    let limit = schedule_publication_limit(control.association().current_schedule());
    let plan = RateSelection::<Measured>::plan(&selection, FrameRates::Data);
    let walk = plan_walk(&plan);
    assert_eq!(walk.len(), usize::from(limit.max(1)));
    assert_eq!(walk[0], initial);
    let mut last = initial;
    for (failures, rate) in walk.iter().enumerate().skip(1) {
        let expected = ordinary_retry_rate(initial, failures as u8).unwrap_or(last);
        assert_eq!(*rate, expected, "transmission {failures}");
        last = expected;
    }
    assert!(plan.stages().count() <= RETRY_STAGES);
}

#[test]
fn data_completions_adapt_the_rate_as_the_vendor_control_does() {
    let mut selection = EspressifRateSelection::<Caps>::new();
    let mut reference = EspressifRateControl::for_peer((), &ht20_peer(), LinkMetric::new(60, 0));
    RateSelection::<Measured>::associated(&mut selection, &ht20_peer(), &metric(60));
    let before = RateSelection::<Measured>::plan(&selection, FrameRates::Data).stages().next().unwrap().rate;
    for _ in 0..64 {
        RateSelection::<Measured>::completed(&mut selection, Instant::from_micros(0), FrameRates::Data, &completion(TxStatus::AckTimeout, 8));
        reference.observe_mpdu(8, false, None);
        // Control frames and ones that never went teach nothing.
        RateSelection::<Measured>::completed(&mut selection, Instant::from_micros(0), FrameRates::Control, &completion(TxStatus::AckTimeout, 8));
        RateSelection::<Measured>::completed(&mut selection, Instant::from_micros(0), FrameRates::Data, &completion(TxStatus::Aborted, 0));
    }
    let after = RateSelection::<Measured>::plan(&selection, FrameRates::Data).stages().next().unwrap().rate;
    assert_eq!(after, reference.mpdu_rate());
    assert_eq!(
        selection.control().unwrap().association().current_schedule(),
        reference.association().current_schedule()
    );
    assert!(after.nominal_kbps() < before.nominal_kbps(), "{before:?} -> {after:?}");
}

#[test]
fn the_control_records_a_station_uses_start_with_a_legacy_rate() {
    for schedule in [
        DEFAULT_CONTROL_SCHEDULE,
        RateScheduleRef::new(RateScheduleKind::BasicOfdm, 0).unwrap(),
    ] {
        assert!(legacy_rate(schedule_state(schedule).rate).is_some(), "{schedule:?}");
    }
}
