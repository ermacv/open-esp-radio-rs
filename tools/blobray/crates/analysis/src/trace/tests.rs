//! Negative retained-fact admission; positive transfer tests start from ELF in Next.
use super::*;

#[test]
fn missing_unsupported_duplicate_and_contradictory_link_effects_fail_closed() {
    let id = ArtifactId::of_bytes(b"caller");
    let analysis: FunctionAnalysisId = id.as_str().parse().unwrap();
    let other: FunctionAnalysisId = ArtifactId::of_bytes(b"callee").as_str().parse().unwrap();
    let object = ObjectId {
        artifact: id.clone(),
        location: ObjectLocation::Standalone,
    };
    let extent = CodeRange {
        start: 0x1000,
        length: 4,
    };
    let manifest = FunctionManifest {
        schema: FUNCTION_SCHEMA,
        recipe: FunctionRecipe {
            research: None,
            abi: RiscvAbi::Ilp32,
            schema: FUNCTION_SCHEMA,
            policy: FUNCTION_POLICY,
            decoder: "negative-fixture".into(),
            semantics: Some("negative-fixture".into()),
            project: id.as_str().parse().unwrap(),
            revision: id.as_str().parse().unwrap(),
            source: FunctionSource::Input { input: 0 },
            address_space: CodeAddressSpace::Image,
            selector: FunctionSelector::Range {
                object,
                section: 1,
                extent,
            },
            payload: id.clone(),
            section: 1,
            extent,
            user_extent: false,
        },
        records: id.clone(),
        coverage: FunctionCoverage::default(),
        instructions: 1,
        blocks: 0,
        edges: 0,
        references: 0,
        gaps: 0,
        semantics: Some(SemanticSummary::default()),
    };
    for case in 0..5 {
        let mut records = vec![
            FunctionRecord::Instruction {
                offset: 0x1000,
                bytes: vec![0xef, 0, 0, 0],
                decoded: DecodedOp {
                    length: 4,
                    text: "unused".into(),
                    flow: InstructionFlow::Jump {
                        displacement: 0,
                        link: true,
                    },
                },
            },
            FunctionRecord::CallInputs {
                offset: 0x1000,
                registers: vec![AbstractValue::Unknown; 32],
            },
        ];
        let effect = FunctionRecord::Value {
            offset: 0x1000,
            register: if case == 1 {
                6
            } else if case == 4 {
                32
            } else {
                1
            },
            value: AbstractValue::ImageAddress {
                address: if case == 3 { 0x1008 } else { 0x1004 },
            },
            relocation: None,
        };
        if case != 0 {
            records.push(effect.clone());
        }
        if case == 2 {
            records.push(effect);
        }
        let functions = [
            TraceFunction {
                id: &analysis,
                manifest: &manifest,
                records: &records,
            },
            TraceFunction {
                id: &other,
                manifest: &manifest,
                records: &records,
            },
        ];
        let target = TraceTarget {
            ir: id.clone(),
            profile: "negative".into(),
            entry: analysis.clone(),
            abi: CallAbi::RiscvInteger,
            registers: vec![],
        };
        let observation = TraceObservation {
            ranges: vec![],
            fences: true,
        };
        let input = TraceInput {
            functions: &functions,
            links: &[TraceLink {
                caller: 0,
                offset: 0x1000,
                callee: Some(1),
                issue: None,
            }],
            entry: 0,
            target: &target,
            observation: &observation,
            side: TraceSide::Left,
        };
        let memory = WorkingMemory::new(1024 * 1024).unwrap();
        let mut canonical = Canonical::new(&memory);
        let mut rows = Vec::new();
        let result = extract(
            &input,
            &memory,
            &mut canonical,
            &mut || Ok(()),
            &mut |row, _| {
                rows.push(row.clone());
                Ok(())
            },
        );
        if case <= 1 {
            assert!(!result.unwrap().outcome.exact);
            assert!(rows.iter().any(|r| matches!(
                r,
                TraceRecord::Blocked {
                    reason: TraceBlocker::UnsupportedEffect,
                    ..
                }
            )));
        } else {
            assert_eq!(result.err().unwrap().code, ErrorCode::Integrity);
        }
        drop(canonical);
        assert_eq!(memory.used(), 0);
    }
    let mut partial = manifest.clone();
    partial.coverage.control_flow = false;
    for case in 0..9 {
        let mut records = vec![
            FunctionRecord::Instruction {
                offset: 0x1000,
                bytes: vec![0xe7, 0x80, 2, 0],
                decoded: DecodedOp {
                    length: 4,
                    text: "unused".into(),
                    flow: InstructionFlow::Indirect {
                        base: 5,
                        offset: 0,
                        link: true,
                    },
                },
            },
            FunctionRecord::Edge {
                from: 0x1000,
                target: None,
                relation: EdgeKind::Call,
                external: true,
            },
        ];
        match case {
            0..=2 => (),
            3 => records.push(FunctionRecord::Gap {
                start: 0x1000,
                length: 2,
                reason: GapReason::ConflictingBoundary,
            }),
            4 => records.push(FunctionRecord::SemanticGap {
                offset: 0x1000,
                reason: SemanticGapReason::UnexpandedControlFlow,
            }),
            5 => records.push(FunctionRecord::Edge {
                from: 0x1000,
                target: Some(0x1001),
                relation: EdgeKind::Conflict,
                external: false,
            }),
            6 => records.push(FunctionRecord::Edge {
                from: 0x1000,
                target: None,
                relation: EdgeKind::Stop,
                external: true,
            }),
            7 => records.push(FunctionRecord::Edge {
                from: 0x1000,
                target: None,
                relation: EdgeKind::Taken,
                external: true,
            }),
            8 => {
                if let FunctionRecord::Instruction { decoded, .. } = &mut records[0] {
                    decoded.flow = InstructionFlow::Next;
                }
            }
            _ => unreachable!(),
        }
        let links = [TraceLink {
            caller: 0,
            offset: 0x1000,
            callee: (case != 1).then_some(1),
            issue: (case == 2).then_some(TraceBlocker::UnresolvedCall),
        }];
        let function = TraceFunction {
            id: &analysis,
            manifest: &partial,
            records: &records,
        };
        let memory = WorkingMemory::new(1024 * 1024).unwrap();
        let index = Index::new(&function, &memory, &mut || Ok(())).unwrap();
        assert_eq!(
            index.complete_for_trace(0, &links, &mut || Ok(())).unwrap(),
            case == 0,
            "case {case}"
        );
        drop(index);
        assert_eq!(memory.used(), 0);
    }
}
