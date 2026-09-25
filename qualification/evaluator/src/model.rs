#[path = "../../../verification/vendor/schema/scenario-evidence.rs"]
pub(crate) mod scenario_evidence;
use sha2::Sha256;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

use serde::Deserialize;

use crate::{
    Result,
    hil::{HilEvidenceIndex, HilEvidenceSummary, HilRequirement, RepositoryState, ScenarioCatalog},
};

mod catalog;
mod development;
mod source_contract;
#[cfg(test)]
pub(crate) use catalog::InventoryItem;
pub(crate) use catalog::{
    CAPABILITY_CATALOG_SCHEMA, CapabilityOrigin, CapabilityScope, CatalogView, SourceIdentity,
};
pub(crate) use development::{Development, WorkKind};
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
    pub(crate) hil_checks: Vec<HilCheckEvidence>,
    pub(crate) hil_decisions: Vec<crate::hil::EvidenceDecision>,
    pub(crate) hil_not_applicable: Option<String>,
    pub(crate) async_not_applicable: Option<String>,
}

/// Per-check diagnostics do not replace the same-run conjunction required by
/// a HIL obligation. Separate observations must not be joined into a new PASS.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct HilCheckEvidence {
    pub(crate) scenario: String,
    pub(crate) check: String,
    pub(crate) minimum_repetitions: u8,
    pub(crate) evidence: Option<String>,
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
    pub(crate) declarations: BTreeMap<String, CapabilityDocument>,
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
    #[serde(default)]
    required_capabilities: Vec<String>,
    #[serde(default)]
    required_capabilities_from: Option<RequiredCapabilitiesFrom>,
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

/// An explicit selection policy, never a fallback for a missing required set.
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum RequiredCapabilitiesFrom {
    CatalogClosure,
}

pub(super) struct ValidatedProgram {
    document: ManifestDocument,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct VerificationConfig {
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
    #[serde(default)]
    pub(crate) checks: Vec<String>,
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
            checks: self.checks.clone(),
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
    pub(crate) hil_reviews: Vec<PathBuf>,
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
    #[serde(default)]
    pub(crate) development: Development,
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

        let scenarios = ScenarioCatalog::load(root, &self.hil.catalog)?;
        let context = StaticContext {
            root,
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
        let repository = RepositoryState::read(root)?;
        let evidence =
            NativeEvidence::load(root, &document.verification.evidence_index, &hil_target)?;
        let scenario_catalog = ScenarioCatalog::load(root, &document.hil.catalog)?;
        let hil_index = HilEvidenceIndex::load(root, &document.hil.runs, &hil_target, &repository)?;
        let evidence_inputs = EvidenceInputs {
            verification_entries: evidence.index.entries.len(),
            verification_current_release_entries: evidence.current_entries(),
            hil: hil_index.summary().clone(),
            vendor_evidence_index: document.verification.evidence_index.clone(),
            hil_catalog: document.hil.catalog.clone(),
            hil_runs: document.hil.runs.clone(),
        };
        let declarations = document
            .capabilities
            .iter()
            .map(|value| (value.id.clone(), value.clone()))
            .collect();
        let context = EvaluationContext {
            root,
            evidence: &evidence,
            scenario_catalog: &scenario_catalog,
            hil_index: &hil_index,
            declarations: &declarations,
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
            declarations,
            program_source,
            catalog_sources: document.catalog_sources,
            capability_origins: document.capability_origins,
            catalog_scopes: document.catalog_scopes,
            catalog: document.catalog,
            direct_catalog_capabilities: document.direct_catalog_capabilities,
        })
    }
}

pub(crate) struct StaticContext<'a> {
    root: &'a Path,
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
    evidence: &'a NativeEvidence,
    scenario_catalog: &'a ScenarioCatalog,
    hil_index: &'a HilEvidenceIndex,
    declarations: &'a BTreeMap<String, CapabilityDocument>,
}

