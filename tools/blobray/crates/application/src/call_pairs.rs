//! Physical admission for reviewed call pairs; both endpoints retain exact captured roots.
use crate::*;
fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, message)
}
pub(crate) fn endpoint(
    capture: &crate::occurrence::CapturedObject<'_>,
    endpoint: &CallEndpoint,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<()> {
    endpoint.validate()?;
    if let ReviewedCallBoundary::Code { address } = endpoint.boundary {
        capture.with_prepared(memory, c, |object, c| {
            let symbol = endpoint
                .occurrence
                .symbol
                .as_ref()
                .ok_or_else(|| invalid("call code endpoint lacks symbol"))?;
            if object.code_symbol_address(&endpoint.occurrence.object, symbol, c)? != address {
                return Err(invalid(
                    "reviewed call address differs from exact captured symbol",
                ));
            }
            Ok(())
        })?;
    }
    Ok(())
}
pub(crate) fn secondary(
    project: &Project,
    pair: &CallCorrespondence,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<Vec<ArtifactId>> {
    let e = &pair.replacement;
    let mut roots = vec![
        e.occurrence.revision.as_str().parse()?,
        e.occurrence.object.artifact.clone(),
    ];
    if let FunctionSource::Image { image } = &e.occurrence.source {
        roots.push(image.as_str().parse()?);
    }
    crate::occurrence::with_source(project, &e.occurrence, memory, c, |capture, c| {
        endpoint(&capture, e, memory, c)?;
        if capture.detached {
            roots.push(capture.payload.clone());
        }
        Ok(())
    })?;
    Ok(roots)
}
pub(crate) fn propose(
    project: &Project,
    request: &CallPairProposalRequest,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<KnowledgeChange> {
    request.correspondence.validate()?;
    let occurrence = &request.correspondence.vendor.occurrence;
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
                claim: KnowledgeClaim::CallPair {
                    correspondence: Box::new(request.correspondence.clone()),
                },
                evidence: vec![evidence],
                note: None,
            },
        },
    })
}
