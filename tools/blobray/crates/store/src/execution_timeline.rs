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
            MemoryTransaction::Read {
                address: 0x3001,
                width: 4,
                value: MemoryReadValue::Unknown,
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
            address: 0x3000,
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
    #[test]
    fn completed_code_cannot_forge_match_for_unknown_selected_memory_reads() {
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
        let mut manifest = ExecutionManifest {
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
            request: ExecutionRequest {
                schema: EXECUTION_SCHEMA,
                vendor: target.clone(),
                replacement: Some(target),
                binding: Some(CompiledBinding::SharedCore),
                max_events: 4,
                cases: vec![ExecutionCase {
                    name: "case".into(),
                    reset: SessionReset::Cold,
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
            },
        };
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
        let check = |manifest: &ExecutionManifest, rows: &[ExecutionEvidence]| {
            let mut bytes = Vec::new();
            for r in rows {
                serde_json::to_writer(&mut bytes, r).unwrap();
                bytes.push(b'\n');
            }
            validate_execution_records(
                manifest,
                &bytes.as_slice(),
                &WorkingMemory::new(1024 * 1024).unwrap(),
                &mut || Ok(()),
            )
        };
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
}
