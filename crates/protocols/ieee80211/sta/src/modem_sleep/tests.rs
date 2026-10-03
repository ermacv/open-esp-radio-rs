use super::*;
use oer_ieee80211_mac::tsf::TsfInstant;

use std::vec::Vec;

const BEACON_INTERVAL_TU: u16 = 100;
const BI: u32 = 100 << 10;

fn shared() -> CoexView {
    CoexView {
        active: true,
        current_period: 6,
        flexible_period: 1,
        interval: BI / 100,
        phase0_share_percent: 50,
    }
}

fn clock(now: u64) -> PmClock {
    PmClock {
        now: oer_time::Instant::from_micros(now),
    }
}

fn beacon(timestamp_tsf: u64, tim: Option<PmTim>) -> PmBeacon {
    PmBeacon {
        timestamp_tsf: TsfInstant::from_micros(timestamp_tsf),
        interval_tu: BEACON_INTERVAL_TU,
        tim,
    }
}

fn quiet_tim() -> PmTim {
    PmTim {
        dtim_count: 1,
        dtim_period: 3,
        unicast: false,
        group: false,
    }
}

fn run(step: impl FnOnce(&mut PmActions)) -> Vec<PmAction> {
    let mut actions = PmActions::new();
    step(&mut actions);
    actions.iter().collect()
}

fn started(sleep_type: SleepType, coex: CoexView) -> ModemSleep {
    let mut pm = ModemSleep::new(sleep_type);
    let _ = run(|actions| pm.start(beacon(BI as u64 * 7 + 1_234, None), coex, actions));
    pm
}

#[test]
fn start_places_the_first_tbtt_on_the_last_beacon_boundary() {
    let mut pm = ModemSleep::new(SleepType::None);
    let actions = run(|actions| {
        pm.start(
            beacon(BI as u64 * 7 + 1_234, None),
            CoexView::INACTIVE,
            actions,
        )
    });
    assert!(pm.is_started());
    assert_eq!(pm.state(), PmState::Awake);
    assert!(actions.contains(&PmAction::StartTbtt(PmTbttSchedule {
        first_tbtt: TsfInstant::from_micros(BI as u64 * 7),
        interval_micros: BI,
        ahead_micros: TBTT_AHEAD_MICROS,
        wake_ahead_micros: TBTT_WAKE_WINDOW_MICROS + TBTT_AHEAD_MICROS,
    })));
    assert!(actions.contains(&PmAction::RxBeaconPriority(true)));
    assert!(actions.contains(&PmAction::SetCoexFlexiblePeriod(1)));
    // Without modem sleep there is no active timer.
    assert!(!actions.iter().any(|action| matches!(
        action,
        PmAction::Arm {
            timer: PmTimer::Active,
            ..
        }
    )));
}

#[test]
fn a_shared_start_sets_the_schedule_interval_from_the_beacon_interval() {
    let mut pm = ModemSleep::new(SleepType::None);
    let actions = run(|actions| pm.start(beacon(0, None), shared(), actions));
    assert!(actions.contains(&PmAction::SetCoexInterval(BI / 100)));
    assert!(actions.iter().any(|action| matches!(
        action,
        PmAction::StartTbtt(PmTbttSchedule {
            ahead_micros: SHARED_TBTT_AHEAD_MICROS,
            ..
        })
    )));
}

#[test]
fn a_wifi_phase_requests_the_slice_and_its_end_arms_the_slice_timer() {
    let coex = shared();
    let mut pm = started(SleepType::None, coex);
    let phase = CoexPhaseView {
        share_percent: 50,
        wifi: CoexPhaseView::WIFI_SLICE | CoexPhaseView::SLICE_END,
    };
    let slice = 50 * u64::from(coex.interval) * u64::from(coex.current_period);
    let actions = run(|actions| pm.coex_phase(phase, clock(1_000_000), coex, actions));
    assert_eq!(
        actions[0],
        PmAction::CoexRequest {
            event: PmCoexEvent::Slice,
            duration_micros: slice as u32,
        }
    );
    assert!(actions.contains(&PmAction::UnblockTx));
    assert!(actions.contains(&PmAction::Arm {
        timer: PmTimer::SliceEnd,
        after: oer_time::Duration::from_micros(slice - u64::from(SLEEP_DELAY_MICROS)),
    }));
    assert!(pm.in_slice());
}

