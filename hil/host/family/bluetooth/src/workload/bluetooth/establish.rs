//! Bounded retry of an LE connection the DUT never received.
//!
//! A peer's CONNECT_IND can collide in the air with another device's
//! response to the same `ADV_IND`: the stand's adapter then reports the
//! connection created and closes it as "Connection Failed to be Established"
//! (0x3E), while the DUT never saw it. A central retries such a connection;
//! the fixture does the same, but only when the DUT's own observation proves
//! the connection never reached it, and it records every attempt so the loss
//! stays visible as a measurement.

use crate::Result;
use oer_hil_run_bundle_format::run::{Better, Measurement, MeasurementUnit};

/// Attempts per connection, the first included.
pub(super) const ATTEMPTS: u32 = 3;

/// The outcome of one connection attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum Outcome {
    /// The attempt succeeded.
    Established,
    /// The attempt failed and the DUT never saw the connection.
    NotEstablished,
}

/// One connection attempt, as the scenario's observations record it.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(super) struct Attempt {
    /// What the connection was for.
    pub phase: &'static str,
    /// 1-based.
    pub attempt: u32,
    pub outcome: Outcome,
    /// The host-side error of a failed attempt.
    pub host_error: Option<String>,
}

/// Run `connect` until it succeeds, retrying a failure while `unseen`
/// confirms that the DUT never saw that connection, at most [`ATTEMPTS`]
/// times. Any other failure, or one `unseen` does not confirm, ends at once.
/// Both closures share `state`, such as the scenario's sample log.
pub(super) fn establish<S, T>(
    phase: &'static str,
    attempts: &mut Vec<Attempt>,
    state: &mut S,
    mut connect: impl FnMut(&mut S) -> Result<T>,
    mut unseen: impl FnMut(&mut S) -> Result<bool>,
) -> Result<T> {
    for attempt in 1..=ATTEMPTS {
        match connect(state) {
            Ok(value) => {
                attempts.push(Attempt {
                    phase,
                    attempt,
                    outcome: Outcome::Established,
                    host_error: None,
                });
                return Ok(value);
            }
            Err(error) => {
                if !unseen(state)? {
                    return Err(error);
                }
                attempts.push(Attempt {
                    phase,
                    attempt,
                    outcome: Outcome::NotEstablished,
                    host_error: Some(error.to_string()),
                });
            }
        }
    }
    Err(format!("{phase}: no connection reached the DUT in {ATTEMPTS} attempts").into())
}

/// The repetition's connection attempts as measurements: all of them, and
/// those the DUT never received. HIL records the loss; whether it is
/// acceptable is qualification's decision.
pub(super) fn measurements(attempts: &[Attempt]) -> [Measurement; 2] {
    let lost = attempts
        .iter()
        .filter(|attempt| attempt.outcome == Outcome::NotEstablished)
        .count() as u64;
    [
        Measurement::observed(
            "connection-attempts",
            attempts.len() as u64,
            MeasurementUnit::Count,
        ),
        Measurement::observed("connections-not-established", lost, MeasurementUnit::Count)
            .better(Better::Lower),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_connection_the_dut_never_saw_is_retried_and_recorded() {
        let mut attempts = Vec::new();
        let mut tries = 0;
        let value = establish(
            "connect",
            &mut attempts,
            &mut (),
            |_| {
                tries += 1;
                if tries < 3 {
                    Err("ATT socket disconnected".into())
                } else {
                    Ok(7)
                }
            },
            |_| Ok(true),
        )
        .unwrap();
        assert_eq!(value, 7);
        assert_eq!(
            attempts.iter().map(|a| a.outcome).collect::<Vec<_>>(),
            [
                Outcome::NotEstablished,
                Outcome::NotEstablished,
                Outcome::Established
            ]
        );
        assert_eq!(
            attempts[0].host_error.as_deref(),
            Some("ATT socket disconnected")
        );
    }

    #[test]
    fn a_failure_the_dut_saw_ends_at_once() {
        let mut attempts = Vec::new();
        let mut tries = 0;
        let error = establish(
            "connect",
            &mut attempts,
            &mut (),
            |_| -> Result<()> {
                tries += 1;
                Err("ATT socket disconnected".into())
            },
            |_| Ok(false),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "ATT socket disconnected");
        assert_eq!(tries, 1);
        assert!(attempts.is_empty());
    }

    #[test]
    fn retries_are_bounded() {
        let mut attempts = Vec::new();
        let mut tries = 0;
        let error = establish(
            "pairing",
            &mut attempts,
            &mut (),
            |_| -> Result<()> {
                tries += 1;
                Err("le-connection-abort-by-local".into())
            },
            |_| Ok(true),
        )
        .unwrap_err();
        assert_eq!(tries, ATTEMPTS);
        assert_eq!(attempts.len(), ATTEMPTS as usize);
        assert!(error.to_string().contains("no connection reached the DUT"));
    }

    #[test]
    fn measurements_count_every_attempt_and_the_lost_ones() {
        let attempt = |outcome| Attempt {
            phase: "connect",
            attempt: 1,
            outcome,
            host_error: None,
        };
        let [all, lost] = measurements(&[
            attempt(Outcome::NotEstablished),
            attempt(Outcome::Established),
            attempt(Outcome::Established),
        ]);
        assert_eq!((all.name.as_str(), all.value), ("connection-attempts", 3));
        assert_eq!(
            (lost.name.as_str(), lost.value),
            ("connections-not-established", 1)
        );
        assert_eq!(lost.better, Some(Better::Lower));
        assert!(all.is_consistent() && lost.is_consistent());
    }

    #[test]
    fn a_failed_observation_of_the_dut_is_the_error() {
        let mut attempts = Vec::new();
        let error = establish(
            "connect",
            &mut attempts,
            &mut (),
            |_| -> Result<()> { Err("ATT socket disconnected".into()) },
            |_| Err("serial link lost".into()),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "serial link lost");
    }
}
