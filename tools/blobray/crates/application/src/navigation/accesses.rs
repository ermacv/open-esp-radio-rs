//! Physical access matching of one selected captured object.
use super::*;
use blobray_analysis::navigation::{Facts, MemoryObservation};
pub(super) struct Target<'a> {
    occurrence: &'a Occurrence,
    location: DataLocation,
    access: Option<AccessDirection>,
}
impl<'a> Target<'a> {
    pub(super) fn new(
        project: &Project,
        query: &'a NavigationQuery,
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
                Some(Self {
                    occurrence,
                    location,
                    access: *access,
                })
            }
            _ => None,
        })
    }
    pub(super) fn data(&self) -> DataLocation {
        self.location.clone()
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
        facts.accesses(c, &mut |r, c| {
            if self.access.is_some_and(|want| !direction(want, r.access)) {
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
                .try_fold(0u64, |n, _: &Match| {
                    n.checked_add(std::mem::size_of::<LocationMatch>() as u64)
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
                });
            }
            emit(
                &NavigationRecord::Access {
                    function: node.function.clone(),
                    record: r.record,
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
    fn match_value(
        &self,
        r: &MemoryObservation<'_>,
        value: &AbstractValue,
        recipe: &FunctionRecipe,
        facts: &Facts<'_, '_>,
        alternative: u8,
        matches: &mut AdmittedVec<'_, Match>,
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
            if *source == self.occurrence.source
                && *object == self.occurrence.object
                && let Some(start) = self.location.image_address
            {
                add_match(
                    matches,
                    alternative,
                    i128::from(*address),
                    i128::from(start),
                    self.location.section_range.length,
                    r.width,
                    c,
                )?;
            }
            return Ok(());
        }
        let (paths, problem) = facts.paths(value, recipe, c)?;
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
            let (occurrence, location) = (self.occurrence, &self.location);
            let same = recipe.source == occurrence.source
                && *recipe.selector.object() == occurrence.object;
            let coordinate = match &path.root {
                AccessRoot::Address { address } if same => location
                    .image_address
                    .map(|start| (i128::from(*address), i128::from(start))),
                AccessRoot::Section { section, offset } if same && *section == location.section => {
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
                    } else if reference
                        .is_none_or(|reference| reference.definition != SymbolDefinition::Section)
                    {
                        *issue = Some(NavigationIssue::MissingReference);
                        None
                    } else {
                        None
                    }
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
                    c,
                )?;
            }
        }
        Ok(())
    }
}
struct Match {
    alternative: u8,
    offset: i64,
    partial: bool,
}
fn add_match(
    matches: &mut AdmittedVec<'_, Match>,
    alternative: u8,
    at: i128,
    start: i128,
    length: u64,
    width: u8,
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
