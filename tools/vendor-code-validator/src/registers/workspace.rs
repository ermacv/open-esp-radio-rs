//! Compatibility boundary between the legacy evidence overlay and register model v2.

use std::{collections::BTreeSet, path::Path};

use super::{
    RegisterFacts, RegisterModel, RegisterWorkspace, RegisterWorkspaceSummary, SvdExportProfile,
    SvdExportSummary,
};
use crate::Result;

#[derive(Clone, Debug)]
pub(crate) enum ProjectRegisterWorkspace {
    Legacy(RegisterWorkspace),
    Model {
        facts: Option<RegisterFacts>,
        model: Box<RegisterModel>,
    },
}

impl ProjectRegisterWorkspace {
    pub(crate) fn load(facts_path: &Path, model_path: &Path) -> Result<Self> {
        if RegisterModel::is_model_file(model_path)? {
            let facts = facts_path
                .is_file()
                .then(|| RegisterFacts::load(facts_path))
                .transpose()?;
            return Ok(Self::Model {
                facts,
                model: Box::new(RegisterModel::load(model_path)?),
            });
        }
        Ok(Self::Legacy(RegisterWorkspace::load(
            facts_path, model_path,
        )?))
    }

    pub(crate) fn summary(&self) -> Result<RegisterWorkspaceSummary> {
        match self {
            Self::Legacy(workspace) => Ok(workspace.summary()),
            Self::Model { facts, model } => {
                let identities = model.register_identities()?;
                let fact_keys = facts
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
                    ranges: facts.as_ref().map_or(0, |facts| facts.ranges.len()),
                    observed: fact_keys.len(),
                    reviewed: fact_keys.intersection(&model_keys).count(),
                    ignored: 0,
                    manual: model_keys.difference(&fact_keys).count(),
                    unreviewed: fact_keys.difference(&model_keys).count(),
                    fields: model.render_svd()?.1.fields,
                })
            }
        }
    }

    pub(crate) fn render_svd(
        &self,
        profile: SvdExportProfile,
    ) -> Result<(String, SvdExportSummary)> {
        match self {
            Self::Legacy(workspace) => workspace.render_svd(profile),
            Self::Model { model, .. } => {
                let _ = profile;
                Ok(model.render_svd()?)
            }
        }
    }

    pub(crate) const fn format_label(&self) -> &'static str {
        match self {
            Self::Legacy(_) => "legacy-overlay-v1",
            Self::Model { .. } => "register-model-v2",
        }
    }
}
