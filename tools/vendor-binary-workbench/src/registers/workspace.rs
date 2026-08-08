//! Project view of the editable register model and optional discovery facts.

use std::{collections::BTreeSet, path::Path};

use super::{RegisterFacts, RegisterModel, SvdExportSummary};
use crate::Result;

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
}

impl ProjectRegisterWorkspace {
    pub(crate) fn load(facts_path: &Path, model_path: &Path) -> Result<Self> {
        if !RegisterModel::is_model_file(model_path)? {
            return Err(crate::Error::invalid(format!(
                "register workspace {} is not a register-model-v2 manifest",
                model_path.display()
            )));
        }
        let facts = facts_path
            .is_file()
            .then(|| RegisterFacts::load(facts_path))
            .transpose()?;
        Ok(Self {
            facts,
            model: Box::new(RegisterModel::load(model_path)?),
        })
    }

    pub(crate) fn summary(&self) -> Result<RegisterWorkspaceSummary> {
        let identities = self.model.register_identities()?;
        let fact_keys = self
            .facts
            .as_ref()
            .map(|facts| {
                facts
                    .registers
                    .iter()
                    .map(|fact| (u64::from(fact.address), u32::from(fact.width)))
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        let model_keys = identities.keys().copied().collect::<BTreeSet<_>>();
        Ok(RegisterWorkspaceSummary {
            ranges: self.facts.as_ref().map_or(0, |facts| facts.ranges.len()),
            observed: fact_keys.len(),
            reviewed: fact_keys.intersection(&model_keys).count(),
            ignored: 0,
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
