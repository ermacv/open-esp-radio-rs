//! The arbiter on a host port it plays itself.

use super::*;
use oer_ieee80211_lower_mac::{
    BandSet, Channel, ChannelWidth, CoexPriority, CoexPrioritySet, HardwareServices,
    LowerMacCapabilities, PhyFormatSet, RateSupport, ReceiveFilter, WidthSet,
};
use oer_ieee80211_mac::phy::{HeMcs, HtMcs};
use std::vec::Vec;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
            he_ldpc: false,
            spatial_streams: 1,
        },
        services: HardwareServices::FCS,
        vifs: 2,
        tx_queues: 4,
        max_mpdu_length: 2304,
        max_backoff_slots: 1023,
        tx_power_ceiling_min_dbm: Some(0),
        coex_priorities: CoexPrioritySet::only(CoexPriority::Normal),
        individual_no_ack: PhyFormatSet::NON_HT,
        station_receive_filters: ReceiveFilter::BSS_MEMBER,
        access_point_receive_filters: ReceiveFilter::BSS_MEMBER,
        key_slots: 4,
        rx_block_ack_agreements: 2,
        rx_block_ack_max_tid: 7,
        rx_block_ack_max_window: 64,
        edca_max_aifsn: 15,
        edca_max_cw_exponent: 10,
    };
}

/// The test's gate tokens: neither `Clone` nor `Copy`.
#[derive(Debug, Eq, PartialEq)]
struct Open;
#[derive(Debug, Eq, PartialEq)]
struct Closed;

struct Gate;

impl GateTokens for Gate {
    type Open = Open;
    type Closed = Closed;
}

fn channel(number: u8) -> SupportedChannel<Caps> {
    SupportedChannel::new(Channel::ghz2_4(number, ChannelWidth::Mhz20).unwrap()).unwrap()
}

fn at(millis: u32) -> Instant {
    Instant::from_micros(u64::from(millis) * 1000)
}

/// What the arbiter did, its tokens handed back as the port would.
#[derive(Debug, Eq, PartialEq)]
enum Seen {
    Station(StationNotice<Caps>),
    AccessPoint(AccessPointCommand<Caps>),
    CloseGate,
    Retune(u8),
    OpenGate,
}

/// Take every action at `now`, playing the port: a closed gate and a
/// retune answer at once.
fn run(arbiter: &mut Arbiter<Gate, Caps>, now: Instant) -> Vec<Seen> {
    let mut seen = Vec::new();
    while let Some(action) = arbiter.poll_action(now) {
        seen.push(match action {
            ArbiterAction::Station(notice) => Seen::Station(notice),
            ArbiterAction::AccessPoint(command) => Seen::AccessPoint(command),
            ArbiterAction::CloseGate(Open) => {
                arbiter.gate_closed(Closed);
                Seen::CloseGate
            }
            ArbiterAction::Retune(channel) => {
                arbiter.retuned(channel);
                Seen::Retune(channel.channel().number())
            }
            ArbiterAction::OpenGate(Closed) => {
                arbiter.gate_opened(Open);
                Seen::OpenGate
            }
        });
    }
    seen
}

fn lease(answer: Answer<Caps>) -> Lease<Caps> {
    match answer {
        Answer::Lease(lease) => lease,
        Answer::Leave(reason) => panic!("left: {reason:?}"),
    }
}

#[test]
fn a_station_alone_leases_a_scan_channel_at_once_and_returns_it_at_its_end() {
    let mut arbiter = Arbiter::<Gate, Caps>::new(ArbiterPolicy::DEFAULT, channel(1), Open);
    let lease = lease(arbiter.request(
        at(0),
        ChannelRequest {
            channel: channel(6),
            purpose: Purpose::Scan {
                dwell: Duration::from_millis(100),
            },
            announces_absence: false,
        },
    ));
    assert_eq!(lease.from(), at(0));
    assert_eq!(lease.until(), Some(at(100)));
    let id = lease.id();
    assert_eq!(
        run(&mut arbiter, at(0)),
        [
            Seen::CloseGate,
            Seen::Retune(6),
            Seen::OpenGate,
            Seen::Station(StationNotice::LeaseStarted(id)),
        ]
    );
    assert_eq!(arbiter.poll_deadline(), Some(at(100)));
    // Alone, the port stays where the scan left it: its end closes the
    // gate, so every terminal of the lease comes before it.
    assert_eq!(
        run(&mut arbiter, at(100)),
        [
            Seen::CloseGate,
            Seen::OpenGate,
            Seen::Station(StationNotice::LeaseEnded(id)),
        ]
    );
    let _ = lease;
}

fn beside_access_point(policy: ArbiterPolicy) -> Arbiter<Gate, Caps> {
    let mut arbiter = Arbiter::<Gate, Caps>::new(policy, channel(1), Open);
    arbiter.access_point(AccessPointFact::Running {
        channel: channel(1),
        next_tbtt: at(50),
        beacon_interval: Duration::from_millis(100),
    });
    arbiter
}

