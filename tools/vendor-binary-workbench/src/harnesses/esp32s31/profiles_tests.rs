use super::*;

#[test]
fn checked_in_profile_parses() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench remains under tools");
    let path = root.join("verification/vendor/targets/esp32s31/profiles/compiled-equivalence.toml");
    let profiles = load(&path).unwrap();
    assert_eq!(profiles.len(), 41);
    assert!(profiles.iter().all(|profile| !profile.scenarios.is_empty()));
    assert_eq!(
        profiles
            .iter()
            .filter(|profile| profile.contract == ProfileContract::State)
            .count(),
        7
    );
    assert_eq!(
        profiles
            .iter()
            .find(|profile| profile.name == "rom-nrx-frequency")
            .unwrap()
            .scenarios
            .len(),
        4
    );
}

#[test]
fn libpp_tx_dma_profiles_cover_all_four_queue_selectors() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench remains under tools");
    let path = root.join("verification/vendor/targets/esp32s31/profiles/libpp-tx-dma.toml");
    let profiles = load(&path).unwrap();

    assert_eq!(profiles.len(), 6);
    assert!(profiles.iter().all(|profile| {
        let range = profile.argument_ranges[0];
        profile.vendor_source == "libpp"
            && profile.argument_ranges.len() == 1
            && range.min == 0
            && range.max == 3
            && profile.coverage_argument_constraints()
                == (0..=3)
                    .map(|queue| {
                        let mut arguments = [None; 8];
                        arguments[range.index] = Some(queue);
                        arguments
                    })
                    .collect::<Vec<_>>()
            && profile.scenarios.len() == 4
            && profile
                .scenarios
                .iter()
                .enumerate()
                .all(|(queue, scenario)| scenario.scenario.arguments[range.index] == queue as u32)
    }));
}

#[test]
fn libpp_sta_tsf_wakeup_profile_closes_the_bool_domain() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench remains under tools");
    let path = root.join("verification/vendor/targets/esp32s31/profiles/libpp-sta-tsf-wakeup.toml");
    let profiles = load(&path).unwrap();

    assert_eq!(profiles.len(), 1);
    let profile = &profiles[0];
    assert_eq!(profile.vendor_symbol, "hal_set_sta_tsf_wakeup");
    assert!(!profile.compare_return);
    assert_eq!(
        profile.argument_ranges,
        [ArgumentRange {
            index: 0,
            min: 0,
            max: 1,
        }]
    );
    assert_eq!(profile.scenarios.len(), 2);
    assert_eq!(profile.scenarios[0].scenario.arguments, [0]);
    assert_eq!(profile.scenarios[1].scenario.arguments, [1]);
}

#[test]
fn coex_timer_profiles_close_the_five_entry_index_domain() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench remains under tools");
    let path = root.join("verification/vendor/targets/esp32s31/profiles/coex-timer.toml");
    let profiles = load(&path).unwrap();

    assert_eq!(profiles.len(), 4);
    assert!(profiles.iter().all(|profile| {
        profile.vendor_source == "coex"
            && profile.argument_ranges
                == [ArgumentRange {
                    index: 0,
                    min: 0,
                    max: 4,
                }]
            && profile.scenarios.len() == 5
            && profile
                .scenarios
                .iter()
                .enumerate()
                .all(|(index, scenario)| scenario.scenario.arguments == [index as u32])
    }));
}

#[test]
fn coex_timer_set_covers_real_and_non_chip_clock_paths() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench remains under tools");
    let path = root.join("verification/vendor/targets/esp32s31/profiles/coex-timer-set.toml");
    let profiles = load(&path).unwrap();

    assert_eq!(profiles.len(), 1);
    let profile = &profiles[0];
    assert_eq!(profile.vendor_symbol, "coex_hw_timer_set");
    assert_eq!(profile.scenarios.len(), 5);
    let selector_eight = profile
        .scenarios
        .iter()
        .filter(|scenario| scenario.scenario.mmio_initial.get(&0x2010_f008) == Some(&0x0000_0008))
        .collect::<Vec<_>>();
    assert_eq!(selector_eight.len(), 2);
    assert_eq!(
        selector_eight
            .iter()
            .map(|scenario| scenario.scenario.arguments[5])
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from([0, 1])
    );
}

