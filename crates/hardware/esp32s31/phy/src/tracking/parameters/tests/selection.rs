use super::*;
use crate::tracking::maintenance::Operation;

#[test]
fn independent_operation_keeps_completion_identity_and_excludes_other_children() {
    for operation in [
        Operation::Temperature,
        Operation::WifiPower,
        Operation::WifiI2c,
        Operation::CommonCalibration,
        Operation::WifiTxCalibration,
    ] {
        let mut parent = PhyParamTrackingTransition::selected(
            PhyParamTrackRequest::new(true, false),
            POLICY,
            operation,
        );
        parent
            .advance(PhyParamTrackingCompletion::EnteredCritical)
            .unwrap();
        let selected = parent.action();
        assert_eq!(
            parent.advance(PhyParamTrackingCompletion::ExitedCritical),
            Err(PhyParamTrackingTransitionError::WrongCompletion)
        );
        assert_eq!(parent.action(), selected);
        parent.advance(completion(selected)).unwrap();
        assert_eq!(parent.action(), PhyParamTrackingAction::ExitCritical);
        parent
            .advance(PhyParamTrackingCompletion::ExitedCritical)
            .unwrap();
        assert!(matches!(
            parent.action(),
            PhyParamTrackingAction::Complete(_)
        ));
    }
}
