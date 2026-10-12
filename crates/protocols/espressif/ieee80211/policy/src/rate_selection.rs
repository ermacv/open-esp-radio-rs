//! The Espressif rate selection: the rates of a station's frames as the
//! vendor's station sends them ([owner decisions on #563]).
//!
//! - A group Probe Request goes once at 1 Mb/s.
//! - Every other frame without network data (management, EAPOL, Nulls,
//!   BlockAckReqs) walks a control schedule record: the interface's
//!   default one ([`DEFAULT_CONTROL_SCHEDULE`], 1 Mb/s DSSS) until the
//!   station associated, then the association's
//!   ([`StaRateControlAssociation::control_schedule`](crate::rate_control::StaRateControlAssociation::control_schedule)),
//!   the four-way handshake's EAPOL included. The rates depend on what is
//!   known of the access point, which the association makes known; the old
//!   ESP32-S31 path switched only once connected, a consequence of when
//!   the vendor calls `ic_set_sta` (a deliberate departure, owner decision
//!   on #563).
//! - A data MPDU starts at the rate the association's rate control holds
//!   ([`EspressifRateControl`]) and steps down the vendor's retry ladder
//!   ([`ordinary_retry_rate`]), within the publication budget of the
//!   current schedule record; its terminal adapts the control.
//!
//! A record is four `(rate, count)` pairs, so each plan holds at most
//! four stages ([`RETRY_STAGES`]); once the ladder has no further rate,
//! the remaining transmissions keep the last one, as the vendor's retry
//! state does.
//!
//! The records were recovered from the ESP32-S31, which has no 5 GHz
//! band, where 1 Mb/s DSSS is not allowed: a selection builds only for a
//! port without 5 GHz, checked at compile time, until the ESP32-C5's
//! records are recovered (#748).
//!
//! [owner decisions on #563]: https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6132066470

use core::marker::PhantomData;
use core::num::NonZeroU8;

use oer_ieee80211_lower_mac::{
    Band, Ieee80211Capabilities, Protection, RETRY_STAGES, RetryPlan, RetryStage, TxCompletion,
    TxStatus,
};
use oer_ieee80211_mac::phy::{LegacyRate, PhyRate};
use oer_ieee80211_upper_mac::{
    rate_control::{LinkMetric, RateControl, RatePeer},
    rate_selection::{FrameRates, RateSelection},
};
use oer_time::Instant;

use crate::{
    rate_code::legacy_rate,
    rate_control::{DEFAULT_CONTROL_SCHEDULE, EspressifRateControl},
    rate_schedule::{RateScheduleRef, schedule_publication_limit, schedule_rate_after_failures},
    retry_ladder::ordinary_retry_rate,
};

