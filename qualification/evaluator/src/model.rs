use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{
    Result,
    hil::{HilEvidenceIndex, HilEvidenceSummary, HilRequirement, RepositoryState, ScenarioCatalog},
};

mod catalog;
mod source_contract;
pub(crate) use catalog::{
    CAPABILITY_CATALOG_SCHEMA, CapabilityOrigin, CapabilityScope, CatalogView, SourceIdentity,
};
pub(crate) use source_contract::SourceContract;

pub(crate) const QUALIFICATION_SCHEMA: u16 = 4;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Axis {
    Implementation,
    Host,
    Vendor,
    Hil,
    Async,
}

impl Axis {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Implementation => "implementation",
            Self::Host => "host",
            Self::Vendor => "vendor",
            Self::Hil => "hil",
            Self::Async => "async",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ImplementationProof {
    Complete,
    Incomplete,
}

impl ImplementationProof {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Incomplete => "incomplete",
        }
    }

    pub(crate) const fn is_terminal(self) -> bool {
        matches!(self, Self::Complete)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum HostProof {
    Covered,
    Incomplete,
}

impl HostProof {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Covered => "covered",
            Self::Incomplete => "incomplete",
        }
    }

    pub(crate) const fn is_terminal(self) -> bool {
        matches!(self, Self::Covered)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum VendorProof {
    Qualified,
    Mapped,
    Unmapped,
    NotApplicable,
}

impl VendorProof {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Qualified => "qualified",
            Self::Mapped => "mapped",
            Self::Unmapped => "unmapped",
            Self::NotApplicable => "not-applicable",
        }
    }

    pub(crate) const fn is_qualified(self) -> bool {
        matches!(self, Self::Qualified)
    }

    pub(crate) const fn is_terminal(self) -> bool {
        matches!(self, Self::Qualified | Self::NotApplicable)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HilProof {
    Qualified,
    Missing,
    NotApplicable,
}

impl HilProof {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Qualified => "qualified",
            Self::Missing => "missing",
            Self::NotApplicable => "not-applicable",
        }
    }

    pub(crate) const fn is_qualified(self) -> bool {
        matches!(self, Self::Qualified)
    }

    pub(crate) const fn is_terminal(self) -> bool {
        matches!(self, Self::Qualified | Self::NotApplicable)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum AsyncProof {
    Bounded,
    Incomplete,
    NotApplicable,
}

impl AsyncProof {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Bounded => "bounded",
            Self::Incomplete => "incomplete",
            Self::NotApplicable => "not-applicable",
        }
    }

    pub(crate) const fn is_terminal(self) -> bool {
        matches!(self, Self::Bounded | Self::NotApplicable)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Gap {
    pub(crate) axis: Axis,
    pub(crate) id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Capability {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) scope: String,
    pub(crate) implementation: ImplementationProof,
    pub(crate) host: HostProof,
    pub(crate) vendor: VendorProof,
    pub(crate) hil: HilProof,
    pub(crate) async_proof: AsyncProof,
    pub(crate) dependencies: Vec<String>,
    pub(crate) gaps: Vec<Gap>,
    pub(crate) evidence: Vec<String>,
    pub(crate) source_contracts: Vec<SourceContract>,
    pub(crate) vendor_evidence: Vec<VendorEvidenceRef>,
    pub(crate) vendor_not_applicable: Option<String>,
    pub(crate) hil_requirements: Vec<HilRequirement>,
    pub(crate) hil_not_applicable: Option<String>,
    pub(crate) async_not_applicable: Option<String>,
}

impl Capability {
    pub(crate) fn proof_ready(&self) -> bool {
        self.implementation.is_terminal()
            && self.host.is_terminal()
            && self.vendor.is_terminal()
            && self.hil.is_terminal()
            && self.async_proof.is_terminal()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Qualification {
    pub(crate) target: String,
    pub(crate) repository: RepositoryState,
    pub(crate) evidence_inputs: EvidenceInputs,
    pub(crate) capabilities: BTreeMap<String, Capability>,
    pub(crate) program_source: SourceIdentity,
    pub(crate) catalog_sources: Vec<SourceIdentity>,
    pub(crate) capability_origins: BTreeMap<String, CapabilityOrigin>,
    pub(crate) catalog_scopes: BTreeMap<String, CapabilityScope>,
    pub(crate) catalog: CatalogView,
    pub(crate) direct_catalog_capabilities: BTreeSet<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct EvidenceInputs {
    pub(crate) verification_entries: usize,
    pub(crate) verification_current_release_entries: usize,
    pub(crate) hil: HilEvidenceSummary,
    pub(crate) verification_project: PathBuf,
    pub(crate) vendor_evidence_index: PathBuf,
    pub(crate) hil_catalog: PathBuf,
    pub(crate) hil_runs: PathBuf,
}

impl Qualification {
    pub(crate) fn load_and_evaluate(path: &Path, root: &Path) -> Result<Self> {
        ManifestDocument::load_and_validate(path, root)?.evaluate(root)
    }

    pub(crate) fn is_ready(&self, id: &str) -> bool {
        fn visit(qualification: &Qualification, id: &str) -> bool {
            let capability = &qualification.capabilities[id];
            capability.proof_ready()
                && capability
                    .dependencies
                    .iter()
                    .all(|dependency| visit(qualification, dependency))
        }
        visit(self, id)
    }

    pub(crate) fn ready_count(&self) -> usize {
        self.capabilities
            .keys()
            .filter(|id| self.is_ready(id))
            .count()
    }

    pub(crate) fn all_required_ready(&self) -> bool {
        !self.capabilities.is_empty() && self.ready_count() == self.capabilities.len()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct ManifestDocument {
    schema: u16,
    target: String,
    required_capabilities: Vec<String>,
    #[serde(default)]
    catalogs: Vec<PathBuf>,
    #[serde(default)]
    catalog_capabilities: Vec<String>,
    verification: VerificationConfig,
    hil: HilConfig,
    #[serde(default)]
    capabilities: Vec<CapabilityDocument>,
    #[serde(skip)]
    program_source: Option<SourceIdentity>,
    #[serde(skip)]
    catalog_sources: Vec<SourceIdentity>,
    #[serde(skip)]
    capability_origins: BTreeMap<String, CapabilityOrigin>,
    #[serde(skip)]
    catalog_scopes: BTreeMap<String, CapabilityScope>,
    #[serde(skip)]
    catalog: CatalogView,
    #[serde(skip)]
    direct_catalog_capabilities: BTreeSet<String>,
}

pub(super) struct ValidatedProgram {
    document: ManifestDocument,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct VerificationConfig {
    project: PathBuf,
    evidence_index: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct HilConfig {
    target: String,
    catalog: PathBuf,
    runs: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct VendorRoot {
    pub(crate) source: String,
    pub(crate) symbol: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct VendorEvidenceRef {
    pub(crate) suite: String,
    pub(crate) source: String,
    pub(crate) symbol: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct HilRequirementDocument {
    pub(crate) scenario: String,
    #[serde(default = "one_repetition")]
    pub(crate) minimum_repetitions: u8,
}

impl HilRequirementDocument {
    fn validated(&self, capability: &str) -> Result<HilRequirement> {
        let scenario = slug(&self.scenario, "HIL scenario")?;
        if !(1..=20).contains(&self.minimum_repetitions) {
            return Err(format!(
                "HIL requirement {} for {} has minimum-repetitions outside 1..=20",
                self.scenario, capability
            )
            .into());
        }
        Ok(HilRequirement {
            scenario,
            minimum_repetitions: self.minimum_repetitions,
        })
    }
}

const fn one_repetition() -> u8 {
    1
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct GapDocument {
    pub(crate) axis: Axis,
    pub(crate) id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct CapabilityDocument {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) scope: String,
    pub(crate) implementation: ImplementationProof,
    pub(crate) host: HostProof,
    #[serde(rename = "async")]
    pub(crate) async_proof: AsyncProof,
    #[serde(default)]
    pub(crate) vendor_roots: Vec<VendorRoot>,
    #[serde(default)]
    pub(crate) vendor_evidence: Vec<VendorEvidenceRef>,
    #[serde(default)]
    pub(crate) vendor_anchors: Vec<PathBuf>,
    #[serde(default)]
    pub(crate) vendor_not_applicable: Option<String>,
    #[serde(default)]
    pub(crate) hil_requirements: Vec<HilRequirementDocument>,
    #[serde(default)]
    pub(crate) hil_not_applicable: Option<String>,
    #[serde(default)]
    pub(crate) async_not_applicable: Option<String>,
    #[serde(default)]
    pub(crate) depends_on: Vec<String>,
    #[serde(default)]
    pub(crate) gaps: Vec<GapDocument>,
    #[serde(default)]
    pub(crate) source_contracts: Vec<SourceContract>,
    #[serde(default)]
    pub(crate) source_fact_refs: Vec<String>,
    #[serde(default)]
    pub(crate) catalog_scope: Option<CapabilityScope>,
}

impl ManifestDocument {
    pub(super) fn load_and_validate(path: &Path, root: &Path) -> Result<ValidatedProgram> {
        if !root.is_dir() {
            return Err(format!("repository root {} is not a directory", root.display()).into());
        }
        let input = fs::read_to_string(path).map_err(|error| {
            format!(
                "cannot read qualification manifest {}: {error}",
                path.display()
            )
        })?;
        let document: Self = toml_edit::de::from_str(&input).map_err(|error| {
            format!(
                "cannot parse qualification manifest {}: {error}",
                path.display()
            )
        })?;
        let resolved = document.resolve_catalogs(root, path, &input)?;
        resolved.validate_program_structure(root)?;
        Ok(ValidatedProgram { document: resolved })
    }

    fn validate_program_structure(&self, root: &Path) -> Result<()> {
        let target = slug(&self.target, "qualification target")?;
        slug(&self.hil.target, "HIL target")?;
        validate_relative_path(&self.verification.project)?;
        validate_relative_path(&self.verification.evidence_index)?;
        validate_relative_path(&self.hil.catalog)?;
        validate_relative_path(&self.hil.runs)?;

        let mut required = BTreeSet::new();
        for id in &self.required_capabilities {
            let id = slug(id, "required capability")?;
            if !required.insert(id.clone()) {
                return Err(format!("duplicate required capability {id}").into());
            }
        }
        if required.is_empty() {
            return Err(
                format!("qualification program {target} has no required capabilities").into(),
            );
        }

        let dispositions = DispositionIndex::load_project(&root.join(&self.verification.project))?;
        validate_evidence_index_binding(root, &self.verification, &dispositions)?;
        let scenarios = ScenarioCatalog::load(root, &self.hil.catalog)?;
        let context = StaticContext {
            root,
            dispositions: &dispositions,
            scenario_catalog: &scenarios,
        };
        let mut declarations = BTreeMap::new();
        for capability in &self.capabilities {
            validate_capability_declaration(capability, &context)?;
            if declarations
                .insert(capability.id.clone(), capability.clone())
                .is_some()
            {
                return Err(format!(
                    "qualification manifest repeats capability {}",
                    capability.id
                )
                .into());
            }
        }
        let actual = declarations.keys().cloned().collect::<BTreeSet<_>>();
        if actual != required {
            let missing = required.difference(&actual).cloned().collect::<Vec<_>>();
            let undeclared = actual.difference(&required).cloned().collect::<Vec<_>>();
            return Err(format!(
                "qualification root mismatch: missing=[{}], undeclared=[{}]",
                missing.join(", "),
                undeclared.join(", ")
            )
            .into());
        }
        catalog::validate_catalog_dependencies(&declarations, &BTreeSet::new())
    }
}

impl ValidatedProgram {
    pub(super) fn catalog(&self) -> &CatalogView {
        &self.document.catalog
    }

    fn evaluate(self, root: &Path) -> Result<Qualification> {
        let document = self.document;
        let program_source = document
            .program_source
            .clone()
            .ok_or("qualification program was not resolved")?;
        let target = slug(&document.target, "qualification target")?;
        let hil_target = slug(&document.hil.target, "HIL target")?;
        let dispositions =
            DispositionIndex::load_project(&root.join(&document.verification.project))?;
        let configured_index = root.join(&document.verification.evidence_index);
        let repository = RepositoryState::read(root)?;
        let vendor_index = VendorEvidenceIndex::load(&configured_index, &dispositions.project_id)?;
        let scenario_catalog = ScenarioCatalog::load(root, &document.hil.catalog)?;
        let hil_index = HilEvidenceIndex::load(root, &document.hil.runs, &hil_target, &repository)?;
        let evidence_inputs = EvidenceInputs {
            verification_entries: vendor_index.entries.len(),
            verification_current_release_entries: vendor_index
                .current_release_count(root, !repository.dirty),
            hil: hil_index.summary().clone(),
            verification_project: document.verification.project.clone(),
            vendor_evidence_index: document.verification.evidence_index.clone(),
            hil_catalog: document.hil.catalog.clone(),
            hil_runs: document.hil.runs.clone(),
        };
        let context = EvaluationContext {
            root,
            dispositions: &dispositions,
            vendor_index: &vendor_index,
            scenario_catalog: &scenario_catalog,
            hil_index: &hil_index,
            evaluator_clean: !repository.dirty,
        };

        let mut capabilities = BTreeMap::new();
        for capability_document in document.capabilities {
            let capability = evaluate_capability(capability_document, &context)?;
            if capabilities
                .insert(capability.id.clone(), capability)
                .is_some()
            {
                return Err("qualification manifest repeats a capability id".into());
            }
        }
        validate_dependencies(&capabilities)?;
        Ok(Qualification {
            target,
            repository,
            evidence_inputs,
            capabilities,
            program_source,
            catalog_sources: document.catalog_sources,
            capability_origins: document.capability_origins,
            catalog_scopes: document.catalog_scopes,
            catalog: document.catalog,
            direct_catalog_capabilities: document.direct_catalog_capabilities,
        })
    }
}

fn validate_evidence_index_binding(
    root: &Path,
    verification: &VerificationConfig,
    dispositions: &DispositionIndex,
) -> Result<()> {
    let configured_index = root.join(&verification.evidence_index);
    let project_index = dispositions
        .vendor_evidence_index
        .as_ref()
        .ok_or("verification project has no evidence-index output")?;
    if configured_index != *project_index {
        return Err(format!(
            "qualification vendor evidence index {} does not match verification project output {}",
            configured_index.display(),
            project_index.display()
        )
        .into());
    }
    Ok(())
}

pub(crate) struct StaticContext<'a> {
    root: &'a Path,
    dispositions: &'a DispositionIndex,
    scenario_catalog: &'a ScenarioCatalog,
}

struct ValidatedDeclaration {
    id: String,
    dependencies: Vec<String>,
    gaps: Vec<Gap>,
    hil_requirements: Vec<HilRequirement>,
}

struct EvaluationContext<'a> {
    root: &'a Path,
    dispositions: &'a DispositionIndex,
    vendor_index: &'a VendorEvidenceIndex,
    scenario_catalog: &'a ScenarioCatalog,
    hil_index: &'a HilEvidenceIndex,
    evaluator_clean: bool,
}

fn evaluate_capability(
    document: CapabilityDocument,
    context: &EvaluationContext<'_>,
) -> Result<Capability> {
    let validated = validate_capability_declaration_inner(
        &document,
        &StaticContext {
            root: context.root,
            dispositions: context.dispositions,
            scenario_catalog: context.scenario_catalog,
        },
    )?;
    let id = validated.id;
    let dependencies = validated.dependencies;
    let mut gaps = validated.gaps;
    let hil_requirements = validated.hil_requirements.clone();
    let implementation = document.implementation;
    let host = document.host;

    let mut evidence = Vec::new();

    let (vendor, vendor_evidence) = derive_vendor_proof(
        &id,
        &document,
        context.vendor_index,
        context.root,
        context.evaluator_clean,
    )?;
    evidence.extend(vendor_evidence);
    validate_vendor_contract(&id, &document, vendor, context.dispositions)?;
    if vendor.is_terminal() {
        if has_gap(&gaps, Axis::Vendor) {
            return Err(format!("terminal vendor axis for {id} retains a vendor gap").into());
        }
    } else {
        ensure_gap(&mut gaps, Axis::Vendor, "vendor-evidence-not-qualified");
    }

    let hil = if let Some(reason) = document.hil_not_applicable.as_deref() {
        validate_reason(reason, "hil-not-applicable", &id)?;
        if !document.hil_requirements.is_empty() || has_gap(&gaps, Axis::Hil) {
            return Err(format!(
                "HIL-not-applicable capability {id} cannot declare HIL requirements or gaps"
            )
            .into());
        }
        HilProof::NotApplicable
    } else {
        let requirements = validated.hil_requirements;
        let mut complete = !requirements.is_empty() && !has_gap(&gaps, Axis::Hil);
        for requirement in &requirements {
            match context.hil_index.evidence_for(requirement) {
                Some(reference) => evidence.push(reference),
                None => complete = false,
            }
        }
        if complete {
            HilProof::Qualified
        } else {
            ensure_gap(&mut gaps, Axis::Hil, "current-hil-evidence-missing");
            HilProof::Missing
        }
    };

    let async_proof = document.async_proof;

    gaps.sort_by(|left, right| {
        left.axis
            .cmp(&right.axis)
            .then_with(|| left.id.cmp(&right.id))
    });
    evidence.sort();
    evidence.dedup();
    Ok(Capability {
        id,
        title: document.title,
        scope: document.scope,
        implementation,
        host,
        vendor,
        hil,
        async_proof,
        dependencies,
        gaps,
        evidence,
        source_contracts: document.source_contracts,
        vendor_evidence: document.vendor_evidence,
        vendor_not_applicable: document.vendor_not_applicable,
        hil_requirements,
        hil_not_applicable: document.hil_not_applicable,
        async_not_applicable: document.async_not_applicable,
    })
}

pub(crate) fn validate_capability_declaration(
    document: &CapabilityDocument,
    context: &StaticContext<'_>,
) -> Result<()> {
    validate_capability_declaration_inner(document, context).map(|_| ())
}

fn validate_capability_declaration_inner(
    document: &CapabilityDocument,
    context: &StaticContext<'_>,
) -> Result<ValidatedDeclaration> {
    let id = slug(&document.id, "capability id")?;
    source_contract::validate(&document.source_contracts, context.root)?;
    if document.title.trim().is_empty() || document.scope.trim().is_empty() {
        return Err(format!("capability {id} needs a non-empty title and scope").into());
    }
    let dependencies = unique_slugs(document.depends_on.clone(), "dependency", &id)?;
    let mut gaps = Vec::new();
    let mut gap_keys = BTreeSet::new();
    for gap in &document.gaps {
        let gap = Gap {
            axis: gap.axis,
            id: slug(&gap.id, "gap id")?,
        };
        if !gap_keys.insert((gap.axis, gap.id.clone())) {
            return Err(format!(
                "capability {id} repeats {} gap {}",
                gap.axis.label(),
                gap.id
            )
            .into());
        }
        gaps.push(gap);
    }
    validate_vendor_anchors(&id, &document.vendor_anchors, context.root)?;
    validate_declared_axis(
        &id,
        Axis::Implementation,
        document.implementation.is_terminal(),
        &gaps,
    )?;
    validate_declared_axis(&id, Axis::Host, document.host.is_terminal(), &gaps)?;
    validate_async_declaration(
        &id,
        document.async_proof,
        document.async_not_applicable.as_deref(),
        &gaps,
    )?;

    if let Some(reason) = document.vendor_not_applicable.as_deref() {
        validate_reason(reason, "vendor-not-applicable", &id)?;
        if !document.vendor_roots.is_empty()
            || !document.vendor_anchors.is_empty()
            || !document.vendor_evidence.is_empty()
            || has_gap(&gaps, Axis::Vendor)
        {
            return Err(format!(
                "vendor-not-applicable capability {id} cannot claim vendor evidence or gaps"
            )
            .into());
        }
    }
    let roots = document
        .vendor_roots
        .iter()
        .map(|root| (root.source.as_str(), root.symbol.as_str()))
        .collect::<BTreeSet<_>>();
    if roots.len() != document.vendor_roots.len() {
        return Err(format!("capability {id} repeats a vendor root").into());
    }
    let mut evidence = BTreeSet::new();
    for reference in &document.vendor_evidence {
        slug(&reference.suite, "vendor evidence suite")?;
        if !matches!(reference.source.as_str(), "rom" | "archive") {
            return Err(format!("invalid vendor source {:?} for {id}", reference.source).into());
        }
        let key = (
            reference.suite.as_str(),
            reference.source.as_str(),
            reference.symbol.as_str(),
        );
        if !evidence.insert(key) {
            return Err(format!(
                "capability {id} repeats vendor evidence {} {} {}",
                reference.suite, reference.source, reference.symbol
            )
            .into());
        }
        if !roots.contains(&(reference.source.as_str(), reference.symbol.as_str())) {
            return Err(format!(
                "vendor evidence {} {} {} for {id} does not name a vendor root",
                reference.suite, reference.source, reference.symbol
            )
            .into());
        }
    }
    for root in &document.vendor_roots {
        if !matches!(root.source.as_str(), "rom" | "archive") {
            return Err(format!("invalid vendor source {:?} for {id}", root.source).into());
        }
        if root.symbol.trim().is_empty() {
            return Err(format!("vendor root for {id} has an empty symbol").into());
        }
        let disposition = context.dispositions.get(root).ok_or_else(|| {
            format!(
                "vendor root {} {} for {id} has no disposition",
                root.source, root.symbol
            )
        })?;
        if !disposition.has_rust_component {
            return Err(format!(
                "vendor root {} {} for {id} has no rust-component",
                root.source, root.symbol
            )
            .into());
        }
    }

    let mut hil_requirements = Vec::new();
    let mut scenarios = BTreeSet::new();
    if let Some(reason) = document.hil_not_applicable.as_deref() {
        validate_reason(reason, "hil-not-applicable", &id)?;
        if !document.hil_requirements.is_empty() || has_gap(&gaps, Axis::Hil) {
            return Err(format!(
                "HIL-not-applicable capability {id} cannot declare HIL requirements or gaps"
            )
            .into());
        }
    } else {
        for requirement in &document.hil_requirements {
            let requirement = requirement.validated(&id)?;
            if !scenarios.insert(requirement.scenario.clone()) {
                return Err(format!(
                    "capability {id} repeats HIL scenario {}",
                    requirement.scenario
                )
                .into());
            }
            context
                .scenario_catalog
                .validate_requirement(&requirement)?;
            hil_requirements.push(requirement);
        }
    }
    Ok(ValidatedDeclaration {
        id,
        dependencies,
        gaps,
        hil_requirements,
    })
}

fn validate_async_declaration(
    id: &str,
    proof: AsyncProof,
    not_applicable_reason: Option<&str>,
    gaps: &[Gap],
) -> Result<()> {
    match (proof, not_applicable_reason) {
        (AsyncProof::NotApplicable, Some(reason)) => {
            validate_reason(reason, "async-not-applicable", id)?;
        }
        (AsyncProof::NotApplicable, None) => {
            return Err(format!(
                "async-not-applicable capability {id} needs an async-not-applicable reason"
            )
            .into());
        }
        (_, Some(_)) => {
            return Err(format!(
                "capability {id} can only declare async-not-applicable when async is not-applicable"
            )
            .into());
        }
        (_, None) => {}
    }
    validate_declared_axis(id, Axis::Async, proof.is_terminal(), gaps)
}

fn validate_declared_axis(id: &str, axis: Axis, terminal: bool, gaps: &[Gap]) -> Result<()> {
    match (terminal, has_gap(gaps, axis)) {
        (true, true) => Err(format!(
            "terminal {} axis for {id} retains a {} gap",
            axis.label(),
            axis.label()
        )
        .into()),
        (false, false) => Err(format!(
            "incomplete {} axis for {id} needs a {} gap",
            axis.label(),
            axis.label()
        )
        .into()),
        _ => Ok(()),
    }
}

fn validate_vendor_anchors(id: &str, anchors: &[PathBuf], root: &Path) -> Result<()> {
    let repository = fs::canonicalize(root)?;
    let mut unique = BTreeSet::new();
    for anchor in anchors {
        validate_relative_path(anchor)?;
        if !unique.insert(anchor) {
            return Err(
                format!("capability {id} repeats vendor anchor {}", anchor.display()).into(),
            );
        }
        let mut path = repository.clone();
        let components = anchor.components().collect::<Vec<_>>();
        for (index, component) in components.iter().enumerate() {
            let Component::Normal(name) = component else {
                return Err(format!("unsafe vendor anchor {}", anchor.display()).into());
            };
            path.push(name);
            let metadata = fs::symlink_metadata(&path).map_err(|error| {
                format!("cannot inspect vendor anchor {}: {error}", anchor.display())
            })?;
            let final_component = index + 1 == components.len();
            if metadata.file_type().is_symlink()
                || (final_component && !metadata.file_type().is_file())
                || (!final_component && !metadata.file_type().is_dir())
            {
                return Err(format!(
                    "vendor anchor path must contain only regular directories and end in a regular file: {}",
                    anchor.display()
                )
                .into());
            }
        }
    }
    Ok(())
}

fn validate_vendor_contract(
    id: &str,
    document: &CapabilityDocument,
    proof: VendorProof,
    dispositions: &DispositionIndex,
) -> Result<()> {
    match proof {
        VendorProof::Qualified => {
            if document.vendor_roots.is_empty() || !document.vendor_anchors.is_empty() {
                return Err(format!(
                    "vendor-qualified capability {id} needs only executable vendor roots"
                )
                .into());
            }
        }
        VendorProof::Mapped => {
            if document.vendor_roots.is_empty() && document.vendor_anchors.is_empty() {
                return Err(format!("vendor-mapped capability {id} has no mapping").into());
            }
        }
        VendorProof::Unmapped | VendorProof::NotApplicable => {}
    }
    for root in &document.vendor_roots {
        if !matches!(root.source.as_str(), "rom" | "archive") {
            return Err(format!("invalid vendor source {:?} for {id}", root.source).into());
        }
        let disposition = dispositions.get(root).ok_or_else(|| {
            format!(
                "vendor root {} {} for {id} has no disposition",
                root.source, root.symbol
            )
        })?;
        if !disposition.has_rust_component {
            return Err(format!(
                "vendor root {} {} for {id} has no rust-component",
                root.source, root.symbol
            )
            .into());
        }
        if proof == VendorProof::Qualified && !disposition.has_contract {
            return Err(format!(
                "vendor-qualified root {} {} for {id} has no executable contract",
                root.source, root.symbol
            )
            .into());
        }
    }
    Ok(())
}

fn derive_vendor_proof(
    id: &str,
    document: &CapabilityDocument,
    index: &VendorEvidenceIndex,
    root: &Path,
    evaluator_clean: bool,
) -> Result<(VendorProof, Vec<String>)> {
    if let Some(reason) = document.vendor_not_applicable.as_deref() {
        validate_reason(reason, "vendor-not-applicable", id)?;
        if !document.vendor_roots.is_empty()
            || !document.vendor_anchors.is_empty()
            || !document.vendor_evidence.is_empty()
        {
            return Err(format!(
                "vendor-not-applicable capability {id} cannot claim vendor evidence"
            )
            .into());
        }
        return Ok((VendorProof::NotApplicable, Vec::new()));
    }
    let roots = document
        .vendor_roots
        .iter()
        .map(|root| (root.source.as_str(), root.symbol.as_str()))
        .collect::<BTreeSet<_>>();
    let mut references = BTreeSet::new();
    for reference in &document.vendor_evidence {
        let key = (
            reference.suite.as_str(),
            reference.source.as_str(),
            reference.symbol.as_str(),
        );
        if !references.insert(key) {
            return Err(format!(
                "capability {id} repeats vendor evidence {} {} {}",
                reference.suite, reference.source, reference.symbol
            )
            .into());
        }
        if !roots.contains(&(reference.source.as_str(), reference.symbol.as_str())) {
            return Err(format!(
                "vendor evidence {} {} {} for {id} does not name a vendor root",
                reference.suite, reference.source, reference.symbol
            )
            .into());
        }
    }
    let all_roots_release_eligible = !roots.is_empty()
        && roots.iter().all(|(source, symbol)| {
            document.vendor_evidence.iter().any(|reference| {
                reference.source == *source
                    && reference.symbol == *symbol
                    && index.get(reference).is_some_and(|entry| {
                        entry.is_current_release_evidence(root, evaluator_clean)
                    })
            })
        });
    if all_roots_release_eligible && document.vendor_anchors.is_empty() {
        let evidence = document
            .vendor_evidence
            .iter()
            .map(|reference| {
                format!(
                    "verification:{}/{}/{}",
                    reference.suite, reference.source, reference.symbol
                )
            })
            .collect();
        Ok((VendorProof::Qualified, evidence))
    } else if !document.vendor_roots.is_empty() || !document.vendor_anchors.is_empty() {
        Ok((VendorProof::Mapped, Vec::new()))
    } else {
        Ok((VendorProof::Unmapped, Vec::new()))
    }
}

fn has_gap(gaps: &[Gap], axis: Axis) -> bool {
    gaps.iter().any(|gap| gap.axis == axis)
}

fn ensure_gap(gaps: &mut Vec<Gap>, axis: Axis, id: &str) {
    if !has_gap(gaps, axis) {
        gaps.push(Gap {
            axis,
            id: id.to_owned(),
        });
    }
}

fn unique_slugs(values: Vec<String>, kind: &str, owner: &str) -> Result<Vec<String>> {
    let mut unique = BTreeSet::new();
    let mut output = Vec::new();
    for value in values {
        let value = slug(&value, kind)?;
        if !unique.insert(value.clone()) {
            return Err(format!("{owner} repeats {kind} {value}").into());
        }
        output.push(value);
    }
    Ok(output)
}

fn slug(value: &str, kind: &str) -> Result<String> {
    let valid = value
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_lowercase())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !value.ends_with('-')
        && !value.contains("--");
    if valid {
        Ok(value.to_owned())
    } else {
        Err(format!("invalid {kind} {value:?}").into())
    }
}

fn validate_reason(reason: &str, kind: &str, capability: &str) -> Result<()> {
    slug(reason, kind)?;
    if reason.len() > 96 {
        return Err(format!("{kind} reason for {capability} is too long").into());
    }
    Ok(())
}

fn validate_relative_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        Err(format!("unsafe repository-relative path {:?}", path).into())
    } else {
        Ok(())
    }
}

fn validate_dependencies(capabilities: &BTreeMap<String, Capability>) -> Result<()> {
    for capability in capabilities.values() {
        for dependency in &capability.dependencies {
            if dependency == &capability.id {
                return Err(format!("capability {} depends on itself", capability.id).into());
            }
            if !capabilities.contains_key(dependency) {
                return Err(format!(
                    "capability {} depends on missing {dependency}",
                    capability.id
                )
                .into());
            }
        }
    }
    fn visit(
        id: &str,
        capabilities: &BTreeMap<String, Capability>,
        active: &mut BTreeSet<String>,
        complete: &mut BTreeSet<String>,
    ) -> Result<()> {
        if complete.contains(id) {
            return Ok(());
        }
        if !active.insert(id.to_owned()) {
            return Err(format!("capability dependency cycle reaches {id}").into());
        }
        for dependency in &capabilities[id].dependencies {
            visit(dependency, capabilities, active, complete)?;
        }
        active.remove(id);
        complete.insert(id.to_owned());
        Ok(())
    }
    let mut active = BTreeSet::new();
    let mut complete = BTreeSet::new();
    for id in capabilities.keys() {
        visit(id, capabilities, &mut active, &mut complete)?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
struct DispositionEntry {
    has_rust_component: bool,
    has_contract: bool,
}

#[derive(Debug)]
struct DispositionIndex {
    entries: BTreeMap<(String, String), DispositionEntry>,
    project_id: String,
    vendor_evidence_index: Option<PathBuf>,
}

impl DispositionIndex {
    fn load_project(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path).map_err(|error| {
            format!(
                "cannot read verification project {}: {error}",
                path.display()
            )
        })?;
        let project: VerificationProjectDocument = toml_edit::de::from_str(&input)?;
        let project_base = path
            .parent()
            .ok_or_else(|| format!("verification project has no parent: {}", path.display()))?;
        validate_relative_path(&project.verification_addon)?;
        let addon_path = project_base.join(project.verification_addon);
        let addon: VerificationAddonDocument =
            toml_edit::de::from_str(&fs::read_to_string(&addon_path)?)?;
        let base = addon_path.parent().ok_or_else(|| {
            format!(
                "verification add-on has no parent: {}",
                addon_path.display()
            )
        })?;
        let vendor_evidence_index = addon
            .evidence_index
            .map(|path| {
                validate_relative_path(&path)?;
                Ok::<_, Box<dyn std::error::Error>>(base.join(path))
            })
            .transpose()?;
        let mut entries = BTreeMap::new();
        for suite in addon.suites {
            for relative in suite.dispositions {
                validate_relative_path(&relative)?;
                let path = base.join(relative);
                let document: DispositionDocument =
                    toml_edit::de::from_str(&fs::read_to_string(&path)?)?;
                for function in document.functions {
                    let key = (function.source, function.symbol);
                    let entry = DispositionEntry {
                        has_rust_component: function.rust_component.is_some(),
                        has_contract: function.semantic_contract.is_some()
                            || function.effect_contract.is_some(),
                    };
                    if let Some(previous) = entries.insert(key.clone(), entry)
                        && (previous.has_rust_component != entry.has_rust_component
                            || previous.has_contract != entry.has_contract)
                    {
                        return Err(
                            format!("conflicting disposition entry {} {}", key.0, key.1).into()
                        );
                    }
                }
            }
        }
        Ok(Self {
            entries,
            project_id: project.id,
            vendor_evidence_index,
        })
    }

    fn get(&self, root: &VendorRoot) -> Option<&DispositionEntry> {
        self.entries
            .get(&(root.source.clone(), root.symbol.clone()))
    }
}

#[derive(Deserialize)]
struct VerificationProjectDocument {
    id: String,
    #[serde(rename = "verification-addon")]
    verification_addon: PathBuf,
}

#[derive(Deserialize)]
struct VerificationAddonDocument {
    #[serde(rename = "evidence-index")]
    evidence_index: Option<PathBuf>,
    #[serde(default)]
    suites: Vec<VerificationSuiteDocument>,
}

#[derive(Deserialize)]
struct VerificationSuiteDocument {
    #[serde(default)]
    dispositions: Vec<PathBuf>,
}

#[derive(Deserialize)]
struct DispositionDocument {
    #[serde(default)]
    functions: Vec<DispositionFunctionDocument>,
}

#[derive(Deserialize)]
struct DispositionFunctionDocument {
    source: String,
    symbol: String,
    #[serde(rename = "rust-component")]
    rust_component: Option<String>,
    #[serde(rename = "semantic-contract")]
    semantic_contract: Option<String>,
    #[serde(rename = "effect-contract")]
    effect_contract: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VendorEvidenceIndex {
    schema_version: u32,
    command: String,
    project: String,
    complete_project_run: bool,
    entries: Vec<VendorEvidenceIndexEntry>,
}

impl VendorEvidenceIndex {
    fn load(path: &Path, expected_project: &str) -> Result<Self> {
        let input = fs::read_to_string(path).map_err(|error| {
            format!(
                "cannot read vendor evidence index {}: {error}",
                path.display()
            )
        })?;
        let index: Self = serde_json::from_str(&input)?;
        if index.schema_version != 1
            || index.command != "project verify vendor evidence index"
            || index.project != expected_project
            || !index.complete_project_run
        {
            return Err(format!(
                "vendor evidence index {} is unsupported or incomplete",
                path.display()
            )
            .into());
        }
        let mut identities = BTreeSet::new();
        for entry in &index.entries {
            if !identities.insert((&entry.suite, &entry.source, &entry.symbol)) {
                return Err(format!(
                    "vendor evidence index repeats {} {} {}",
                    entry.suite, entry.source, entry.symbol
                )
                .into());
            }
        }
        Ok(index)
    }

    fn get(&self, reference: &VendorEvidenceRef) -> Option<&VendorEvidenceIndexEntry> {
        self.entries.iter().find(|entry| {
            entry.suite == reference.suite
                && entry.source == reference.source
                && entry.symbol == reference.symbol
        })
    }

    fn current_release_count(&self, root: &Path, evaluator_clean: bool) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.is_current_release_evidence(root, evaluator_clean))
            .count()
    }
}

#[derive(Debug, Deserialize)]
struct VendorEvidenceIndexEntry {
    suite: String,
    source: String,
    symbol: String,
    evidence_class: String,
    status: String,
    release_eligible: bool,
    rust_component: Option<String>,
    evidence_digest: Option<String>,
    baseline_passed: bool,
    artifact_hashes: Vec<VendorEvidenceArtifactHash>,
    source_hashes: Vec<VendorEvidenceSourceHash>,
    #[serde(default)]
    release_blockers: Vec<String>,
}

impl VendorEvidenceIndexEntry {
    fn is_current_release_evidence(&self, root: &Path, evaluator_clean: bool) -> bool {
        let artifact_roles = self
            .artifact_hashes
            .iter()
            .map(|artifact| artifact.role.as_str())
            .collect::<BTreeSet<_>>();
        let source_paths = self
            .source_hashes
            .iter()
            .map(|source| source.path.as_path())
            .collect::<BTreeSet<_>>();
        if !evaluator_clean
            || !self.release_eligible
            || self.evidence_class != "production-trace"
            || !matches!(self.status.as_str(), "match" | "bounded-match")
            || !self.baseline_passed
            || self.rust_component.is_none()
            || !self.evidence_digest.as_deref().is_some_and(valid_sha256)
            || self.artifact_hashes.is_empty()
            || artifact_roles.len() != self.artifact_hashes.len()
            || artifact_roles.iter().any(|role| role.is_empty())
            || self
                .artifact_hashes
                .iter()
                .any(|artifact| !valid_sha256(&artifact.sha256))
            || self.source_hashes.is_empty()
            || source_paths.len() != self.source_hashes.len()
            || !self.release_blockers.is_empty()
        {
            return false;
        }
        self.source_hashes.iter().all(|source| {
            validate_relative_path(&source.path).is_ok()
                && valid_sha256(&source.sha256)
                && fs::symlink_metadata(root.join(&source.path))
                    .is_ok_and(|metadata| metadata.file_type().is_file())
                && fs::read(root.join(&source.path)).is_ok_and(|contents| {
                    format!("{:x}", Sha256::digest(contents)) == source.sha256
                })
        })
    }
}

#[derive(Debug, Deserialize)]
struct VendorEvidenceArtifactHash {
    role: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
struct VendorEvidenceSourceHash {
    path: PathBuf,
    sha256: String,
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests;
