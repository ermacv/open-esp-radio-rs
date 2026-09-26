use super::*;

#[test]
fn separate_calibration_branches_restore_before_completion_and_commit_only_their_reference() {
    for scope in [Scope::Common, Scope::Transmit] {
        let mut child = PhyCalibrationTrackingTransition::selected(
            PhyCalibrationTrackingRequest {
                clients: (PhyCalibrationTrackClass::Wifi).clients(),
            },
            PARAMETERS,
            scope,
        );
        let mut actions = Vec::new();
        loop {
            let action = child.action();
            if let PhyCalibrationTrackingAction::Complete(result) = action {
                assert_eq!(result.common_updated, scope == Scope::Common);
                assert_eq!(result.transmit_updated, scope == Scope::Transmit);
                // Restoration completes before grant protection is withdrawn.
                assert_eq!(
                    actions[actions.len() - 2..],
                    [
                        PhyCalibrationTrackingAction::RestoreTxGainCompensation,
                        PhyCalibrationTrackingAction::SetGrantProtect { enabled: false },
                    ]
                );
                break;
            }
            assert!(actions.len() < 32);
            actions.push(action);
            child.advance(completion(action)).unwrap();
        }
        assert_eq!(
            actions.contains(&PhyCalibrationTrackingAction::CalibrateDcode),
            scope == Scope::Common
        );
        assert_eq!(
            actions.iter().any(|action| matches!(
                action,
                PhyCalibrationTrackingAction::CalibrateTxDcPwdet { .. }
            )),
            scope == Scope::Transmit
        );
    }
}