#[test]
fn the_slice_end_blocks_tx_and_advertises_power_save() {
    let coex = shared();
    let mut pm = started(SleepType::None, coex);
    let actions = run(|actions| pm.slice_end_timer(coex, PmTraffic::IDLE, actions));
    assert!(!pm.in_slice());
    assert_eq!(actions[0], PmAction::BlockTx);
    assert!(actions.contains(&PmAction::SendNull { power_save: true }));
    assert_eq!(pm.state(), PmState::PowerSave);
}

#[test]
fn without_coexistence_a_station_without_power_save_never_leaves_the_air() {
    let mut pm = started(SleepType::None, CoexView::INACTIVE);
    let actions = run(|actions| pm.slice_end_timer(CoexView::INACTIVE, PmTraffic::IDLE, actions));
    assert!(actions.is_empty());
    assert_eq!(pm.state(), PmState::Awake);
    let actions = run(|actions| pm.active_timer(CoexView::INACTIVE, PmTraffic::IDLE, actions));
    assert!(actions.is_empty());
}

#[test]
fn an_acknowledged_power_save_null_leads_to_rf_sleep_after_the_sleep_delay() {
    let coex = shared();
    let mut pm = started(SleepType::None, coex);
    // The first beacon is parsed and received.
    let _ = run(|actions| {
        pm.beacon(
            beacon(BI as u64 * 8, Some(quiet_tim())),
            clock(1_000),
            coex,
            PmTraffic::IDLE,
            actions,
        )
    });
    let _ = run(|actions| pm.slice_end_timer(coex, PmTraffic::IDLE, actions));
    let actions =
        run(|actions| pm.null_done(true, true, clock(2_000), coex, PmTraffic::IDLE, actions));
    assert!(actions.contains(&PmAction::Arm {
        timer: PmTimer::SleepDelay,
        after: oer_time::Duration::from_micros(u64::from(SLEEP_DELAY_MICROS)),
    }));
    let actions = run(|actions| pm.sleep_delay_timer(clock(7_000), coex, PmTraffic::IDLE, actions));
    assert_eq!(
        actions,
        [
            PmAction::CoexRelease(PmCoexEvent::Slice),
            PmAction::CoexRelease(PmCoexEvent::GroupTraffic),
            PmAction::CoexRelease(PmCoexEvent::BeaconWindow),
            PmAction::ClearRxBeaconPriority,
            PmAction::RfSleep,
        ]
    );
    assert_eq!(pm.state(), PmState::Dozing);
}

#[test]
fn a_shared_tbtt_restarts_the_phases_requests_the_beacon_window_and_wakes() {
    let coex = shared();
    let mut pm = started(SleepType::None, coex);
    let _ = run(|actions| {
        pm.beacon(
            beacon(BI as u64 * 8, Some(quiet_tim())),
            clock(1_000),
            coex,
            PmTraffic::IDLE,
            actions,
        )
    });
    let _ = run(|actions| pm.slice_end_timer(coex, PmTraffic::IDLE, actions));
    let _ = run(|actions| pm.null_done(true, true, clock(2_000), coex, PmTraffic::IDLE, actions));
    let _ = run(|actions| pm.sleep_delay_timer(clock(7_000), coex, PmTraffic::IDLE, actions));
    assert_eq!(pm.state(), PmState::Dozing);

    let now = u64::from(BI) * 9;
    let actions = run(|actions| pm.tbtt(clock(now), coex, PmTraffic::IDLE, actions));
    assert!(actions.contains(&PmAction::RfWake));
    assert!(actions.contains(&PmAction::SetCoexInterval(BI / 100)));
    assert!(actions.contains(&PmAction::RestartCoexPhases));
    assert!(actions.contains(&PmAction::CoexRequest {
        event: PmCoexEvent::BeaconWindow,
        duration_micros: BEACON_WINDOW_MICROS,
    }));
    // The station counts its cycle from this TBTT and, inside its slice,
    // announces that it is awake again.
    assert!(actions.contains(&PmAction::SendNull { power_save: false }));
}

