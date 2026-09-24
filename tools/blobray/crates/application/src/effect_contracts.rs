//! Proposal preparation; physical endpoint validation is shared with code correspondences.
use crate::*;
pub(crate) fn propose(
    project: &Project,
    request: &EffectProposalRequest,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<KnowledgeChange> {
    request.contract.validate()?;
    let occurrence = &request.contract.vendor.occurrence;
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
                claim: KnowledgeClaim::EffectContract {
                    contract: Box::new(request.contract.clone()),
                },
                evidence: vec![evidence],
                note: None,
            },
        },
    })
}
