//! Physical access matching; declaration roles and observed access kinds stay separate.
use super::*;
use blobray_analysis::navigation::{Facts, MemoryObservation};
pub(super) enum Target<'a> {
    Object {
        occurrence: &'a KnowledgeOccurrence,
        location: DataLocation,
        access: Option<AccessDirection>,
    },
    Context {
        proposal: &'a KnowledgeProposal,
        contract: &'a FunctionContract,
        words: Vec<ArgumentWord>,
        field: Option<&'a ContextFieldKey>,
        access: Option<AccessDirection>,
    },
}
impl<'a> Target<'a> {
    pub(super) fn new(
        project: &Project,
        query: &'a NavigationQuery,
        snapshot: &'a blobray_store::KnowledgeSnapshot<'_>,
        memory: &WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Option<Self>> {
        Ok(match &query.filter {
            NavigationFilter::Object {
                occurrence,
                selector,
                access,
            } => {
                if occurrence.revision != query.scope.revision {
                    return Err(invalid("object revision differs from navigation scope"));
                }
                let location = crate::occurrence::with_source(
                    project,
                    occurrence,
                    memory,
                    c,
                    |capture, c| {
                        capture.with_prepared(memory, c, |object, c| {
                            object.data_location(&occurrence.object, selector, c)
                        })
                    },
                )?;
                Some(Self::Object {
                    occurrence,
                    location,
                    access: *access,
                })
            }
            NavigationFilter::Context {
                assertion,
                field,
                arguments,
                access,
            } => {
                if arguments.len() > 16 {
                    return Err(invalid("too many context argument mappings"));
                }
                let mut found = None;
                for entry in snapshot.entries() {
                    c.checkpoint(1)?;
                    if &entry.id == assertion {
                        found = Some(entry);
                        break;
                    }
                }
                let entry = found.ok_or_else(|| {
                    Error::new(
                        ErrorCode::NotFound,
                        "context assertion is absent from selected knowledge",
                    )
                })?;
                if entry.state != AssertionState::Accepted {
                    return Err(invalid(
                        "context navigation requires an accepted declaration",
                    ));
                }
                if entry.proposal.occurrence.revision != query.scope.revision {
                    return Err(invalid("context declaration belongs to another revision"));
                }
                blobray_knowledge::validate_proposal(&entry.proposal)?;
                let KnowledgeClaim::Function { contract } = &entry.proposal.claim else {
                    return Err(invalid("context assertion is not a function contract"));
                };
                for (i, mapping) in arguments.iter().enumerate() {
                    if mapping.word >= 64
                        || !contract
                            .contexts
                            .iter()
                            .any(|ctx| ctx.argument == mapping.argument)
                        || arguments[..i]
                            .iter()
                            .any(|a| a.argument == mapping.argument || a.word == mapping.word)
                    {
                        return Err(invalid(
                            "context argument mappings must name distinct declared roots and supported ABI words",
                        ));
                    }
                }
                if field.as_ref().is_some_and(|f| {
                    !contract.contexts.iter().any(|ctx| {
                        ctx.argument == f.argument && ctx.fields.iter().any(|x| x.name == f.name)
                    })
                }) {
                    return Err(Error::new(
                        ErrorCode::NotFound,
                        "selected context field is absent",
                    ));
                }
                let mut words = Vec::with_capacity(contract.contexts.len());
                for context in &contract.contexts {
                    let supplied = arguments
                        .iter()
                        .find(|a| a.argument == context.argument)
                        .map(|a| a.word);
                    let word = if let Some(signature) = &contract.signature {
                        let word = signature.argument_word(context.argument).ok_or_else(|| {
                            invalid("context signature has no supported argument placement")
                        })?;
                        if supplied.is_some_and(|x| x != word) {
                            return Err(invalid("argument mapping contradicts reviewed signature"));
                        }
                        word
                    } else {
                        supplied.ok_or_else(|| invalid("unknown signature requires an explicit context argument-to-ABI-word mapping"))?
                    };
                    words.push(ArgumentWord {
                        argument: context.argument,
                        word,
                    });
                }
                Some(Self::Context {
                    proposal: &entry.proposal,
                    contract,
                    words,
                    field: field.as_ref(),
                    access: *access,
                })
            }
            _ => None,
        })
    }
    pub(super) fn relevant(&self, recipe: &FunctionRecipe) -> bool {
        match self {
            Self::Object { .. } => true,
            Self::Context {
                proposal, contract, ..
            } => {
                proposal.occurrence.source == recipe.source && contract.selector == recipe.selector
            }
        }
    }
    pub(super) fn data(&self) -> Option<DataLocation> {
        match self {
            Self::Object { location, .. } => Some(location.clone()),
            _ => None,
        }
    }
    pub(super) fn visit(
        &self,
        node: &Node<'_>,
        recipe: &FunctionRecipe,
        facts: &Facts<'_, '_>,
        memory: &WorkingMemory,
        c: &mut dyn RunControl,
        emit: &mut dyn FnMut(&NavigationRecord, &mut dyn RunControl) -> Result<()>,
    ) -> Result<()> {
        let access_filter = match self {
            Self::Object { access, .. } | Self::Context { access, .. } => access,
        };
        facts.accesses(c, &mut |r, c| {
            if access_filter.is_some_and(|want| !direction(want, r.access)) {
                return Ok(());
            }
            if !matches!(r.width, 1 | 2 | 4 | 8) {
                return Err(Error::new(
                    ErrorCode::Integrity,
                    "invalid saved memory access width",
                ));
            }
            let mut matches = AdmittedVec::new(memory);
            let mut issue = None;
            let mut path_issue = None;
            let mut alternatives = 0usize;
            if let AbstractValue::Alternatives { values } = r.address {
                for (i, leaf) in values.values().iter().enumerate() {
                    c.checkpoint(1)?;
                    self.match_value(
                        &r,
                        &leaf.as_value(),
                        recipe,
                        facts,
                        i as u8,
                        &mut matches,
                        &mut issue,
                        &mut path_issue,
                        &mut alternatives,
                        c,
                    )?;
                }
                alternatives = alternatives.max(values.values().len());
            } else {
                self.match_value(
                    &r,
                    r.address,
                    recipe,
                    facts,
                    0,
                    &mut matches,
                    &mut issue,
                    &mut path_issue,
                    &mut alternatives,
                    c,
                )?;
            }
            if matches.is_empty() && issue.is_none() && path_issue.is_none() {
                return Ok(());
            }
            if issue.is_none() && alternatives > 1 {
                issue = Some(NavigationIssue::AmbiguousAddress);
            }
            let bytes = matches
                .iter()
                .try_fold(0u64, |n, m: &Match<'_>| {
                    n.checked_add(
                        std::mem::size_of::<LocationMatch>() as u64
                            + m.field.map_or(0, |s| s.len() as u64),
                    )
                })
                .ok_or_else(|| invalid("navigation match capacity overflow"))?;
            let _capacity = memory.reserve(bytes, c.position())?;
            let mut owned = Vec::new();
            owned.try_reserve_exact(matches.len()).map_err(|_| {
                Error::new(
                    ErrorCode::ResourceLimited,
                    "navigation match allocation refused",
                )
            })?;
            for m in matches.iter() {
                owned.push(LocationMatch {
                    alternative: m.alternative,
                    offset: m.offset,
                    partial_overlap: m.partial,
                    field: m.field.map(str::to_owned),
                    argument: m.argument,
                });
            }
            emit(
                &NavigationRecord::Access {
                    function: node.function.clone(),
                    record: r.record,
                    origin: r.origin.cloned(),
                    offset: r.offset,
                    access: r.access,
                    width: r.width,
                    address: r.address.clone(),
                    value: r.value.cloned(),
                    matches: owned,
                    issue,
                    path_issue,
                },
                c,
            )
        })
    }
    #[allow(clippy::too_many_arguments)]
    fn match_value<'b>(
        &'b self,
        r: &MemoryObservation<'_>,
        value: &AbstractValue,
        recipe: &FunctionRecipe,
        facts: &Facts<'_, '_>,
        alternative: u8,
        matches: &mut AdmittedVec<'_, Match<'b>>,
        issue: &mut Option<NavigationIssue>,
        path_issue: &mut Option<AccessIssue>,
        alternatives: &mut usize,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        if let AbstractValue::ScopedAddress {
            source,
            object,
            address,
        } = value
        {
            if let Self::Object {
                occurrence,
                location,
                ..
            } = self
                && *source == occurrence.source
                && *object == occurrence.object
                && let Some(start) = location.image_address
            {
                add_match(
                    matches,
                    alternative,
                    i128::from(*address),
                    i128::from(start),
                    location.section_range.length,
                    r.width,
                    None,
                    None,
                    c,
                )?;
            }
            return Ok(());
        }
        // The stack slot holding an incoming pointer is not a field of its pointee.
        if matches!(self, Self::Context { .. }) && matches!(value, AbstractValue::EntryStack { .. })
        {
            return Ok(());
        }
        // Composed unqualified addresses do not inherit the caller's object namespace.
        // ScopedAddress above carries sufficient physical identity; other composed
        // expressions must retain uncertainty until their provenance is resolved.
        if r.origin.is_some() && matches!(self, Self::Object { .. }) {
            *issue = Some(NavigationIssue::ForeignOccurrence);
            return Ok(());
        }
        let abi = match self {
            Self::Context { contract, .. } => Some(contract.abi),
            _ => None,
        };
        let (paths, problem) = facts.paths(value, recipe, abi, c)?;
        *alternatives = (*alternatives).max(paths.len());
        if let Some(problem) = problem {
            *path_issue = Some(problem);
            *issue = Some(NavigationIssue::UnknownAddress);
        }
        for (i, path) in paths.iter().enumerate() {
            c.checkpoint(1)?;
            let alternative = alternative.saturating_add(i as u8);
            if !path.path.is_empty() {
                *issue = Some(NavigationIssue::IndirectPath);
                continue;
            }
            match self {
                Self::Context {
                    contract,
                    words,
                    field,
                    ..
                } => {
                    if let AccessRoot::EntryWord { function, word } = &path.root
                        && function == &recipe.selector
                    {
                        for context in &contract.contexts {
                            c.checkpoint(1)?;
                            if !words
                                .iter()
                                .any(|a| a.argument == context.argument && a.word == *word)
                            {
                                continue;
                            }
                            for f in &context.fields {
                                c.checkpoint(1)?;
                                if field.is_some_and(|key| {
                                    key.argument != context.argument || key.name != f.name
                                }) {
                                    continue;
                                }
                                add_match(
                                    matches,
                                    alternative,
                                    i128::from(path.offset),
                                    i128::from(f.offset),
                                    u64::from(f.width),
                                    r.width,
                                    Some(&f.name),
                                    Some(context.argument),
                                    c,
                                )?;
                            }
                        }
                    }
                }
                Self::Object {
                    occurrence,
                    location,
                    ..
                } => {
                    let same = recipe.source == occurrence.source
                        && *recipe.selector.object() == occurrence.object;
                    let coordinate = match &path.root {
                        AccessRoot::Address { address } if same => location
                            .image_address
                            .map(|start| (i128::from(*address), i128::from(start))),
                        AccessRoot::Section { section, offset }
                            if same && *section == location.section =>
                        {
                            Some((
                                i128::from(*offset),
                                i128::from(location.section_range.start),
                            ))
                        }
                        AccessRoot::Symbol { symbol, addend } => {
                            let reference = facts.reference(symbol, c)?;
                            if same && symbol.object == occurrence.object {
                                if matches!(&location.selector,DataSelector::Symbol { symbol: selected, .. } if selected == symbol)
                                {
                                    Some((i128::from(*addend), 0))
                                } else if let Some(reference) = reference.filter(|reference| {
                                    reference.definition == SymbolDefinition::Section
                                        && reference.section == Some(location.section)
                                }) {
                                    let start = if recipe.address_space == CodeAddressSpace::Image {
                                        location.image_address
                                    } else {
                                        Some(location.section_range.start)
                                    };
                                    start.map(|start| {
                                        (
                                            i128::from(reference.offset) + i128::from(*addend),
                                            i128::from(start),
                                        )
                                    })
                                } else {
                                    *issue = Some(NavigationIssue::MissingReference);
                                    None
                                }
                            } else if reference.is_none_or(|reference| {
                                reference.definition != SymbolDefinition::Section
                            }) {
                                *issue = Some(NavigationIssue::MissingReference);
                                None
                            } else {
                                None
                            }
                        }
                        AccessRoot::EntryWord { .. } => {
                            *issue = Some(NavigationIssue::UnknownAddress);
                            None
                        }
                        _ => None,
                    };
                    if let Some((at, start)) = coordinate {
                        add_match(
                            matches,
                            alternative,
                            at + i128::from(path.offset),
                            start,
                            location.section_range.length,
                            r.width,
                            None,
                            None,
                            c,
                        )?;
                    }
                }
            }
        }
        Ok(())
    }
}
struct Match<'a> {
    alternative: u8,
    offset: i64,
    partial: bool,
    field: Option<&'a str>,
    argument: Option<u8>,
}
#[allow(clippy::too_many_arguments)]
fn add_match<'a>(
    matches: &mut AdmittedVec<'_, Match<'a>>,
    alternative: u8,
    at: i128,
    start: i128,
    length: u64,
    width: u8,
    field: Option<&'a str>,
    argument: Option<u8>,
    c: &mut dyn RunControl,
) -> Result<()> {
    let end = at + i128::from(width);
    let limit = start + i128::from(length);
    if at < limit && start < end {
        matches.push(
            Match {
                alternative,
                offset: i64::try_from(at - start)
                    .map_err(|_| invalid("navigation offset overflow"))?,
                partial: at < start || end > limit,
                field,
                argument,
            },
            c.position(),
        )?;
    }
    Ok(())
}
fn direction(want: AccessDirection, actual: MemoryKind) -> bool {
    match want {
        AccessDirection::Readers => matches!(
            actual,
            MemoryKind::Load | MemoryKind::LoadReserved | MemoryKind::Atomic
        ),
        AccessDirection::Writers => matches!(
            actual,
            MemoryKind::Store | MemoryKind::StoreConditional | MemoryKind::Atomic
        ),
    }
}
