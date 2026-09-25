//! Frozen accepted contract selection with operation-owned admitted copies.
use super::*;
pub struct ExecutionEffects<'m> {
    pub contracts: Vec<ResolvedEffectContract>,
    _payloads: AdmittedVec<'m, MemoryReservation<'m>>,
    _capacity: MemoryReservation<'m>,
}
impl Project {
    pub fn execution_effects<'m>(
        &self,
        request: &ExecutionRequest,
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<ExecutionEffects<'m>> {
        let mut reviews = AdmittedVec::new(memory);
        for case in &request.cases {
            if let Some(review) = case.relation.as_ref().and_then(|r| r.effects.as_ref()) {
                c.checkpoint(reviews.len() as u64 + 1)?;
                if !reviews.contains(&review) {
                    if reviews.len() == MAX_EFFECT_RULES {
                        return Err(Error::new(
                            ErrorCode::ResourceLimited,
                            "effect review capacity exceeded",
                        ));
                    }
                    reviews.push(review, c.position())?;
                }
            }
        }
        let capacity = memory.reserve(
            (reviews.len() * std::mem::size_of::<ResolvedEffectContract>()) as u64,
            c.position(),
        )?;
        let mut contracts = Vec::new();
        contracts
            .try_reserve_exact(reviews.len())
            .map_err(|_| Error::new(ErrorCode::ResourceLimited, "effect allocation refused"))?;
        let mut payloads = AdmittedVec::new(memory);
        for (i, review) in reviews.iter().enumerate() {
            c.checkpoint(i as u64 + 1)?;
            if reviews[..i].iter().any(|r| r.knowledge == review.knowledge) {
                continue;
            }
            let snapshot = self.knowledge_snapshot(Some(&review.knowledge), memory, c)?;
            for selected in &*reviews {
                c.checkpoint(1)?;
                if selected.knowledge != review.knowledge {
                    continue;
                }
                let entry = snapshot
                    .get(&selected.assertion, c)?
                    .ok_or_else(|| integrity("selected effect assertion missing"))?;
                let KnowledgeClaim::EffectContract { contract } = &entry.proposal.claim else {
                    return Err(Error::new(
                        ErrorCode::InvalidRequest,
                        "selected assertion is not an effect contract",
                    ));
                };
                if entry.state != AssertionState::Accepted
                    || entry.proposal.occurrence != contract.vendor.occurrence
                {
                    return Err(Error::new(
                        ErrorCode::InvalidRequest,
                        "effect contract requires a selected accepted review",
                    ));
                }
                c.checkpoint((contract.rules.len() + 1).pow(2) as u64)?;
                contract.validate()?;
                payloads.push(
                    memory.reserve(
                        contract.allocated_bytes()
                            + selected.knowledge.allocated_bytes()
                            + selected.assertion.allocated_bytes(),
                        c.position(),
                    )?,
                    c.position(),
                )?;
                contracts.push(ResolvedEffectContract {
                    review: (*selected).clone(),
                    contract: (**contract).clone(),
                });
            }
        }
        for (phase, case) in request.cases.iter().enumerate() {
            c.checkpoint(contracts.len() as u64 + 1)?;
            if let Some(p) = selected_effect_contract(case.relation.as_ref(), &contracts)? {
                c.checkpoint((p.contract.rules.len() + 1).pow(2) as u64)?;
                p.contract.validate_use(request, phase)?;
            }
        }
        Ok(ExecutionEffects {
            contracts,
            _payloads: payloads,
            _capacity: capacity,
        })
    }
    pub(super) fn validate_execution_effects(
        &self,
        manifest: &ExecutionManifest,
        request: &ExecutionRequest,
        memory: &WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        let selected = self.execution_effects(request, memory, c)?;
        if selected.contracts != manifest.effect_contracts {
            return Err(integrity(
                "retained effect contracts differ from selected accepted reviews",
            ));
        }
        Ok(())
    }
}

