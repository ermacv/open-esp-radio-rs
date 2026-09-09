use super::*;

#[test]
fn shutdown_retires_a_failed_publication_and_does_not_hide_cleanup_failure() {
    let mut resources = CoexResources::<NoopRawMutex, 2>::new();
    let (mut control, owner) = resources.split();
    let mut core = CoexCore::new(CoexPtiTable::reviewed_vendor());
    let mut hardware = Hardware {
        fail_enable_once: true,
        fail_disable_once: true,
        ..Hardware::default()
    };
    let mut clock = Clock(CoexTimerClock::from_hardware_fields(
        CoexClockSelector::Selector8,
        0,
        40,
        true,
    ));
    let request = CoexClientRequest {
        event: CoexEventId::new(1).unwrap(),
        latency: 1_000,
        duration: 2_000,
    };
    let (_, result) = block_on(join(
        async {
            control.execute(CoexCommand::Enable).await.unwrap();
            assert_eq!(
                control.execute(CoexCommand::WifiRequest(request)).await,
                Err(CoexControlError::Operation(CoexError::Hardware))
            );
            assert_eq!(
                control
                    .execute(CoexCommand::BluetoothRequest(request))
                    .await,
                Err(CoexControlError::Operation(CoexError::RecoveryRequired))
            );
            assert_eq!(
                control.execute(CoexCommand::Shutdown).await,
                Err(CoexControlError::Operation(CoexError::Hardware))
            );
            let CoexOutcome::Status(status) = control.execute(CoexCommand::Status).await.unwrap()
            else {
                panic!("status response expected");
            };
            assert!(status.enabled, "failed shutdown must keep the owner alive");
            assert_ne!(status.uncertain_timers, 0);
            assert_eq!(
                control.execute(CoexCommand::Shutdown).await,
                Ok(CoexOutcome::Stopped)
            );
        },
        owner.run(&mut core, &mut hardware, &mut clock),
    ));
    assert_eq!(result, Ok(()));
    assert_eq!(
        hardware.enabled, 0,
        "failed publication must not leave its timer enabled"
    );
    assert!(!core.status().enabled);
    assert_eq!(core.status().uncertain_timers, 0);
}
