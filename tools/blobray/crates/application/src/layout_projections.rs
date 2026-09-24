//! Physical acquisition for layout review; runtime byte fields remain explicit assumptions.
use crate::*;
pub(crate) fn endpoint(
    capture: &crate::occurrence::CapturedObject<'_>,
    projection: &LayoutProjection,
    side: bool,
    decoder: &dyn FunctionDecoder,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<()> {
    let endpoint = &projection.endpoint(side).entry;
    capture.with_prepared(memory,c,|object,c| {
        let symbol = endpoint.occurrence.symbol.as_ref().ok_or_else(||Error::new(ErrorCode::InvalidRequest,"projection entry lacks physical symbol"))?;
        if object.code_symbol_address(&endpoint.occurrence.object,symbol,c)? != endpoint.address() {
            return Err(Error::new(ErrorCode::InvalidRequest,"projection entry differs from captured symbol"));
        }
        for pair in &projection.branches {
            c.checkpoint(1)?;
            let branch = if side {pair.replacement} else {pair.vendor};
            let prefix = object.executable_bytes(branch.site,2,c)?;
            let length = if prefix[0] & 3 == 3 {4} else {2};
            let bytes = object.executable_bytes(branch.site,length,c)?;
            let decoded = decoder.decode(bytes).ok_or_else(||Error::new(ErrorCode::InvalidRequest,"projection branch is not decoded"))?;
            if !matches!(decoded.flow, InstructionFlow::Branch { displacement } if branch.site.wrapping_add_signed(displacement) == branch.target)
                || branch.site.checked_add(u32::from(decoded.length)) != Some(branch.fallthrough) {
                return Err(Error::new(ErrorCode::InvalidRequest,"projection branch differs from captured instruction"));
            }
        }
        Ok(())
    })
}
pub(crate) fn secondary(
    project: &Project,
    projection: &LayoutProjection,
    decoder: &dyn FunctionDecoder,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<Vec<ArtifactId>> {
    let occurrence = &projection.replacement.entry.occurrence;
    let mut roots = vec![
        occurrence.revision.as_str().parse()?,
        occurrence.object.artifact.clone(),
    ];
    if let FunctionSource::Image { image } = &occurrence.source {
        roots.push(image.as_str().parse()?);
    }
    crate::occurrence::with_source(project, occurrence, memory, c, |capture, c| {
        endpoint(&capture, projection, true, decoder, memory, c)?;
        if capture.detached {
            roots.push(capture.payload.clone());
        }
        Ok(())
    })?;
    Ok(roots)
}
pub(crate) fn propose(
    project: &Project,
    request: &ProjectionProposalRequest,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<KnowledgeChange> {
    request.projection.validate()?;
    let occurrence = &request.projection.vendor.entry.occurrence;
    let evidence = crate::occurrence::with_source(project, occurrence, memory, c, |capture, _| {
        Ok(EvidenceRef::Source {
            payload: capture.payload.clone(),
            range: CodeRange {
                start: 0,
                length: capture.bytes.len(),
            },
        })
    })?;
    Ok(KnowledgeChange {
        expected_base: request.expected_base.clone(),
        actor: request.actor.clone(),
        reason: request.reason.clone(),
        action: KnowledgeAction::Propose {
            proposal: KnowledgeProposal {
                subject: request.subject.clone(),
                occurrence: occurrence.clone(),
                claim: KnowledgeClaim::LayoutProjection {
                    projection: Box::new(request.projection.clone()),
                },
                evidence: vec![evidence],
                note: None,
            },
        },
    })
}
