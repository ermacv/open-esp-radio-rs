use super::interfaces::{contract, fixture_code, request, review, run};
use super::*;
pub(super) fn setup(initial: Vec<u32>) -> (Fixture, ExecutionRequest) {
    setup_queues(initial, false)
}
fn setup_queues(initial: Vec<u32>, second: bool) -> (Fixture, ExecutionRequest) {
    setup_code(
        initial,
        second,
        &[
            0x00008413, 0xff010113, 0x00c12023, 0x00010693, 0x00072303, 0x000300e7, 0x01010113,
            0x00040067,
        ],
    )
}
fn setup_code(initial: Vec<u32>, second: bool, code: &[u32]) -> (Fixture, ExecutionRequest) {
    let (f, occurrence) = fixture_code(code);
    let mut c = contract(
        AccessRoot::Address { address: 0x3000 },
        occurrence.object.artifact.clone(),
    );
    for (offset, name) in [(8, "dequeue"), (12, "length")] {
        let mut slot = c.slots[0].clone();
        slot.offset = offset;
        slot.name = name.into();
        c.slots.push(slot);
    }
    if second {
        c.layout_bytes = 32;
        for offset in [16, 20, 24] {
            let mut slot = c.slots[0].clone();
            slot.offset = offset;
            slot.name = format!("other-{offset}");
            c.slots.push(slot);
        }
    }
    let selection = review(&f, occurrence, c);
    let mut r = request(&f, selection);
    r.max_events = 128;
    r.cases[0].vendor.arguments = vec![Some(0x3000), Some(0x55), Some(42), None, Some(0x3004)];
    r.cases[0].vendor.tables[0].lifetime = RegionLifetime::Session;
    r.cases[0].vendor.tables[0].slots = vec![
        RuntimeSlot {
            offset: 4,
            target: RuntimeSlotTarget::Service { address: 0x2000 },
        },
        RuntimeSlot {
            offset: 8,
            target: RuntimeSlotTarget::Service { address: 0x2004 },
        },
        RuntimeSlot {
            offset: 12,
            target: RuntimeSlotTarget::Service { address: 0x2008 },
        },
    ];
    let operations = [
        FifoOperation::Enqueue {
            input: FifoInput::Argument { word: 2, width: 4 },
            success: 1,
            full: 0,
            wake: Some(FifoOutput { word: 3, width: 4 }),
        },
        FifoOperation::Dequeue {
            output: FifoOutput { word: 3, width: 4 },
            success: 1,
            empty: 0,
        },
        FifoOperation::Length,
    ];
    r.cases[0].vendor.services = vec![FifoService {
        id: "queue".into(),
        applicability: "synthetic reviewed callbacks".into(),
        lifetime: RegionLifetime::Session,
        handle: 0x55,
        item_width: 4,
        capacity: 2,
        items: initial,
        bindings: operations
            .into_iter()
            .enumerate()
            .map(|(i, operation)| FifoBinding {
                table: "callbacks".into(),
                slot: 4 + i as u32 * 4,
                call: CallBinding {
                    address: 0x2000 + i as u32 * 4,
                    boundary: CallBoundary::Unmapped,
                    allow_tail: false,
                },
                argument_words: 5,
                handle_word: 1,
                operation,
            })
            .collect(),
    }];
    if second {
        r.cases[0].vendor.tables[0].seed.length = 32;
        let mut other = r.cases[0].vendor.services[0].clone();
        other.id = "other".into();
        other.handle = 0x66;
        other.items = vec![99];
        for (i, b) in other.bindings.iter_mut().enumerate() {
            b.slot = 16 + i as u32 * 4;
            b.call.address += 16;
            r.cases[0].vendor.tables[0].slots.push(RuntimeSlot {
                offset: b.slot,
                target: RuntimeSlotTarget::Service {
                    address: b.call.address,
                },
            });
        }
        r.cases[0].vendor.services.push(other);
    }
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    (f, r)
}
fn append(r: &mut ExecutionRequest, name: &str, slot: u32, value: u32, goal: ExecutionGoal) {
    let mut phase = r.cases[0].clone();
    phase.name = name.into();
    phase.reset = SessionReset::Warm;
    phase.vendor.tables.clear();
    phase.vendor.services.clear();
    phase.vendor.arguments[2] = Some(value);
    phase.vendor.arguments[4] = Some(slot);
    phase.vendor.goal = goal;
    phase.replacement = Some(phase.vendor.clone());
    r.cases.push(phase);
}
#[test]
fn reviewed_fifo_order_full_empty_length_and_wake_are_retained() {
    let (f, mut r) = setup(vec![]);
    append(&mut r, "enqueue-second", 0x3004, 7, ExecutionGoal::Return);
    append(&mut r, "full", 0x3004, 9, ExecutionGoal::Return);
    append(&mut r, "length", 0x300c, 0, ExecutionGoal::Return);
    append(&mut r, "first", 0x3008, 0, ExecutionGoal::Return);
    append(&mut r, "second", 0x3008, 0, ExecutionGoal::Return);
    append(&mut r, "empty", 0x3008, 0, ExecutionGoal::Return);
    let (m, rows) = run(&f, r);
    assert!(m.complete, "{rows:?}");
    assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
    let transitions: Vec<_> = rows
        .iter()
        .filter_map(|r| match r {
            ExecutionEvidence::Event {
                replacement: false,
                event:
                    ExecutionEvent::ServiceResult {
                        transition,
                        depth,
                        words,
                        ..
                    },
                ..
            } => Some((*transition, *depth, words[0])),
            _ => None,
        })
        .collect();
    assert_eq!(
        transitions,
        vec![
            (
                FifoTransition::Enqueued {
                    value: 42,
                    woke: true
                },
                1,
                Some(1)
            ),
            (
                FifoTransition::Enqueued {
                    value: 7,
                    woke: false
                },
                2,
                Some(1)
            ),
            (FifoTransition::Full { value: 9 }, 2, Some(0)),
            (FifoTransition::Length, 2, Some(2)),
            (FifoTransition::Dequeued { value: 42 }, 1, Some(1)),
            (FifoTransition::Dequeued { value: 7 }, 0, Some(1)),
            (FifoTransition::Empty, 0, Some(0)),
        ]
    );
    let outputs: Vec<_> = rows
        .iter()
        .filter_map(|r| match r {
            ExecutionEvidence::Event {
                replacement: false,
                event: ExecutionEvent::ServiceOutput { value, .. },
                ..
            } => Some(*value),
            _ => None,
        })
        .collect();
    assert_eq!(outputs, vec![1, 0, 0, 42, 7]);
}
#[test]
fn only_selected_successful_dequeue_completes_the_service_goal() {
    for (initial, expected, completed) in [
        (vec![42], Some(42), true),
        (vec![], None, false),
        (vec![7], Some(42), false),
    ] {
        let (f, mut r) = setup(initial);
        for phase in &mut r.cases {
            phase.relation.as_mut().unwrap().returns.low = false;
        }
        r.cases[0].vendor.arguments[4] = Some(0x3008);
        r.cases[0].vendor.goal = ExecutionGoal::ObserveDequeue {
            service: "queue".into(),
            value: expected,
        };
        r.cases[0].replacement = Some(r.cases[0].vendor.clone());
        let (m, rows) = run(&f, r);
        assert_eq!(m.complete, completed, "{rows:?}");
        assert!(
            rows.iter().any(|r| matches!(
                r,
                ExecutionEvidence::Outcome {
                    stop: ExecutionStop::ObservedDequeue { value: 42, .. },
                    ..
                }
            )) == completed
        );
        if !completed {
            assert!(rows.iter().any(|r| matches!(
                r,
                ExecutionEvidence::Outcome {
                    stop: ExecutionStop::GoalNotReached { .. },
                    ..
                }
            )));
        }
    }
}
fn service(rows: &[ExecutionEvidence], phase: u32) -> &FifoObservation {
    rows.iter()
        .find_map(|r| match r {
            ExecutionEvidence::FifoService {
                case,
                replacement: false,
                observation,
            } if *case == phase => Some(observation),
            _ => None,
        })
        .unwrap()
}
fn width(input: &mut Invocation, w: u8, private: bool) {
    let q = &mut input.services[0];
    q.item_width = w;
    if let FifoOperation::Enqueue { input, .. } = &mut q.bindings[0].operation {
        *input = if private {
            FifoInput::PrivateStack { word: 3, width: w }
        } else {
            FifoInput::Argument { word: 2, width: w }
        };
    }
    if let FifoOperation::Dequeue { output, .. } = &mut q.bindings[1].operation {
        output.width = w;
    }
}
#[test]
fn private_stack_items_keep_width_and_queue_lifetime() {
    for w in [1, 2, 4] {
        let (f, mut r) = setup(vec![]);
        width(&mut r.cases[0].vendor, w, true);
        r.cases[0].replacement = Some(r.cases[0].vendor.clone());
        append(&mut r, "dequeue", 0x3008, 0, ExecutionGoal::Return);
        let mut cold = r.cases[0].clone();
        cold.name = "cold".into();
        r.cases.push(cold);
        let (m, rows) = run(&f, r);
        assert!(m.complete);
        assert_eq!(
            (
                service(&rows, 0).depth,
                service(&rows, 1).depth,
                service(&rows, 2).depth
            ),
            (1, 0, 1)
        );
        assert_eq!(
            (
                service(&rows, 0).operations,
                service(&rows, 1).operations,
                service(&rows, 2).operations
            ),
            (1, 2, 1)
        );
        assert!(rows.iter().any(|r| matches!(r,ExecutionEvidence::Event {event:ExecutionEvent::ServiceInput {width,value:42,..},..} if *width==w)));
        assert!(rows.iter().any(|r| matches!(
            r,
            ExecutionEvidence::Event {
                event: ExecutionEvent::ServiceResult {
                    transition: FifoTransition::Dequeued { value: 42 },
                    ..
                },
                ..
            }
        )));
    }
}
#[test]
fn invalid_handles_items_and_outputs_leave_queue_unchanged_and_block_warm_use() {
    for variant in 0..6 {
        let (f, mut r) = setup(if variant == 5 { vec![42] } else { vec![] });
        let expected = match variant {
            0 => {
                r.cases[0].vendor.arguments[1] = Some(99);
                FifoIssue::Handle {
                    expected: 0x55,
                    actual: 99,
                }
            }
            1 => {
                r.cases[0].vendor.arguments[1] = None;
                FifoIssue::Call {
                    issue: CallIssue::UnknownArgument { word: 1 },
                }
            }
            2 => {
                width(&mut r.cases[0].vendor, 1, false);
                r.cases[0].vendor.arguments[2] = Some(256);
                FifoIssue::ItemWidth { value: 256 }
            }
            3 => {
                if let FifoOperation::Enqueue { input, .. } =
                    &mut r.cases[0].vendor.services[0].bindings[0].operation
                {
                    *input = FifoInput::PrivateStack { word: 0, width: 4 };
                }
                FifoIssue::InputAccess { address: 0x3000 }
            }
            4 => {
                if let FifoOperation::Enqueue { wake, .. } =
                    &mut r.cases[0].vendor.services[0].bindings[0].operation
                {
                    *wake = Some(FifoOutput { word: 0, width: 4 });
                }
                FifoIssue::Call {
                    issue: CallIssue::OutputAccess {
                        address: 0x3000,
                        scope: CallOutputScope::PrivateStack,
                    },
                }
            }
            _ => {
                r.cases[0].vendor.arguments[4] = Some(0x3008);
                if let FifoOperation::Dequeue { output, .. } =
                    &mut r.cases[0].vendor.services[0].bindings[1].operation
                {
                    output.word = 0;
                }
                FifoIssue::Call {
                    issue: CallIssue::OutputAccess {
                        address: 0x3000,
                        scope: CallOutputScope::PrivateStack,
                    },
                }
            }
        };
        r.cases[0].replacement = Some(r.cases[0].vendor.clone());
        append(&mut r, "blocked", 0x3008, 0, ExecutionGoal::Return);
        let (m, rows) = run(&f, r);
        assert!(!m.complete);
        for phase in [0, 1] {
            let o = service(&rows, phase);
            assert_eq!(o.issue, Some(expected));
            assert_eq!(o.operations, 0);
            assert_eq!(o.depth, u32::from(variant == 5));
        }
        assert!(rows.iter().any(|r| matches!(
            r,
            ExecutionEvidence::Outcome {
                case: 1,
                stop: ExecutionStop::BlockedByPriorPhase,
                ..
            }
        )));
        assert!(!rows.iter().any(|r| matches!(
            r,
            ExecutionEvidence::Event {
                event: ExecutionEvent::ServiceOutput { .. } | ExecutionEvent::ServiceResult { .. },
                ..
            }
        )));
    }
}
#[test]
fn phase_queues_release_state_and_can_reuse_the_same_binding_on_warm_entry() {
    let (f, mut r) = setup(vec![]);
    r.cases[0].vendor.services[0].lifetime = RegionLifetime::Phase;
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    append(
        &mut r,
        "fresh-phase-queue",
        0x3008,
        0,
        ExecutionGoal::Return,
    );
    r.cases[1].vendor.services = r.cases[0].vendor.services.clone();
    r.cases[1].replacement = Some(r.cases[1].vendor.clone());
    let (m, rows) = run(&f, r);
    assert!(m.complete);
    assert_eq!(service(&rows, 0).depth, 1);
    assert_eq!(service(&rows, 1).depth, 0);
    assert_eq!(service(&rows, 1).operations, 1);
    assert_eq!(service(&rows, 0).instance, service(&rows, 1).instance);
    assert!(rows.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Event {
            case: 1,
            event: ExecutionEvent::ServiceResult {
                transition: FifoTransition::Empty,
                ..
            },
            ..
        }
    )));
}
#[test]
fn services_and_comparison_sides_keep_isolated_queues() {
    let (f, mut r) = setup_queues(vec![], true);
    append(&mut r, "other", 0x3014, 0, ExecutionGoal::Return);
    r.cases[1].vendor.arguments[1] = Some(0x66);
    r.cases[1].replacement = Some(r.cases[1].vendor.clone());
    append(&mut r, "original", 0x3008, 0, ExecutionGoal::Return);
    let (m, rows) = run(&f, r);
    assert!(m.complete);
    for side in [false, true] {
        let values: Vec<_> = rows
            .iter()
            .filter_map(|r| match r {
                ExecutionEvidence::Event {
                    replacement,
                    event:
                        ExecutionEvent::ServiceResult {
                            instance,
                            transition: FifoTransition::Dequeued { value },
                            ..
                        },
                    ..
                } if *replacement == side => Some((*instance, *value)),
                _ => None,
            })
            .collect();
        assert_eq!(values, vec![(1, 99), (0, 42)]);
    }
}
#[test]
fn fifo_workflow_restores_and_replays_without_sources_and_failed_runs_publish_nothing() {
    let (f, mut r) = setup(vec![]);
    for phase in &mut r.cases {
        phase.relation.as_mut().unwrap().returns.low = false;
    }
    append(
        &mut r,
        "consume",
        0x3008,
        0,
        ExecutionGoal::ObserveDequeue {
            service: "queue".into(),
            value: Some(42),
        },
    );
    // Comparison requires the same goal kind on both sides of each phase, not across phases.
    let record = f.run(r.clone(), budget());
    assert_eq!(record.state, RunState::Completed, "{record:?}");
    let id = record.execution.unwrap();
    let original = f.read(&id);
    for variant in 0..3 {
        let mut b = budget();
        let mut limited = r.clone();
        match variant {
            0 => limited.max_events = 1,
            1 => b.max_work_units = Some(1),
            _ => b.working_memory_bytes = Some(1024),
        };
        let failed = f.run(limited, b);
        assert_eq!(failed.error.unwrap().code, ErrorCode::ResourceLimited);
        assert!(failed.execution.is_none());
    }
    let handle = f
        .app
        .start_execution(
            &f.project,
            r.clone(),
            &blobray_backend_riscv::RiscvExecutor,
            budget(),
        )
        .unwrap();
    handle.cancel();
    assert_eq!(handle.wait().state, RunState::Cancelled);
    let path = f._dir.path().join("fifo.json");
    fs::write(&path, serde_json::to_vec(&r).unwrap()).unwrap();
    let cli = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["--format", "json", "compare", "--project"])
        .arg(&f.project)
        .arg("--request")
        .arg(path)
        .args(["--limit-mode", "watchdog"])
        .output()
        .unwrap();
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let cli: serde_json::Value = serde_json::from_slice(&cli.stdout).unwrap();
    assert_eq!(cli["run"]["execution"], id.as_str());
    let backup = f._dir.path().join("fifo.blobray");
    let restored = f._dir.path().join("restored");
    for (command, project, flag, path) in [
        ("backup", &f.project, "--output", &backup),
        ("restore", &restored, "--backup", &backup),
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_blobray"))
            .args([command, "--project"])
            .arg(project)
            .arg(flag)
            .arg(path)
            .args(["--limit-mode", "watchdog"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    for command in ["execution", "replay"] {
        let out = Command::new(env!("CARGO_BIN_EXE_blobray"))
            .args(["--format", "json", command, "--project"])
            .arg(&restored)
            .args(["--id", id.as_str(), "--limit-mode", "watchdog"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        if command == "execution" {
            assert_eq!(value["records"], original["records"]);
            assert_eq!(value["summary"], original["summary"]);
        } else {
            assert_eq!(value["run"]["execution"], id.as_str());
        }
    }
}
#[test]
fn a_dequeue_from_another_queue_cannot_satisfy_the_selected_goal() {
    let (f, mut r) = setup_queues(vec![42], true);
    for phase in &mut r.cases {
        phase.relation.as_mut().unwrap().returns.low = false;
    }
    r.cases[0].vendor.arguments[4] = Some(0x3008);
    r.cases[0].vendor.goal = ExecutionGoal::ObserveDequeue {
        service: "other".into(),
        value: Some(42),
    };
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    let (m, rows) = run(&f, r);
    assert!(!m.complete);
    assert!(rows.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Event {
            event: ExecutionEvent::ServiceResult {
                instance: 0,
                transition: FifoTransition::Dequeued { value: 42 },
                ..
            },
            ..
        }
    )));
    assert!(!rows.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Outcome {
            stop: ExecutionStop::ObservedDequeue { .. },
            ..
        }
    )));
}
#[test]
fn service_abi_words_include_current_private_stack_arguments() {
    let (f, mut r) = setup(vec![]);
    r.cases[0].vendor.arguments[2] = Some(0x55);
    let binding = &mut r.cases[0].vendor.services[0].bindings[0];
    binding.argument_words = 9;
    binding.handle_word = 8;
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    let (m, rows) = run(&f, r);
    assert!(m.complete);
    assert!(rows.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Event {
            event: ExecutionEvent::ServiceArgument {
                word: 8,
                value: Some(0x55)
            },
            ..
        }
    )));
}
#[test]
fn missing_or_conflicting_service_bindings_fail_before_publication() {
    for variant in 0..5 {
        let (f, mut r) = setup(vec![]);
        match variant {
            0 => r.cases[0].vendor.services[0].bindings[0].table = "absent".into(),
            1 => r.cases[0].vendor.services[0].bindings[0].slot = 0,
            2 => {
                r.cases[0].vendor.calls = vec![CallDeclaration {
                    id: "conflict".into(),
                    applicability: "fixture".into(),
                    lifetime: RegionLifetime::Phase,
                    binding: CallBinding {
                        address: 0x2000,
                        boundary: CallBoundary::Unmapped,
                        allow_tail: false,
                    },
                    argument_words: 0,
                    responses: vec![],
                }]
            }
            3 => {
                r.cases[0].vendor.tables[0].slots[0].target =
                    RuntimeSlotTarget::Model { address: 0x2000 }
            }
            _ => {
                for phase in &mut r.cases {
                    phase.relation.as_mut().unwrap().returns.low = false;
                }
                r.cases[0].vendor.goal = ExecutionGoal::ObserveDequeue {
                    service: "missing".into(),
                    value: None,
                };
            }
        }
        r.cases[0].replacement = Some(r.cases[0].vendor.clone());
        let result = f.run(r, budget());
        assert!(matches!(
            result.error.unwrap().code,
            ErrorCode::InvalidRequest | ErrorCode::Conflict
        ));
        assert!(result.execution.is_none());
    }
}
#[test]
fn direct_calls_cannot_activate_services_and_expired_owners_do_not_return_zero() {
    let offset = 0x2000u32 - 0x1014;
    let jal = ((offset >> 20) & 1) << 31
        | ((offset >> 1) & 0x3ff) << 21
        | ((offset >> 11) & 1) << 20
        | ((offset >> 12) & 0xff) << 12
        | 0xef;
    let (f, r) = setup_code(
        vec![],
        false,
        &[
            0x00008413, 0xff010113, 0x00c12023, 0x00010693, 0x00072303, jal, 0x01010113, 0x00040067,
        ],
    );
    let (m, rows) = run(&f, r);
    assert!(!m.complete);
    assert_eq!(
        service(&rows, 0).issue,
        Some(FifoIssue::UnreviewedBinding { target: 0x2000 })
    );
    assert_eq!(service(&rows, 0).operations, 0);
    let (f, mut r) = setup(vec![]);
    r.cases[0].vendor.services[0].lifetime = RegionLifetime::Phase;
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    append(&mut r, "expired", 0x3008, 0, ExecutionGoal::Return);
    let (m, rows) = run(&f, r);
    assert!(!m.complete);
    assert!(rows.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Outcome {
            case: 1,
            stop: ExecutionStop::Incomplete {
                reason: ExecutionGap::RuntimeInterface {
                    issue: RuntimeTableIssue::UnavailableTarget { target: 0x2004 },
                    ..
                },
                ..
            },
            ..
        }
    )));
}
