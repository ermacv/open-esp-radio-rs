//! Session-owned register observations and publication inspection inputs.
//! Capture happens once on first use; later queries never reopen these files.

use super::{ProjectSession, RegisterInventory, RegisterWorkspaceSummary};
use crate::{
    Result,
    registers::{RegisterFacts, RegisterModel},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[cfg(test)]
mod tests;

/// Immutable register graph retained independently of the session that loaded it.
/// The ID addresses the complete inventory, including sources and gap-only data.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RegisterInventorySnapshot {
    id: String,
    inventory: RegisterInventory,
    #[serde(skip)]
    subjects: Vec<String>,
}

impl RegisterInventorySnapshot {
    pub(crate) fn new(inventory: RegisterInventory) -> Result<Self> {
        let id = format!(
            "register-inventory:{:x}",
            Sha256::digest(serde_json::to_vec(&inventory)?)
        );
        let subjects = inventory.registers.keys().cloned().collect();
        Ok(Self {
            id,
            inventory,
            subjects,
        })
    }

    pub fn register_at(&self, index: usize) -> Option<&super::InventoryRegister> {
        self.subjects
            .get(index)
            .and_then(|id| self.inventory.registers.get(id))
    }

    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn inventory(&self) -> &RegisterInventory {
        &self.inventory
    }
}

pub(crate) struct RegisterQueryCapture {
    pub(crate) snapshot: Arc<RegisterInventorySnapshot>,
    pub(crate) publication: PublicationInputs,
}

pub(crate) struct PublicationInputs {
    model: std::result::Result<Option<RegisterModel>, String>,
    facts: std::result::Result<Option<RegisterFacts>, String>,
    scopes: std::result::Result<Option<crate::review_scopes::ReviewScopesDocument>, String>,
    assertions: std::result::Result<Vec<open_radio_vendor_review::EffectiveAssertion>, String>,
    pub(crate) summary: std::result::Result<Option<RegisterWorkspaceSummary>, String>,
}

fn captured<T>(value: &std::result::Result<T, String>) -> Result<&T> {
    value
        .as_ref()
        .map_err(|reason| crate::Error::invalid(reason.clone()))
}

impl PublicationInputs {
    pub(crate) fn capture(
        project: &crate::ProjectSpec,
        facts: std::result::Result<Option<RegisterFacts>, String>,
    ) -> Self {
        let model = project
            .registers
            .as_ref()
            .map(crate::registers::load_effective_register_model)
            .transpose()
            .map_err(|error| error.to_string());
        let scopes = project
            .review
            .as_ref()
            .map(|_| crate::review_scopes::load_for_project(project))
            .transpose()
            .map_err(|error| error.to_string());
        let assertions =
            open_radio_vendor_review::ReviewKnowledge::load_all(&project.reviewed_knowledge)
                .and_then(|knowledge| knowledge.select_for(&project.review_context))
                .map(|knowledge| knowledge.assertions().values().cloned().collect())
                .map_err(|error| error.to_string());
        let summary = (|| -> Result<Option<RegisterWorkspaceSummary>> {
            let Some(paths) = &project.registers else {
                return Ok(None);
            };
            let Some(model) = captured(&model)? else {
                return Ok(None);
            };
            let facts = captured(&facts)?;
            crate::registers::ProjectRegisterWorkspace::from_captured(
                paths,
                model.clone(),
                facts.clone(),
            )
            .summary()
            .map(Some)
        })()
        .map_err(|error| error.to_string());
        Self {
            model,
            facts,
            scopes,
            assertions,
            summary,
        }
    }

    pub(crate) fn model(&self) -> Result<&RegisterModel> {
        captured(&self.model)?
            .as_ref()
            .ok_or_else(|| crate::Error::invalid("register model is not configured"))
    }
    pub(crate) fn facts(&self) -> Result<Option<&RegisterFacts>> {
        Ok(captured(&self.facts)?.as_ref())
    }
    pub(crate) fn scopes(&self) -> Result<Option<&crate::review_scopes::ReviewScopesDocument>> {
        Ok(captured(&self.scopes)?.as_ref())
    }
    pub(crate) fn assertions(&self) -> Result<&[open_radio_vendor_review::EffectiveAssertion]> {
        Ok(captured(&self.assertions)?)
    }
}

impl RegisterQueryCapture {
    pub(crate) fn capture(session: &ProjectSession) -> Result<Self> {
        let inventory = super::register_inventory::load(session)?;
        let facts = session
            .project
            .registers
            .as_ref()
            .map(|paths| {
                let input = session.artifacts.read_text(&paths.facts)?;
                RegisterFacts::parse(&input)
            })
            .transpose()
            .map_err(|error: crate::Error| error.to_string());
        let publication = PublicationInputs::capture(&session.project, facts);
        Ok(Self {
            snapshot: Arc::new(RegisterInventorySnapshot::new(inventory)?),
            publication,
        })
    }
}