/// The Espressif rate selection of one station interface on a port of the
/// capabilities `K`.
///
/// A port with a 5 GHz band builds none, nor one without the rates the
/// vendor's records name: DSSS/CCK, OFDM, HT MCS 0-7 and HE MCS 0-9 with
/// LDPC.
///
/// ```compile_fail,E0080
/// # use oer_espressif_ieee80211_policy::rate_selection::EspressifRateSelection;
/// # use oer_ieee80211_lower_mac::*;
/// # use oer_ieee80211_mac::phy::{HeMcs, HtMcs};
/// # const fn caps(bands: BandSet) -> LowerMacCapabilities {
/// #     LowerMacCapabilities {
/// #         bands, widths: WidthSet::MHZ20,
/// #         rates: RateSupport { dsss_cck: true, ofdm: true, ht_max_mcs: HtMcs::new(7),
/// #             he_max_mcs: HeMcs::new(9), he_max_bandwidth_mhz: 20, he_dcm: false,
/// #             he_ldpc: true, spatial_streams: 1 },
/// #         services: HardwareServices::FCS, vifs: 1, tx_queues: 4, max_mpdu_length: 2304,
/// #         max_backoff_slots: 1023, tx_power_ceiling_min_dbm: None,
/// #         coex_priorities: CoexPrioritySet::only(CoexPriority::Normal),
/// #         individual_no_ack: PhyFormatSet::NON_HT,
/// #         station_receive_filters: ReceiveFilter::BSS_MEMBER,
/// #         access_point_receive_filters: ReceiveFilter::BSS_MEMBER, key_slots: 4,
/// #         rx_block_ack_agreements: 0, rx_block_ack_max_tid: 0, rx_block_ack_max_window: 0,
/// #         edca_max_aifsn: 15, edca_max_cw_exponent: 10,
/// #     }
/// # }
/// struct DualBand;
/// impl Ieee80211Capabilities for DualBand {
///     const CAPS: LowerMacCapabilities = caps(BandSet::GHZ2_4.union(BandSet::GHZ5));
/// }
/// let selection = EspressifRateSelection::<DualBand>::new();
/// # core::hint::black_box(selection);
/// ```
///
/// The same port without its 5 GHz band builds one:
///
/// ```
/// # use oer_espressif_ieee80211_policy::rate_selection::EspressifRateSelection;
/// # use oer_ieee80211_lower_mac::*;
/// # use oer_ieee80211_mac::phy::{HeMcs, HtMcs};
/// # const fn caps(bands: BandSet) -> LowerMacCapabilities {
/// #     LowerMacCapabilities {
/// #         bands, widths: WidthSet::MHZ20,
/// #         rates: RateSupport { dsss_cck: true, ofdm: true, ht_max_mcs: HtMcs::new(7),
/// #             he_max_mcs: HeMcs::new(9), he_max_bandwidth_mhz: 20, he_dcm: false,
/// #             he_ldpc: true, spatial_streams: 1 },
/// #         services: HardwareServices::FCS, vifs: 1, tx_queues: 4, max_mpdu_length: 2304,
/// #         max_backoff_slots: 1023, tx_power_ceiling_min_dbm: None,
/// #         coex_priorities: CoexPrioritySet::only(CoexPriority::Normal),
/// #         individual_no_ack: PhyFormatSet::NON_HT,
/// #         station_receive_filters: ReceiveFilter::BSS_MEMBER,
/// #         access_point_receive_filters: ReceiveFilter::BSS_MEMBER, key_slots: 4,
/// #         rx_block_ack_agreements: 0, rx_block_ack_max_tid: 0, rx_block_ack_max_window: 0,
/// #         edca_max_aifsn: 15, edca_max_cw_exponent: 10,
/// #     }
/// # }
/// struct SingleBand;
/// impl Ieee80211Capabilities for SingleBand {
///     const CAPS: LowerMacCapabilities = caps(BandSet::GHZ2_4);
/// }
/// let selection = EspressifRateSelection::<SingleBand>::new();
/// # core::hint::black_box(selection);
/// ```
pub struct EspressifRateSelection<K> {
    /// The rate control of the associated peer.
    control: Option<EspressifRateControl>,
    port: PhantomData<fn() -> K>,
}

/// What a port must carry for the vendor's records.
struct Port<K>(PhantomData<fn() -> K>);

impl<K: Ieee80211Capabilities> Port<K> {
    const CHECKED: () = {
        assert!(
            !K::CAPS.bands.contains(Band::Ghz5),
            "the vendor's rate records are recovered for 2.4 GHz only (#748)"
        );
        let rates = K::CAPS.rates;
        assert!(
            rates.dsss_cck && rates.ofdm,
            "the vendor's records name DSSS/CCK and OFDM rates"
        );
        assert!(
            matches!(rates.ht_max_mcs, Some(mcs) if mcs.index() >= 7),
            "the vendor's records name HT MCS 0-7"
        );
        assert!(
            matches!(rates.he_max_mcs, Some(mcs) if mcs.index() >= 9) && rates.he_ldpc,
            "the vendor's records name HE MCS 0-9 with LDPC"
        );
    };
}

impl<K: Ieee80211Capabilities> EspressifRateSelection<K> {
    /// The selection of an interface that is not connected.
    pub const fn new() -> Self {
        let () = Port::<K>::CHECKED;
        Self {
            control: None,
            port: PhantomData,
        }
    }

    /// The rate control of the connected peer.
    pub const fn control(&self) -> Option<&EspressifRateControl> {
        self.control.as_ref()
    }

