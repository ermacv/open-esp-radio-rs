use super::*;
use crate::evidence::air::tests::frame;

const AP: MacAddress = MacAddress([0xbe, 0xfc, 0xe7, 0xae, 0xbb, 0xd3]);
const TARGET: MacAddress = MacAddress([0x02, 0, 0, 0, 0, 0x10]);
const LAPTOP: MacAddress = MacAddress([0x02, 0, 0, 0, 0, 0x20]);

fn at(
    time: u64,
    kind: FrameKind,
    transmitter: Option<MacAddress>,
    receiver: MacAddress,
) -> AirFrame {
    let mut record = frame(time, kind);
    record.mac_time_micros = Some(time);
    record.transmitter = transmitter;
    record.receiver = Some(receiver);
    record
}

fn control(mut record: AirFrame, phy: AirPhy, rate_kbps: u32) -> AirFrame {
    record.phy = Some(phy);
    record.rate_kbps = Some(rate_kbps);
    record.short_preamble = Some(false);
    record
}

/// One exchange starting at `start`: optional RTS/CTS at `phy`/`rate`, two
/// A-MPDU subframes and a BlockAck, with the RTS carrying `duration`.
fn exchange(
    start: u64,
    protected: bool,
    phy: AirPhy,
    rate_kbps: u32,
    duration: u16,
) -> Vec<AirFrame> {
    let mut frames = Vec::new();
    let mut time = start;
    if protected {
        let mut rts = control(at(time, FrameKind::RTS, Some(TARGET), AP), phy, rate_kbps);
        rts.duration_micros = Some(duration);
        let rts_airtime = control_airtime(&rts, RTS_BYTES).unwrap();
        frames.push(rts);
        time += rts_airtime + 10;
        let mut cts = control(at(time, FrameKind::CTS, None, TARGET), phy, rate_kbps);
        cts.duration_micros =
            Some(duration.saturating_sub(u16::try_from(rts_airtime + 10).unwrap()));
        frames.push(cts);
        time += control_airtime(&frames[1], ACK_BYTES).unwrap() + 10;
    }
    // The observer stamps TSFT on one MPDU of the A-MPDU.
    let mut subframe = at(time, FrameKind(0x28), Some(TARGET), AP);
    subframe.mac_time_micros = None;
    subframe.ampdu_reference = Some(u32::try_from(start).unwrap());
    frames.push(subframe.clone());
    subframe.mac_time_micros = Some(time);
    frames.push(subframe);
    time += 300;
    frames.push(control(
        at(time, FrameKind::BLOCK_ACK, Some(AP), TARGET),
        AirPhy::Ofdm,
        24_000,
    ));
    frames
}

#[test]
fn every_protected_ppdu_with_covering_nav_passes() {
    let frames = (0..10)
        .flat_map(|index| exchange(index * 10_000, true, AirPhy::Ofdm, 24_000, 400))
        .collect::<Vec<_>>();
    let evidence = analyze(&frames, AP, None, Expectation { erp: false }).unwrap();
    assert_eq!(evidence.target.as_deref(), Some("02:00:00:00:00:10"));
    assert_eq!((evidence.data_ppdus, evidence.protected_ppdus), (10, 10));
    assert_eq!(evidence.protected_basis_points(), 10_000);
    assert_eq!(evidence.target_rts, 10);
    assert_eq!(evidence.wrong_control_rate, 0);
    assert_eq!((evidence.nav_evaluated, evidence.nav_short), (10, 0));
}

#[test]
fn unprotected_ppdus_wrong_rates_and_short_nav_are_counted() {
    let mut frames = exchange(0, true, AirPhy::Ofdm, 24_000, 400);
    frames.extend(exchange(10_000, false, AirPhy::Ofdm, 24_000, 0));
    frames.extend(exchange(20_000, true, AirPhy::Ofdm, 24_000, 100));
    let evidence = analyze(&frames, AP, None, Expectation { erp: true }).unwrap();
    assert_eq!((evidence.data_ppdus, evidence.protected_ppdus), (3, 2));
    assert_eq!(evidence.protected_basis_points(), 6_666);
    assert_eq!(evidence.wrong_control_rate, 2);
    assert_eq!(evidence.nav_short, 1);
    assert!(evidence.first_nav_shortfall_micros.unwrap() > 0);
    let dsss = exchange(0, true, AirPhy::HrDsss, 11_000, 600);
    let evidence = analyze(&dsss, AP, None, Expectation { erp: true }).unwrap();
    assert_eq!((evidence.wrong_control_rate, evidence.nav_short), (0, 0));
}

#[test]
fn either_observed_half_of_an_exchange_shows_protection() {
    let mut lost_cts = exchange(0, true, AirPhy::Ofdm, 24_000, 600);
    lost_cts.remove(1);
    let mut lost_rts = exchange(10_000, true, AirPhy::Ofdm, 24_000, 600);
    lost_rts.remove(0);
    let frames = [lost_cts, lost_rts].concat();
    let evidence = analyze(&frames, AP, None, Expectation { erp: false }).unwrap();
    assert_eq!((evidence.data_ppdus, evidence.protected_ppdus), (2, 2));
    assert_eq!((evidence.cts_unobserved, evidence.rts_unobserved), (1, 1));
    // Only the exchange whose RTS was observed has a judged NAV.
    assert_eq!(evidence.nav_evaluated, 1);
}

#[test]
fn the_target_is_the_dominant_sender_other_than_the_peer() {
    let mut frames = exchange(0, true, AirPhy::Ofdm, 24_000, 400);
    for index in 0..5 {
        frames.push(at(50_000 + index, FrameKind(0x28), Some(LAPTOP), AP));
    }
    assert!(analyze(&frames, AP, None, Expectation { erp: false }).is_err());
    let evidence = analyze(&frames, AP, Some(LAPTOP), Expectation { erp: false }).unwrap();
    assert_eq!(evidence.target.as_deref(), Some("02:00:00:00:00:10"));
    assert!(analyze(&[], AP, None, Expectation { erp: false }).is_err());
    let mut untimed = frames.clone();
    untimed[1].mac_time_micros = None;
    assert!(analyze(&untimed, AP, Some(LAPTOP), Expectation { erp: false }).is_err());
}

#[test]
fn control_airtime_follows_the_phy() {
    let mut rts = control(frame(0, FrameKind::RTS), AirPhy::Dsss, 1_000);
    assert_eq!(control_airtime(&rts, RTS_BYTES), Some(192 + 160));
    rts.phy = Some(AirPhy::HrDsss);
    rts.rate_kbps = Some(11_000);
    rts.short_preamble = Some(true);
    assert_eq!(control_airtime(&rts, RTS_BYTES), Some(96 + 15));
    rts.phy = Some(AirPhy::Ofdm);
    rts.rate_kbps = Some(24_000);
    assert_eq!(control_airtime(&rts, RTS_BYTES), Some(20 + 2 * 4 + 6));
}
