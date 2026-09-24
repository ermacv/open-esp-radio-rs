//! Review admission uses exact retained occurrences and evidence, never display names.
use crate::*;
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeWork {
    pub schema: u32,
    pub run: RunId,
    pub project: OriginPath,
    pub change: KnowledgeChange,
    pub budget: ResourceBudget,
    pub started_ms: u64,
    pub deadline_ms: u64,
}
fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, message)
}
fn validate_evidence(
    project: &Project,
    proposal: &KnowledgeProposal,
    decoder: &dyn FunctionDecoder,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<Vec<ArtifactId>> {
    blobray_knowledge::validate_proposal(proposal)?;
    let occurrence = &proposal.occurrence;
    let mut roots = vec![
        occurrence.revision.as_str().parse()?,
        occurrence.object.artifact.clone(),
    ];
    if let FunctionSource::Image { image } = &occurrence.source {
        roots.push(image.as_str().parse()?);
    }
    let payload =
        crate::occurrence::with_source(project, occurrence, memory, control, |capture, c| {
            for evidence in &proposal.evidence {
                if let EvidenceRef::Source { payload, range } = evidence
                    && (payload != capture.payload
                        || range
                            .start
                            .checked_add(range.length)
                            .is_none_or(|end| end > capture.bytes.len()))
                {
                    return Err(invalid(
                        "source evidence does not address the selected captured object",
                    ));
                }
            }
            let extent = match proposal.claim {
                KnowledgeClaim::FunctionExtent { extent } => Some(extent),
                _ => None,
            };
            if let KnowledgeClaim::EffectContract { contract } = &proposal.claim {
                crate::call_pairs::endpoint(&capture, &contract.vendor, memory, c)?;
            } else if let KnowledgeClaim::LayoutProjection { projection } = &proposal.claim {
                crate::layout_projections::endpoint(
                    &capture, projection, false, decoder, memory, c,
                )?;
            } else if let KnowledgeClaim::CallPair { correspondence } = &proposal.claim {
                crate::call_pairs::endpoint(&capture, &correspondence.vendor, memory, c)?;
            } else if let KnowledgeClaim::Function { contract } = &proposal.claim {
                let request = FunctionRequest {
                    research: None,
                    revision: Some(occurrence.revision.clone()),
                    source: occurrence.source.clone(),
                    selector: contract.selector.clone(),
                    extent: None,
                };
                capture.with_prepared(memory, c, |object, c| {
                    object.with_function(&request, c, |_, _| Ok(()))
                })?;
            } else if let KnowledgeClaim::Interface { contract } = &proposal.claim {
                capture.with_prepared(memory, c, |object, c| {
                    for guard in &contract.guards {
                        c.checkpoint(1)?;
                        if let InterfaceGuard::CapturedPayload { payload } = guard
                            && payload != capture.payload
                        {
                            return Err(invalid(
                                "interface payload guard differs from captured object bytes",
                            ));
                        }
                    }
                    match &contract.root {
                        AccessRoot::Symbol { symbol, .. } => {
                            object.validate_data_symbol(&occurrence.object, symbol)?
                        }
                        AccessRoot::EntryWord { function, .. } => {
                            let request = FunctionRequest {
                                research: None,
                                revision: Some(occurrence.revision.clone()),
                                source: occurrence.source.clone(),
                                selector: function.clone(),
                                extent: None,
                            };
                            object.with_function(&request, c, |_, _| Ok(()))?;
                        }
                        AccessRoot::Section { section, offset } => {
                            object.validate_section_root(*section, *offset)?
                        }
                        AccessRoot::Address { .. } => (),
                    }
                    Ok(())
                })?;
            } else if let KnowledgeClaim::IntegerTable { selector, .. }
            | KnowledgeClaim::PointerTable { selector, .. } = &proposal.claim
            {
                capture.with_prepared(memory, c, |object, c| {
                    object.with_data(&occurrence.object, selector, c, |view, _| {
                        crate::data::validate_table(capture.payload, &view, proposal)
                    })
                })?;
            } else if let KnowledgeClaim::ExecutableRange { section, extent } = proposal.claim {
                let request = FunctionRequest {
                    research: None,
                    revision: Some(occurrence.revision.clone()),
                    source: occurrence.source.clone(),
                    extent: None,
                    selector: FunctionSelector::Range {
                        object: occurrence.object.clone(),
                        section,
                        extent,
                    },
                };
                capture.with_prepared(memory, c, |object, c| {
                    object.with_function(&request, c, |_, _| Ok(()))
                })?;
            } else if extent.is_some()
                || matches!(occurrence.source, FunctionSource::Image { .. })
                    && occurrence.symbol.is_some()
            {
                let request = FunctionRequest {
                    research: None,
                    revision: Some(occurrence.revision.clone()),
                    source: occurrence.source.clone(),
                    selector: (occurrence
                        .symbol
                        .clone()
                        .ok_or_else(|| invalid("boundary requires symbol"))?)
                    .into(),
                    extent,
                };
                capture.with_prepared(memory, c, |object, c| {
                    object.with_function(&request, c, |_, _| Ok(()))
                })?;
            }
            if capture.detached {
                roots.push(capture.payload.clone());
            }
            Ok(capture.payload.clone())
        })?;
    if let KnowledgeClaim::EffectContract { contract } = &proposal.claim {
        roots.extend(crate::call_pairs::secondary(
            project,
            &contract.replacement,
            memory,
            control,
        )?);
    }
    if let KnowledgeClaim::LayoutProjection { projection } = &proposal.claim {
        roots.extend(crate::layout_projections::secondary(
            project, projection, decoder, memory, control,
        )?);
    }
    if let KnowledgeClaim::CallPair { correspondence } = &proposal.claim {
        roots.extend(crate::call_pairs::secondary(
            project,
            &correspondence.replacement,
            memory,
            control,
        )?);
    }
    if let KnowledgeClaim::Path { path } = &proposal.claim {
        roots.extend(crate::flow::validate_path(
            project, proposal, path, memory, control,
        )?);
    }
    if let KnowledgeClaim::EventRoute { route } = &proposal.claim {
        roots.extend(crate::event_routes::validate(
            project, proposal, route, memory, control,
        )?);
    }
    for evidence in &proposal.evidence {
        control.checkpoint(1)?;
        match evidence {
            EvidenceRef::Source { .. } => (),
            EvidenceRef::Document { payload } => {
                project.open_payload(payload, control)?;
                roots.push(payload.clone());
            }
            EvidenceRef::Analysis { analysis, record } => {
                let lease = project.analysis(analysis, control)?;
                let recipe = &lease.manifest.recipe;
                if recipe.revision != occurrence.revision
                    || recipe.source != occurrence.source
                    || *recipe.selector.object() != occurrence.object
                    || occurrence
                        .symbol
                        .as_ref()
                        .is_some_and(|s| Some(s) != recipe.selector.symbol())
                    || recipe.payload != payload
                {
                    return Err(invalid("analysis evidence belongs to another occurrence"));
                }
                let mut count = 0u64;
                blobray_store::visit_jsonl::<FunctionRecord>(&lease.records, control, |r, c| {
                    crate::data::validate_constant(proposal, analysis, count, &r)?;
                    c.checkpoint(1)?;
                    count += 1;
                    Ok(())
                })?;
                if record.is_some_and(|n| n >= count) {
                    return Err(invalid("analysis evidence record does not exist"));
                }
                roots.push(analysis.as_str().parse()?);
                roots.push(lease.manifest.records);
            }
            EvidenceRef::Publication { publication } => {
                let lease = project.publication(publication, control)?;
                if lease.manifest.plan.recipe.request.revision.as_ref()
                    != Some(&occurrence.revision)
                {
                    return Err(invalid("publication evidence belongs to another revision"));
                }
                let mut found = false;
                blobray_store::visit_jsonl::<InvestigationMember>(
                    &lease.members,
                    control,
                    |member, c| {
                        c.checkpoint(1)?;
                        let matches = match &member.entry {
                            PlanEntry::Object { input, object, .. } => {
                                occurrence.symbol.is_none()
                                    && Some(*input) == occurrence.source.input()
                                    && object == &occurrence.object
                            }
                            PlanEntry::Image { image, payload, .. } => {
                                occurrence.symbol.is_none()
                                    && occurrence.source
                                        == (FunctionSource::Image {
                                            image: image.clone(),
                                        })
                                    && payload == &occurrence.object.artifact
                            }
                            PlanEntry::Function { request, .. } => {
                                request.source == occurrence.source
                                    && *request.selector.object() == occurrence.object
                                    && occurrence
                                        .symbol
                                        .as_ref()
                                        .is_none_or(|s| Some(s) == request.selector.symbol())
                            }
                            _ => false,
                        };
                        found |= matches;
                        Ok(())
                    },
                )?;
                if !found {
                    return Err(invalid(
                        "publication does not contain the selected occurrence",
                    ));
                }
                roots.push(publication.as_str().parse()?);
                roots.push(lease.manifest.members);
            }
        }
    }
    roots.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    roots.dedup();
    Ok(roots)
}
pub fn prepare_knowledge_worker(
    stage: &Path,
    work: &KnowledgeWork,
    decoder: &dyn FunctionDecoder,
    control: &mut dyn RunControl,
) -> Result<PreparedKnowledgeReceipt> {
    if work.schema != 1 {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "unsupported knowledge worker request",
        ));
    }
    let memory = WorkingMemory::new(
        work.budget
            .working_memory_bytes
            .ok_or_else(|| invalid("working capacity missing"))?,
    )?;
    let disk = blobray_store::TemporaryBudget::open(stage)?;
    let mut control = blobray_store::TemporaryControl {
        control,
        budget: &disk,
    };
    let result = prepare_with(stage, work, decoder, &memory, &disk, &mut control);
    control.memory_phases(&memory.phase_observations());
    control.working_memory(memory.observation());
    result
}