#[test]
fn rom_sta_tsf_snapshot_profile_closes_both_pointer_branches() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench remains under tools");
    let path = root.join("verification/vendor/targets/esp32s31/profiles/rom-sta-tsf-snapshot.toml");
    let profiles = load(&path).unwrap();

    assert_eq!(profiles.len(), 1);
    let profile = &profiles[0];
    assert_eq!(profile.vendor_source, "rom");
    assert_eq!(profile.vendor_symbol, "hal_get_sta_tsf");
    assert_eq!(profile.scenarios.len(), 4);
    assert_eq!(profile.scenarios[0].scenario.arguments, [0, 0]);
    assert_eq!(
        profile.scenarios[3].scenario.arguments,
        [0x3fff_0000, 0x3fff_0004]
    );
    assert!(
        profile.scenarios[3]
            .scenario
            .observed_memory
            .iter()
            .any(|range| range.start == 0x3fff_0000 && range.length == 8)
    );
}

#[test]
fn declared_argument_domain_requires_an_executed_case_for_every_value() {
    let ranges = [ArgumentRange {
        index: 0,
        min: 0,
        max: 3,
    }];
    let scenarios = (0..3)
        .map(|queue| {
            let mut scenario = NamedScenario::new(format!("queue-{queue}"));
            scenario.scenario.arguments.push(queue);
            scenario
        })
        .collect::<Vec<_>>();

    let error = validate_argument_domain("incomplete", &ranges, &[], &scenarios)
        .unwrap_err()
        .to_string();
    assert!(error.contains("a0=0x3"), "{error}");
}

#[test]
fn sparse_argument_domain_does_not_admit_intermediate_selectors() {
    let profiles = parse(
        "schema = 3\n\n[[profiles]]\nname = \"sparse\"\nvendor-source = \"vendor\"\nvendor-symbol = \"dispatch\"\nrust-symbol = \"replacement\"\nclaim = \"whole-function-equivalence\"\n\n[[profiles.argument-values]]\nindex = 0\nvalues = [6, 8]\n\n[[profiles.cases]]\nname = \"six\"\narguments = [6]\n\n[[profiles.cases]]\nname = \"eight\"\narguments = [8]\n",
    )
    .unwrap();

    assert_eq!(
        profiles[0].argument_values,
        [ArgumentValues {
            index: 0,
            values: vec![6, 8],
        }]
    );
    assert_eq!(
        profiles[0]
            .coverage_argument_constraints()
            .into_iter()
            .map(|arguments| arguments[0].unwrap())
            .collect::<Vec<_>>(),
        [6, 8]
    );
}

#[test]
fn reviewed_domain_claim_requires_a_named_finite_precondition() {
    let missing_precondition = parse(
        "schema = 3\n\n[[profiles]]\nname = \"bounded\"\nvendor-source = \"vendor\"\nvendor-symbol = \"dispatch\"\nrust-symbol = \"replacement\"\nclaim = \"reviewed-domain-equivalence\"\n\n[[profiles.argument-values]]\nindex = 0\nvalues = [1]\n\n[[profiles.cases]]\nname = \"one\"\narguments = [1]\n",
    )
    .unwrap_err()
    .to_string();
    assert!(missing_precondition.contains("requires a non-empty precondition"));

    let missing_domain = parse(
        "schema = 3\n\n[[profiles]]\nname = \"bounded\"\nvendor-source = \"vendor\"\nvendor-symbol = \"dispatch\"\nrust-symbol = \"replacement\"\nclaim = \"reviewed-domain-equivalence\"\nprecondition = \"valid-input\"\n\n[[profiles.cases]]\nname = \"one\"\narguments = [1]\n",
    )
    .unwrap_err()
    .to_string();
    assert!(missing_domain.contains("must declare a finite argument or MMIO domain"));
}

#[test]
fn whole_function_claim_rejects_a_precondition() {
    let error = parse(
        "schema = 3\n\n[[profiles]]\nname = \"whole\"\nvendor-source = \"vendor\"\nvendor-symbol = \"entry\"\nrust-symbol = \"replacement\"\nclaim = \"whole-function-equivalence\"\nprecondition = \"selected-input\"\n\n[[profiles.cases]]\nname = \"one\"\n",
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("cannot declare a precondition"));
}

