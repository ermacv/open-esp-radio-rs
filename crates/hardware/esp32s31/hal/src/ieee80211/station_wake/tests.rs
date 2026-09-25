use super::{
    StaModemWakePrepareError, StaModemWakeRestoreError, StaTbttWakePrepareError,
    StaTbttWakeRestoreError, StationWakeState,
};

#[test]
fn forgotten_modem_wakeup_token_permanently_quarantines_the_route() {
    let mut state = StationWakeState::default();
    assert_eq!(
        state.require_modem_wakeup(),
        Err(StaModemWakeRestoreError::NotConfigured)
    );
    state.acquire_modem_wakeup().unwrap();

    // Dropping the rollback token leaves the obligation configured.
    assert!(state.modem_wakeup_configured);
    assert_eq!(
        state.acquire_modem_wakeup(),
        Err(StaModemWakePrepareError::AlreadyConfigured)
    );
    assert_eq!(state.require_modem_wakeup(), Ok(()));
}

#[test]
fn station_tbtt_prefix_admits_one_outstanding_obligation() {
    let mut state = StationWakeState::default();
    assert_eq!(state.require_tbtt_idle(), Ok(()));
    assert_eq!(
        state.require_tbtt_prepared(),
        Err(StaTbttWakeRestoreError::NotPrepared)
    );
    state.tbtt_prepared = true;
    assert_eq!(
        state.require_tbtt_idle(),
        Err(StaTbttWakePrepareError::AlreadyPrepared)
    );
    assert_eq!(state.require_tbtt_prepared(), Ok(()));
}
