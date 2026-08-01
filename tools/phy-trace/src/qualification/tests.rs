use super::*;

fn execution_result_with_timeline(
    timeline: Vec<execution::ExecutionTimelineEvent>,
) -> execution::ExecutionResult {
    execution::ExecutionResult {
        events: Vec::new(),
        timeline,
        return_value: 0,
        steps: 0,
        branches: std::collections::BTreeSet::new(),
        ordered_branches: Vec::new(),
        calls: std::collections::BTreeSet::new(),
        ordered_calls: Vec::new(),
        indirect_calls: std::collections::BTreeSet::new(),
        memory_changes: Vec::new(),
        initial_memory: std::collections::BTreeMap::new(),
        persistent_memory: std::collections::BTreeMap::new(),
    }
}

#[test]
fn rust_channel_model_exposes_complete_action_order() {
    let events = rust_channel_events(11, 0).unwrap();
    assert_eq!(events.first(), Some(&ChannelEvent::SetAgc(false)));
    assert!(events.contains(&ChannelEvent::FrequencyReady { samples: 0 }));
    assert_eq!(
        events.last(),
        Some(&ChannelEvent::Complete {
            channel: 11,
            frequency_mhz: 2_462,
            cbw: 0,
            init_complete: false,
        })
    );
}

#[test]
fn rust_rf_init_model_preserves_typed_state_across_a_second_run() {
    let (first, state) = rust_rf_init_events(PhyColdState::new()).unwrap();
    let (second, _) = rust_rf_init_events(state).unwrap();

    assert_eq!(
        first.first(),
        Some(&rf_phase(
            RfInitPhase::ConfigureFeBbClock,
            RfInitPhaseParameters::None,
        ))
    );
    assert_eq!(first.last(), second.last());
    assert!(matches!(first.last(), Some(RfInitEvent::Complete(_))));
    assert!(first.contains(&rf_phase(
        RfInitPhase::InitializeRcCalibration,
        RfInitPhaseParameters::RcCalibrationPrestate {
            already_complete: false,
        },
    )));
    assert!(second.contains(&rf_phase(
        RfInitPhase::InitializeRcCalibration,
        RfInitPhaseParameters::RcCalibrationPrestate {
            already_complete: true,
        },
    )));
    assert!(first.contains(&rf_phase(
        RfInitPhase::ConfigureBbpllCalibration,
        RfInitPhaseParameters::Enabled(true),
    )));
    assert!(first.contains(&rf_phase(
        RfInitPhase::PostOpenI2cDelay,
        RfInitPhaseParameters::SymbolicValue(10),
    )));
    assert!(first.contains(&rf_phase(
        RfInitPhase::ConfigureI2cClockSelection,
        RfInitPhaseParameters::SymbolicValue(8),
    )));
}

#[test]
fn state_footprints_reject_unknown_offsets_and_access_directions() {
    let state_base = 0x1000;
    let unknown =
        execution_result_with_timeline(vec![execution::ExecutionTimelineEvent::RamRead {
            width: 8,
            address: state_base + 0x123,
            value: 0,
        }]);
    let error = vendor_rf_init_state_footprint(&unknown, state_base).unwrap_err();
    assert!(error.to_string().contains("reads=[0x123]"));

    let wrong_direction =
        execution_result_with_timeline(vec![execution::ExecutionTimelineEvent::RamWrite {
            width: 8,
            address: state_base + 0x007,
            value: 0,
        }]);
    let error = vendor_channel_state_footprint(&wrong_direction, state_base).unwrap_err();
    assert!(error.to_string().contains("writes=[0x007]"));
}

#[test]
fn vendor_rf_phase_rejects_mutated_direct_call_arguments() {
    let call = execution::OrderedCall {
        site: 0x1000,
        symbol: "ets_delay_us".to_owned(),
        arguments: [11, 0, 0, 0, 0, 0, 0, 0],
    };
    let error = vendor_rf_init_phase(&call).unwrap_err();
    assert!(error.to_string().contains("expected 0xa"));
}
