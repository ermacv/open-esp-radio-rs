//! Project view of the editable register model and optional discovery facts.

use std::collections::BTreeSet;

use super::{
    RegisterFacts, RegisterModel, SvdExportSummary, observation_is_reviewed,
    physical_register_is_observed,
};
use crate::{Result, project::RegisterWorkspacePaths};

/// Load reusable chip register geometry and apply only the sparse facts
/// selected by the project. Project-aware consumers use this function so
/// inspection, validation and publication all see the same effective model.
pub(crate) fn load_effective_register_model(
    paths: &RegisterWorkspacePaths,
) -> Result<RegisterModel> {
    let mut model = RegisterModel::load(&paths.model)?;
    if !paths.reviewed_knowledge.is_empty() {
        let knowledge =
            open_radio_vendor_review::ReviewKnowledge::load_all(&paths.reviewed_knowledge)
                .map_err(|error| {
                    crate::Error::invalid(format!(
                        "cannot compose reviewed knowledge over register model: {error}"
                    ))
                })?;
        model.apply_review_knowledge(&knowledge)?;
    }
    Ok(model)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RegisterWorkspaceSummary {
    pub(crate) ranges: usize,
    pub(crate) observed: usize,
    pub(crate) reviewed: usize,
    pub(crate) ignored: usize,
    pub(crate) non_operational: usize,
    pub(crate) manual: usize,
    pub(crate) unreviewed: usize,
    pub(crate) fields: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct ProjectRegisterWorkspace {
    facts: Option<RegisterFacts>,
    model: Box<RegisterModel>,
    owned_ranges: BTreeSet<String>,
    non_operational_functions: BTreeSet<String>,
}

impl ProjectRegisterWorkspace {
    pub(crate) fn load(paths: &RegisterWorkspacePaths) -> Result<Self> {
        if !RegisterModel::is_model_file(&paths.model)? {
            return Err(crate::Error::invalid(format!(
                "register workspace {} is not a register-model-v2 manifest",
                paths.model.display()
            )));
        }
        let facts = paths
            .facts
            .is_file()
            .then(|| RegisterFacts::load(&paths.facts))
            .transpose()?;
        Ok(Self {
            facts,
            model: Box::new(load_effective_register_model(paths)?),
            owned_ranges: paths.owned_ranges.iter().cloned().collect(),
            non_operational_functions: paths.non_operational_functions.iter().cloned().collect(),
        })
    }

    pub(crate) fn summary(&self) -> Result<RegisterWorkspaceSummary> {
        let identities = self.model.register_identities()?;
        let observations = self.facts.as_ref().map_or_else(
            || Ok(ObservationKeys::default()),
            |facts| observation_keys(facts, &self.owned_ranges, &self.non_operational_functions),
        )?;
        let fact_keys = observations.all();
        let model_keys = identities.keys().copied().collect::<BTreeSet<_>>();
        if let Some(facts) = &self.facts
            && let Some((address, width)) = model_keys.iter().find(|(address, width)| {
                let Ok(address) = u32::try_from(*address) else {
                    return true;
                };
                let byte_width = u64::from(*width).div_ceil(8);
                let end = u64::from(address).saturating_add(byte_width);
                !facts.ranges.iter().any(|range| {
                    self.owned_ranges.contains(&range.name)
                        && range.contains(address)
                        && end <= u64::from(range.end)
                })
            })
        {
            return Err(crate::Error::invalid(format!(
                "register model entry at {address:#010x}/{width} lies outside [registers].owned-ranges"
            )));
        }
        Ok(RegisterWorkspaceSummary {
            ranges: self.facts.as_ref().map_or(0, |facts| facts.ranges.len()),
            observed: fact_keys.len(),
            reviewed: observations
                .owned
                .iter()
                .filter(|observation| observation_is_reviewed(&identities, observation))
                .count(),
            ignored: observations.outside_scope.len(),
            non_operational: observations
                .non_operational
                .iter()
                .filter(|observation| !observation_is_reviewed(&identities, observation))
                .count(),
            manual: model_keys
                .iter()
                .filter(|register| !physical_register_is_observed(register, &fact_keys))
                .count(),
            unreviewed: observations
                .owned
                .iter()
                .filter(|observation| !observation_is_reviewed(&identities, observation))
                .filter(|key| !observations.non_operational.contains(key))
                .count(),
            fields: self.model.render_svd()?.1.fields,
        })
    }

    pub(crate) fn unreviewed_in_mmio_scope(&self, scope: &BTreeSet<(u32, u8)>) -> Result<usize> {
        Ok(self.unreviewed_mmio_in_scope(scope)?.len())
    }

    pub(crate) fn unreviewed_mmio_in_scope(
        &self,
        scope: &BTreeSet<(u32, u8)>,
    ) -> Result<BTreeSet<(u32, u8)>> {
        if scope.is_empty() {
            return Ok(BTreeSet::new());
        }
        let identities = self.model.register_identities()?;
        let facts = self.facts.as_ref().ok_or_else(|| {
            crate::Error::invalid(
                "release-scoped register validation requires MMIO discovery facts",
            )
        })?;
        validate_non_operational_functions(facts, &self.non_operational_functions)?;
        let mut unreviewed = BTreeSet::new();
        for &(address, width) in scope {
            let byte_width = u64::from(width).div_ceil(8);
            let end = u64::from(address).saturating_add(byte_width);
            let Some(range) = facts
                .ranges
                .iter()
                .find(|range| range.contains(address) && end <= u64::from(range.end))
            else {
                return Err(crate::Error::invalid(format!(
                    "publication scope MMIO {address:#010x}/{width} lies outside discovery ranges"
                )));
            };
            if !self.owned_ranges.contains(&range.name) {
                continue;
            }
            let key = (u64::from(address), u32::from(width));
            if observation_is_reviewed(&identities, &key) {
                continue;
            }
            let non_operational = facts
                .registers
                .iter()
                .find(|fact| fact.address == address && fact.width == width)
                .is_some_and(|fact| fact_is_non_operational(fact, &self.non_operational_functions));
            if !non_operational {
                unreviewed.insert((address, width));
            }
        }
        Ok(unreviewed)
    }

    pub(crate) fn render_svd(&self) -> Result<(String, SvdExportSummary)> {
        Ok(self.model.render_svd()?)
    }

    pub(crate) const fn format_label(&self) -> &'static str {
        "register-model-v2"
    }
}

#[derive(Debug, Default)]
struct ObservationKeys {
    owned: BTreeSet<(u64, u32)>,
    outside_scope: BTreeSet<(u64, u32)>,
    non_operational: BTreeSet<(u64, u32)>,
}

impl ObservationKeys {
    fn all(&self) -> BTreeSet<(u64, u32)> {
        self.owned.union(&self.outside_scope).copied().collect()
    }
}

fn observation_keys(
    facts: &RegisterFacts,
    owned_ranges: &BTreeSet<String>,
    non_operational_functions: &BTreeSet<String>,
) -> Result<ObservationKeys> {
    let available = facts
        .ranges
        .iter()
        .map(|range| range.name.as_str())
        .collect::<BTreeSet<_>>();
    if let Some(missing) = owned_ranges
        .iter()
        .find(|name| !available.contains(name.as_str()))
    {
        return Err(crate::Error::invalid(format!(
            "register owned range {missing:?} is absent from MMIO discovery facts"
        )));
    }
    validate_non_operational_functions(facts, non_operational_functions)?;
    let mut observations = ObservationKeys::default();
    for fact in &facts.registers {
        let key = (u64::from(fact.address), u32::from(fact.width));
        let range = facts
            .ranges
            .iter()
            .find(|range| range.contains(fact.address))
            .expect("validated register facts have exactly one owning range");
        if owned_ranges.contains(&range.name) {
            observations.owned.insert(key);
            if fact_is_non_operational(fact, non_operational_functions) {
                observations.non_operational.insert(key);
            }
        } else {
            observations.outside_scope.insert(key);
        }
    }
    Ok(observations)
}

pub(crate) fn fact_is_non_operational(
    fact: &super::RegisterFact,
    non_operational_functions: &BTreeSet<String>,
) -> bool {
    let mut functions = fact.read_functions.iter().chain(&fact.write_functions);
    let Some(first) = functions.next() else {
        return false;
    };
    non_operational_functions.contains(first)
        && functions.all(|function| non_operational_functions.contains(function))
}

pub(crate) fn validate_non_operational_functions(
    facts: &RegisterFacts,
    non_operational_functions: &BTreeSet<String>,
) -> Result<()> {
    let observed = facts
        .registers
        .iter()
        .flat_map(|fact| fact.read_functions.iter().chain(&fact.write_functions))
        .collect::<BTreeSet<_>>();
    if let Some(function) = non_operational_functions
        .iter()
        .find(|function| !observed.contains(function))
    {
        return Err(crate::Error::invalid(format!(
            "register review non-operational function {function:?} is absent from MMIO discovery facts"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registers::{FactRange, RegisterFact};

    fn fact(address: u32) -> RegisterFact {
        RegisterFact {
            address,
            width: 32,
            catalog_name: "UNMAPPED".to_owned(),
            reads: 1,
            writes: 0,
            read_functions: BTreeSet::new(),
            write_functions: BTreeSet::new(),
            read_sites: BTreeSet::new(),
            write_sites: BTreeSet::new(),
            write_patterns: Vec::new(),
            candidate_masks: Vec::new(),
        }
    }

    #[test]
    fn publication_scope_keeps_external_observations_visible_but_non_blocking() {
        let facts = RegisterFacts {
            ranges: vec![
                FactRange {
                    name: "radio".to_owned(),
                    start: 0x1000,
                    end: 0x2000,
                },
                FactRange {
                    name: "system".to_owned(),
                    start: 0x3000,
                    end: 0x4000,
                },
            ],
            registers: vec![fact(0x1010), fact(0x3010)],
        };
        let observations =
            observation_keys(&facts, &["radio".to_owned()].into(), &BTreeSet::new()).unwrap();
        assert_eq!(observations.owned, [(0x1010, 32)].into());
        assert_eq!(observations.outside_scope, [(0x3010, 32)].into());
    }

    #[test]
    fn publication_scope_rejects_a_range_missing_from_generated_facts() {
        let facts = RegisterFacts {
            ranges: vec![FactRange {
                name: "radio".to_owned(),
                start: 0x1000,
                end: 0x2000,
            }],
            registers: Vec::new(),
        };
        let error =
            observation_keys(&facts, &["missing".to_owned()].into(), &BTreeSet::new()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("absent from MMIO discovery facts")
        );
    }

    #[test]
    fn non_operational_policy_excludes_only_exclusive_observations() {
        let mut diagnostic = fact(0x1010);
        diagnostic.read_functions = ["archive:dump".to_owned()].into();
        let mut mixed = fact(0x1020);
        mixed.read_functions = ["archive:dump".to_owned(), "rom:enable".to_owned()].into();
        let facts = RegisterFacts {
            ranges: vec![FactRange {
                name: "radio".to_owned(),
                start: 0x1000,
                end: 0x2000,
            }],
            registers: vec![diagnostic, mixed],
        };
        let observations = observation_keys(
            &facts,
            &["radio".to_owned()].into(),
            &["archive:dump".to_owned()].into(),
        )
        .unwrap();
        assert_eq!(observations.non_operational, [(0x1010, 32)].into());
        assert!(observations.owned.contains(&(0x1020, 32)));
        assert!(!observations.non_operational.contains(&(0x1020, 32)));
    }

    #[test]
    fn non_operational_policy_rejects_stale_function_names() {
        let facts = RegisterFacts {
            ranges: vec![FactRange {
                name: "radio".to_owned(),
                start: 0x1000,
                end: 0x2000,
            }],
            registers: vec![fact(0x1010)],
        };
        let error = observation_keys(
            &facts,
            &["radio".to_owned()].into(),
            &["archive:missing".to_owned()].into(),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("absent from MMIO discovery facts")
        );
    }
}
