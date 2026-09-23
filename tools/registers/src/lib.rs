//! Source-only publication from one validated reviewed model. No binary-analysis authority.
use oer_reviewed_contracts::ApplicabilityContext;
use open_esp_radio_register_model::{
    PacApiPack, RegisterEvidenceSet, RegisterLintPack, RegisterModel,
};
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

mod host;
mod memory;
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct Manifest {
    schema: u32,
    model: PathBuf,
    reviewed: Vec<PathBuf>,
    memory: PathBuf,
    ownership: PathBuf,
    api: PathBuf,
    lints: PathBuf,
    evidence: Vec<PathBuf>,
    applicability: Context,
    outputs: Outputs,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct Context {
    ecosystems: Vec<String>,
    chip: String,
    chip_revisions: Vec<String>,
    artifact_lineages: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct Outputs {
    svd: PathBuf,
    pac_raw: PathBuf,
    pac_api: PathBuf,
    bindings: PathBuf,
    crate_name: String,
    target: String,
    edition: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct Ownership {
    schema: u32,
    owned_ranges: Vec<String>,
}

/// Owns validated model, policy and output destinations. Rendering never reloads inputs.
pub struct Publication {
    model: RegisterModel,
    api: PacApiPack,
    outputs: Outputs,
}
impl Publication {
    /// Load explicitly selected source documents. Artifact-specific assertions cannot
    /// be authenticated by this source-only tool: no binary identities are invented.
    pub fn load(path: &Path) -> Result<Self> {
        let path = path.canonicalize()?;
        let base = path.parent().ok_or("manifest has no parent")?;
        let m: Manifest = toml_edit::de::from_str(&fs::read_to_string(&path)?)?;
        if m.schema != 1 || m.reviewed.is_empty() || m.evidence.is_empty() {
            return Err("schema 1 requires explicit reviewed packs and evidence catalogs".into());
        }
        let context = ApplicabilityContext::new(
            m.applicability.ecosystems,
            vec![m.applicability.chip],
            m.applicability.chip_revisions,
            m.applicability.artifact_lineages,
            vec![],
        )?;
        let mut model = RegisterModel::load(&base.join(&m.model))?;
        if context.chips != [model.chip()] {
            return Err("publication chip differs from register model".into());
        }
        let paths = |items: &[PathBuf]| items.iter().map(|p| base.join(p)).collect::<Vec<_>>();
        let review = open_radio_vendor_review::ReviewKnowledge::load_all(&paths(&m.reviewed))?
            .select_for(&context)?;
        model.apply_review_knowledge(&review)?;
        let api = PacApiPack::load(&base.join(&m.api))?;
        let lints = RegisterLintPack::load(&base.join(&m.lints))?;
        model.validate_lints(&lints)?;
        let evidence = RegisterEvidenceSet::load_all(&paths(&m.evidence))?;
        evidence.validate_references(
            "model review",
            model
                .review()
                .iter()
                .flat_map(|r| r.sources.iter().map(String::as_str))
                .chain(
                    model
                        .reviewed_register_facts()
                        .iter()
                        .flat_map(|a| a.metadata.evidence.iter().map(|e| e.source.as_str())),
                ),
        )?;
        evidence.validate_references("PAC API", api.source_ids())?;
        let memory = memory::Memory::load(&base.join(&m.memory))?;
        let ownership: Ownership =
            toml_edit::de::from_str(&fs::read_to_string(base.join(&m.ownership))?)?;
        if ownership.schema != 1 || ownership.owned_ranges.is_empty() {
            return Err("invalid ownership policy".into());
        }
        memory.validate(&model, &ownership.owned_ranges, &evidence)?;
        let (svd, _) = model.render_svd()?;
        api.validate_against_svd(&svd)?;
        let mut outputs = m.outputs;
        if !matches!(outputs.target.as_str(), "none" | "riscv")
            || !matches!(outputs.edition.as_str(), "2021" | "2024")
        {
            return Err("unsupported PAC target or edition".into());
        }
        open_esp_radio_register_model::validate_pac_crate_name(&outputs.crate_name)?;
        let mut input_paths: BTreeSet<_> = model.loaded_inputs().keys().cloned().collect();
        for p in m.reviewed.iter().chain(m.evidence.iter()).chain([
            &m.api,
            &m.lints,
            &m.memory,
            &m.ownership,
        ]) {
            input_paths.insert(base.join(p).canonicalize()?);
        }
        input_paths.insert(path.clone());
        let mut destinations = BTreeSet::new();
        for p in [
            &mut outputs.svd,
            &mut outputs.pac_raw,
            &mut outputs.pac_api,
            &mut outputs.bindings,
        ] {
            *p = host::destination(&base.join(&*p))?;
            if input_paths.contains(p) || !destinations.insert(p.clone()) {
                return Err("publication output aliases an input or another output".into());
            }
        }
        Ok(Self {
            model,
            api,
            outputs,
        })
    }

    /// Prepare all four outputs before checking or replacing any destination.
    /// Check is read-only. Each replacement is atomic; a failed batch is reported
    /// as incomplete rather than claiming cross-directory atomicity.
    pub fn generate(&self, check: bool) -> Result<()> {
        let (svd, _) = self.model.render_svd()?;
        let mut config = svd2rust::config::Config::default();
        config.target = if self.outputs.target == "none" {
            svd2rust::Target::None
        } else {
            svd2rust::Target::RISCV
        };
        config.edition = if self.outputs.edition == "2024" {
            svd2rust::config::RustEdition::E2024
        } else {
            svd2rust::config::RustEdition::E2021
        };
        config.strict = true;
        let mut raw = svd2rust::generate(&svd, &config)
            .map_err(|e| e.to_string())?
            .lib_rs;
        if self.api.options.allow_clippy_empty_docs {
            raw.insert_str(0, "#![allow(clippy::empty_docs)]\n");
        }
        raw.push_str(&self.api.render_rust(&svd)?);
        let raw = host::format(&raw, &self.outputs.edition)?;
        let api = host::format(&self.api.render_facade_rust()?, &self.outputs.edition)?;
        let bindings = open_esp_radio_register_model::generate_pac_binding_index(
            &svd,
            &self.outputs.crate_name,
        )?;
        let outputs = [
            (&self.outputs.svd, svd),
            (&self.outputs.pac_raw, raw),
            (&self.outputs.pac_api, api),
            (&self.outputs.bindings, bindings),
        ];
        host::publish(&outputs, check)
    }
}
