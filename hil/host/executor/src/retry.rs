//! Retries of a protocol operation whose loss the protocol itself admits.
//!
//! A retry exists only where the protocol admits loss, is counted and is
//! reported (#761 invariant). A type implementing [`AdmitsLoss`] names
//! that loss and its attempt limit with the protocol basis; every attempt
//! and every loss becomes a count in the run's [`Recorder`], so the
//! requirement can bound the loss rate. A flaky result is a finding, not a
//! reason for another retry.

use core::{future::Future, marker::PhantomData, num::NonZeroU32};

use crate::Recorder;

/// A protocol operation that may be lost on the air without a fault of
/// the device under test.
pub trait AdmitsLoss {
    /// The counts' prefix: `<NAME>.attempts`, `<NAME>.lost`.
    const NAME: &'static str;
    /// Attempts per operation, the first included.
    const ATTEMPTS: NonZeroU32;
    /// Why the protocol admits the loss and why this many attempts, for
    /// example: "a CONNECT_IND may collide with another device's response
    /// to the same ADV_IND; a central retries it".
    const BASIS: &'static str;
}

/// What one attempt did.
#[derive(Debug)]
pub enum Attempt<T, E> {
    /// The operation succeeded.
    Done(T),
    /// The protocol lost the operation, as the device under test's own
    /// observation proves; it is retried.
    Lost(String),
    /// The operation failed in a way the protocol does not excuse; it is
    /// not retried.
    Failed(E),
}

/// Why a retried operation ended without success.
#[derive(Debug, Eq, PartialEq)]
pub enum RetryEnd<E> {
    /// Every attempt was lost; the last loss's cause.
    Exhausted {
        attempts: u32,
        last: String,
    },
    Failed(E),
}

/// Runs operations of the protocol loss `P`.
pub struct ProtocolRetry<P>(PhantomData<P>);

impl<P: AdmitsLoss> ProtocolRetry<P> {
    /// Runs `attempt` (given the attempt number, from one) until it is
    /// done, fails or `P::ATTEMPTS` are lost, counting attempts and losses.
    pub async fn run<T, E, F, Fut>(
        recorder: &mut Recorder,
        mut attempt: F,
    ) -> Result<T, RetryEnd<E>>
    where
        F: FnMut(u32) -> Fut,
        Fut: Future<Output = Attempt<T, E>>,
    {
        let attempts = format!("{}.attempts", P::NAME);
        let lost = format!("{}.lost", P::NAME);
        // Both counts exist even when nothing is lost, so a bound over
        // them has its denominator.
        recorder.count(&lost, 0);
        let mut last = String::new();
        for number in 1..=P::ATTEMPTS.get() {
            recorder.count(&attempts, 1);
            match attempt(number).await {
                Attempt::Done(value) => return Ok(value),
                Attempt::Failed(error) => return Err(RetryEnd::Failed(error)),
                Attempt::Lost(cause) => {
                    recorder.count(&lost, 1);
                    last = cause;
                }
            }
        }
        Err(RetryEnd::Exhausted {
            attempts: P::ATTEMPTS.get(),
            last,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ConnectInd;

    impl AdmitsLoss for ConnectInd {
        const NAME: &'static str = "connect";
        const ATTEMPTS: NonZeroU32 = NonZeroU32::new(3).unwrap();
        const BASIS: &'static str = "a CONNECT_IND may collide with another device's response";
    }

    fn counts(recorder: &Recorder) -> (u64, u64) {
        let measurements = recorder.measurements();
        let value = |name: &str| {
            measurements
                .iter()
                .find(|m| m.name == name)
                .map(|m| m.value)
                .unwrap()
        };
        (value("connect.attempts"), value("connect.lost"))
    }

    #[test]
    fn a_lost_attempt_is_retried_and_counted() {
        let mut recorder = Recorder::default();
        let result = embassy_futures::block_on(ProtocolRetry::<ConnectInd>::run(
            &mut recorder,
            |n| async move {
                if n < 2 {
                    Attempt::<u32, ()>::Lost("the DUT saw no connection".into())
                } else {
                    Attempt::Done(n)
                }
            },
        ));
        assert_eq!(result, Ok(2));
        assert_eq!(counts(&recorder), (2, 1));
    }

    #[test]
    fn a_failure_the_protocol_does_not_excuse_is_not_retried() {
        let mut recorder = Recorder::default();
        let result =
            embassy_futures::block_on(ProtocolRetry::<ConnectInd>::run(&mut recorder, |_| async {
                Attempt::<(), _>::Failed("the DUT saw the connection and dropped it")
            }));
        assert_eq!(
            result,
            Err(RetryEnd::Failed(
                "the DUT saw the connection and dropped it"
            ))
        );
        assert_eq!(counts(&recorder), (1, 0));
    }

    #[test]
    fn attempts_stop_at_the_protocol_limit() {
        let mut recorder = Recorder::default();
        let result = embassy_futures::block_on(ProtocolRetry::<ConnectInd>::run(
            &mut recorder,
            |n| async move { Attempt::<(), ()>::Lost(format!("lost {n}")) },
        ));
        assert_eq!(
            result,
            Err(RetryEnd::Exhausted {
                attempts: 3,
                last: "lost 3".into()
            })
        );
        assert_eq!(counts(&recorder), (3, 3));
    }
}
