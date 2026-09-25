//! Selected immutable review contracts and their phase applicability, with admitted ownership.
use super::*;
pub struct ExecutionCallPairs<'m> {
    pub pairs: Vec<ResolvedCallPair>,
    _payloads: AdmittedVec<'m, MemoryReservation<'m>>,
    _capacity: MemoryReservation<'m>,
}
impl Project {
    pub fn execution_call_pairs<'m>(
        &self,
        request: &ExecutionRequest,
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<ExecutionCallPairs<'m>> {
        let mut reviews = AdmittedVec::new(memory);
        for case in &request.cases {
            if let Some(selected) = case
                .relation
                .as_ref()
                .and_then(|r| r.reviewed_calls.as_ref())
            {
                for review in &selected.pairs {
                    c.checkpoint(reviews.len() as u64 + 1)?;
                    if !reviews.contains(&review) {
                        if reviews.len() == MAX_CALL_PAIRS {
                            return Err(Error::new(
                                ErrorCode::ResourceLimited,
                                "execution call review capacity exceeded",
                            ));
                        }
                        reviews.push(review, c.position())?;
                    }
                }
            }
        }
        let capacity = memory.reserve(
            (reviews.len() * std::mem::size_of::<ResolvedCallPair>()) as u64,
            c.position(),
        )?;
        let mut pairs = Vec::new();
        pairs
            .try_reserve_exact(reviews.len())
            .map_err(|_| Error::new(ErrorCode::ResourceLimited, "call pair allocation refused"))?;
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
                    .ok_or_else(|| integrity("selected call assertion missing"))?;
                let KnowledgeClaim::CallPair { correspondence } = &entry.proposal.claim else {
                    return Err(Error::new(
                        ErrorCode::InvalidRequest,
                        "selected assertion is not a call pair",
                    ));
                };
                if entry.state != AssertionState::Accepted
                    || entry.proposal.occurrence != correspondence.vendor.occurrence
                {
                    return Err(Error::new(
                        ErrorCode::InvalidRequest,
                        "call pair requires a selected accepted review",
                    ));
                }
                correspondence.validate()?;
                let bytes = correspondence.allocated_bytes()
                    + selected.knowledge.allocated_bytes()
                    + selected.assertion.allocated_bytes();
                payloads.push(memory.reserve(bytes, c.position())?, c.position())?;
                pairs.push(ResolvedCallPair {
                    review: (*selected).clone(),
                    correspondence: (**correspondence).clone(),
                });
            }
        }
        for (phase, case) in request.cases.iter().enumerate() {
            if let Some(selected) = case
                .relation
                .as_ref()
                .and_then(|r| r.reviewed_calls.as_ref())
            {
                for (i, review) in selected.pairs.iter().enumerate() {
                    c.checkpoint((pairs.len() * (i + 1)) as u64)?;
                    let p = &pairs
                        .iter()
                        .find(|p| &p.review == review)
                        .unwrap()
                        .correspondence;
                    validate_call_pair_use(request, phase, p, c)?;
                    for prior in &selected.pairs[..i] {
                        let q = &pairs
                            .iter()
                            .find(|p| &p.review == prior)
                            .unwrap()
                            .correspondence;
                        if q.vendor.address() == p.vendor.address()
                            || q.replacement.address() == p.replacement.address()
                        {
                            return Err(Error::new(
                                ErrorCode::Conflict,
                                "selected call pairs have ambiguous physical endpoints",
                            ));
                        }
                    }
                }
            }
        }
        Ok(ExecutionCallPairs {
            pairs,
            _payloads: payloads,
            _capacity: capacity,
        })
    }
    pub(super) fn validate_execution_call_pairs(
        &self,
        manifest: &ExecutionManifest,
        request: &ExecutionRequest,
        memory: &WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        let selected = self.execution_call_pairs(request, memory, c)?;
        if selected.pairs != manifest.call_pairs {
            return Err(integrity(
                "retained call relations differ from selected accepted reviews",
            ));
        }
        Ok(())
    }
}