#[test]
fn checked_in_sta_ap_receive_profile_closes_only_policy_six_and_eight() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench remains under tools");
    let profiles =
        load(&root.join("verification/vendor/targets/esp32s31/profiles/wifi-sta-ap-receive.toml"))
            .unwrap();

    assert_eq!(profiles.len(), 1);
    let profile = &profiles[0];
    assert_eq!(profile.vendor_symbol, "wifi_set_rx_policy");
    assert_eq!(
        profile.argument_values,
        [ArgumentValues {
            index: 0,
            values: vec![6, 8],
        }]
    );
    assert_eq!(profile.scenarios.len(), 4);
    assert_eq!(profile.coverage_argument_constraints().len(), 4);
    assert!(
        profile
            .coverage_argument_constraints()
            .iter()
            .all(|arguments| {
                matches!(arguments[0], Some(6 | 8)) && matches!(arguments[3], Some(1 | 2))
            })
    );
}

#[test]
fn malformed_profile_retains_its_physical_source_line() {
    let input = "schema = 3\n\n[[profiles]]\nname = \"fixture\"\nvendor-source = \"fixture\"\nvendor-symbol = \"vendor\"\nrust-symbol = \"rust\"\nclaim = \"whole-function-equivalence\"\nunknown = \"value\"\n";
    let path = std::env::temp_dir().join(format!(
        "vendor-workbench-profile-diagnostic-{}.toml",
        std::process::id()
    ));
    std::fs::write(&path, input).unwrap();
    let error = load(&path).unwrap_err();
    std::fs::remove_file(&path).unwrap();

    assert!(matches!(
        error,
        crate::error::WorkbenchError::ManifestSource {
            path: reported,
            span,
            ..
        } if reported == path && span.offset() == input.find("unknown").unwrap()
    ));
}

#[test]
fn profile_models_runtime_tables_as_layout_instances() {
    let profiles = parse(
        "schema = 3\n\n[[profiles]]\nname = \"callback-table\"\nvendor-source = \"rom\"\nvendor-symbol = \"vendor_entry\"\nrust-symbol = \"rust_entry\"\nclaim = \"whole-function-equivalence\"\n\n[[profiles.cases]]\nname = \"installed\"\n\n[[profiles.cases.vendor-tables]]\nlayout-id = \"reviewed-services-v1\"\nbase-address = 0x4000\nlayout-size = 0x20\npointer-cells = [0x3000]\nslots = [{ offset = 0x4, target = { kind = \"symbol\", value = \"vendor_callback\" } }]\n\n[[profiles.cases.rust-tables]]\nlayout-id = \"reviewed-services-v1\"\nbase-address = 0x5000\nlayout-size = 0x20\npointer-cells = []\nslots = [{ offset = 0x4, target = { kind = \"symbol\", value = \"rust_callback\" } }]\n",
    )
    .unwrap();

    let scenario = &profiles[0].scenarios[0];
    assert_eq!(scenario.vendor_table_instances.len(), 1);
    assert_eq!(
        scenario.vendor_table_instances[0].layout_id,
        "reviewed-services-v1"
    );
    assert_eq!(scenario.vendor_table_instances[0].pointer_cells, [0x3000]);
    assert_eq!(scenario.vendor_table_instances[0].slots[0].offset, 4);
    assert!(matches!(
        &scenario.vendor_table_instances[0].slots[0].target,
        crate::execution_model::TableSlotTarget::Symbol(symbol) if symbol == "vendor_callback"
    ));
    assert_eq!(scenario.rust_table_instances[0].base_address, 0x5000);

    let error = parse(
        "schema = 3\n\n[[profiles]]\nname = \"callback-table\"\nvendor-source = \"rom\"\nvendor-symbol = \"vendor_entry\"\nrust-symbol = \"rust_entry\"\nclaim = \"whole-function-equivalence\"\n\n[[profiles.cases]]\nname = \"missing-layout\"\n\n[[profiles.cases.vendor-tables]]\nlayout-id = \"missing\"\n",
    )
    .unwrap_err();
    assert!(error.to_string().contains("missing field"));
}

