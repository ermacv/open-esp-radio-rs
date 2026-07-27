//! Event-driven WPA2 retransmission state without sleeps or timer polling.

use crate::state::Wpa2Transmit;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Wpa2RetryConfig {
    pub interval_us: u32,
    /// Number of retransmissions after the original transmission.
    pub attempts: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2RetryError {
    ZeroInterval,
    ZeroAttempts,
    DeadlineOverflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Wpa2RetryAlarm {
    pub generation: u32,
    pub deadline_us: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2RetryAction {
    Stale,
    Transmit {
        frame: Wpa2Transmit,
        next_alarm: Option<Wpa2RetryAlarm>,
    },
}

/// One outstanding finite retransmission schedule.
///
/// The caller programs `Wpa2RetryAlarm.deadline_us` into a one-shot hardware
/// or executor alarm. `on_alarm` consumes an interrupt/event edge; it never
/// reads time, sleeps, or loops over missed deadlines.
pub struct Wpa2Retry {
    config: Wpa2RetryConfig,
    generation: u32,
    pending: Option<Wpa2Transmit>,
    attempts_left: u8,
}

impl Wpa2Retry {
    pub const fn new(config: Wpa2RetryConfig) -> Result<Self, Wpa2RetryError> {
        if config.interval_us == 0 {
            return Err(Wpa2RetryError::ZeroInterval);
        }
        if config.attempts == 0 {
            return Err(Wpa2RetryError::ZeroAttempts);
        }
        Ok(Self {
            config,
            generation: 0,
            pending: None,
            attempts_left: 0,
        })
    }

    pub fn arm(
        &mut self,
        original: Wpa2Transmit,
        now_us: u64,
    ) -> Result<Wpa2RetryAlarm, Wpa2RetryError> {
        let deadline_us = now_us
            .checked_add(self.config.interval_us as u64)
            .ok_or(Wpa2RetryError::DeadlineOverflow)?;
        self.generation = next_generation(self.generation);
        self.pending = Some(Wpa2Transmit {
            retransmission: true,
            ..original
        });
        self.attempts_left = self.config.attempts;
        Ok(Wpa2RetryAlarm {
            generation: self.generation,
            deadline_us,
        })
    }

    pub fn cancel(&mut self) {
        self.generation = next_generation(self.generation);
        self.pending = None;
        self.attempts_left = 0;
    }

    pub fn on_alarm(
        &mut self,
        alarm: Wpa2RetryAlarm,
        now_us: u64,
    ) -> Result<Wpa2RetryAction, Wpa2RetryError> {
        if alarm.generation != self.generation {
            return Ok(Wpa2RetryAction::Stale);
        }
        let Some(frame) = self.pending else {
            return Ok(Wpa2RetryAction::Stale);
        };

        self.attempts_left -= 1;
        let next_alarm = if self.attempts_left == 0 {
            None
        } else {
            Some(self.alarm_after(now_us)?)
        };
        if next_alarm.is_none() {
            // Keep `pending` until the caller reports failure or completion.
            // A duplicate delivery of this same edge is rejected by advancing
            // the generation after emitting the final retransmission.
            self.generation = next_generation(self.generation);
            self.pending = None;
        }
        Ok(Wpa2RetryAction::Transmit { frame, next_alarm })
    }

    pub const fn is_armed(&self) -> bool {
        self.pending.is_some()
    }

    fn alarm_after(&self, now_us: u64) -> Result<Wpa2RetryAlarm, Wpa2RetryError> {
        Ok(Wpa2RetryAlarm {
            generation: self.generation,
            deadline_us: now_us
                .checked_add(self.config.interval_us as u64)
                .ok_or(Wpa2RetryError::DeadlineOverflow)?,
        })
    }
}

const fn next_generation(current: u32) -> u32 {
    let next = current.wrapping_add(1);
    if next == 0 {
        1
    } else {
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Wpa2TxMessage;

    fn transmit() -> Wpa2Transmit {
        Wpa2Transmit {
            message: Wpa2TxMessage::PairwiseMessage3,
            replay_counter: 8,
            retransmission: false,
        }
    }

    #[test]
    fn each_alarm_edge_emits_at_most_one_bounded_retransmission() {
        let mut retry = Wpa2Retry::new(Wpa2RetryConfig {
            interval_us: 100_000,
            attempts: 2,
        })
        .unwrap();
        let first = retry.arm(transmit(), 10).unwrap();
        assert_eq!(first.deadline_us, 100_010);

        let Wpa2RetryAction::Transmit {
            frame,
            next_alarm: Some(second),
        } = retry.on_alarm(first, first.deadline_us).unwrap()
        else {
            panic!("first alarm must retransmit and rearm")
        };
        assert!(frame.retransmission);
        assert_eq!(second.deadline_us, 200_010);

        assert!(matches!(
            retry.on_alarm(second, second.deadline_us).unwrap(),
            Wpa2RetryAction::Transmit {
                next_alarm: None,
                ..
            }
        ));
        assert!(!retry.is_armed());
        assert_eq!(
            retry.on_alarm(second, second.deadline_us).unwrap(),
            Wpa2RetryAction::Stale
        );
    }

    #[test]
    fn cancel_invalidates_an_already_programmed_alarm() {
        let mut retry = Wpa2Retry::new(Wpa2RetryConfig {
            interval_us: 1,
            attempts: 1,
        })
        .unwrap();
        let alarm = retry.arm(transmit(), 0).unwrap();
        retry.cancel();
        assert_eq!(retry.on_alarm(alarm, 1).unwrap(), Wpa2RetryAction::Stale);
    }
}
