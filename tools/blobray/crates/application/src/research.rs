//! Image-qualified call closure. Owns retained readers and bounded composition;
//! analysis receives values only, never a project or reader capability.
use crate::*;
use std::io::Write;
struct Node<'a> {
    id: FunctionAnalysisId,
    recipe: FunctionRecipe,
    records: Vec<FunctionRecord>,
    calls: Vec<(u64, Option<usize>)>,
    done: bool,
    _capacity: MemoryReservation<'a>,
    composed: Option<blobray_analysis::summaries::Composed<'a>>,
}
impl Node<'_> {
    fn facts(&self) -> &[FunctionRecord] {
        self.composed.as_ref().map_or(&self.records, |c| &c.records)
    }
}
fn load<'a>(
    id: FunctionAnalysisId,
    manifest: FunctionManifest,
    source: &dyn ByteSource,
    memory: &'a WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<Node<'a>> {
    let capacity = memory.reserve(
        source
            .len()
            .checked_mul(32)
            .and_then(|n| n.checked_add(65536))
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "research capacity overflow"))?,
        c.position(),
    )?;
    let mut records = Vec::new();
    blobray_store::visit_jsonl(source, c, |r, c| {
        c.checkpoint(1)?;
        records.try_reserve(1).map_err(|_| {
            Error::new(
                ErrorCode::ResourceLimited,
                "research records allocation refused",
            )
        })?;
        records.push(r);
        Ok(())
    })?;
    Ok(Node {
        id,
        recipe: manifest.recipe,
        records,
        calls: Vec::new(),
        done: false,
        _capacity: capacity,
        composed: None,
    })
}
pub(crate) fn enrich(
    project: &Project,
    staging: &Staging,
    manifest: &mut FunctionManifest,
    memory: &WorkingMemory,
    disk: &blobray_store::TemporaryBudget,
    directory: &Path,
    c: &mut dyn RunControl,
) -> Result<()> {
    let Some(options) = manifest.recipe.research.clone() else {
        return Ok(());
    };
    if options.companions.len() > 64 {
        return Err(Error::new(
            ErrorCode::ResourceLimited,
            "too many companion publications",
        ));
    }
    // Each retained plan has a 64 KiB encoded cap; admit its decoded metadata
    // and read-validation workspace before opening any dependency.
    let _dependencies = memory.reserve(
        (options.companions.len() as u64 + 1) * 4 * 1024 * 1024,
        c.position(),
    )?;
    let mut publications = vec![project.publication(&options.publication, c)?];
    for id in &options.companions {
        let lease = project.publication(id, c)?;
        if lease.manifest.plan.recipe.request.revision.as_ref() != Some(&manifest.recipe.revision) {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "companion publication belongs to another revision",
            ));
        }
        publications.push(lease);
    }
    let publication = &publications[0];
    if publication.manifest.plan.recipe.request.revision.as_ref() != Some(&manifest.recipe.revision)
    {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "research publication belongs to another revision",
        ));
    }
    let _index_capacity = memory.reserve(1024 * 1024, c.position())?;
    let mut root = None;
    blobray_store::visit_jsonl(&publication.members, c, |member: InvestigationMember, _| {
        if let PlanEntry::Function { request, .. } = &member.entry
            && request.source == manifest.recipe.source
            && request.symbol == manifest.recipe.symbol
            && let InvestigationOutcome::Analyzed { analysis, .. } = member.outcome
        {
            root = Some(analysis);
        }
        Ok(())
    })?;
    let root = root.ok_or_else(|| {
        Error::new(
            ErrorCode::InvalidRequest,
            "research root is absent from publication",
        )
    })?;
    let source = staging.open_payload(&manifest.records, c)?;
    let mut nodes = vec![load(root, manifest.clone(), &source, memory, c)?];
    let mut cursor = 0;
    while cursor < nodes.len() {
        c.checkpoint(1)?;
        let transfers: Vec<_> = nodes[cursor]
            .records
            .iter()
            .filter_map(|r| {
                if let FunctionRecord::Transfer {
                    offset,
                    target,
                    call: true,
                } = r
                {
                    Some((*offset, target.clone()))
                } else {
                    None
                }
            })
            .collect();
        for (offset, target) in transfers {
            c.checkpoint(1)?;
            let mut candidate = None;
            let mut matches = 0;
            if options.abi.is_some()
                && let AbstractValue::ImageAddress { address } = target
            {
                let external = if let FunctionSource::Image { image } = &nodes[cursor].recipe.source
                {
                    project.image(image, c)?.manifest.plan.recipe.companions
                } else {
                    Vec::new()
                };
                for publication in &publications {
                    blobray_store::visit_jsonl(
                        &publication.members,
                        c,
                        |member: InvestigationMember, c| {
                            c.checkpoint(1)?;
                            if let PlanEntry::Function {
                                request,
                                declared_extent,
                                address_space: CodeAddressSpace::Image,
                                ..
                            } = &member.entry
                                && ((request.source == nodes[cursor].recipe.source
                                    && request.symbol.object == nodes[cursor].recipe.symbol.object)
                                    || external.iter().any(|e| {
                                        request.source.input() == Some(e.input)
                                            && request.symbol == e.symbol
                                    }))
                                && declared_extent.start == u64::from(address)
                            {
                                if let InvestigationOutcome::Analyzed { analysis, .. } =
                                    member.outcome
                                {
                                    if candidate.as_ref() != Some(&analysis) {
                                        matches += 1;
                                        candidate = Some(analysis);
                                    }
                                } else {
                                    matches += 1;
                                }
                            }
                            Ok(())
                        },
                    )?;
                }
            }
            let resolved = if matches == 1
                && let Some(id) = candidate
            {
                if let Some(i) = nodes.iter().position(|n| n.id == id) {
                    Some(i)
                } else {
                    if nodes.len() >= 1024 {
                        return Err(Error::new(
                            ErrorCode::ResourceLimited,
                            "research closure exceeds 1024 functions",
                        ));
                    }
                    let lease = project.analysis(&id, c)?;
                    nodes.push(load(id, lease.manifest, &lease.records, memory, c)?);
                    Some(nodes.len() - 1)
                }
            } else {
                None
            };
            nodes[cursor].calls.push((offset, resolved));
        }
        cursor += 1;
    }
    let mut remaining = nodes.len();
    while remaining != 0 {
        c.checkpoint(1)?;
        let ready = nodes.iter().position(|n| {
            !n.done
                && n.calls
                    .iter()
                    .all(|(_, to)| to.is_none_or(|i| nodes[i].done))
        });
        let Some(index) = ready else {
            // Recursive components and their dependent summaries stay local/partial.
            for node in nodes.iter_mut().filter(|n| !n.done) {
                for (offset, _) in &node.calls {
                    node.records.push(FunctionRecord::CallResolution {
                        offset: *offset,
                        analysis: None,
                        reason: Some("recursive component or dependency on it".into()),
                    });
                }
                node.done = true;
            }
            break;
        };
        let mut result = {
            let calls: Vec<_> = nodes[index]
                .calls
                .iter()
                .filter_map(|(offset, target)| {
                    target.map(|i| blobray_analysis::summaries::Callee {
                        offset: *offset,
                        analysis: &nodes[i].id,
                        source: &nodes[i].recipe.source,
                        object: &nodes[i].recipe.symbol.object,
                        records: nodes[i].facts(),
                    })
                })
                .collect();
            blobray_analysis::summaries::compose(&nodes[index].records, &calls, memory, c)?
        };
        for (offset, target) in &nodes[index].calls {
            result.records.push(FunctionRecord::CallResolution {
                offset: *offset,
                analysis: target.map(|i| nodes[i].id.clone()),
                reason: target.is_none().then(|| {
                    if options.abi.is_none() {
                        "explicit ABI contract required"
                    } else {
                        "target unresolved, ambiguous, or outside selected publication"
                    }
                    .into()
                }),
            });
        }
        nodes[index].composed = Some(result);
        nodes[index].done = true;
        remaining -= 1;
    }
    let mut output = disk.temporary(&directory.join("staging"))?;
    let mut gaps = 0u64;
    let mut known_values = 0u64;
    let mut known_addresses = 0u64;
    for record in nodes[0].facts() {
        c.checkpoint(1)?;
        gaps += u64::from(matches!(record, FunctionRecord::SemanticGap { .. }));
        let known = |v: &AbstractValue| {
            !matches!(v, AbstractValue::Unknown | AbstractValue::Expression { .. })
        };
        match record {
            FunctionRecord::Value { value, .. } => known_values += u64::from(known(value)),
            FunctionRecord::MemoryAccess { address, .. } => {
                known_addresses += u64::from(known(address))
            }
            _ => (),
        }
        write_control_message(&mut output, record)?;
        output.write_all(b"\n").map_err(storage_io)?;
        if let Some(knowledge) = &options.knowledge
            && let FunctionRecord::MemoryAccess {
                offset,
                address:
                    AbstractValue::Constant { value: address } | AbstractValue::ImageAddress { address },
                width,
                ..
            } = record
        {
            project.knowledge_entries(Some(knowledge), c, &mut |entry, c| {
                let p = &entry.proposal;
                if entry.state == AssertionState::Accepted
                    && p.occurrence.revision == manifest.recipe.revision
                    && p.occurrence.source == manifest.recipe.source
                    && p.occurrence.object == manifest.recipe.symbol.object
                    && let KnowledgeClaim::MmioRegion { region } = &p.claim
                    && region
                        .range
                        .contains(u64::from(*address), u64::from(*width))
                {
                    write_control_message(
                        &mut output,
                        &FunctionRecord::MmioRange {
                            offset: *offset,
                            assertion: entry.id.clone(),
                            region: region.clone(),
                        },
                    )?;
                    output.write_all(b"\n").map_err(storage_io)?;
                }

                if entry.state == AssertionState::Accepted
                    && p.occurrence.revision == manifest.recipe.revision
                    && p.occurrence.source == manifest.recipe.source
                    && p.occurrence.object == manifest.recipe.symbol.object
                    && let KnowledgeClaim::MmioRegister { register } = &p.claim
                    && register.address == *address
                    && register.width == *width
                {
                    write_control_message(
                        &mut output,
                        &FunctionRecord::Mmio {
                            offset: *offset,
                            assertion: entry.id.clone(),
                            register: register.clone(),
                        },
                    )?;
                    output.write_all(b"\n").map_err(storage_io)?;
                }
                c.checkpoint(1)
            })?;
        }
    }
    // Validate even when this function has no MMIO accesses.
    if let Some(knowledge) = &options.knowledge {
        project.knowledge_entries(Some(knowledge), c, &mut |_, c| c.checkpoint(1))?;
    }
    manifest.records = staging.retain_temporary(output, c)?;
    if let Some(summary) = &mut manifest.semantics {
        summary.gaps = gaps;
        summary.known_values = known_values;
        summary.known_addresses = known_addresses;
        summary.complete = gaps == 0 && manifest.coverage.complete();
    }
    Ok(())
}