#[test]
fn profile_models_vendor_and_rust_call_responses_independently() {
    let profiles = parse(
        "schema = 3\n\n[[profiles]]\nname = \"external-call\"\nvendor-source = \"rom\"\nvendor-symbol = \"vendor_entry\"\nrust-symbol = \"rust_entry\"\nclaim = \"whole-function-equivalence\"\n\n[[profiles.cases]]\nname = \"ready\"\n\n[[profiles.cases.vendor-calls]]\nsymbol = \"queue_send_from_isr\"\nreturn-words = [1, 2]\noutputs = [{ kind = \"private-stack\", pointer-argument = 2, width = 8, value = 90 }]\n\n[[profiles.cases.rust-calls]]\nsymbol = \"wake_task\"\nreturn-words = [7]\n",
    )
    .unwrap();

    let scenario = &profiles[0].scenarios[0];
    assert_eq!(scenario.vendor_call_responses.len(), 1);
    assert_eq!(scenario.vendor_call_responses[0].0, "queue_send_from_isr");
    assert_eq!(
        scenario.vendor_call_responses[0].1.return_words,
        [Some(1), Some(2)]
    );
    assert_eq!(
        scenario.vendor_call_responses[0].1.outputs,
        [crate::execution::ModeledCallOutput::PrivateStack {
            pointer_argument: 2,
            width: 8,
            value: 90,
        }]
    );
    assert_eq!(scenario.rust_call_responses[0].0, "wake_task");
    assert_eq!(
        scenario.rust_call_responses[0].1.return_words,
        [Some(7), None]
    );
}

#[test]
fn profile_models_stateful_fifo_services_without_rtos_vocabulary() {
    let profiles = parse(
        r#"schema = 3

[[profiles]]
name = "event-delivery"
vendor-source = "libpp"
vendor-symbol = "ppTask"
rust-symbol = "rust_event_task"
claim = "whole-function-equivalence"

[[profiles.cases]]
name = "selector-25"
vendor-goal = { kind = "reach-symbol", symbol = "wdevProcessRxSucDataAll" }
rust-goal = { kind = "observe-fifo-dequeue", service-id = "pp-events", value = 25 }

[[profiles.cases.vendor-fifo-services]]
id = "pp-events"
handle = 8192
item-width = 32
capacity = 8
items = [25]

[[profiles.cases.vendor-fifo-bindings]]
symbol = "queue_recv"
service-id = "pp-events"
handle-argument = 0
operation = { kind = "dequeue", output = { kind = "private-stack-pointer", pointer-argument = 1, width = 32 }, success-return = 1, empty-return = 0 }

[[profiles.cases.rust-fifo-services]]
id = "pp-events"
handle = 12288
item-width = 32
capacity = 8
items = [25]

[[profiles.cases.rust-fifo-bindings]]
symbol = "receive_event"
service-id = "pp-events"
handle-argument = 0
operation = { kind = "dequeue", output = { kind = "private-stack-pointer", pointer-argument = 1, width = 32 }, success-return = 1, empty-return = 0 }
"#,
    )
    .unwrap();

    let scenario = &profiles[0].scenarios[0];
    assert_eq!(scenario.vendor_fifo_services[0].items, [25]);
    assert_eq!(scenario.rust_fifo_services[0].handle, 12288);
    assert_eq!(scenario.vendor_fifo_bindings[0].service_id, "pp-events");
    assert!(matches!(
        &scenario.vendor_goal,
        crate::execution_model::ExecutionGoal::ReachSymbol { symbol }
            if symbol == "wdevProcessRxSucDataAll"
    ));
    assert!(matches!(
        &scenario.rust_goal,
        crate::execution_model::ExecutionGoal::ObserveFifoDequeue {
            service_id,
            value: Some(25),
        } if service_id == "pp-events"
    ));
    assert!(matches!(
        scenario.vendor_fifo_bindings[0].operation,
        crate::execution_model::FifoServiceOperation::Dequeue {
            success_return: 1,
            empty_return: 0,
            ..
        }
    ));
}