#[test]
fn a_frame_offered_outside_the_slice_waits_for_the_next_one() {
    let coex = shared();
    let mut pm = started(SleepType::None, coex);
    let _ = run(|actions| pm.slice_end_timer(coex, PmTraffic::IDLE, actions));
    // Late in the cycle, far from the Wi-Fi slice at its start.
    let late = coex.cycle_micros() - 1_000;
    let mut actions = PmActions::new();
    let hold = pm.tx_data(true, clock(late), coex, PmTraffic::IDLE, &mut actions);
    assert!(hold);
}

#[test]
fn the_tbtt_interval_keeps_step_with_the_dtim() {
    let mut pm = started(SleepType::None, shared());
    let tim = |dtim_count, dtim_period| {
        Some(PmTim {
            dtim_count,
            dtim_period,
            unicast: false,
            group: false,
        })
    };
    // Six beacons against a DTIM period of four: incompatible.
    assert_eq!(pm.handle_tbtt_interval(6, tim(0, 4)), 6 * BI);
    // Compatible and aligned with the DTIM: the whole period.
    pm.update_tbtt_at_next_beacon = false;
    assert_eq!(pm.handle_tbtt_interval(3, tim(0, 3)), 3 * BI);
    assert!(!pm.update_tbtt_at_next_beacon);
    // A DTIM every beacon keeps a TBTT every beacon.
    assert_eq!(pm.handle_tbtt_interval(1, tim(0, 1)), BI);
    assert!(!pm.update_tbtt_at_next_beacon);
    // Compatible, but the DTIM is not aligned yet: run up to it, then
    // re-derive the interval at the next beacon.
    assert_eq!(pm.handle_tbtt_interval(3, tim(1, 3)), BI);
    assert!(pm.update_tbtt_at_next_beacon);
    // No TIM: one beacon interval, resynchronized at the next beacon.
    assert_eq!(pm.handle_tbtt_interval(6, None), BI);
}

#[test]
fn a_preemption_shortens_the_beacon_window_until_it_ends() {
    let coex = shared();
    let mut pm = started(SleepType::None, coex);
    let actions = run(|actions| {
        pm.preemption_end(Some(50_000), clock(10_000), coex, PmTraffic::IDLE, actions)
    });
    assert!(actions.contains(&PmAction::RxBeaconTime {
        window_micros: PREEMPTED_BEACON_WINDOW_MICROS,
        time_micros: PREEMPTED_BEACON_TIME_MICROS,
    }));
    assert!(actions.contains(&PmAction::Arm {
        timer: PmTimer::Preemption,
        after: oer_time::Duration::from_micros(50_000 - 2_000 - 10_000),
    }));
    let actions =
        run(|actions| pm.preemption_end(None, clock(60_000), coex, PmTraffic::IDLE, actions));
    assert!(actions.contains(&PmAction::RxBeaconTime {
        window_micros: BEACON_WINDOW_MICROS,
        time_micros: BEACON_TIME_MICROS,
    }));
}

#[test]
fn a_failed_active_null_is_retried_inside_the_slice() {
    let coex = shared();
    let mut pm = started(SleepType::None, coex);
    let _ = run(|actions| pm.slice_end_timer(coex, PmTraffic::IDLE, actions));
    let _ = run(|actions| pm.null_done(true, true, clock(2_000), coex, PmTraffic::IDLE, actions));
    // A TBTT wakes the station with an active Null.
    let _ = run(|actions| pm.tbtt(clock(u64::from(BI) * 9), coex, PmTraffic::IDLE, actions));
    let actions = run(|actions| {
        pm.null_done(
            false,
            false,
            clock(u64::from(BI) * 9 + 100),
            coex,
            PmTraffic::IDLE,
            actions,
        )
    });
    assert!(actions.contains(&PmAction::SendNull { power_save: false }));
}

