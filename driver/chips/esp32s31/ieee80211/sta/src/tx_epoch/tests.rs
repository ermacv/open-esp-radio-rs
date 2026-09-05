use super::*;

#[test]
fn epoch_never_overwrites_or_duplicates_the_phase_owner() {
    let config = ControlTxConfig {
        unicast_attempt_limit: 4,
        completion_timeout_us: 250_000,
        poll_interval_us: 1,
    };
    let mut epoch = Esp32s31StaTxEpoch::from_control(7_u8, config);
    assert_eq!(epoch.control(), Ok(&7));
    assert_eq!(epoch.take_control(), Ok(7));
    assert_eq!(
        epoch.take_control(),
        Err(Esp32s31StaTxEpochError::OwnerUnavailable)
    );
    assert_eq!(epoch.restore_control(9), Ok(()));
    assert_eq!(
        epoch.restore_control(11),
        Err((Esp32s31StaTxEpochError::OwnerAlreadyPresent, 11))
    );
    assert_eq!(epoch.config(), config);
}
