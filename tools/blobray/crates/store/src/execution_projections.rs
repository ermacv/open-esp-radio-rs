//! Frozen accepted projection selection with operation-owned admitted copies.
use super::*;
pub struct ExecutionProjections<'m> {
    pub projections: Vec<ResolvedProjection>,
    _payloads: AdmittedVec<'m, MemoryReservation<'m>>,
    _capacity: MemoryReservation<'m>,
}
impl Project {
    pub fn execution_projections<'m>(
        &self,
        request: &ExecutionRequest,
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<ExecutionProjections<'m>> {
        let mut reviews = AdmittedVec::new(memory);
        for case in &request.cases {
            if let Some(selection) = case.relation.as_ref().and_then(|r| r.projection.as_ref()) {
                let ProjectionRef::Reviewed(review) = selection else {
                    return Err(Error::new(
                        ErrorCode::InvalidRequest,
                        "a content-identified projection is supplied with an in-process comparison",
                    ));
                };
                c.checkpoint(reviews.len() as u64 + 1)?;
                if !reviews.contains(&review) {
                    if reviews.len() == MAX_LAYOUT_FIELDS {
                        return Err(Error::new(
                            ErrorCode::ResourceLimited,
                            "projection review capacity exceeded",
                        ));
                    }
                    reviews.push(review, c.position())?;
                }
            }
        }
        let capacity = memory.reserve(
            (reviews.len() * std::mem::size_of::<ResolvedProjection>()) as u64,
            c.position(),
        )?;
        let mut projections = Vec::new();
        projections
            .try_reserve_exact(reviews.len())
            .map_err(|_| Error::new(ErrorCode::ResourceLimited, "projection allocation refused"))?;
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
                    .ok_or_else(|| integrity("selected projection assertion missing"))?;
                let KnowledgeClaim::LayoutProjection { projection } = &entry.proposal.claim else {
                    return Err(Error::new(
                        ErrorCode::InvalidRequest,
                        "selected assertion is not a layout projection",
                    ));
                };
                if entry.state != AssertionState::Accepted
                    || entry.proposal.occurrence != projection.vendor.entry.occurrence
                {
                    return Err(Error::new(
                        ErrorCode::InvalidRequest,
                        "projection requires a selected accepted review",
                    ));
                }
                c.checkpoint(
                    (projection.fields.len() + projection.branches.len() + 1).pow(2) as u64,
                )?;
                projection.validate()?;
                payloads.push(
                    memory.reserve(
                        projection.allocated_bytes()
                            + selected.knowledge.allocated_bytes()
                            + selected.assertion.allocated_bytes(),
                        c.position(),
                    )?,
                    c.position(),
                )?;
                projections.push(ResolvedProjection {
                    review: ProjectionRef::Reviewed((*selected).clone()),
                    projection: (**projection).clone(),
                });
            }
        }
        for (phase, case) in request.cases.iter().enumerate() {
            c.checkpoint(projections.len() as u64 + 1)?;
            if let Some(p) = selected_projection(case.relation.as_ref(), &projections)? {
                c.checkpoint(
                    (p.projection.fields.len() + p.projection.branches.len() + 1).pow(2) as u64,
                )?;
                p.projection.validate_use(request, phase)?;
            }
        }
        Ok(ExecutionProjections {
            projections,
            _payloads: payloads,
            _capacity: capacity,
        })
    }
    pub(super) fn validate_execution_projections(
        &self,
        manifest: &ExecutionManifest,
        request: &ExecutionRequest,
        memory: &WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        let selected = self.execution_projections(request, memory, c)?;
        if selected.projections != manifest.projections {
            return Err(integrity(
                "retained projections differ from selected accepted reviews",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_projection_payloads_release_capacity_and_retained_policy_cannot_change() {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::create(dir.path()).unwrap();
        let id = ArtifactId::of_bytes(b"projection fixture");
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
        let projection = LayoutProjection {
            vendor: LayoutEndpoint {
                entry: endpoint.clone(),
                domains: vec![LayoutDomain {
                    address: 0x3000,
                    length: 16,
                }],
            },
            replacement: LayoutEndpoint {
                entry: endpoint.clone(),
                domains: vec![LayoutDomain {
                    address: 0x4000,
                    length: 16,
                }],
            },
            fields: vec![LayoutField {
                count: 1,
                name: "field".into(),
                vendor: FieldLocation {
                    domain: 0,
                    offset: 4,
                },
                replacement: FieldLocation {
                    domain: 0,
                    offset: 8,
                },
                width: 4,
                final_state: true,
                timeline: false,
            }],
            branches: vec![],
            applicability: "test".into(),
            reason: "test".into(),
        };
        // A directly constructed projection is useful for structural byte-mask admission too.
        let input = |address| Invocation {
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
            observe_memory: vec![MemorySelection {
                name: "full domain".into(),
                address,
                length: 16,
            }],
        };
        let mut memory_state = crate::execution_observation::MemoryState::new();
        let input = input(0x3000);
        memory_state
            .begin_projection(&projection, &input, false, &mut || Ok(()))
            .unwrap();
        let chunk = FinalMemoryChunk {
            selection: 0,
            offset: 0,
            length: 16,
            bytes: [0; 16],
            available: u16::MAX,
            known: 0xf0,
        };
        memory_state.chunk(&input, &chunk, &mut || Ok(())).unwrap();
        memory_state.finish(&input, false).unwrap();
        assert!(memory_state.known); // unknown padding is explicitly outside the selected field
        let mut missing = crate::execution_observation::MemoryState::new();
        missing
            .begin_projection(&projection, &input, false, &mut || Ok(()))
            .unwrap();
        missing
            .chunk(
                &input,
                &FinalMemoryChunk {
                    known: 0xe0,
                    ..chunk
                },
                &mut || Ok(()),
            )
            .unwrap();
        assert!(!missing.known);
        let assertion: AssertionId = ArtifactId::of_bytes(b"review").as_str().parse().unwrap();
        let mut base = None;
        for action in [
            KnowledgeAction::Propose {
                proposal: KnowledgeProposal {
                    subject: "fixture".to_owned().try_into().unwrap(),
                    occurrence: endpoint.occurrence.clone(),
                    claim: KnowledgeClaim::LayoutProjection {
                        projection: Box::new(projection.clone()),
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
        let selected = ProjectionReview {
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
        let mut right = input.clone();
        right.observe_memory[0].address = 0x4000;
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
                    effects: None,
                    projection: Some(ProjectionRef::Reviewed(selected.clone())),
                    calls: false,
                    reviewed_calls: None,
                    returns: ReturnWords {
                        low: false,
                        high: false,
                    },
                    events: EventChannels {
                        timeline: TimelineCapture::default(),
                        mmio_read: false,
                        mmio_write: false,
                        fence: false,
                        delay: false,
                    },
                    memory: vec![],
                }),
            }],
        };
        let memory = WorkingMemory::new(4 * 1024 * 1024).unwrap();
        for _ in 0..3 {
            let value = project
                .execution_projections(&request, &memory, &mut || Ok(()))
                .unwrap();
            assert_eq!(
                value.projections,
                vec![ResolvedProjection {
                    review: ProjectionRef::Reviewed(selected.clone()),
                    projection: projection.clone()
                }]
            );
            assert!(memory.used() > 0);
            drop(value);
            assert_eq!(memory.used(), 0);
        }
        let small = WorkingMemory::new(1).unwrap();
        assert!(matches!(
            project.execution_projections(&request, &small, &mut || Ok(())),
            Err(Error {
                code: ErrorCode::ResourceLimited,
                ..
            })
        ));
        assert_eq!(small.used(), 0);
        let mut manifest = ExecutionManifest {
            effect_contracts: vec![],
            schema: EXECUTION_SCHEMA,
            project: project.id().clone(),
            request: encode_execution_request(&request).unwrap().0,
            call_pairs: vec![],
            projections: vec![ResolvedProjection {
                review: ProjectionRef::Reviewed(selected),
                projection,
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
        project
            .validate_execution_projections(&manifest, &request, &memory, &mut || Ok(()))
            .unwrap();
        manifest.projections[0].projection.fields[0].width = 2;
        assert_eq!(
            project
                .validate_execution_projections(&manifest, &request, &memory, &mut || Ok(()))
                .unwrap_err()
                .code,
            ErrorCode::Integrity
        );
        manifest.projections.clear();
        assert_eq!(
            project
                .validate_execution_projections(&manifest, &request, &memory, &mut || Ok(()))
                .unwrap_err()
                .code,
            ErrorCode::Integrity
        );
        assert_eq!(memory.used(), 0);
    }
}