pub(crate) fn prepare_with(
    stage: &Path,
    work: &KnowledgeWork,
    decoder: &dyn FunctionDecoder,
    memory: &WorkingMemory,
    disk: &blobray_store::TemporaryBudget,
    control: &mut dyn RunControl,
) -> Result<PreparedKnowledgeReceipt> {
    (|| {
        let _fixed = memory.reserve(2 * 1024 * 1024, control.position())?;
        blobray_knowledge::validate_change(&work.change)?;
        let project = Project::open(&work.project.to_path()?)?;
        project.check_knowledge_base(&work.change.expected_base)?;
        let (assertion, roots) = match &work.change.action {
            KnowledgeAction::Propose { proposal } => {
                // Include the explicit base and attribution so a later reproposal has a new identity.
                let encoded =
                    serde_json::to_vec(&work.change).map_err(|e| invalid(&e.to_string()))?;
                let id = ArtifactId::of_bytes(&encoded).as_str().parse()?;
                (
                    id,
                    validate_evidence(&project, proposal, decoder, memory, control)?,
                )
            }
            KnowledgeAction::Review {
                assertion,
                decision,
                supersedes,
            } => {
                let base = work
                    .change
                    .expected_base
                    .as_ref()
                    .ok_or_else(|| invalid("review requires an existing knowledge revision"))?;
                let target = project.knowledge_entry(base, assertion, control)?;
                let replaced = supersedes
                    .as_ref()
                    .map(|id| project.knowledge_entry(base, id, control))
                    .transpose()?;
                blobray_knowledge::validate_review(&target, *decision, replaced.as_ref())?;
                let mut roots =
                    validate_evidence(&project, &target.proposal, decoder, memory, control)?;
                if *decision == ReviewDecision::Accept {
                    project.knowledge_entries(Some(base),control,&mut |entry,_|{
                        if entry.state==AssertionState::Accepted && Some(&entry.id)!=supersedes.as_ref() && blobray_knowledge::conflicts(&target.proposal,&entry.proposal) {return Err(Error::new(ErrorCode::Conflict,"acceptance conflicts with an existing assertion; select it explicitly for supersession"));}Ok(())
                    })?;
                }
                roots.push(target.proposed_in.as_str().parse()?);
                (assertion.clone(), roots)
            }
        };
        let manifest = KnowledgeManifest {
            schema: 2,
            project: project.id().clone(),
            change: work.change.clone(),
            assertion,
            evidence_roots: roots,
        };
        Staging::with_temporary_budget(stage, disk.clone())?.knowledge_receipt(&manifest, control)
    })()
}
