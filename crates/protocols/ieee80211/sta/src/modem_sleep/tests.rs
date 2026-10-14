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

fn run(step: impl FnOnce() -> PmActions) -> Vec<PmAction> {
    step().into_iter().collect()
}

fn started(sleep_type: SleepType, coex: CoexView) -> ModemSleep {
    let mut pm = ModemSleep::new(sleep_type);
    let _ = run(|| pm.start(beacon(BI as u64 * 7 + 1_234, None), coex));
    pm
}

#[test]
fn every_start_and_stop_keeps_its_complete_ordered_effects() {
    let mut pm = ModemSleep::new(SleepType::MinModem);
    let coex = shared();
    let mut actions = Vec::new();
    for _ in 0..6 {
        actions.extend(pm.start(beacon(0, None), coex));
        assert!(pm.is_started());
        actions.extend(pm.stop(coex));
        assert!(!pm.is_started());
    }
    let expected_cycle = [
        PmAction::Coex(PmCoexAction::SetInterval(BI / 100)),
        PmAction::Coex(PmCoexAction::SetFlexiblePeriod(1)),
        PmAction::StartTbtt(PmTbttSchedule {
            first_tbtt: TsfInstant::from_micros(0),
            interval_micros: BI,
            ahead_micros: SHARED_TBTT_AHEAD_MICROS,
            wake_ahead_micros: SHARED_TBTT_AHEAD_MICROS + TBTT_WAKE_WINDOW_MICROS,
        }),
        PmAction::RxBeaconPriority(true),
        PmAction::RxBeaconTime {
            window_micros: BEACON_WINDOW_MICROS,
            time_micros: BEACON_TIME_MICROS,
        },
        PmAction::Disarm(PmTimer::Active),
        PmAction::Arm {
            timer: PmTimer::Active,
            after: Duration::from_micros(u64::from(ACTIVE_TIMEOUT_MICROS)),
        },
        PmAction::UnblockTx,
        PmAction::Disarm(PmTimer::SleepDelay),
        PmAction::Disarm(PmTimer::Active),
        PmAction::StopTbtt,
        PmAction::RxBeaconPriority(false),
        PmAction::Disarm(PmTimer::Dream),
        PmAction::Disarm(PmTimer::SliceEnd),
        PmAction::Coex(PmCoexAction::SetFlexiblePeriod(1)),
    ];
    assert_eq!(actions, expected_cycle.repeat(6));
}

#[test]
fn a_tbtt_keeps_wake_slice_and_parameter_effects_in_order() {
    let coex = shared();
    let mut pm = started(SleepType::MinModem, CoexView::INACTIVE);
    let _ = pm.beacon(
        beacon(0, Some(quiet_tim())),
        clock(1_000),
        CoexView::INACTIVE,
        PmTraffic::IDLE,
    );
    let _ = pm.active_timer(CoexView::INACTIVE, PmTraffic::IDLE);
    let _ = pm.null_done(
        true,
        true,
        clock(2_000),
        CoexView::INACTIVE,
        PmTraffic::IDLE,
    );
    let _ = pm.sleep_delay_timer(clock(7_000), CoexView::INACTIVE, PmTraffic::IDLE);
    assert_eq!(pm.state(), PmState::Dozing);
    let _ = pm.slice_end_timer(coex, PmTraffic::IDLE);
    let (hold, _) = pm.tx_data(
        true,
        clock(coex.cycle_micros() - 1_000),
        coex,
        PmTraffic::IDLE,
    );
    assert!(hold);

    let actions = pm.tbtt(
        clock(coex.cycle_micros() * 2),
        coex,
        PmTraffic {
            tx_pending: true,
            connection_pending: true,
        },
    );
    assert_eq!(
        actions.into_iter().collect::<Vec<_>>(),
        [
            PmAction::RfWake,
            PmAction::Disarm(PmTimer::SliceEnd),
            PmAction::Coex(PmCoexAction::SetInterval(BI / 100)),
            PmAction::Coex(PmCoexAction::RestartPhases),
            PmAction::RxBeaconPriority(true),
            PmAction::Coex(PmCoexAction::Request {
                event: PmCoexEvent::BeaconWindow,
                duration_micros: BEACON_WINDOW_MICROS
            }),
            PmAction::UnblockTx,
            PmAction::Disarm(PmTimer::Active),
            PmAction::Arm {
                timer: PmTimer::Active,
                after: Duration::from_micros(u64::from(ACTIVE_TIMEOUT_MICROS))
            },
            PmAction::ReleaseHeldFrames,
            PmAction::Disarm(PmTimer::SliceEnd),
            PmAction::Arm {
                timer: PmTimer::SliceEnd,
                after: Duration::from_micros(
                    coex.cycle_micros() / 2 - u64::from(SLEEP_DELAY_MICROS)
                )
            },
            PmAction::SendNull { power_save: false },
            PmAction::SetTbttInterval(BI * 6),
            PmAction::SetTbttAhead {
                ahead_micros: SHARED_TBTT_AHEAD_MICROS,
                wake_ahead_micros: SHARED_TBTT_AHEAD_MICROS + TBTT_WAKE_WINDOW_MICROS
            },
        ]
    );
    assert_eq!(pm.state(), PmState::Awake);
    assert!(pm.active_null_in_flight);
    assert!(!pm.wake_deferred);
}