    /// The control schedule record in use.
    fn control_schedule(&self) -> RateScheduleRef {
        match &self.control {
            Some(control) => control.association().control_schedule(),
            None => DEFAULT_CONTROL_SCHEDULE,
        }
    }
}

impl<K: Ieee80211Capabilities> Default for EspressifRateSelection<K> {
    fn default() -> Self {
        Self::new()
    }
}

/// The plan of `limit` transmissions, the first at `initial` and each
/// after `failures` failures at `rate(failures)`, the last rate where it
/// has none.
fn walk(initial: PhyRate, limit: u8, rate: impl Fn(u8) -> Option<PhyRate>) -> RetryPlan<PhyRate> {
    let stage = |rate, transmissions| RetryStage {
        rate,
        transmissions,
        protection: Protection::None,
    };
    let mut stages: [Option<(PhyRate, NonZeroU8)>; RETRY_STAGES] = [None; RETRY_STAGES];
    let mut count = 0;
    let mut current = initial;
    for failures in 0..limit.max(1) {
        let next = if failures == 0 {
            initial
        } else {
            rate(failures).unwrap_or(current)
        };
        current = next;
        match stages[..count].last_mut().and_then(Option::as_mut) {
            Some((last, transmissions)) if *last == next => {
                *transmissions = transmissions.saturating_add(1);
            }
            // A record holds four pairs; a fifth rate never comes.
            _ if count == RETRY_STAGES => break,
            _ => {
                stages[count] = Some((next, NonZeroU8::MIN));
                count += 1;
            }
        }
    }
    let mut stages = stages.into_iter().flatten();
    let Some((rate, transmissions)) = stages.next() else {
        unreachable!("a walk makes at least one transmission");
    };
    let mut plan = RetryPlan::new(stage(rate, transmissions));
    for (rate, transmissions) in stages {
        plan = match plan.then(stage(rate, transmissions)) {
            Ok(plan) => plan,
            Err(_) => unreachable!("a walk keeps at most the plan's stages"),
        };
    }
    plan
}

/// The plan of a control schedule record: its rates, its budget.
fn control_plan(schedule: RateScheduleRef) -> RetryPlan<PhyRate> {
    let rate = |failures| {
        schedule_rate_after_failures(schedule, failures)
            .and_then(legacy_rate)
            .map(PhyRate::Legacy)
    };
    let Some(initial) = rate(0) else {
        unreachable!("every recovered control schedule starts with a legacy rate");
    };
    walk(initial, schedule_publication_limit(schedule), rate)
}

impl<K: Ieee80211Capabilities> RateSelection for EspressifRateSelection<K> {
    fn plan(&self, frame: FrameRates) -> RetryPlan<PhyRate> {
        match (frame, &self.control) {
            // The vendor station's probe: once, at 1 Mb/s.
            (FrameRates::Probe, _) => RetryPlan::new(RetryStage {
                rate: PhyRate::Legacy(LegacyRate::Dsss1M),
                transmissions: NonZeroU8::MIN,
                protection: Protection::None,
            }),
            (FrameRates::Data, Some(control)) => {
                let initial = control.mpdu_rate();
                walk(
                    initial,
                    schedule_publication_limit(control.association().current_schedule()),
                    |failures| ordinary_retry_rate(initial, failures),
                )
            }
            // No data goes before an association; it would go as control.
            (FrameRates::Control | FrameRates::Data, _) => control_plan(self.control_schedule()),
        }
    }

    fn associated(&mut self, peer: &RatePeer, link_metric: LinkMetric) {
        self.control = Some(EspressifRateControl::for_peer((), peer, link_metric));
    }

    fn disconnected(&mut self) {
        self.control = None;
    }

    fn completed(&mut self, _now: Instant, frame: FrameRates, completion: &TxCompletion) {
        // As the vendor station: a data MPDU's exchange adapts the control;
        // one that never reached the air teaches nothing.
        if frame != FrameRates::Data || completion.transmissions == 0 {
            return;
        }
        if let Some(control) = &mut self.control {
            let acknowledged = completion.status == TxStatus::Success;
            control.observe_mpdu(
                completion.transmissions,
                acknowledged,
                completion.ack_snr_db.filter(|_| acknowledged),
            );
        }
    }
}

#[cfg(test)]
mod tests;