#[test]
fn stop_wakes_the_rf_and_releases_held_frames() {
    let coex = shared();
    let mut pm = started(SleepType::None, coex);
    let _ = run(|actions| pm.slice_end_timer(coex, PmTraffic::IDLE, actions));
    let actions = run(|actions| pm.stop(coex, actions));
    assert!(actions.contains(&PmAction::UnblockTx));
    assert!(actions.contains(&PmAction::StopTbtt));
    assert!(actions.contains(&PmAction::ReleaseHeldFrames));
    assert!(!pm.is_started());
    assert_eq!(pm.state(), PmState::Awake);
}

#[test]
fn the_listen_interval_scales_to_beacons_rounded_to_the_dtim() {
    // At 100 TU beacons a listen interval counts beacons directly.
    // At or above one DTIM: the nearest multiple of the DTIM, a tie up.
    assert_eq!(scaled_listen_interval(10, BI, 3), 9);
    assert_eq!(scaled_listen_interval(11, BI, 3), 12);
    assert_eq!(scaled_listen_interval(10, BI, 4), 12);
    assert_eq!(scaled_listen_interval(10, BI, 1), 10);
    // Below one DTIM: the nearest divisor of it, a tie up.
    assert_eq!(scaled_listen_interval(3, BI, 10), 2);
    assert_eq!(scaled_listen_interval(4, BI, 10), 5);
    assert_eq!(scaled_listen_interval(3, BI, 8), 4);
    // The listen interval counts 100-TU units: at 200 TU beacons ten units
    // are five beacons.
    assert_eq!(scaled_listen_interval(10, 2 * BI, 1), 5);
    // Fewer than one beacon is one.
    assert_eq!(scaled_listen_interval(1, 4 * BI, 1), 1);
}

#[test]
fn max_modem_programs_the_tbtt_at_the_listen_interval_not_every_beacon() {
    let listen_interval = crate::request::StationListenInterval::new(10).unwrap();
    let mut pm = started(SleepType::MaxModem(listen_interval), CoexView::INACTIVE);
    let tim = PmTim {
        dtim_count: 0,
        dtim_period: 1,
        unicast: false,
        group: false,
    };
    let actions = run(|actions| {
        pm.beacon(
            beacon(BI as u64 * 9, Some(tim)),
            clock(1_000_000),
            CoexView::INACTIVE,
            PmTraffic::IDLE,
            actions,
        )
    });
    let interval = actions.iter().find_map(|action| match action {
        PmAction::StartTbtt(schedule) => Some(schedule.interval_micros),
        _ => None,
    });
    assert_eq!(interval, Some(10 * BI));
}

#[test]
fn a_dtim_change_rescales_the_listen_interval_and_reprograms_max_modem() {
    let listen_interval = crate::request::StationListenInterval::new(10).unwrap();
    let mut pm = started(SleepType::MaxModem(listen_interval), CoexView::INACTIVE);
    let tim = |dtim_period| PmTim {
        dtim_count: 0,
        dtim_period,
        unicast: false,
        group: false,
    };
    let mut beacon_with = |dtim_period| {
        run(|actions| {
            pm.beacon(
                beacon(BI as u64 * 9, Some(tim(dtim_period))),
                clock(1_000_000),
                CoexView::INACTIVE,
                PmTraffic::IDLE,
                actions,
            )
        })
    };
    let _ = beacon_with(1);
    let actions = beacon_with(3);
    let interval = actions.iter().find_map(|action| match action {
        PmAction::StartTbtt(schedule) => Some(schedule.interval_micros),
        _ => None,
    });
    // Ten beacons against a DTIM of three: nine.
    assert_eq!(interval, Some(9 * BI));
}

#[test]
fn the_listen_interval_starts_at_one_beacon_as_pm_attach_sets_it() {
    assert_eq!(ModemSleep::new(SleepType::None).listen_interval_beacons, 1);
}