#[test]
fn a_tx_wake_has_the_same_effects_and_state_as_an_acknowledged_active_null() {
    for sleep in [
        SleepType::None,
        SleepType::MinModem,
        SleepType::MaxModem(crate::request::StationListenInterval::DEFAULT),
    ] {
        for coex in [CoexView::INACTIVE, shared()] {
            for state in [PmState::Awake, PmState::PowerSave, PmState::Dozing] {
                let mut pm = started(sleep, coex);
                pm.state = state;
                pm.receiving = true;
                pm.active_null_failures = NULL_RETRY_LIMIT;
                let mut expected = pm.clone();
                expected.active_null_in_flight = true;
                let expected_effects =
                    expected.null_done(false, true, clock(1_000), coex, PmTraffic::IDLE);
                let (wake, effects) = pm.transition(|pm, [a, b, c, d, e, f, g, ..]| {
                    pm.go_to_wake(
                        true,
                        clock(1_000),
                        coex,
                        PmTraffic::IDLE,
                        [a, b, c, d, e, f, g],
                    )
                });
                assert_eq!(wake, WakeResult::Awake);
                assert_eq!(pm, expected);
                assert_eq!(
                    effects.into_iter().collect::<Vec<_>>(),
                    expected_effects.into_iter().collect::<Vec<_>>()
                );
            }
        }
    }
}