#[test]
fn profile_models_zeroed_allocator_response_without_scalar_return() {
    let profiles = parse(
        "schema = 3\n\n[[profiles]]\nname = \"allocator\"\nvendor-source = \"libpp\"\nvendor-symbol = \"vendor_entry\"\nrust-symbol = \"rust_entry\"\nclaim = \"whole-function-equivalence\"\n\n[[profiles.cases]]\nname = \"allocated\"\n\n[[profiles.cases.vendor-calls]]\nsymbol = \"wifi_zalloc\"\nallocation = { address = 0x3ffe0000, size-argument = 0, capacity = 0x98 }\n",
    )
    .unwrap();

    let response = &profiles[0].scenarios[0].vendor_call_responses[0].1;
    assert_eq!(response.return_words, [None, None]);
    assert_eq!(
        response.allocation,
        Some(crate::execution::ModeledAllocation {
            address: 0x3ffe_0000,
            size_argument: 0,
            capacity: 0x98,
        })
    );

    let error = parse(
        "schema = 3\n\n[[profiles]]\nname = \"allocator\"\nvendor-source = \"libpp\"\nvendor-symbol = \"vendor_entry\"\nrust-symbol = \"rust_entry\"\nclaim = \"whole-function-equivalence\"\n\n[[profiles.cases]]\nname = \"invalid\"\n\n[[profiles.cases.vendor-calls]]\nsymbol = \"wifi_zalloc\"\nreturn-words = [0x3ffe0000]\nallocation = { address = 0x3ffe0000, size-argument = 0, capacity = 0x98 }\n",
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("cannot also declare return words")
    );
}

#[test]
fn profile_keeps_runtime_memory_identity_separate_from_logical_types() {
    let profiles = parse(
        "schema = 3\n\n[[profiles]]\nname = \"memory-alias\"\nvendor-source = \"rom\"\nvendor-symbol = \"vendor_entry\"\nrust-symbol = \"rust_entry\"\nclaim = \"whole-function-equivalence\"\n\n[[profiles.cases]]\nname = \"shared-state\"\narguments = [0x3fff0000]\n\n[[profiles.cases.vendor-memory-instances]]\nid = \"state-0\"\nbase-address = 0x3fff0000\nlength = 0x40\nbindings = [{ kind = \"argument\", index = 0 }, { kind = \"dereferenced-global\", symbol = \"g_state\", pointer_offset = 0x4 }]\n\n[[profiles.cases.rust-memory-instances]]\nid = \"state-0\"\nbase-address = 0x3fff0000\nlength = 0x40\nbindings = [{ kind = \"argument\", index = 0 }, { kind = \"absolute\", address_space = \"dram\", address = 0x3fff0000 }]\n",
    )
    .unwrap();

    let scenario = &profiles[0].scenarios[0];
    let vendor = &scenario.vendor_memory_instances[0];
    assert_eq!(vendor.id, "state-0");
    assert_eq!(vendor.base_address, 0x3fff_0000);
    assert_eq!(vendor.length, 0x40);
    assert!(matches!(
        &vendor.bindings[1],
        crate::RuntimeMemoryObjectBinding::DereferencedGlobal {
            symbol,
            pointer_offset: 4,
        } if symbol == "g_state"
    ));
    assert!(matches!(
        &scenario.rust_memory_instances[0].bindings[1],
        crate::RuntimeMemoryObjectBinding::Absolute {
            address_space,
            address: 0x3fff_0000,
        } if address_space == "dram"
    ));
}

#[test]
fn profile_models_reviewed_register_behavior_as_a_device_factory() {
    let profiles = parse(
        "schema = 3\n\n[[profiles]]\nname = \"device\"\nvendor-source = \"rom\"\nvendor-symbol = \"vendor_entry\"\nrust-symbol = \"rust_entry\"\nclaim = \"whole-function-equivalence\"\n\n[[profiles.cases]]\nname = \"irq\"\n\n[[profiles.cases.device-models]]\nkind = \"w1c\"\nid = \"irq-status\"\naddress = 0x60008020\nwidth = 32\ninitial_value = 0xf\nclear_mask = 0x3\nread_clear_mask = 0xc\n",
    )
    .unwrap();

    let models = &profiles[0].scenarios[0].scenario.device_models;
    assert_eq!(models.len(), 1);
    let descriptor = models[0].descriptor();
    assert_eq!(descriptor.id, "irq-status");
    assert_eq!(descriptor.kind, "w1c");
    assert_eq!(descriptor.range.start, 0x6000_8020);
    assert_eq!(descriptor.range.length, 4);
    assert_eq!(descriptor.configuration["clear-mask"], "0x00000003");
}
