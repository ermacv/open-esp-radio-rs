//! Reviewed physical call pairs; names never resolve executable identities.
use crate::*;
pub const MAX_CALL_PAIRS: usize = 128;
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallEndpoint {
    pub occurrence: KnowledgeOccurrence,
    pub boundary: ReviewedCallBoundary,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ReviewedCallBoundary {
    /// The occurrence must name the exact symbol, including its table identity.
    Code { address: u32 },
    Model {
        binding: CallBinding,
        definition: ArtifactId,
    },
    Service {
        binding: CallBinding,
        definition: ArtifactId,
        binding_index: u16,
    },
}
impl CallEndpoint {
    pub fn address(&self) -> u32 {
        match self.boundary {
            ReviewedCallBoundary::Code { address } => address,
            ReviewedCallBoundary::Model { binding, .. }
            | ReviewedCallBoundary::Service { binding, .. } => binding.address,
        }
    }
    pub fn target_kind(&self) -> ObservedCallTarget {
        match self.boundary {
            ReviewedCallBoundary::Code { .. } => ObservedCallTarget::CapturedCode,
            ReviewedCallBoundary::Model { .. } => ObservedCallTarget::CallModel,
            ReviewedCallBoundary::Service { .. } => ObservedCallTarget::FifoService,
        }
    }
    pub fn in_target(&self, target: &ExecutionTarget) -> bool {
        self.occurrence.revision == target.revision
            && self.occurrence.object.location == ObjectLocation::Standalone
            && (self.occurrence.source == target.source
                || matches!(self.occurrence.source, FunctionSource::Input { input } if target.companions.contains(&input)))
    }
    pub fn validate(&self) -> Result<()> {
        if self.address() & 1 != 0
            || self.address() >= u32::MAX - 1
            || self.occurrence.object.location != ObjectLocation::Standalone
            || self
                .occurrence
                .symbol
                .as_ref()
                .is_some_and(|s| s.object != self.occurrence.object)
            || matches!(self.boundary, ReviewedCallBoundary::Code { .. })
                != self.occurrence.symbol.is_some()
        {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "invalid reviewed call endpoint",
            ));
        }
        Ok(())
    }
    pub fn allocated_bytes(&self) -> u64 {
        self.occurrence.revision.allocated_bytes()
            + self.occurrence.source.allocated_bytes()
            + self.occurrence.object.artifact.allocated_bytes()
            + self
                .occurrence
                .symbol
                .as_ref()
                .map_or(0, |s| s.object.artifact.allocated_bytes())
            + match &self.boundary {
                ReviewedCallBoundary::Code { .. } => 0,
                ReviewedCallBoundary::Model { definition, .. }
                | ReviewedCallBoundary::Service { definition, .. } => definition.allocated_bytes(),
            }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallWordPair {
    pub vendor: u16,
    pub replacement: u16,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum CallArguments {
    /// Explicit 32-bit ABI word positions; values remain exact, including pointer bits.
    Projected {
        words: Vec<CallWordPair>,
    },
    Exact {
        words: u16,
    },
    Selected {
        words: Vec<u16>,
    },
    Ignore,
}
impl CallArguments {
    pub fn selection_work(&self) -> u64 {
        match self {
            Self::Selected { words } => words.len() as u64 + 1,
            Self::Projected { words } => words.len() as u64 + 1,
            _ => 1,
        }
    }
    pub fn selects(&self, word: u16, replacement: bool) -> bool {
        match self {
            Self::Exact { words } => word < *words,
            Self::Selected { words } => words.contains(&word),
            Self::Projected { words } => words
                .iter()
                .any(|p| if replacement { p.replacement } else { p.vendor } == word),
            Self::Ignore => false,
        }
    }
    pub fn validate_capture(&self, left: u16, right: u16) -> Result<()> {
        let valid = match self {
            Self::Exact { words } => *words == left && *words == right,
            Self::Projected { words } => words
                .iter()
                .all(|p| p.vendor < left && p.replacement < right),
            Self::Selected { words } => words.iter().all(|w| *w < left && *w < right),
            Self::Ignore => true,
        };
        if valid {
            Ok(())
        } else {
            Err(Error::new(
                ErrorCode::InvalidRequest,
                "reviewed arguments differ from capture width",
            ))
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallCorrespondence {
    pub vendor: CallEndpoint,
    pub replacement: CallEndpoint,
    pub arguments: CallArguments,
    pub applicability: String,
    pub reason: String,
}
impl CallCorrespondence {
    pub fn validate(&self) -> Result<()> {
        self.vendor.validate()?;
        self.replacement.validate()?;
        let arguments = match &self.arguments {
            CallArguments::Exact { words } => usize::from(*words) <= MAX_EXECUTION_ARGUMENT_WORDS,
            CallArguments::Projected { words } => {
                !words.is_empty()
                    && words.len() <= MAX_EXECUTION_ARGUMENT_WORDS
                    && words.iter().enumerate().all(|(i, p)| {
                        usize::from(p.vendor) < MAX_EXECUTION_ARGUMENT_WORDS
                            && usize::from(p.replacement) < MAX_EXECUTION_ARGUMENT_WORDS
                            && !words[..i]
                                .iter()
                                .any(|q| q.vendor == p.vendor || q.replacement == p.replacement)
                    })
            }
            CallArguments::Selected { words } => {
                !words.is_empty()
                    && words.len() <= MAX_EXECUTION_ARGUMENT_WORDS
                    && words.iter().enumerate().all(|(i, w)| {
                        usize::from(*w) < MAX_EXECUTION_ARGUMENT_WORDS && !words[..i].contains(w)
                    })
            }
            CallArguments::Ignore => true,
        };
        if !arguments
            || self.applicability.trim().is_empty()
            || self.applicability.len() > 1024
            || self.reason.trim().is_empty()
            || self.reason.len() > 4096
        {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "invalid reviewed call correspondence",
            ));
        }
        Ok(())
    }
    pub fn allocated_bytes(&self) -> u64 {
        self.vendor.allocated_bytes()
            + self.replacement.allocated_bytes()
            + self.applicability.capacity() as u64
            + self.reason.capacity() as u64
            + match &self.arguments {
                CallArguments::Selected { words } => (words.capacity() * 2) as u64,
                CallArguments::Projected { words } => {
                    (words.capacity() * std::mem::size_of::<CallWordPair>()) as u64
                }
                _ => 0,
            }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallPairReview {
    pub knowledge: KnowledgeRevisionId,
    pub assertion: AssertionId,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnlistedCalls {
    Exact,
    Exclude,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewedCalls {
    pub pairs: Vec<CallPairReview>,
    pub unlisted: UnlistedCalls,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedCallPair {
    pub review: CallPairReview,
    pub correspondence: CallCorrespondence,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallPairProposalRequest {
    pub subject: SubjectId,
    pub correspondence: CallCorrespondence,
    pub expected_base: Option<KnowledgeRevisionId>,
    pub actor: String,
    pub reason: String,
}

/// Check immutable endpoint applicability against the explicit phase declaration history.
/// No model is run and no captured source is acquired here.
pub fn validate_call_pair_use(
    request: &ExecutionRequest,
    phase: usize,
    pair: &CallCorrespondence,
    c: &mut dyn RunControl,
) -> Result<()> {
    pair.validate()?;
    let bad = || {
        Error::new(
            ErrorCode::InvalidRequest,
            "reviewed call pair does not apply to execution phase",
        )
    };
    let case = request.cases.get(phase).ok_or_else(bad)?;
    let mut widths = [0; 2];
    for (side, (endpoint, target, input)) in [
        (&pair.vendor, &request.vendor, &case.vendor),
        (
            &pair.replacement,
            request.replacement.as_ref().ok_or_else(bad)?,
            case.replacement.as_ref().ok_or_else(bad)?,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        if !endpoint.in_target(target) {
            return Err(bad());
        }
        let capture = input.observe_calls.as_ref().ok_or_else(bad)?;
        widths[side] = capture
            .overrides
            .iter()
            .find(|o| o.target == endpoint.address())
            .map(|o| o.words)
            .unwrap_or(capture.argument_words);
        let start = request.cases[..=phase]
            .iter()
            .rposition(|p| p.reset == SessionReset::Cold)
            .ok_or_else(bad)?;
        let mut boundaries = 0;
        let mut matched = false;
        for (index, p) in request.cases[start..=phase].iter().enumerate() {
            let input = if side == 0 {
                &p.vendor
            } else {
                p.replacement.as_ref().ok_or_else(bad)?
            };
            let current = start + index == phase;
            for model in &input.calls {
                c.checkpoint(1)?;
                if (current || model.lifetime == RegionLifetime::Session)
                    && model.binding.address == endpoint.address()
                {
                    boundaries += 1;
                    if let ReviewedCallBoundary::Model {
                        binding,
                        definition,
                    } = &endpoint.boundary
                    {
                        matched = binding == &model.binding && definition == &model.identity(c)?;
                    }
                }
            }
            for service in &input.services {
                if !current && service.lifetime != RegionLifetime::Session {
                    continue;
                }
                for (slot, binding) in service.bindings.iter().enumerate() {
                    c.checkpoint(1)?;
                    if binding.call.address == endpoint.address() {
                        boundaries += 1;
                        if let ReviewedCallBoundary::Service {
                            binding: expected,
                            definition,
                            binding_index: expected_slot,
                        } = &endpoint.boundary
                        {
                            matched = expected == &binding.call
                                && usize::from(*expected_slot) == slot
                                && definition == &service.identity(c)?;
                        }
                    }
                }
            }
        }
        match endpoint.boundary {
            ReviewedCallBoundary::Code { .. } if boundaries == 0 => (),
            ReviewedCallBoundary::Model { .. } | ReviewedCallBoundary::Service { .. }
                if boundaries == 1 && matched => {}
            _ => return Err(bad()),
        }
    }
    pair.arguments.validate_capture(widths[0], widths[1])
}

/// Bounded borrowed index shared by comparison and retained knownness validation.
pub struct CallRelationIndex<'a> {
    pairs: [Option<&'a ResolvedCallPair>; MAX_CALL_PAIRS],
    length: usize,
    side: bool,
    unlisted: UnlistedCalls,
    enabled: bool,
}
#[derive(Clone, Copy)]
pub enum CallSelection<'a> {
    Physical,
    Reviewed(&'a ResolvedCallPair),
    Excluded,
}
impl CallSelection<'_> {
    pub fn selection_work(self) -> u64 {
        match self {
            Self::Reviewed(p) => p.correspondence.arguments.selection_work(),
            _ => 1,
        }
    }
    pub fn selects_word(self, word: u16, replacement: bool) -> bool {
        match self {
            Self::Physical => true,
            Self::Reviewed(p) => p.correspondence.arguments.selects(word, replacement),
            Self::Excluded => false,
        }
    }
}
impl<'a> CallRelationIndex<'a> {
    pub fn replacement(&self) -> bool {
        self.side
    }
    pub fn new(
        relation: Option<&ComparisonRelation>,
        resolved: &'a [ResolvedCallPair],
        side: bool,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        let mut result = Self {
            pairs: [None; MAX_CALL_PAIRS],
            length: 0,
            side,
            unlisted: UnlistedCalls::Exact,
            enabled: relation.is_some_and(ComparisonRelation::observes_calls),
        };
        if let Some(selected) = relation.and_then(|r| r.reviewed_calls.as_ref()) {
            if selected.pairs.len() > MAX_CALL_PAIRS {
                return Err(Error::new(
                    ErrorCode::Integrity,
                    "call relation index capacity exceeded",
                ));
            }
            result.unlisted = selected.unlisted;
            for review in &selected.pairs {
                c.checkpoint(resolved.len() as u64 + 1)?;
                let p = resolved
                    .iter()
                    .find(|p| &p.review == review)
                    .ok_or_else(|| {
                        Error::new(ErrorCode::Integrity, "selected resolved call pair missing")
                    })?;
                result.pairs[result.length] = Some(p);
                result.length += 1;
            }
            let endpoint = |p: &'a ResolvedCallPair| {
                if side {
                    &p.correspondence.replacement
                } else {
                    &p.correspondence.vendor
                }
            };
            result.pairs[..result.length].sort_unstable_by_key(|p| endpoint(p.unwrap()).address());
            for adjacent in result.pairs[..result.length].windows(2) {
                if endpoint(adjacent[0].unwrap()).address()
                    == endpoint(adjacent[1].unwrap()).address()
                {
                    return Err(Error::new(
                        ErrorCode::Integrity,
                        "ambiguous call relation endpoints",
                    ));
                }
            }
        }
        Ok(result)
    }
    pub fn select(
        &self,
        target: u32,
        kind: ObservedCallTarget,
        c: &mut dyn RunControl,
    ) -> Result<CallSelection<'a>> {
        c.checkpoint(self.length.max(1).ilog2() as u64 + 1)?;
        if !self.enabled {
            return Ok(CallSelection::Excluded);
        }
        let endpoint = |p: &'a ResolvedCallPair| {
            if self.side {
                &p.correspondence.replacement
            } else {
                &p.correspondence.vendor
            }
        };
        if let Ok(i) = self.pairs[..self.length]
            .binary_search_by_key(&target, |p| endpoint(p.unwrap()).address())
        {
            let p = self.pairs[i].unwrap();
            if endpoint(p).target_kind() != kind {
                return Err(Error::new(
                    ErrorCode::Integrity,
                    "captured call kind differs from reviewed boundary",
                ));
            }
            return Ok(CallSelection::Reviewed(p));
        }
        Ok(if self.unlisted == UnlistedCalls::Exact {
            CallSelection::Physical
        } else {
            CallSelection::Excluded
        })
    }
}