#[test]
fn beside_an_access_point_a_scan_goes_in_absences_after_its_tbtts() {
    let mut arbiter = beside_access_point(ArbiterPolicy {
        reserve_air: true,
        ..ArbiterPolicy::DEFAULT
    });
    let lease = lease(arbiter.request(
        at(0),
        ChannelRequest {
            channel: channel(6),
            purpose: Purpose::Scan {
                dwell: Duration::from_millis(100),
            },
            announces_absence: false,
        },
    ));
    // The first absence: 2 ms after the TBTT at 50 ms, 20 ms long.
    assert_eq!(lease.from(), at(52));
    assert_eq!(lease.until(), Some(at(72)));
    assert_eq!(
        run(&mut arbiter, at(0)),
        [Seen::AccessPoint(AccessPointCommand::ReserveAir {
            from: at(52),
            until: at(72)
        })]
    );
    assert_eq!(arbiter.poll_deadline(), Some(at(52)));
    let id = lease.id();
    assert_eq!(
        run(&mut arbiter, at(52)),
        [
            Seen::CloseGate,
            Seen::Retune(6),
            Seen::OpenGate,
            Seen::Station(StationNotice::LeaseStarted(id)),
        ]
    );
    // Back on the access point's channel at the absence's end.
    assert_eq!(
        run(&mut arbiter, at(72)),
        [
            Seen::CloseGate,
            Seen::Retune(1),
            Seen::OpenGate,
            Seen::Station(StationNotice::LeaseEnded(id)),
        ]
    );
    let _ = lease;
}

#[test]
fn the_air_reservation_is_off_by_default() {
    let mut arbiter = beside_access_point(ArbiterPolicy::DEFAULT);
    let lease = lease(arbiter.request(
        at(0),
        ChannelRequest {
            channel: channel(6),
            purpose: Purpose::Scan {
                dwell: Duration::from_millis(10),
            },
            announces_absence: false,
        },
    ));
    assert_eq!(run(&mut arbiter, at(0)), []);
    let _ = lease;
}

#[test]
fn an_absence_waits_for_the_stations_readiness_and_is_cancelled_without_it() {
    let mut arbiter = beside_access_point(ArbiterPolicy::DEFAULT);
    let request = ChannelRequest {
        channel: channel(6),
        purpose: Purpose::Scan {
            dwell: Duration::from_millis(20),
        },
        announces_absence: true,
    };
    // At 45 ms the preparation window ends at 55 ms: past the TBTT slot at
    // 52 ms, so the absence follows the next TBTT.
    let late = lease(arbiter.request(at(45), request));
    assert_eq!(late.from(), at(152));
    let id = late.id();
    // The station never said it was ready.
    assert_eq!(
        run(&mut arbiter, at(152)),
        [Seen::Station(StationNotice::LeaseCancelled(
            id,
            CancelReason::NotReady
        ))]
    );
    let _ = late;

    // Ready in time: the absence goes.
    let ready = lease(arbiter.request(at(0), request));
    assert_eq!(ready.from(), at(52));
    arbiter.ready(ready.id());
    assert_eq!(
        run(&mut arbiter, at(52)).last(),
        Some(&Seen::Station(StationNotice::LeaseStarted(ready.id())))
    );
    let _ = ready;
}

#[test]
fn a_refused_move_cancels_the_lease_and_the_port_returns() {
    let mut arbiter = beside_access_point(ArbiterPolicy::DEFAULT);
    let lease = lease(arbiter.request(
        at(0),
        ChannelRequest {
            channel: channel(6),
            purpose: Purpose::Scan {
                dwell: Duration::from_millis(10),
            },
            announces_absence: false,
        },
    ));
    let id = lease.id();
    assert_eq!(arbiter.poll_action(at(52)).map(|_| ()), Some(()));
    arbiter.gate_closed(Closed);
    assert!(matches!(
        arbiter.poll_action(at(52)),
        Some(ArbiterAction::Retune(_))
    ));
    arbiter.retune_refused();
    assert_eq!(
        run(&mut arbiter, at(52)),
        [
            Seen::Retune(1),
            Seen::Station(StationNotice::LeaseCancelled(id, CancelReason::Refused)),
            Seen::OpenGate,
        ]
    );
    let _ = lease;
}

#[test]
fn a_failed_return_stops_the_station_and_the_access_point() {
    let mut arbiter = beside_access_point(ArbiterPolicy::DEFAULT);
    let lease = lease(arbiter.request(
        at(0),
        ChannelRequest {
            channel: channel(6),
            purpose: Purpose::Scan {
                dwell: Duration::from_millis(10),
            },
            announces_absence: false,
        },
    ));
    run(&mut arbiter, at(52));
    // The return's retune is refused.
    assert!(matches!(
        arbiter.poll_action(at(62)),
        Some(ArbiterAction::CloseGate(_))
    ));
    arbiter.gate_closed(Closed);
    assert!(matches!(
        arbiter.poll_action(at(62)),
        Some(ArbiterAction::Retune(_))
    ));
    arbiter.retune_refused();
    let seen = run(&mut arbiter, at(62));
    assert!(seen.contains(&Seen::Station(StationNotice::Leave(LeaveReason::PortFailed))));
    assert!(seen.contains(&Seen::AccessPoint(AccessPointCommand::Stop)));
    let _ = lease;
}

#[test]
fn a_gate_that_closes_after_the_lease_end_cancels_it() {
    let mut arbiter = beside_access_point(ArbiterPolicy::DEFAULT);
    let lease = lease(arbiter.request(
        at(0),
        ChannelRequest {
            channel: channel(6),
            purpose: Purpose::Scan {
                dwell: Duration::from_millis(10),
            },
            announces_absence: false,
        },
    ));
    let id = lease.id();
    assert!(matches!(
        arbiter.poll_action(at(52)),
        Some(ArbiterAction::CloseGate(_))
    ));
    // The attempts in flight outlast the absence.
    arbiter.gate_closed(Closed);
    assert_eq!(
        run(&mut arbiter, at(63)),
        [
            Seen::Station(StationNotice::LeaseCancelled(id, CancelReason::Late)),
            Seen::OpenGate,
        ]
    );
    let _ = lease;
}