#[test]
fn start_places_the_first_tbtt_on_the_last_beacon_boundary() {
    let mut pm = ModemSleep::new(SleepType::None);
    let actions = run(|| pm.start(beacon(BI as u64 * 7 + 1_234, None), CoexView::INACTIVE));
    assert!(pm.is_started());
    assert_eq!(pm.state(), PmState::Awake);
    assert!(actions.contains(&PmAction::StartTbtt(PmTbttSchedule {
        first_tbtt: TsfInstant::from_micros(BI as u64 * 7),
        interval_micros: BI,
        ahead_micros: TBTT_AHEAD_MICROS,
        wake_ahead_micros: TBTT_WAKE_WINDOW_MICROS + TBTT_AHEAD_MICROS,
    })));
    assert!(actions.contains(&PmAction::RxBeaconPriority(true)));
    assert!(actions.contains(&PmAction::Coex(PmCoexAction::SetFlexiblePeriod(1))));
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
    let actions = run(|| pm.start(beacon(0, None), shared()));
    assert!(actions.contains(&PmAction::Coex(PmCoexAction::SetInterval(BI / 100))));
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
    let actions = run(|| pm.coex_phase(phase, clock(1_000_000), coex));
    assert_eq!(
        actions[0],
        PmAction::Coex(PmCoexAction::Request {
            event: PmCoexEvent::Slice,
            duration_micros: slice as u32,
        })
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
    let actions = run(|| pm.slice_end_timer(coex, PmTraffic::IDLE));
    assert!(!pm.in_slice());
    assert_eq!(actions[0], PmAction::BlockTx);
    assert!(actions.contains(&PmAction::SendNull { power_save: true }));
    assert_eq!(pm.state(), PmState::PowerSave);
}

#[test]
fn without_coexistence_a_station_without_power_save_never_leaves_the_air() {
    let mut pm = started(SleepType::None, CoexView::INACTIVE);
    let actions = run(|| pm.slice_end_timer(CoexView::INACTIVE, PmTraffic::IDLE));
    assert!(actions.is_empty());
    assert_eq!(pm.state(), PmState::Awake);
    let actions = run(|| pm.active_timer(CoexView::INACTIVE, PmTraffic::IDLE));
    assert!(actions.is_empty());
}

#[test]
fn an_acknowledged_power_save_null_leads_to_rf_sleep_after_the_sleep_delay() {
    let coex = shared();
    let mut pm = started(SleepType::None, coex);
    // The first beacon is parsed and received.
    let _ = run(|| {
        pm.beacon(
            beacon(BI as u64 * 8, Some(quiet_tim())),
            clock(1_000),
            coex,
            PmTraffic::IDLE,
        )
    });
    let _ = run(|| pm.slice_end_timer(coex, PmTraffic::IDLE));
    let actions = run(|| pm.null_done(true, true, clock(2_000), coex, PmTraffic::IDLE));
    assert!(actions.contains(&PmAction::Arm {
        timer: PmTimer::SleepDelay,
        after: oer_time::Duration::from_micros(u64::from(SLEEP_DELAY_MICROS)),
    }));
    let actions = run(|| pm.sleep_delay_timer(clock(7_000), coex, PmTraffic::IDLE));
    assert_eq!(
        actions,
        [
            PmAction::Coex(PmCoexAction::Release(PmCoexEvent::Slice)),
            PmAction::Coex(PmCoexAction::Release(PmCoexEvent::GroupTraffic)),
            PmAction::Coex(PmCoexAction::Release(PmCoexEvent::BeaconWindow)),
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
    let _ = run(|| {
        pm.beacon(
            beacon(BI as u64 * 8, Some(quiet_tim())),
            clock(1_000),
            coex,
            PmTraffic::IDLE,
        )
    });
    let _ = run(|| pm.slice_end_timer(coex, PmTraffic::IDLE));
    let _ = run(|| pm.null_done(true, true, clock(2_000), coex, PmTraffic::IDLE));
    let _ = run(|| pm.sleep_delay_timer(clock(7_000), coex, PmTraffic::IDLE));
    assert_eq!(pm.state(), PmState::Dozing);

    let now = u64::from(BI) * 9;
    let actions = run(|| pm.tbtt(clock(now), coex, PmTraffic::IDLE));
    assert!(actions.contains(&PmAction::RfWake));
    assert!(actions.contains(&PmAction::Coex(PmCoexAction::SetInterval(BI / 100))));
    assert!(actions.contains(&PmAction::Coex(PmCoexAction::RestartPhases)));
    assert!(actions.contains(&PmAction::Coex(PmCoexAction::Request {
        event: PmCoexEvent::BeaconWindow,
        duration_micros: BEACON_WINDOW_MICROS,
    })));
    // The station counts its cycle from this TBTT and, inside its slice,
    // announces that it is awake again.
    assert!(actions.contains(&PmAction::SendNull { power_save: false }));
}

#[test]
fn a_frame_offered_outside_the_slice_waits_for_the_next_one() {
    let coex = shared();
    let mut pm = started(SleepType::None, coex);
    let _ = run(|| pm.slice_end_timer(coex, PmTraffic::IDLE));
    // Late in the cycle, far from the Wi-Fi slice at its start.
    let late = coex.cycle_micros() - 1_000;
    let (hold, actions) = pm.tx_data(true, clock(late), coex, PmTraffic::IDLE);
    assert_eq!(
        actions.into_iter().collect::<Vec<_>>(),
        [PmAction::Disarm(PmTimer::SliceEnd)]
    );
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
    let actions = run(|| pm.preemption_end(Some(50_000), clock(10_000), coex, PmTraffic::IDLE));
    assert!(actions.contains(&PmAction::RxBeaconTime {
        window_micros: PREEMPTED_BEACON_WINDOW_MICROS,
        time_micros: PREEMPTED_BEACON_TIME_MICROS,
    }));
    assert!(actions.contains(&PmAction::Arm {
        timer: PmTimer::Preemption,
        after: oer_time::Duration::from_micros(50_000 - 2_000 - 10_000),
    }));
    let actions = run(|| pm.preemption_end(None, clock(60_000), coex, PmTraffic::IDLE));
    assert!(actions.contains(&PmAction::RxBeaconTime {
        window_micros: BEACON_WINDOW_MICROS,
        time_micros: BEACON_TIME_MICROS,
    }));
}

#[test]
fn a_failed_active_null_is_retried_inside_the_slice() {
    let coex = shared();
    let mut pm = started(SleepType::None, coex);
    let _ = run(|| pm.slice_end_timer(coex, PmTraffic::IDLE));
    let _ = run(|| pm.null_done(true, true, clock(2_000), coex, PmTraffic::IDLE));
    // A TBTT wakes the station with an active Null.
    let _ = run(|| pm.tbtt(clock(u64::from(BI) * 9), coex, PmTraffic::IDLE));
    let actions = run(|| {
        pm.null_done(
            false,
            false,
            clock(u64::from(BI) * 9 + 100),
            coex,
            PmTraffic::IDLE,
        )
    });
    assert!(actions.contains(&PmAction::SendNull { power_save: false }));
}

#[test]
fn stop_wakes_the_rf_and_releases_held_frames() {
    let coex = shared();
    let mut pm = started(SleepType::None, coex);
    let _ = run(|| pm.slice_end_timer(coex, PmTraffic::IDLE));
    let actions = run(|| pm.stop(coex));
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
    assert_eq!(
        scaled_listen_interval(
            10,
            core::num::NonZeroU32::new(BI).unwrap_or(core::num::NonZeroU32::MIN),
            3
        ),
        9
    );
    assert_eq!(
        scaled_listen_interval(
            11,
            core::num::NonZeroU32::new(BI).unwrap_or(core::num::NonZeroU32::MIN),
            3
        ),
        12
    );
    assert_eq!(
        scaled_listen_interval(
            10,
            core::num::NonZeroU32::new(BI).unwrap_or(core::num::NonZeroU32::MIN),
            4
        ),
        12
    );
    assert_eq!(
        scaled_listen_interval(
            10,
            core::num::NonZeroU32::new(BI).unwrap_or(core::num::NonZeroU32::MIN),
            1
        ),
        10
    );
    // Below one DTIM: the nearest divisor of it, a tie up.
    assert_eq!(
        scaled_listen_interval(
            3,
            core::num::NonZeroU32::new(BI).unwrap_or(core::num::NonZeroU32::MIN),
            10
        ),
        2
    );
    assert_eq!(
        scaled_listen_interval(
            4,
            core::num::NonZeroU32::new(BI).unwrap_or(core::num::NonZeroU32::MIN),
            10
        ),
        5
    );
    assert_eq!(
        scaled_listen_interval(
            3,
            core::num::NonZeroU32::new(BI).unwrap_or(core::num::NonZeroU32::MIN),
            8
        ),
        4
    );
    // The listen interval counts 100-TU units: at 200 TU beacons ten units
    // are five beacons.
    assert_eq!(
        scaled_listen_interval(
            10,
            core::num::NonZeroU32::new(2 * BI).unwrap_or(core::num::NonZeroU32::MIN),
            1
        ),
        5
    );
    // Fewer than one beacon is one.
    assert_eq!(
        scaled_listen_interval(
            1,
            core::num::NonZeroU32::new(4 * BI).unwrap_or(core::num::NonZeroU32::MIN),
            1
        ),
        1
    );
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
    let actions = run(|| {
        pm.beacon(
            beacon(BI as u64 * 9, Some(tim)),
            clock(1_000_000),
            CoexView::INACTIVE,
            PmTraffic::IDLE,
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
        run(|| {
            pm.beacon(
                beacon(BI as u64 * 9, Some(tim(dtim_period))),
                clock(1_000_000),
                CoexView::INACTIVE,
                PmTraffic::IDLE,
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
