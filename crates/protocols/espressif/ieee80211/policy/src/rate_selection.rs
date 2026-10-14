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
use core::num::{NonZeroU8, NonZeroU16};

use oer_ieee80211_lower_mac::{
    Band, Ieee80211Capabilities, Protection, RETRY_STAGES, RetryPlan, RetryStage, SignalMetadata,
    TxEndStatus, TxOutcome,
};
use oer_ieee80211_mac::phy::{LegacyRate, PhyRate};
use oer_ieee80211_upper_mac::{
    rate_control::{MpduOutcome, RateControl, RatePeer},
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
        #[cfg_attr(not(test), expect(clippy::panic, reason = "evaluated at build"))]
        const fn checked<K: Ieee80211Capabilities>() {
            if K::CAPS.bands.contains(Band::Ghz5) {
                panic!("the vendor's rate records are recovered for 2.4 GHz only (#748)");
            }
            let rates = K::CAPS.rates;
            if !(rates.dsss_cck && rates.ofdm) {
                panic!("the vendor's records name DSSS/CCK and OFDM rates");
            }
            if !matches!(rates.ht_max_mcs, Some(mcs) if mcs.index() >= 7) {
                panic!("the vendor's records name HT MCS 0-7");
            }
            if !(matches!(rates.he_max_mcs, Some(mcs) if mcs.index() >= 9) && rates.he_ldpc) {
                panic!("the vendor's records name HE MCS 0-9 with LDPC");
            }
        }
        checked::<K>();
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
/// has none. A record holds four `(rate, count)` pairs, so its walk makes
/// at most four stages; were a fifth rate to come, the plan would end with
/// the fourth stage.
fn walk(initial: PhyRate, limit: u8, rate: impl Fn(u8) -> Option<PhyRate>) -> RetryPlan<PhyRate> {
    let stage = |(rate, transmissions): (PhyRate, NonZeroU8)| RetryStage {
        rate,
        transmissions,
        protection: Protection::None,
    };
    let mut first = (initial, NonZeroU8::MIN);
    let mut rest: [Option<(PhyRate, NonZeroU8)>; RETRY_STAGES - 1] = [None; RETRY_STAGES - 1];
    let mut current = initial;
    for failures in 1..limit {
        let next = rate(failures).unwrap_or(current);
        current = next;
        let last = rest.iter_mut().rev().find_map(Option::as_mut);
        match last {
            Some((rate, transmissions)) if *rate == next => {
                *transmissions = transmissions.saturating_add(1);
            }
            None if first.0 == next => first.1 = first.1.saturating_add(1),
            _ => match rest.iter_mut().find(|slot| slot.is_none()) {
                Some(slot) => *slot = Some((next, NonZeroU8::MIN)),
                None => break,
            },
        }
    }
    let mut plan = RetryPlan::new(stage(first));
    for next in rest.into_iter().flatten() {
        plan = match plan.then(stage(next)) {
            Ok(plan) => plan,
            // The walk keeps at most the plan's stages.
            Err((plan, _)) => plan,
        };
    }
    plan
}

/// The plan of a control schedule record: its rates, its budget; `None`
/// for a record whose first rate is no legacy rate (none the station uses:
/// a test pins it), and the frame does not go.
fn control_plan(schedule: RateScheduleRef) -> Option<RetryPlan<PhyRate>> {
    let rate = |failures| {
        schedule_rate_after_failures(schedule, failures)
            .and_then(legacy_rate)
            .map(PhyRate::Legacy)
    };
    Some(walk(rate(0)?, schedule_publication_limit(schedule), rate))
}

/// The selection for a port whose metadata measures every received frame's
/// signal ([`SignalMetadata`]): the association's rate control starts from
/// the Association Response's signal over the noise floor. A port whose
/// metadata does not measure it has no Espressif selection:
///
/// ```compile_fail,E0277
/// # use oer_espressif_ieee80211_policy::rate_selection::EspressifRateSelection;
/// # use oer_ieee80211_lower_mac::*;
/// # use oer_ieee80211_upper_mac::rate_selection::RateSelection;
/// # struct Caps;
/// # impl Ieee80211Capabilities for Caps { const CAPS: LowerMacCapabilities = PORT; }
/// # const PORT: LowerMacCapabilities = LowerMacCapabilities {
/// #     bands: BandSet::GHZ2_4, widths: WidthSet::MHZ20,
/// #     rates: RateSupport { dsss_cck: true, ofdm: true,
/// #         ht_max_mcs: oer_ieee80211_mac::phy::HtMcs::new(7),
/// #         he_max_mcs: oer_ieee80211_mac::phy::HeMcs::new(9), he_max_bandwidth_mhz: 20,
/// #         he_dcm: false, he_ldpc: true, spatial_streams: 1 },
/// #     services: HardwareServices::FCS, vifs: 1, tx_queues: 4, max_mpdu_length: 2304,
/// #     max_backoff_slots: 1023, tx_power_ceiling_min_dbm: None,
/// #     coex_priorities: CoexPrioritySet::only(CoexPriority::Normal),
/// #     individual_no_ack: PhyFormatSet::NON_HT,
/// #     station_receive_filters: ReceiveFilter::BSS_MEMBER,
/// #     access_point_receive_filters: ReceiveFilter::BSS_MEMBER, key_slots: 4,
/// #     rx_block_ack_agreements: 0, rx_block_ack_max_tid: 0, rx_block_ack_max_window: 0,
/// #     edca_max_aifsn: 15, edca_max_cw_exponent: 10,
/// # };
/// fn for_port<M: RxMetadata>(selection: impl RateSelection<M>) {}
/// // The portable metadata measures nothing.
/// for_port::<RxMeta>(EspressifRateSelection::<Caps>::new());
/// ```
///
/// Metadata that measures the signal takes one:
///
/// ```
/// # use oer_espressif_ieee80211_policy::rate_selection::EspressifRateSelection;
/// # use oer_ieee80211_lower_mac::*;
/// # use oer_ieee80211_upper_mac::rate_selection::RateSelection;
/// # struct Caps;
/// # impl Ieee80211Capabilities for Caps { const CAPS: LowerMacCapabilities = PORT; }
/// # const PORT: LowerMacCapabilities = LowerMacCapabilities {
/// #     bands: BandSet::GHZ2_4, widths: WidthSet::MHZ20,
/// #     rates: RateSupport { dsss_cck: true, ofdm: true,
/// #         ht_max_mcs: oer_ieee80211_mac::phy::HtMcs::new(7),
/// #         he_max_mcs: oer_ieee80211_mac::phy::HeMcs::new(9), he_max_bandwidth_mhz: 20,
/// #         he_dcm: false, he_ldpc: true, spatial_streams: 1 },
/// #     services: HardwareServices::FCS, vifs: 1, tx_queues: 4, max_mpdu_length: 2304,
/// #     max_backoff_slots: 1023, tx_power_ceiling_min_dbm: None,
/// #     coex_priorities: CoexPrioritySet::only(CoexPriority::Normal),
/// #     individual_no_ack: PhyFormatSet::NON_HT,
/// #     station_receive_filters: ReceiveFilter::BSS_MEMBER,
/// #     access_point_receive_filters: ReceiveFilter::BSS_MEMBER, key_slots: 4,
/// #     rx_block_ack_agreements: 0, rx_block_ack_max_tid: 0, rx_block_ack_max_window: 0,
/// #     edca_max_aifsn: 15, edca_max_cw_exponent: 10,
/// # };
/// #[derive(Clone, Copy)]
/// struct Measured {
///     common: RxMeta,
///     rssi_dbm: i8,
///     noise_floor_dbm: i8,
/// }
/// impl RxMetadata for Measured {
///     fn common(&self) -> &RxMeta {
///         &self.common
///     }
/// }
/// impl SignalMetadata for Measured {
///     fn link_metric(&self) -> LinkMetric {
///         LinkMetric::new(self.rssi_dbm, self.noise_floor_dbm)
///     }
/// }
/// fn for_port<M: RxMetadata>(selection: impl RateSelection<M>) {}
/// for_port::<Measured>(EspressifRateSelection::<Caps>::new());
/// ```
impl<K: Ieee80211Capabilities, M: SignalMetadata> RateSelection<M> for EspressifRateSelection<K> {
    fn plan<R>(
        &self,
        frame: FrameRates,
        carried: impl Fn(PhyRate) -> Option<R>,
    ) -> Option<RetryPlan<R>> {
        on_channel(self.vendor_plan(frame)?, carried)
    }

    fn associated(&mut self, peer: &RatePeer, response: &M) {
        self.control = Some(EspressifRateControl::for_peer(
            (),
            peer,
            response.link_metric(),
        ));
    }

    fn disconnected(&mut self) {
        self.control = None;
    }

    fn completed(&mut self, _now: Instant, frame: FrameRates, completion: &TxOutcome) {
        if frame != FrameRates::Data {
            return;
        }
        let TxOutcome::Ended {
            status,
            transmissions,
            ack,
            ..
        } = *completion
        else {
            return;
        };
        let Some(transmissions) = NonZeroU16::new(transmissions) else {
            return;
        };
        let outcome = match status {
            TxEndStatus::Delivered => MpduOutcome::Acknowledged,
            TxEndStatus::AckTimeout | TxEndStatus::CtsTimeout => MpduOutcome::Unacknowledged,
            TxEndStatus::ContentionLimit => return,
        };
        if let Some(control) = &mut self.control {
            control.observe_mpdu(transmissions, outcome, ack.and_then(|ack| ack.snr_db));
        }
    }
}

impl<K: Ieee80211Capabilities> EspressifRateSelection<K> {
    /// The vendor's plan for `frame`, before the channel's rule.
    fn vendor_plan(&self, frame: FrameRates) -> Option<RetryPlan<PhyRate>> {
        match (frame, &self.control) {
            // The vendor station's probe: once, at 1 Mb/s.
            (FrameRates::Probe, _) => Some(RetryPlan::new(RetryStage {
                rate: PhyRate::Legacy(LegacyRate::Dsss1M),
                transmissions: NonZeroU8::MIN,
                protection: Protection::None,
            })),
            (FrameRates::Data, Some(control)) => {
                let initial = control.mpdu_rate();
                Some(walk(
                    initial,
                    schedule_publication_limit(control.association().current_schedule()),
                    |failures| ordinary_retry_rate(initial, failures),
                ))
            }
            // No data goes before an association; it would go as control.
            (FrameRates::Control | FrameRates::Data, _) => control_plan(self.control_schedule()),
        }
    }
}

/// The vendor's plan on the port's channel: a stage whose rate the channel
/// does not carry (an HT40 rate on a 20 MHz channel) is left out with its
/// transmissions, and the plan is what remains in order; `None` when no
/// stage is left. The vendor's station never meets this: its records are
/// for its own channel, and the rule only keeps a plan within the port's.
fn on_channel<R>(
    plan: RetryPlan<PhyRate>,
    carried: impl Fn(PhyRate) -> Option<R>,
) -> Option<RetryPlan<R>> {
    let mut stages = plan.stages().filter_map(|stage| {
        Some(RetryStage {
            rate: carried(stage.rate)?,
            transmissions: stage.transmissions,
            protection: stage.protection,
        })
    });
    let mut on_channel = RetryPlan::new(stages.next()?);
    for stage in stages {
        on_channel = match on_channel.then(stage) {
            Ok(plan) => plan,
            // Never more stages than the vendor's plan held.
            Err((plan, _)) => plan,
        };
    }
    Some(on_channel)
}

#[cfg(test)]
mod tests;
