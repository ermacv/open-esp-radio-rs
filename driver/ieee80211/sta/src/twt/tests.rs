use super::*;
use open_esp_radio_ieee80211::twt::IndividualTwtFlowType;

const CONFIG: IndividualTwtRequesterConfig =
    match IndividualTwtRequesterConfig::new(1_000, 100, 2, 2) {
        Ok(config) => config,
        Err(_) => panic!("valid requester config"),
    };

fn parameters(implicit: bool) -> IndividualTwtParameterSet {
    IndividualTwtParameterSet {
        requesting_sta: true,
        setup_command: IndividualTwtSetupCommand::Request,
        trigger: false,
        implicit,
        flow_type: IndividualTwtFlowType::Announced,
        flow_id: IndividualTwtFlowId::new(2).unwrap(),
        wake_interval_exponent: 0,
        protection: false,
        target_wake_time_tsf: 10_000,
        nominal_minimum_wake_duration: 1,
        wake_interval_mantissa: 1_024,
        twt_channel: 0,
    }
}

fn proposal(implicit: bool) -> IndividualTwtProposal {
    IndividualTwtProposal {
        control: IndividualTwtControl::REQUEST,
        parameters: parameters(implicit),
    }
}

#[test]
fn generation_exhaustion_never_reissues_a_stale_identity() {
    let mut requester = IndividualTwtRequester::new(CONFIG);
    requester.generation = u32::MAX;
    requester.queue_setup(proposal(true), 0).unwrap();

    assert_eq!(
        requester.service(0),
        Err(IndividualTwtRequesterError::GenerationExhausted)
    );
    assert_eq!(
        requester.status(IndividualTwtFlowId::new(2).unwrap()),
        IndividualTwtFlowStatus::SetupQueued
    );
    assert_eq!(requester.next_dialog_token, 1);

    requester.reset_for_reconnect();
    assert_eq!(requester.generation, u32::MAX);
}

#[test]
fn explicit_proposal_reports_the_exact_information_frontier() {
    assert_eq!(
        proposal(false).validate(),
        Err(
            IndividualTwtRequesterError::ExplicitTwtInformationUnsupported(
                IndividualTwtInformationFrontier {
                    flow_id: IndividualTwtFlowId::new(2).unwrap(),
                    initial_target_wake_time_tsf: 10_000,
                    information_frames_disabled: false,
                }
            )
        )
    );
}

#[test]
fn peer_accepted_explicit_agreement_is_torn_down_not_installed() {
    let mut requester = IndividualTwtRequester::new(CONFIG);
    requester.queue_setup(proposal(true), 0).unwrap();
    let IndividualTwtService::Transmit(transmission) = requester.service(0).unwrap() else {
        panic!("setup must be ready");
    };
    requester
        .complete_transmission(transmission, true, 10)
        .unwrap();

    let mut response_parameters = parameters(false);
    response_parameters.requesting_sta = false;
    response_parameters.setup_command = IndividualTwtSetupCommand::Accept;
    let disposition = requester
        .on_setup_response(IndividualTwtSetup {
            dialog_token: 1,
            control: IndividualTwtControl::REQUEST,
            parameters: response_parameters,
        })
        .unwrap();
    assert_eq!(
        disposition,
        IndividualTwtSetupDisposition::ExplicitInformationUnsupported {
            flow_id: IndividualTwtFlowId::new(2).unwrap(),
            frontier: IndividualTwtInformationFrontier {
                flow_id: IndividualTwtFlowId::new(2).unwrap(),
                initial_target_wake_time_tsf: 10_000,
                information_frames_disabled: false,
            },
        }
    );
    assert_eq!(requester.next_deadline_micros(), Some(0));
    let IndividualTwtService::Transmit(teardown) = requester.service(10).unwrap() else {
        panic!("rollback teardown must be ready immediately");
    };
    assert_eq!(teardown.kind, IndividualTwtTxKind::Teardown);
}

#[test]
fn wake_plan_rejects_a_window_crossing_the_comparable_tsf_half() {
    let agreement = IndividualTwtAgreement {
        flow_id: IndividualTwtFlowId::new(0).unwrap(),
        control: IndividualTwtControl::REQUEST,
        trigger: false,
        implicit: true,
        flow_type: IndividualTwtFlowType::Announced,
        protection: false,
        target_wake_time_tsf: i64::MAX as u64,
        wake_interval_micros: i64::MAX as u64,
        wake_duration_micros: 256,
    };
    assert_eq!(
        plan_agreement_wake(agreement, 0, 10),
        Err(IndividualTwtWakePlanError::AmbiguousTsfDistance)
    );
}

#[test]
fn wake_plan_preserves_a_future_window_across_tsf_wrap() {
    let agreement = IndividualTwtAgreement {
        flow_id: IndividualTwtFlowId::new(0).unwrap(),
        control: IndividualTwtControl::REQUEST,
        trigger: false,
        implicit: true,
        flow_type: IndividualTwtFlowType::Announced,
        protection: false,
        target_wake_time_tsf: 50,
        wake_interval_micros: 1_000,
        wake_duration_micros: 256,
    };
    assert_eq!(
        plan_agreement_wake(agreement, u64::MAX - 100, 10),
        Ok(IndividualTwtWakePlan {
            flow_bitmap: 1,
            wake_tsf: 40,
            service_start_tsf: 50,
            service_end_tsf: 306,
            service_open: false,
        })
    );
}
