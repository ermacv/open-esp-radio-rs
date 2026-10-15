//! Checked projection of the portable PM schedule to S31 timer fields.

use oer_ieee80211_sta::modem_sleep::{PmAction, PmActions, PmScheduleError};
use oer_time::Instant;

pub(super) fn validate(actions: &PmActions, now: Instant) -> Result<(), PmScheduleError> {
    actions.validate_deadlines(now)?;
    for action in actions.iter() {
        match action {
            PmAction::StartTbtt(schedule) => {
                interval(schedule.interval_micros)?;
            }
            PmAction::SetTbttInterval(micros) => {
                interval(micros)?;
            }
            _ => {}
        }
    }
    Ok(())
}

pub(super) fn interval(micros: u64) -> Result<u32, PmScheduleError> {
    u32::try_from(micros).map_err(|_| PmScheduleError::TbttIntervalOutOfRange { micros })
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_ieee80211_mac::tsf::TsfInstant;
    use oer_ieee80211_sta::{
        modem_sleep::{CoexView, ModemSleep, PmBeacon, PmClock, PmTim, PmTraffic, SleepType},
        request::StationListenInterval,
    };

    #[test]
    fn projection_accepts_the_exact_limit_and_refuses_the_next_interval() {
        assert_eq!(interval(u64::from(u32::MAX)), Ok(u32::MAX));
        assert_eq!(
            interval(u64::from(u32::MAX) + 1),
            Err(PmScheduleError::TbttIntervalOutOfRange {
                micros: u64::from(u32::MAX) + 1
            })
        );
    }

    #[test]
    fn a_complete_batch_is_checked_before_its_first_effect() {
        let mut pm = ModemSleep::new(SleepType::MaxModem(
            StationListenInterval::new(u16::MAX).unwrap(),
        ));
        let beacon = PmBeacon {
            timestamp_tsf: TsfInstant::from_micros(0),
            interval_tu: 1,
            tim: None,
        };
        let initial = pm.start(beacon, CoexView::INACTIVE);
        assert_eq!(validate(&initial, Instant::from_micros(0)), Ok(()));
        assert_eq!(
            validate(&initial, Instant::from_micros(u64::MAX)),
            Err(PmScheduleError::DeadlineOverflow)
        );
        let actions = pm
            .beacon(
                PmBeacon {
                    tim: Some(PmTim {
                        dtim_count: 0,
                        dtim_period: 1,
                        unicast: false,
                        group: false,
                    }),
                    ..beacon
                },
                PmClock {
                    now: Instant::from_micros(0),
                },
                CoexView::INACTIVE,
                PmTraffic::IDLE,
            )
            .unwrap();
        assert_eq!(
            validate(&actions, Instant::from_micros(0)),
            Err(PmScheduleError::TbttIntervalOutOfRange {
                micros: 6_710_784_000
            })
        );
        assert!(!actions.is_empty());
    }
}
