use super::*;

#[test]
fn a_link_failure_is_retried_and_an_image_failure_is_not() {
    let mut calls = 0;
    let mut resets = Vec::new();
    let written = retry_transient(
        ATTEMPTS,
        &mut || {
            calls += 1;
            if calls == 1 {
                Err("espflash: Protocol error (os error 71)".into())
            } else {
                Ok(calls)
            }
        },
        |attempt, _| resets.push(attempt),
    );
    assert_eq!(written.unwrap(), 2);
    assert_eq!(resets, [1]);

    let mut calls = 0;
    let refused = retry_transient(
        ATTEMPTS,
        &mut || -> crate::Result<()> {
            calls += 1;
            Err("the HIL application overlaps the next flash region".into())
        },
        |_, _| {},
    );
    assert!(refused.is_err());
    assert_eq!(calls, 1);
}

#[test]
fn a_link_that_keeps_failing_ends_after_the_last_attempt() {
    let mut calls = 0;
    let mut resets = 0;
    let result = retry_transient(
        ATTEMPTS,
        &mut || -> crate::Result<()> {
            calls += 1;
            Err("serial port timed out".into())
        },
        |_, _| resets += 1,
    );
    assert!(result.is_err());
    assert_eq!(calls, ATTEMPTS);
    assert_eq!(resets, ATTEMPTS - 1);
}

#[test]
fn a_start_by_power_leaves_the_chip_in_its_rom() {
    assert_eq!(After::of(oer_chip_profile::Start::Reset), After::HardReset);
    assert_eq!(
        After::of(oer_chip_profile::Start::PowerOn),
        After::StayInBootloader
    );
}
