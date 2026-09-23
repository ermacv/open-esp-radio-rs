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
    c.phase(RunPhase::IndexResearch)?;
    let mut root = None;
    let mut functions = AdmittedVec::new(memory);
    let mut images = AdmittedVec::new(memory);
    for (publication_index, publication) in publications.iter().enumerate() {
        if let Some(id) = &publication.manifest.plan.recipe.request.image
            && !images.iter().any(|(image, _, _)| image == id)
        {
            let capacity = memory.reserve(4 * 1024 * 1024, c.position())?;
            let image = project.image(id, c)?;
            images.push(
                (id.clone(), image.manifest.plan.recipe.companions, capacity),
                c.position(),
            )?;
        }
        c.measure(WorkMetric::PublicationPasses, 1);
        blobray_store::visit_jsonl(&publication.members, c, |member: InvestigationMember, c| {
            if let PlanEntry::Function {
                request,
                declared_extent,
                address_space,
                ..
            } = member.entry
            {
                let analysis = match member.outcome {
                    InvestigationOutcome::Analyzed { analysis, .. } => Some(analysis),
                    _ => None,
                };
                if publication_index == 0
                    && request.source == manifest.recipe.source
                    && request.symbol == manifest.recipe.symbol
                {
                    root = analysis.clone();
                }
                if address_space == CodeAddressSpace::Image {
                    // Three content identities at most; fixed structural bytes are
                    // admitted by the vector separately. No names or full plans retained.
                    let capacity = memory.reserve(3 * 64, c.position())?;
                    functions.push(
                        (
                            declared_extent.start,
                            request.source,
                            request.symbol,
                            analysis,
                            capacity,
                        ),
                        c.position(),
                    )?;
                }
            }
            Ok(())
        })?;
    }
    c.checkpoint(functions.len() as u64 * (functions.len().max(1).ilog2() as u64 + 1))?;
    functions.sort_unstable_by_key(|f| f.0);
    let knowledge = project.knowledge_snapshot(options.knowledge.as_ref(), memory, c)?;
    let mut registers = AdmittedVec::new(memory);
    let mut regions = AdmittedVec::new(memory);
    for entry in knowledge.entries() {
        c.checkpoint(1)?;
        let p = &entry.proposal;
        if entry.state != AssertionState::Accepted
            || p.occurrence.revision != manifest.recipe.revision
            || p.occurrence.source != manifest.recipe.source
            || p.occurrence.object != manifest.recipe.symbol.object
        {
            continue;
        }
        match &p.claim {
            KnowledgeClaim::MmioRegister { register } => registers.push(
                (register.address, register.width, entry, register),
                c.position(),
            )?,
            KnowledgeClaim::MmioRegion { region } => {
                regions.push((u64::from(region.range.start), entry, region), c.position())?
            }
            _ => (),
        }
    }
    c.checkpoint(
        (registers.len() + regions.len()) as u64
            * ((registers.len() + regions.len()).max(1).ilog2() as u64 + 1),
    )?;
    registers.sort_unstable_by_key(|r| (r.0, r.1));
    regions.sort_unstable_by_key(|r| r.0);
    let mut region_ends = AdmittedVec::new(memory);
    let mut end = 0;
    for (start, _, region) in regions.iter() {
        end = end.max(
            start
                .checked_add(region.range.length)
                .ok_or_else(|| Error::new(ErrorCode::Integrity, "knowledge range overflow"))?,
        );
        region_ends.push(end, c.position())?;
    }
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
                    images
                        .iter()
                        .find(|(id, _, _)| id == image)
                        .map(|(_, companions, _)| companions.as_slice())
                        .ok_or_else(|| {
                            Error::new(
                                ErrorCode::Integrity,
                                "research image absent from selected publications",
                            )
                        })?
                } else {
                    &[]
                };
                c.checkpoint(functions.len().max(1).ilog2() as u64 + 1)?;
                let first = functions.partition_point(|f| f.0 < u64::from(address));
                for (_, source, symbol, analysis, _) in functions[first..]
                    .iter()
                    .take_while(|f| f.0 == u64::from(address))
                {
                    c.checkpoint(1)?;
                    if (*source == nodes[cursor].recipe.source
                        && symbol.object == nodes[cursor].recipe.symbol.object)
                        || external
                            .iter()
                            .any(|e| source.input() == Some(e.input) && *symbol == e.symbol)
                    {
                        if let Some(analysis) = analysis {
                            if candidate.as_ref() != Some(analysis) {
                                matches += 1;
                                candidate = Some(analysis.clone());
                            }
                        } else {
                            matches += 1;
                        }
                    }
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
        if let FunctionRecord::MemoryAccess {
            offset,
            address:
                AbstractValue::Constant { value: address } | AbstractValue::ImageAddress { address },
            width,
            ..
        } = record
        {
            c.checkpoint(
                registers.len().max(1).ilog2() as u64 + regions.len().max(1).ilog2() as u64 + 2,
            )?;
            let first = registers.partition_point(|r| (r.0, r.1) < (*address, *width));
            for (_, _, entry, register) in registers[first..]
                .iter()
                .take_while(|r| (r.0, r.1) == (*address, *width))
            {
                c.checkpoint(1)?;
                write_control_message(
                    &mut output,
                    &FunctionRecord::Mmio {
                        offset: *offset,
                        assertion: entry.id.clone(),
                        register: (*register).clone(),
                    },
                )?;
                output.write_all(b"\n").map_err(storage_io)?;
            }
            let mut index = regions.partition_point(|r| r.0 <= u64::from(*address));
            while index > 0 && region_ends[index - 1] > u64::from(*address) {
                index -= 1;
                c.checkpoint(1)?;
                let (_, entry, region) = &regions[index];
                if region
                    .range
                    .contains(u64::from(*address), u64::from(*width))
                {
                    write_control_message(
                        &mut output,
                        &FunctionRecord::MmioRange {
                            offset: *offset,
                            assertion: entry.id.clone(),
                            region: (*region).clone(),
                        },
                    )?;
                    output.write_all(b"\n").map_err(storage_io)?;
                }
            }
        }
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
