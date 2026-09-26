//! Structural timeline admission; physical execution remains the application/backend's authority.
use super::*;
pub(super) fn validate(input: &Invocation, event: &ExecutionEvent) -> Result<()> {
    let pc_valid = |pc: u32| pc & 1 == 0 && pc < u32::MAX - 1;
    match event {
        ExecutionEvent::Memory { site, transaction } => {
            if !pc_valid(*site)
                || !input.observe_timeline.selects(transaction)
                || matches!(transaction, MemoryTransaction::InitializeZeroed { .. })
            {
                return Err(integrity(
                    "unrequested memory timeline or invalid instruction identity",
                ));
            }
            transaction.validate()?;
        }
        ExecutionEvent::Branch {
            site,
            target,
            fallthrough,
            ..
        } if !input.observe_timeline.branches
            || !pc_valid(*site)
            || target & 1 != 0
            || !pc_valid(*fallthrough)
            || !matches!(fallthrough.checked_sub(*site), Some(2 | 4)) =>
        {
            return Err(integrity(
                "unrequested or invalid conditional branch observation",
            ));
        }
        _ => (),
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn input() -> Invocation {
        Invocation {
            entry: 0x1000,
            goal: ExecutionGoal::Return,
            arguments: vec![],
            memory: vec![],
            models: vec![],
            calls: vec![],
            tables: vec![],
            services: vec![],
            observe_memory: vec![],
            observe_calls: None,
            observe_timeline: TimelineCapture {
                reads: true,
                writes: true,
                atomics: true,
                branches: true,
                written: false,
            },
        }
    }
    #[test]
    fn unrequested_and_malformed_physical_timeline_records_are_rejected() {
        let mut input = input();
        let event = |transaction| ExecutionEvent::Memory {
            site: 0x1000,
            transaction,
        };
        for bad in [
            MemoryTransaction::InitializeZeroed {
                address: 0x3000,
                length: 8,
            },
            MemoryTransaction::Read {
                address: 0x3000,
                width: 0,
                value: MemoryReadValue::Unknown,
            },
            MemoryTransaction::LoadReserved {
                address: 0x3001,
                order: ExecutionOrdering {
                    acquire: false,
                    release: false,
                },
                value: 1,
            },
            MemoryTransaction::Write {
                address: 0x3000,
                width: 1,
                value: 256,
            },
            MemoryTransaction::Write {
                address: u32::MAX - 3,
                width: 4,
                value: 1,
            },
        ] {
            assert_eq!(
                validate(&input, &event(bad)).unwrap_err().code,
                ErrorCode::Integrity
            );
        }
        let good = event(MemoryTransaction::Read {
            address: 0x3001,
            width: 4,
            value: MemoryReadValue::Unknown,
        });
        validate(&input, &good).unwrap();
        input.observe_timeline.reads = false;
        assert!(validate(&input, &good).is_err());
        for (target, fallthrough) in [(0x1001, 0x1004), (0x1000, 0x1006)] {
            assert!(
                validate(
                    &input,
                    &ExecutionEvent::Branch {
                        site: 0x1000,
                        target,
                        fallthrough,
                        taken: true
                    }
                )
                .is_err()
            );
        }
        input.observe_timeline.branches = false;
        assert!(
            validate(
                &input,
                &ExecutionEvent::Branch {
                    site: 0x1000,
                    target: 0x1000,
                    fallthrough: 0x1004,
                    taken: true
                }
            )
            .is_err()
        );
    }
    /// One compared phase whose selected read is unknown, with its rows but
    /// without the trailing coverage records.
    fn unknown_read() -> (TestExecution, Vec<ExecutionEvidence>) {
        let id = ArtifactId::of_bytes(b"fixture");
        let target = ExecutionTarget {
            revision: id.as_str().parse().unwrap(),
            source: FunctionSource::Input { input: 0 },
            companions: vec![],
            abi: CallAbi::RiscvInteger,
            stack: MemorySeed {
                address: 0x8000,
                length: 4096,
                fill: None,
                bytes: vec![],
            },
        };
        let request = ExecutionRequest {
            schema: EXECUTION_SCHEMA,
            vendor: target.clone(),
            replacement: Some(target),
            binding: Some(CompiledBinding::SharedCore),
            max_events: 4,
            cases: vec![ExecutionCase {
                name: "case".into(),
                reset: SessionReset::Cold,
                stack_fill: None,
                vendor: input(),
                replacement: Some(input()),
                relation: Some(ComparisonRelation {
                    effects: None,
                    projection: None,
                    returns: ReturnWords {
                        low: false,
                        high: false,
                    },
                    events: EventChannels {
                        mmio_read: false,
                        mmio_write: false,
                        fence: false,
                        delay: false,
                        timeline: TimelineCapture {
                            reads: true,
                            ..Default::default()
                        },
                    },
                    memory: vec![],
                    calls: false,
                    reviewed_calls: None,
                }),
            }],
        };
        let manifest = ExecutionManifest {
            effect_contracts: vec![],
            projections: vec![],
            schema: EXECUTION_SCHEMA,
            project: id.as_str().parse().unwrap(),
            call_pairs: vec![],
            records: id,
            producer: ExecutionProducer {
                executor: "test".into(),
                environment: "test".into(),
                verifier: "test".into(),
            },
            complete: true,
            verdict: Some(ComparisonVerdict::Incomplete),
            request: ArtifactId::of_bytes(b"request"),
        };
        let manifest = TestExecution::new(manifest, request);
        let mut rows = Vec::new();
        for replacement in [false, true] {
            rows.push(ExecutionEvidence::Event {
                case: 0,
                replacement,
                event: ExecutionEvent::Memory {
                    site: 0x1000,
                    transaction: MemoryTransaction::Read {
                        address: 0x3000,
                        width: 4,
                        value: MemoryReadValue::Unknown,
                    },
                },
            });
            rows.push(ExecutionEvidence::Outcome {
                case: 0,
                replacement,
                steps: 1,
                stop: ExecutionStop::Returned {
                    low: Some(0),
                    high: None,
                },
            });
        }
        rows.push(ExecutionEvidence::Comparison {
            case: 0,
            result: CaseComparison {
                effect_claim: None,
                effect_gap: None,
                verdict: ComparisonVerdict::Incomplete,
                difference: None,
            },
        });
        (manifest, rows)
    }
    fn validate_rows(manifest: &TestExecution, rows: &[ExecutionEvidence]) -> Result<()> {
        let mut bytes = Vec::new();
        for row in rows {
            serde_json::to_writer(&mut bytes, row).unwrap();
            bytes.push(b'\n');
        }
        manifest.validate_records(
            &bytes.as_slice(),
            &WorkingMemory::new(1024 * 1024).unwrap(),
            &mut || Ok(()),
        )
    }
    fn check(manifest: &TestExecution, rows: &[ExecutionEvidence]) -> Result<()> {
        let mut rows = rows.to_vec();
        rows.extend(crate::executions::coverage_rows(&manifest.request));
        validate_rows(manifest, &rows)
    }
    #[test]
    fn completed_code_cannot_forge_match_for_unknown_selected_memory_reads() {
        let (mut manifest, mut rows) = unknown_read();
        check(&manifest, &rows).unwrap();
        manifest.verdict = Some(ComparisonVerdict::Match);
        if let ExecutionEvidence::Comparison { result, .. } = rows.last_mut().unwrap() {
            result.verdict = ComparisonVerdict::Match;
        }
        assert_eq!(
            check(&manifest, &rows).unwrap_err().code,
            ErrorCode::Integrity
        );
        let relation = manifest.request.cases[0].relation.as_mut().unwrap();
        relation.events.timeline.reads = false;
        relation.returns.low = true;
        check(&manifest, &rows).unwrap(); // explicit exclusion preserves unknown raw evidence
    }
    #[test]
    fn coverage_is_required_once_per_side_after_the_last_case_and_consistent() {
        let (manifest, rows) = unknown_read();
        let reached = |instructions: Vec<u32>, branches: Vec<BranchCoverage>| ExecutionCoverage {
            instructions,
            branches,
            transfers: vec![],
        };
        let row = |replacement, coverage| ExecutionEvidence::Coverage {
            replacement,
            coverage,
        };
        let valid = reached(
            vec![0x1000, 0x1004],
            vec![BranchCoverage {
                site: 0x1000,
                taken: true,
                fallthrough: false,
            }],
        );
        let with = |tail: Vec<ExecutionEvidence>| {
            let mut all = rows.clone();
            all.extend(tail);
            validate_rows(&manifest, &all)
        };
        with(vec![row(false, valid.clone()), row(true, valid.clone())]).unwrap();
        // A side may continue in ascending records; empty coverage is one record.
        let later = reached(vec![0x2000], vec![]);
        with(vec![
            row(false, valid.clone()),
            row(false, later.clone()),
            row(true, reached(vec![], vec![])),
        ])
        .unwrap();
        for tail in [
            vec![],
            vec![row(false, valid.clone())],
            vec![row(true, valid.clone()), row(false, valid.clone())],
            vec![
                row(false, valid.clone()),
                row(false, valid.clone()),
                row(true, valid.clone()),
            ],
            // Records of a side must ascend, and an empty record stands alone.
            vec![
                row(false, later.clone()),
                row(false, valid.clone()),
                row(true, valid.clone()),
            ],
            vec![
                row(false, reached(vec![], vec![])),
                row(false, later.clone()),
                row(true, valid.clone()),
            ],
            vec![
                row(false, valid.clone()),
                row(true, valid.clone()),
                row(false, later.clone()),
            ],
            // A branch direction of an instruction that never executed.
            vec![
                row(false, reached(vec![0x1004], valid.branches.clone())),
                row(true, valid.clone()),
            ],
            // Unordered instructions and a branch with no direction.
            vec![
                row(false, reached(vec![0x1004, 0x1000], vec![])),
                row(true, valid.clone()),
            ],
            vec![
                row(
                    false,
                    reached(
                        vec![0x1000],
                        vec![BranchCoverage {
                            site: 0x1000,
                            taken: false,
                            fallthrough: false,
                        }],
                    ),
                ),
                row(true, valid.clone()),
            ],
        ] {
            assert_eq!(with(tail).unwrap_err().code, ErrorCode::Integrity);
        }
        // Coverage cannot precede the last case's records.
        let mut early = vec![row(false, valid.clone())];
        early.extend(rows.clone());
        early.push(row(true, valid));
        assert_eq!(
            validate_rows(&manifest, &early).unwrap_err().code,
            ErrorCode::Integrity
        );
    }
}
