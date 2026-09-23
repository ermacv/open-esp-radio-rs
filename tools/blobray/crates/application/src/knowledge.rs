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
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<Vec<ArtifactId>> {
    blobray_knowledge::validate_proposal(proposal)?;
    let occurrence = &proposal.occurrence;
    let payload = match &occurrence.source {
        FunctionSource::Input { input } => {
            let scope = match &occurrence.symbol {
                Some(symbol) => InspectionScope::Symbol {
                    input: *input,
                    symbol: symbol.clone(),
                },
                None => InspectionScope::Object {
                    input: *input,
                    object: occurrence.object.clone(),
                },
            };
            let mut probe = crate::selection::Probe::new(&scope);
            project.read_inventory(Some(&occurrence.revision), memory, control, &mut probe)?;
            if !probe.found {
                return Err(Error::new(
                    ErrorCode::NotFound,
                    "knowledge occurrence absent from revision",
                ));
            }
            probe
                .binding
                .and_then(|b| b.payload)
                .ok_or_else(|| invalid("knowledge object bytes unavailable"))?
        }
        FunctionSource::Image { image } => {
            let lease = project.image(image, control)?;
            if lease.manifest.plan.recipe.revision != occurrence.revision
                || occurrence.object
                    != (ObjectId {
                        artifact: lease.manifest.elf.clone(),
                        location: ObjectLocation::Standalone,
                    })
            {
                return Err(invalid("knowledge image and occurrence differ"));
            }
            if let Some(symbol) = &occurrence.symbol {
                let request = FunctionRequest {
                    research: None,
                    revision: Some(occurrence.revision.clone()),
                    source: occurrence.source.clone(),
                    symbol: symbol.clone(),
                    extent: match proposal.claim {
                        KnowledgeClaim::FunctionExtent { extent } => Some(extent),
                        _ => None,
                    },
                };
                blobray_artifacts::with_function(
                    &lease.elf,
                    &lease.manifest.elf,
                    &request,
                    memory,
                    control,
                    |_, _| Ok(()),
                )?;
            }
            lease.manifest.elf
        }
    };
    let mut roots = vec![
        occurrence.revision.as_str().parse()?,
        occurrence.object.artifact.clone(),
    ];
    if let FunctionSource::Image { image } = &occurrence.source {
        roots.push(image.as_str().parse()?);
    }
    let container = project.open_payload(&occurrence.object.artifact, control)?;
    let check_source = |source: &dyn ByteSource, control: &mut dyn RunControl| -> Result<()> {
        for evidence in &proposal.evidence {
            if let EvidenceRef::Source {
                payload: expected,
                range,
            } = evidence
                && (expected != &payload
                    || range
                        .start
                        .checked_add(range.length)
                        .is_none_or(|end| end > source.len()))
            {
                return Err(invalid(
                    "source evidence does not address the selected captured object",
                ));
            }
        }
        if let KnowledgeClaim::FunctionExtent { extent } = proposal.claim {
            let request = FunctionRequest {
                research: None,
                revision: Some(occurrence.revision.clone()),
                source: occurrence.source.clone(),
                symbol: occurrence
                    .symbol
                    .clone()
                    .ok_or_else(|| invalid("boundary requires symbol"))?,
                extent: Some(extent),
            };
            blobray_artifacts::with_function(
                source,
                &payload,
                &request,
                memory,
                control,
                |_, _| Ok(()),
            )?;
        }
        Ok(())
    };
    match occurrence.object.location {
        ObjectLocation::Standalone => check_source(&container, control)?,
        ObjectLocation::ArchiveMember { ordinal } => {
            let mut cursor = MemberCursor::new(&container, control)?;
            let mut found = false;
            while let Some(member) = cursor.next(memory, control)? {
                if member.ordinal != ordinal {
                    continue;
                }
                if let Some((offset, length)) = member.payload {
                    check_source(&SourceRange::new(&container, offset, length)?, control)?;
                } else {
                    check_source(&project.open_payload(&payload, control)?, control)?;
                    roots.push(payload.clone());
                }
                found = true;
                break;
            }
            if !found {
                return Err(Error::new(
                    ErrorCode::Integrity,
                    "captured archive member disappeared",
                ));
            }
        }
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
                    || recipe.symbol.object != occurrence.object
                    || occurrence
                        .symbol
                        .as_ref()
                        .is_some_and(|s| s != &recipe.symbol)
                    || recipe.payload != payload
                {
                    return Err(invalid("analysis evidence belongs to another occurrence"));
                }
                let mut count = 0u64;
                blobray_store::visit_jsonl::<FunctionRecord>(&lease.records, control, |_, c| {
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
                                    && request.symbol.object == occurrence.object
                                    && occurrence
                                        .symbol
                                        .as_ref()
                                        .is_none_or(|s| s == &request.symbol)
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
    let result = prepare_with(stage, work, &memory, &disk, &mut control);
    control.working_memory(memory.observation());
    result
}

pub(crate) fn prepare_with(
    stage: &Path,
    work: &KnowledgeWork,
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
                (id, validate_evidence(&project, proposal, memory, control)?)
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
                let mut roots = validate_evidence(&project, &target.proposal, memory, control)?;
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