fn evaluate_capability(
    document: CapabilityDocument,
    context: &EvaluationContext<'_>,
) -> Result<Capability> {
    let validated = validate_capability_declaration_inner(
        &document,
        &StaticContext {
            root: context.root,
            scenario_catalog: context.scenario_catalog,
        },
    )?;
    let id = validated.id;
    let dependencies = validated.dependencies;
    let mut gaps = validated.gaps;
    let hil_requirements = validated.hil_requirements.clone();
    let (reviewed_index, reviews) = crate::hil::review::apply(
        context.root,
        &document,
        context.declarations,
        context.hil_index,
        context.scenario_catalog,
    )?;
    let hil_decisions = hil_requirements
        .iter()
        .map(|requirement| {
            let mut decision = reviewed_index.decision_for(requirement, context.scenario_catalog);
            decision.attach_reviews(
                context.root,
                &document,
                context.declarations,
                context.scenario_catalog,
                &reviews,
            )?;
            Ok(decision)
        })
        .collect::<Result<Vec<_>>>()?;
    let hil_checks = hil_requirements
        .iter()
        .flat_map(|requirement| {
            requirement.checks.iter().map(|check| {
                let selected = HilRequirement {
                    checks: vec![check.clone()],
                    ..requirement.clone()
                };
                HilCheckEvidence {
                    scenario: requirement.scenario.clone(),
                    check: check.clone(),
                    minimum_repetitions: requirement.minimum_repetitions,
                    evidence: reviewed_index.evidence_for(&selected, context.scenario_catalog),
                }
            })
        })
        .collect();
    let implementation = document.implementation;
    let host = document.host;

    let mut evidence = Vec::new();

    let (vendor, vendor_evidence) = derive_vendor_proof(&id, &document, context.evidence)?;
    evidence.extend(vendor_evidence);
    validate_vendor_contract(&id, &document, vendor)?;
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
        for decision in &hil_decisions {
            match &decision.evidence {
                Some(reference) => evidence.push(reference.clone()),
                None => complete = false,
            }
            if decision.status == crate::hil::EvidenceStatus::UnresolvedFailure {
                ensure_gap(&mut gaps, Axis::Hil, "current-hil-failure-unresolved");
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
        hil_checks,
        hil_decisions,
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
    crate::hil::review::validate(context.root, document, context.scenario_catalog)?;
    document
        .development
        .validate(&document.gaps, context.root)?;
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
        slug(&reference.source, "vendor source")?;
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
        slug(&root.source, "vendor source")?;
        if root.symbol.trim().is_empty() {
            return Err(format!("vendor root for {id} has an empty symbol").into());
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
    Ok(())
}

fn derive_vendor_proof(
    id: &str,
    document: &CapabilityDocument,
    evidence: &NativeEvidence,
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
                    && evidence.supports(reference)
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

/// The native scenario evidence index and whether its sources are current.
/// A missing index supports no claim.
pub(crate) struct NativeEvidence {
    pub(crate) index: scenario_evidence::Index,
    pub(crate) current: bool,
}

impl NativeEvidence {
    fn load(root: &Path, path: &Path, target: &str) -> Result<Self> {
        let index = match fs::read_to_string(root.join(path)) {
            Ok(input) => {
                let index: scenario_evidence::Index = serde_json::from_str(&input)?;
                index.validate(target).map_err(|error| {
                    format!("scenario evidence index {}: {error}", path.display())
                })?;
                index
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self {
                    index: scenario_evidence::Index {
                        schema: scenario_evidence::SCHEMA,
                        command: scenario_evidence::COMMAND.into(),
                        target: target.into(),
                        inputs: BTreeMap::new(),
                        sources: vec![],
                        entries: vec![],
                        untriaged: vec![],
                        unobserved: vec![],
                    },
                    current: false,
                });
            }
            Err(error) => {
                return Err(format!(
                    "cannot read scenario evidence index {}: {error}",
                    path.display()
                )
                .into());
            }
        };
        let current = index.is_current(root);
        Ok(Self { index, current })
    }

    /// A current index holds a MATCH entry for the referenced suite and root.
    fn supports(&self, reference: &VendorEvidenceRef) -> bool {
        self.current
            && self.index.entries.iter().any(|entry| {
                entry.suite == reference.suite
                    && entry.source == reference.source
                    && entry.symbol == reference.symbol
                    && entry.verdict == scenario_evidence::MATCH
            })
    }

    fn current_entries(&self) -> usize {
        if self.current {
            self.index.entries.len()
        } else {
            0
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
