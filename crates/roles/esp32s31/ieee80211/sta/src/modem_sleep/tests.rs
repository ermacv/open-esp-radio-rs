use super::*;

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

fn clock(now_micros: u64) -> PmClock {
    PmClock {
        now_micros,
        mac_time: now_micros as u32,
    }
}

fn beacon(timestamp_tsf: u64, tim: Option<PmTim>) -> PmBeacon {
    PmBeacon {
        timestamp_tsf,
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
        first_tbtt_tsf: BI as u64 * 7,
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
        after_micros: slice - u64::from(SLEEP_DELAY_MICROS),
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
        after_micros: u64::from(SLEEP_DELAY_MICROS),
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
    // Compatible, but the DTIM is not aligned yet.
    assert_eq!(pm.handle_tbtt_interval(3, tim(1, 3)), 3 * BI);
    // Aligned: the vendor re-derives it at the next beacon.
    pm.update_tbtt_at_next_beacon = false;
    assert_eq!(pm.handle_tbtt_interval(3, tim(0, 3)), 0);
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
        after_micros: 50_000 - 2_000 - 10_000,
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
