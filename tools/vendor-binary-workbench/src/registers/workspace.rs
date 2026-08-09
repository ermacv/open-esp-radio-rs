//! Project view of the editable register model and optional discovery facts.

use std::collections::BTreeSet;

use super::{RegisterFacts, RegisterModel, SvdExportSummary};
use crate::{Result, project::RegisterWorkspacePaths};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RegisterWorkspaceSummary {
    pub(crate) ranges: usize,
    pub(crate) observed: usize,
    pub(crate) reviewed: usize,
    pub(crate) ignored: usize,
    pub(crate) manual: usize,
    pub(crate) unreviewed: usize,
    pub(crate) fields: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct ProjectRegisterWorkspace {
    facts: Option<RegisterFacts>,
    model: Box<RegisterModel>,
    owned_ranges: BTreeSet<String>,
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
            model: Box::new(RegisterModel::load(&paths.model)?),
            owned_ranges: paths.owned_ranges.iter().cloned().collect(),
        })
    }

    pub(crate) fn summary(&self) -> Result<RegisterWorkspaceSummary> {
        let identities = self.model.register_identities()?;
        let (fact_keys, ignored_keys) = self.facts.as_ref().map_or_else(
            || Ok((BTreeSet::new(), BTreeSet::new())),
            |facts| observation_keys(facts, &self.owned_ranges),
        )?;
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
            observed: fact_keys.len() + ignored_keys.len(),
            reviewed: fact_keys.intersection(&model_keys).count(),
            ignored: ignored_keys.len(),
            manual: model_keys.difference(&fact_keys).count(),
            unreviewed: fact_keys.difference(&model_keys).count(),
            fields: self.model.render_svd()?.1.fields,
        })
    }

    pub(crate) fn render_svd(&self) -> Result<(String, SvdExportSummary)> {
        Ok(self.model.render_svd()?)
    }

    pub(crate) const fn format_label(&self) -> &'static str {
        "register-model-v2"
    }
}

fn observation_keys(
    facts: &RegisterFacts,
    owned_ranges: &BTreeSet<String>,
) -> Result<(BTreeSet<(u64, u32)>, BTreeSet<(u64, u32)>)> {
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
    let mut owned = BTreeSet::new();
    let mut ignored = BTreeSet::new();
    for fact in &facts.registers {
        let key = (u64::from(fact.address), u32::from(fact.width));
        let range = facts
            .ranges
            .iter()
            .find(|range| range.contains(fact.address))
            .expect("validated register facts have exactly one owning range");
        if owned_ranges.contains(&range.name) {
            owned.insert(key);
        } else {
            ignored.insert(key);
        }
    }
    Ok((owned, ignored))
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
        let (owned, ignored) = observation_keys(&facts, &["radio".to_owned()].into()).unwrap();
        assert_eq!(owned, [(0x1010, 32)].into());
        assert_eq!(ignored, [(0x3010, 32)].into());
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
        let error = observation_keys(&facts, &["missing".to_owned()].into()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("absent from MMIO discovery facts")
        );
    }
}
