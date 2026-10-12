//! The arbiter on a host port it plays itself.

use super::*;
use oer_ieee80211_lower_mac::{
    BandSet, Channel, ChannelWidth, CoexPriority, CoexPrioritySet, HardwareServices,
    LowerMacCapabilities, PhyFormatSet, RateSupport, ReceiveFilter, WidthSet,
};
use oer_ieee80211_mac::phy::{HeMcs, HtMcs};
use std::vec::Vec;

/// A 2.4 and 5 GHz backend.
#[derive(Debug, Eq, PartialEq)]
struct Caps;

impl Ieee80211Capabilities for Caps {
    const CAPS: LowerMacCapabilities = LowerMacCapabilities {
        bands: BandSet::GHZ2_4.union(BandSet::GHZ5),
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

type TestArbiter<R = NoReservation> = Arbiter<'static, Gate, Caps, R>;

fn channel(number: u8) -> SupportedChannel<Caps> {
    SupportedChannel::new(Channel::ghz2_4(number, ChannelWidth::Mhz20).unwrap()).unwrap()
}

fn at(millis: u32) -> Instant {
    Instant::from_micros(u64::from(millis) * 1000)
}

/// What the arbiter did, its tokens handed back as the port would.
#[derive(Debug, Eq, PartialEq)]
enum Seen<Reserve> {
    Station(StationNotice<Caps>),
    AccessPoint(AccessPointCommand<Caps>),
    CloseGate(Reserve),
    Retune(u8),
    OpenGate,
}

/// The port the test plays: whether it refuses the next retune.
#[derive(Default)]
struct Port {
    refuse: bool,
}

/// Take every action at `now`, playing the port: a closed gate and a
/// retune answer at once.
fn run<R: ReservationMode>(
    arbiter: &mut TestArbiter<R>,
    port: &mut Port,
    now: Instant,
) -> Vec<Seen<R::Reserve>> {
    let mut seen = Vec::new();
    while let Some(action) = arbiter.poll_action(now) {
        seen.push(match action {
            ArbiterAction::Station(notice) => Seen::Station(notice),
            ArbiterAction::AccessPoint(command) => Seen::AccessPoint(command),
            ArbiterAction::CloseGate {
                gate: Open,
                reserve,
            } => {
                arbiter.gate_closed(Closed);
                Seen::CloseGate(reserve)
            }
            ArbiterAction::Retune(to) => {
                if core::mem::take(&mut port.refuse) {
                    arbiter.retune_refused();
                } else {
                    arbiter.retuned(to);
                }
                Seen::Retune(to.channel().number())
            }
            ArbiterAction::OpenGate(Closed) => {
                arbiter.gate_opened(Open);
                Seen::OpenGate
            }
        });
    }
    seen
}

fn alone<R: ReservationMode>() -> (TestArbiter<R>, LeasePlace) {
    Arbiter::new(ArbiterPolicy::DEFAULT, channel(1), Open)
}

fn running(arbiter: &mut TestArbiter<impl ReservationMode>) {
    arbiter.access_point(
        at(0),
        AccessPointFact::Running {
            channel: channel(1),
            next_tbtt: at(50),
            beacon_interval: Duration::from_millis(100),
        },
    );
}

fn beside_access_point<R: ReservationMode>() -> (TestArbiter<R>, LeasePlace) {
    let (mut arbiter, place) = alone::<R>();
    running(&mut arbiter);
    (arbiter, place)
}

fn scan(number: u8, dwell: u32, announces_absence: bool) -> ChannelRequest<Caps> {
    ChannelRequest {
        channel: channel(number),
        purpose: Purpose::Scan {
            dwell: Duration::from_millis(dwell),
        },
        announces_absence,
    }
}

fn lease(answer: Answer<Caps>) -> Lease<Caps> {
    match answer {
        Answer::Lease(lease) => lease,
        Answer::Leave(reason, _) => panic!("left: {reason:?}"),
    }
}

/// A place to compare notices with: places are equal.
fn place() -> LeasePlace {
    LeasePlace { private: () }
}

#[test]
fn a_station_alone_leases_a_scan_channel_at_once_and_ends_it_in_place() {
    let (mut arbiter, place) = alone::<NoReservation>();
    let mut port = Port::default();
    let lease = lease(arbiter.request(at(0), scan(6, 100, false), place));
    // The move there and the dwell; alone, the port stays.
    assert_eq!((lease.from(), lease.until()), (at(0), Some(at(102))));
    assert_eq!(lease.listen_until(), Some(at(102)));
    let id = lease.id();
    assert_eq!(
        run(&mut arbiter, &mut port, at(0)),
        [
            Seen::CloseGate(()),
            Seen::Retune(6),
            Seen::OpenGate,
            Seen::Station(StationNotice::LeaseStarted(id)),
        ]
    );
    assert_eq!(arbiter.poll_deadline(), Some(at(102)));
    // Its end closes the gate, so every terminal of the lease comes
    // before it; the place comes back.
    assert_eq!(
        run(&mut arbiter, &mut port, at(102)),
        [
            Seen::CloseGate(()),
            Seen::OpenGate,
            Seen::Station(StationNotice::LeaseEnded(id, self::place())),
        ]
    );
    let _ = lease;
}

#[test]
fn beside_an_access_point_a_scan_goes_in_an_absence_with_the_air_reserved() {
    let (mut arbiter, place) = beside_access_point::<AirReservation>();
    let mut port = Port::default();
    let lease = lease(arbiter.request(at(0), scan(6, 100, false), place));
    // 2 ms after the TBTT at 50 ms, 20 ms with its two retunes; the
    // station listens until the return's retune.
    assert_eq!((lease.from(), lease.until()), (at(52), Some(at(72))));
    assert_eq!(lease.listen_until(), Some(at(70)));
    assert_eq!(run(&mut arbiter, &mut port, at(0)), []);
    assert_eq!(arbiter.poll_deadline(), Some(at(52)));
    let id = lease.id();
    assert_eq!(
        run(&mut arbiter, &mut port, at(52)),
        [
            Seen::CloseGate(Some(at(72))),
            Seen::Retune(6),
            Seen::OpenGate,
            Seen::Station(StationNotice::LeaseStarted(id)),
        ]
    );
    assert_eq!(arbiter.poll_deadline(), Some(at(70)));
    assert_eq!(
        run(&mut arbiter, &mut port, at(70)),
        [
            Seen::CloseGate(None),
            Seen::Retune(1),
            Seen::OpenGate,
            Seen::Station(StationNotice::LeaseEnded(id, self::place())),
        ]
    );
    let _ = lease;
}

#[test]
fn an_absence_longer_than_the_duration_field_goes_without_a_reservation() {
    let (mut arbiter, place) = alone::<AirReservation>();
    arbiter.policy.absence = Duration::from_millis(40);
    running(&mut arbiter);
    let mut port = Port::default();
    let lease = lease(arbiter.request(at(0), scan(6, 100, false), place));
    assert_eq!(lease.until(), Some(at(92)));
    assert_eq!(
        run(&mut arbiter, &mut port, at(52)).first(),
        Some(&Seen::CloseGate(None))
    );
    let _ = lease;
}

#[test]
fn an_absence_waits_for_the_stations_readiness_and_is_cancelled_without_it() {
    let (mut arbiter, place) = beside_access_point::<NoReservation>();
    let mut port = Port::default();
    // At 45 ms the preparation window ends at 55 ms, past the slot at
    // 52 ms: the absence follows the next TBTT.
    let late = lease(arbiter.request(at(45), scan(6, 10, true), place));
    assert_eq!(late.from(), at(152));
    let id = late.id();
    assert_eq!(
        run(&mut arbiter, &mut port, at(152)),
        [Seen::Station(StationNotice::LeaseCancelled(
            id,
            CancelReason::NotReady,
            self::place()
        ))]
    );
    let _ = late;
    let ready = lease(arbiter.request(at(200), scan(6, 10, true), self::place()));
    assert_eq!(ready.from(), at(252));
    arbiter.ready(ready.id());
    assert_eq!(
        run(&mut arbiter, &mut port, at(252)).last(),
        Some(&Seen::Station(StationNotice::LeaseStarted(ready.id())))
    );
    let _ = ready;
}

#[test]
fn a_released_lease_always_ends_or_is_cancelled() {
    let mut port = Port::default();

    // Before its start: cancelled.
    let (mut arbiter, place) = beside_access_point::<NoReservation>();
    let waiting = lease(arbiter.request(at(0), scan(6, 10, false), place));
    let id = waiting.id();
    arbiter.release(waiting);
    assert_eq!(
        run(&mut arbiter, &mut port, at(0)),
        [Seen::Station(StationNotice::LeaseCancelled(
            id,
            CancelReason::Released,
            self::place()
        ))]
    );

    // While its gate closes: the cycle onto its channel ends, then the
    // lease's, back home.
    let closing = lease(arbiter.request(at(0), scan(6, 10, false), self::place()));
    let id = closing.id();
    assert!(matches!(
        arbiter.poll_action(at(52)),
        Some(ArbiterAction::CloseGate { .. })
    ));
    arbiter.release(closing);
    arbiter.gate_closed(Closed);
    let seen = run(&mut arbiter, &mut port, at(52));
    assert_eq!(
        seen.last(),
        Some(&Seen::Station(StationNotice::LeaseEnded(id, self::place())))
    );
    assert_eq!(arbiter.channel(), Some(channel(1)));

    // While the port retunes onto its channel.
    let moving = lease(arbiter.request(at(100), scan(6, 10, false), self::place()));
    let id = moving.id();
    assert!(matches!(
        arbiter.poll_action(at(152)),
        Some(ArbiterAction::CloseGate { .. })
    ));
    arbiter.gate_closed(Closed);
    assert!(matches!(
        arbiter.poll_action(at(152)),
        Some(ArbiterAction::Retune(_))
    ));
    arbiter.release(moving);
    arbiter.retuned(channel(6));
    let seen = run(&mut arbiter, &mut port, at(152));
    assert_eq!(
        seen.last(),
        Some(&Seen::Station(StationNotice::LeaseEnded(id, self::place())))
    );
    assert_eq!(arbiter.channel(), Some(channel(1)));

    // On the home channel: its own cycle, then it ends.
    let home = lease(arbiter.request(
        at(200),
        ChannelRequest {
            channel: channel(1),
            purpose: Purpose::Connect,
            announces_absence: false,
        },
        self::place(),
    ));
    let id = home.id();
    run(&mut arbiter, &mut port, at(200));
    arbiter.release(home);
    assert_eq!(
        run(&mut arbiter, &mut port, at(200)),
        [
            Seen::CloseGate(()),
            Seen::OpenGate,
            Seen::Station(StationNotice::LeaseEnded(id, self::place())),
        ]
    );
}

#[test]
fn an_access_point_that_stops_during_an_absence_cancels_it_and_the_port_returns() {
    let (mut arbiter, place) = beside_access_point::<NoReservation>();
    let mut port = Port::default();
    let lease = lease(arbiter.request(at(0), scan(6, 10, false), place));
    let id = lease.id();
    run(&mut arbiter, &mut port, at(52));
    arbiter.access_point(at(55), AccessPointFact::Stopped);
    assert_eq!(
        run(&mut arbiter, &mut port, at(55)),
        [
            Seen::CloseGate(()),
            Seen::Retune(1),
            Seen::OpenGate,
            Seen::Station(StationNotice::LeaseCancelled(
                id,
                CancelReason::AccessPointStopped,
                self::place()
            )),
        ]
    );
    let _ = lease;
}

#[test]
fn a_refused_move_cancels_the_lease_and_the_port_returns() {
    let (mut arbiter, place) = beside_access_point::<NoReservation>();
    let mut port = Port { refuse: true };
    let lease = lease(arbiter.request(at(0), scan(6, 10, false), place));
    let id = lease.id();
    assert_eq!(
        run(&mut arbiter, &mut port, at(52)),
        [
            Seen::CloseGate(()),
            Seen::Retune(6),
            Seen::Station(StationNotice::LeaseCancelled(
                id,
                CancelReason::Refused,
                self::place()
            )),
            Seen::Retune(1),
            Seen::OpenGate,
        ]
    );
    assert_eq!(arbiter.channel(), Some(channel(1)));
    let _ = lease;
}

#[test]
fn a_failed_return_or_a_poisoned_port_stops_the_station_and_the_access_point() {
    let (mut arbiter, place) = beside_access_point::<NoReservation>();
    let mut port = Port::default();
    let returning = lease(arbiter.request(at(0), scan(6, 10, false), place));
    let id = returning.id();
    run(&mut arbiter, &mut port, at(52));
    port.refuse = true;
    let seen = run(&mut arbiter, &mut port, at(64));
    assert!(seen.contains(&Seen::Station(StationNotice::LeaseCancelled(
        id,
        CancelReason::PortFailed,
        self::place()
    ))));
    assert!(seen.contains(&Seen::Station(StationNotice::Leave(
        LeaveReason::PortFailed
    ))));
    assert!(seen.contains(&Seen::AccessPoint(AccessPointCommand::Stop)));
    let _ = returning;

    let (mut arbiter, place) = beside_access_point::<NoReservation>();
    let poisoned = lease(arbiter.request(at(0), scan(6, 10, false), place));
    arbiter.poisoned();
    let seen = run(&mut arbiter, &mut port, at(0));
    assert!(seen.contains(&Seen::Station(StationNotice::Leave(
        LeaveReason::PortFailed
    ))));
    assert!(seen.contains(&Seen::AccessPoint(AccessPointCommand::Stop)));
    // A failed port answers every request with a leave and the place.
    assert!(matches!(
        arbiter.request(at(1), scan(6, 10, false), self::place()),
        Answer::Leave(LeaveReason::PortFailed, _)
    ));
    let _ = poisoned;
}

#[test]
fn a_gate_that_closes_too_late_for_two_retunes_and_the_shortest_listening_cancels_the_lease() {
    let (mut arbiter, place) = beside_access_point::<NoReservation>();
    let mut port = Port::default();
    let lease = lease(arbiter.request(at(0), scan(6, 10, false), place));
    let id = lease.id();
    assert!(matches!(
        arbiter.poll_action(at(52)),
        Some(ArbiterAction::CloseGate { .. })
    ));
    // The attempts in flight end at 58 ms: 58 + 2 + 2 + 5 > 66.
    arbiter.gate_closed(Closed);
    assert_eq!(
        run(&mut arbiter, &mut port, at(58)),
        [
            Seen::Station(StationNotice::LeaseCancelled(
                id,
                CancelReason::Late,
                self::place()
            )),
            Seen::OpenGate,
        ]
    );
    let _ = lease;
}

#[test]
fn an_upstream_switch_moves_the_port_with_the_access_points_announcement() {
    let (mut arbiter, _place) = beside_access_point::<NoReservation>();
    let mut port = Port::default();
    arbiter.upstream(at(0), UpstreamFact::Connected(channel(1)));
    // The upstream moves to channel 6 at 260 ms: the access point
    // announces it in its beacons before, and the port moves then.
    arbiter.upstream(
        at(10),
        UpstreamFact::SwitchAnnounced {
            target: channel(6),
            mode: ChannelSwitchMode::Continue,
            at: at(260),
        },
    );
    assert_eq!(
        run(&mut arbiter, &mut port, at(10)),
        [Seen::AccessPoint(AccessPointCommand::AnnounceSwitch {
            target: channel(6),
            mode: ChannelSwitchMode::Continue,
            count: 3,
        })]
    );
    // The access point's switch is due at its TBTT.
    arbiter.access_point(at(250), AccessPointFact::SwitchDue(channel(6)));
    assert_eq!(
        run(&mut arbiter, &mut port, at(250)),
        [
            Seen::CloseGate(()),
            Seen::Retune(6),
            Seen::OpenGate,
            Seen::Station(StationNotice::Moved(channel(6))),
        ]
    );
}

#[test]
fn a_lost_upstream_is_searched_for_in_absences_and_its_loss_stops_the_access_point() {
    let (mut arbiter, place) = beside_access_point::<NoReservation>();
    let mut port = Port::default();
    arbiter.upstream(at(0), UpstreamFact::Connected(channel(1)));
    arbiter.upstream(at(10), UpstreamFact::Lost);
    let search = |number| ChannelRequest {
        channel: channel(number),
        purpose: Purpose::Search {
            dwell: Duration::from_millis(10),
        },
        announces_absence: false,
    };
    let first = lease(arbiter.request(at(10), search(6), place));
    assert_eq!(first.from(), at(52));
    run(&mut arbiter, &mut port, at(52));
    let seen = run(&mut arbiter, &mut port, at(64));
    let Some(Seen::Station(StationNotice::LeaseEnded(_, place))) = seen.into_iter().last() else {
        panic!("the absence ends");
    };
    let _ = first;
    // The next absence follows one beacon interval later while dense.
    let second = lease(arbiter.request(at(64), search(11), place));
    assert_eq!(second.from(), at(152));
    arbiter.release(second);
    run(&mut arbiter, &mut port, at(64));
    // Past the loss policy's 30 s the access point stops, and the
    // station scans alone.
    let seen = run(&mut arbiter, &mut port, at(30_010));
    assert!(seen.contains(&Seen::AccessPoint(AccessPointCommand::Stop)));
}

#[test]
fn a_connect_away_from_a_running_access_point_is_refused_with_the_place() {
    let (mut arbiter, place) = beside_access_point::<NoReservation>();
    assert!(matches!(
        arbiter.request(
            at(0),
            ChannelRequest {
                channel: channel(6),
                purpose: Purpose::Connect,
                announces_absence: false,
            },
            place,
        ),
        Answer::Leave(LeaveReason::AwayFromAccessPoint, _)
    ));
}
