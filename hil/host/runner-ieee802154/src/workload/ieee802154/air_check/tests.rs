use super::*;

fn request() -> Ieee802154AirCheckRequest {
    Ieee802154AirCheckRequest {
        channel: 15,
        cycles: 2,
        energy_scan_micros: 5_000,
        receive_window_millis: 200,
        scheduled_lead_micros: 20_000,
    }
}

fn transmit(requested_at_micros: u64, done_at_micros: u64) -> Ieee802154AirTransmit {
    Ieee802154AirTransmit {
        outcome: Ieee802154AirTxOutcome::Success,
        requested_at_micros,
        done_at_micros,
    }
}

fn cycle() -> Ieee802154AirCycle {
    Ieee802154AirCycle {
        energy: Ieee802154AirEnergyOutcome::Energy(-92),
        cca: Ieee802154AirCcaOutcome::Clear,
        direct: transmit(1_000, 2_200),
        scheduled: [transmit(30_000, 31_100), transmit(60_000, 61_050)],
        received_frames: 0,
        strongest_rssi_dbm: None,
    }
}

fn nominal() -> Ieee802154AirCheckEvidence {
    let mut evidence = Ieee802154AirCheckEvidence {
        stop: Ieee802154AirCheckStop::Complete,
        completed_cycles: 2,
        ..Default::default()
    };
    evidence.cycles[0] = cycle();
    evidence.cycles[1] = cycle();
    evidence
}

#[test]
fn nominal_cycles_pass_and_a_busy_channel_is_an_outcome() {
    validate(nominal(), request()).unwrap();
    let mut busy = nominal();
    busy.cycles[1].cca = Ieee802154AirCcaOutcome::Busy;
    busy.cycles[1].received_frames = 3;
    busy.cycles[1].strongest_rssi_dbm = Some(-40);
    validate(busy, request()).unwrap();
}

#[test]
fn incomplete_runs_fail() {
    let mut stopped = nominal();
    stopped.stop = Ieee802154AirCheckStop::StartFailed;
    assert!(validate(stopped, request()).is_err());
    let mut short = nominal();
    short.completed_cycles = 1;
    assert!(validate(short, request()).is_err());
}

#[test]
fn failed_operations_fail_their_cycle() {
    let mut scan = nominal();
    scan.cycles[0].energy = Ieee802154AirEnergyOutcome::Failed;
    assert!(validate(scan, request()).is_err());
    let mut implausible = nominal();
    implausible.cycles[0].energy = Ieee802154AirEnergyOutcome::Energy(40);
    assert!(validate(implausible, request()).is_err());
    let mut cca = nominal();
    cca.cycles[1].cca = Ieee802154AirCcaOutcome::Failed;
    assert!(validate(cca, request()).is_err());
    let mut direct = nominal();
    direct.cycles[0].direct.outcome = Ieee802154AirTxOutcome::HardwareFailure;
    assert!(validate(direct, request()).is_err());
}

#[test]
fn scheduled_transmits_must_honour_their_start() {
    let mut early = nominal();
    early.cycles[0].scheduled[0] = transmit(30_000, 29_999);
    assert!(validate(early, request()).is_err());
    let mut late = nominal();
    late.cycles[0].scheduled[1] = transmit(60_000, 60_000 + SCHEDULED_COMPLETION_BOUND_MICROS + 1);
    assert!(validate(late, request()).is_err());
    let mut overlapping = nominal();
    overlapping.cycles[0].scheduled[1] = transmit(31_000, 32_000);
    assert!(validate(overlapping, request()).is_err());
}

#[test]
fn the_command_timeout_covers_every_cycle() {
    let one = command_timeout(Ieee802154AirCheckRequest {
        cycles: 1,
        ..request()
    });
    let four = command_timeout(Ieee802154AirCheckRequest {
        cycles: 4,
        ..request()
    });
    assert!(four > one);
    assert!(one >= Duration::from_secs(25));
}