/// Check policy accounting against the entire retained raw stream; no ISA replay.
pub(super) fn validate_result(
    result: &CaseComparison,
    trackers: &[Option<EffectTracker<'_>>; 2],
) -> Result<()> {
    let claim = trackers[0].as_ref().map(EffectTracker::claim_ceiling);
    let gap = trackers.iter().flatten().find_map(EffectTracker::gap);
    let violation = trackers
        .iter()
        .flatten()
        .find_map(EffectTracker::first_violation);
    if result.effect_claim != claim
        || result.effect_gap != gap
        || (result.verdict == ComparisonVerdict::Match && (gap.is_some() || violation.is_some()))
        || (violation.is_some() && result.verdict != ComparisonVerdict::Diff)
        || matches!(&result.difference, Some(ComparisonDifference::EffectViolation { violation: v }) if Some(*v) != violation)
    {
        return Err(integrity(
            "retained effect accounting differs from captured observations",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_effect_payloads_release_capacity_and_retained_policy_cannot_change() {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::create(dir.path()).unwrap();
        let id = ArtifactId::of_bytes(b"effect fixture");
        let occurrence = KnowledgeOccurrence {
            revision: id.as_str().parse().unwrap(),
            source: FunctionSource::Input { input: 0 },
            object: ObjectId {
                artifact: id.clone(),
                location: ObjectLocation::Standalone,
            },
            symbol: None,
        };
        // Store tests use admitted opaque physical identities; application checks captured code.
        let mut endpoint = CallEndpoint {
            occurrence,
            boundary: ReviewedCallBoundary::Code { address: 0x1000 },
        };
        endpoint.occurrence.symbol = Some(SymbolId {
            object: endpoint.occurrence.object.clone(),
            table: SymbolTableKind::Static,
            table_section: 3,
            index: 1,
        });
        let pattern = EffectPattern {
            followed_by: None,
            selector: EffectSelector::MmioWrite {
                address: 0x3000,
                width: 4,
            },
            value: EffectValue::Any,
        };
        let contract = EffectContract {
            unclassified: UnclassifiedEffects::Incomplete,
            vendor: endpoint.clone(),
            replacement: endpoint.clone(),
            rules: vec![EffectRule {
                name: "register".into(),
                vendor: Some(pattern),
                replacement: Some(pattern),
                disposition: EffectDisposition::Required,
                min_occurrences: 1,
                max_occurrences: 4,
                reason: "fixture".into(),
            }],
            claim_ceiling: EffectClaimCeiling::SelectedEffectEquality,
            applicability: "fixture".into(),
            reason: "fixture".into(),
        };
        let input = Invocation {
            entry: 0x1000,
            goal: ExecutionGoal::Return,
            arguments: vec![],
            memory: vec![],
            models: vec![],
            calls: vec![],
            tables: vec![],
            services: vec![],
            observe_calls: None,
            observe_timeline: TimelineCapture::default(),
            observe_memory: vec![],
        };
        let assertion: AssertionId = ArtifactId::of_bytes(b"review").as_str().parse().unwrap();
        let mut base = None;
        for action in [
            KnowledgeAction::Propose {
                proposal: KnowledgeProposal {
                    subject: "fixture".to_owned().try_into().unwrap(),
                    occurrence: endpoint.occurrence.clone(),
                    claim: KnowledgeClaim::EffectContract {
                        contract: Box::new(contract.clone()),
                    },
                    evidence: vec![EvidenceRef::Document {
                        payload: id.clone(),
                    }],
                    note: None,
                },
            },
            KnowledgeAction::Review {
                assertion: assertion.clone(),
                decision: ReviewDecision::Accept,
                supersedes: None,
            },
        ] {
            let mut writer = project.writer().unwrap();
            let change = KnowledgeChange {
                expected_base: base,
                actor: "fixture".into(),
                reason: "test".into(),
                action,
            };
            let (mut run, path) = writer
                .register_operation(
                    ResourceBudget::default(),
                    OwnerIdentity {
                        pid: 123,
                        start_ticks: 42,
                        boot_id: "test".into(),
                    },
                    RunOperation::Knowledge {
                        change: change.clone(),
                    },
                    |_| {},
                )
                .unwrap();
            let receipt = Staging::open(&path)
                .unwrap()
                .knowledge_receipt(
                    &KnowledgeManifest {
                        schema: 2,
                        project: project.id().clone(),
                        change,
                        assertion: assertion.clone(),
                        evidence_roots: vec![],
                    },
                    &mut || Ok(()),
                )
                .unwrap();
            run.state = RunState::Running;
            writer.update_run(&run).unwrap();
            run.state = RunState::Validating;
            writer.update_run(&run).unwrap();
            let retained = writer
                .retain_knowledge(&run, &receipt, &mut || Ok(()))
                .unwrap();
            writer
                .publish_knowledge(&mut run, retained, &mut || Ok(()))
                .unwrap();
            base = Some(receipt.revision);
        }
        let selected = EffectReview {
            knowledge: base.unwrap(),
            assertion,
        };
        let target = ExecutionTarget {
            revision: endpoint.occurrence.revision.clone(),
            source: endpoint.occurrence.source.clone(),
            companions: vec![],
            abi: CallAbi::RiscvInteger,
            stack: MemorySeed {
                address: 0x8000,
                length: 4096,
                fill: None,
                bytes: vec![],
            },
        };
        let right = input.clone();
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
                vendor: input,
                replacement: Some(right),
                relation: Some(ComparisonRelation {
                    effects: Some(selected.clone()),
                    projection: None,
                    calls: false,
                    reviewed_calls: None,
                    returns: ReturnWords {
                        low: false,
                        high: false,
                    },
                    events: EventChannels {
                        timeline: TimelineCapture::default(),
                        mmio_read: true,
                        mmio_write: true,
                        fence: true,
                        delay: true,
                    },
                    memory: vec![],
                }),
            }],
        };
        let memory = WorkingMemory::new(4 * 1024 * 1024).unwrap();
        for _ in 0..3 {
            let value = project
                .execution_effects(&request, &memory, &mut || Ok(()))
                .unwrap();
            assert_eq!(
                value.contracts,
                vec![ResolvedEffectContract {
                    review: selected.clone(),
                    contract: contract.clone()
                }]
            );
            assert!(memory.used() > 0);
            drop(value);
            assert_eq!(memory.used(), 0);
        }
        let small = WorkingMemory::new(1).unwrap();
        assert!(matches!(
            project.execution_effects(&request, &small, &mut || Ok(())),
            Err(Error {
                code: ErrorCode::ResourceLimited,
                ..
            })
        ));
        assert_eq!(small.used(), 0);
        let manifest = ExecutionManifest {
            projections: vec![],
            schema: EXECUTION_SCHEMA,
            project: project.id().clone(),
            request: ArtifactId::of_bytes(b"request"),
            call_pairs: vec![],
            effect_contracts: vec![ResolvedEffectContract {
                review: selected,
                contract,
            }],
            records: id,
            complete: true,
            verdict: Some(ComparisonVerdict::Match),
            producer: ExecutionProducer {
                executor: "test".into(),
                environment: "test".into(),
                verifier: "test".into(),
            },
        };
        let mut manifest = TestExecution::new(manifest, request);
        project
            .validate_execution_effects(&manifest, &manifest.request, &memory, &mut || Ok(()))
            .unwrap();
        check_accounting(&manifest);

        manifest.effect_contracts[0].contract.rules[0].max_occurrences = 2;
        assert_eq!(
            project
                .validate_execution_effects(&manifest, &manifest.request, &memory, &mut || Ok(()))
                .unwrap_err()
                .code,
            ErrorCode::Integrity
        );
        manifest.effect_contracts.clear();
        assert_eq!(
            project
                .validate_execution_effects(&manifest, &manifest.request, &memory, &mut || Ok(()))
                .unwrap_err()
                .code,
            ErrorCode::Integrity
        );
        assert_eq!(memory.used(), 0);
    }
    fn check_accounting(manifest: &TestExecution) {
        let make = |events: Vec<ExecutionEvent>, result: CaseComparison| {
            let mut rows = Vec::new();
            for replacement in [false, true] {
                for event in &events {
                    rows.push(ExecutionEvidence::Event {
                        case: 0,
                        replacement,
                        event: event.clone(),
                    });
                }
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
            rows.push(ExecutionEvidence::Comparison { case: 0, result });
            rows
        };
        let check = |manifest: &TestExecution, rows: &[ExecutionEvidence]| {
            let mut bytes = Vec::new();
            for row in rows
                .iter()
                .chain(&crate::executions::coverage_rows(&manifest.request))
            {
                serde_json::to_writer(&mut bytes, row).unwrap();
                bytes.push(b'\n');
            }
            manifest.validate_records(
                &bytes.as_slice(),
                &WorkingMemory::new(1024 * 1024).unwrap(),
                &mut || Ok(()),
            )
        };
        let result = CaseComparison {
            effect_claim: Some(EffectClaimCeiling::SelectedEffectEquality),
            effect_gap: None,
            verdict: ComparisonVerdict::Match,
            difference: None,
        };
        let event = ExecutionEvent::Write {
            address: 0x3000,
            width: 4,
            value: 7,
        };
        let rows = make(vec![event.clone()], result.clone());
        check(manifest, &rows).unwrap();
        let mut forged = result.clone();
        forged.effect_claim = None;
        assert!(check(manifest, &make(vec![event.clone()], forged)).is_err());
        assert!(check(manifest, &make(vec![], result.clone())).is_err());
        let mut incomplete = manifest.clone();
        incomplete.verdict = Some(ComparisonVerdict::Incomplete);
        let mut missing = result.clone();
        missing.verdict = ComparisonVerdict::Incomplete;
        missing.effect_gap = Some(EffectGap::Unexercised {
            replacement: false,
            rule: 0,
        });
        check(&incomplete, &make(vec![], missing.clone())).unwrap();
        missing.effect_gap = None;
        assert!(check(&incomplete, &make(vec![], missing)).is_err());
        let mut unknown = result.clone();
        unknown.verdict = ComparisonVerdict::Incomplete;
        unknown.effect_gap = Some(EffectGap::Unclassified {
            replacement: false,
            event: 1,
        });
        let events = vec![
            event.clone(),
            ExecutionEvent::Fence {
                predecessor: 3,
                successor: 3,
            },
        ];
        check(&incomplete, &make(events.clone(), unknown.clone())).unwrap();
        unknown.effect_gap = Some(EffectGap::Unclassified {
            replacement: false,
            event: 0,
        });
        assert!(check(&incomplete, &make(events.clone(), unknown)).is_err());
        assert!(check(manifest, &make(events, result.clone())).is_err());
        let mut strict = manifest.clone();
        strict.effect_contracts[0].contract.rules[0]
            .vendor
            .as_mut()
            .unwrap()
            .value = EffectValue::Exact { value: 9 };
        strict.effect_contracts[0].contract.rules[0].replacement =
            strict.effect_contracts[0].contract.rules[0].vendor;
        strict.verdict = Some(ComparisonVerdict::Diff);
        let mut difference = result.clone();
        difference.verdict = ComparisonVerdict::Diff;
        difference.difference = Some(ComparisonDifference::EffectViolation {
            violation: EffectViolation {
                replacement: false,
                event: 0,
                rule: 0,
                kind: EffectViolationKind::Value,
            },
        });
        check(&strict, &make(vec![event.clone()], difference.clone())).unwrap();
        if let Some(ComparisonDifference::EffectViolation { violation }) =
            &mut difference.difference
        {
            violation.event = 1;
        }
        assert!(check(&strict, &make(vec![event.clone()], difference)).is_err());
        // Every case gets fresh exercise accounting, including a warm continuation.
        let mut phases = incomplete.clone();
        let mut second = phases.request.cases[0].clone();
        second.name = "second".into();
        second.reset = SessionReset::Warm;
        phases.request.cases.push(second);
        let mut rows = rows;
        let missing = CaseComparison {
            effect_claim: result.effect_claim,
            effect_gap: Some(EffectGap::Unexercised {
                replacement: false,
                rule: 0,
            }),
            verdict: ComparisonVerdict::Incomplete,
            difference: None,
        };
        for mut row in make(vec![], missing) {
            match &mut row {
                ExecutionEvidence::Outcome { case, .. }
                | ExecutionEvidence::Comparison { case, .. } => *case = 1,
                _ => unreachable!(),
            };
            rows.push(row);
        }
        check(&phases, &rows).unwrap();
        phases.verdict = Some(ComparisonVerdict::Match);
        if let ExecutionEvidence::Comparison { result: r, .. } = rows.last_mut().unwrap() {
            *r = result;
        }
        assert!(check(&phases, &rows).is_err());
    }
}
