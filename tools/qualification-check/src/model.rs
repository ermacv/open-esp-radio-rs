use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::Result;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum Axis {
    Implementation,
    Host,
    Vendor,
    Hil,
    Async,
}

impl Axis {
    fn parse(value: &str, line: usize) -> Result<Self> {
        match value {
            "implementation" => Ok(Self::Implementation),
            "host" => Ok(Self::Host),
            "vendor" => Ok(Self::Vendor),
            "hil" => Ok(Self::Hil),
            "async" => Ok(Self::Async),
            _ => Err(format!("unknown capability axis {value:?} at line {line}").into()),
        }
    }

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

macro_rules! proof_axis {
    ($name:ident, $terminal:expr, {$($text:literal => $variant:ident),+ $(,)?}) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub(crate) enum $name { $($variant),+ }

        impl $name {
            fn parse(value: &str, line: usize) -> Result<Self> {
                match value {
                    $($text => Ok(Self::$variant),)+
                    _ => Err(format!("invalid {} value {value:?} at line {line}", stringify!($name)).into()),
                }
            }

            pub(crate) const fn label(self) -> &'static str {
                match self { $(Self::$variant => $text,)+ }
            }

            pub(crate) fn is_terminal(self) -> bool { $terminal(self) }
        }
    };
}

proof_axis!(ImplementationProof, |value| matches!(value, ImplementationProof::Complete), {
    "complete" => Complete,
    "partial" => Partial,
    "missing" => Missing,
});
proof_axis!(HostProof, |value| matches!(value, HostProof::Covered), {
    "covered" => Covered,
    "partial" => Partial,
    "missing" => Missing,
});
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

    pub(crate) const fn is_terminal(self) -> bool {
        matches!(self, Self::Qualified | Self::NotApplicable)
    }
}
proof_axis!(HilProof, |value| matches!(value, HilProof::Qualified), {
    "qualified" => Qualified,
    "partial" => Partial,
    "blocked" => Blocked,
    "missing" => Missing,
});
proof_axis!(AsyncProof, |value| matches!(value, AsyncProof::Bounded | AsyncProof::NotApplicable), {
    "bounded" => Bounded,
    "partial" => Partial,
    "blocking" => Blocking,
    "not-applicable" => NotApplicable,
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FileReference {
    pub(crate) path: PathBuf,
    pub(crate) token: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct VendorRoot {
    pub(crate) source: String,
    pub(crate) symbol: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct VendorEvidenceRef {
    pub(crate) suite: String,
    pub(crate) source: String,
    pub(crate) symbol: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Gap {
    pub(crate) axis: Axis,
    pub(crate) id: String,
}

#[derive(Clone, Debug)]
pub(crate) struct Capability {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) scope: String,
    pub(crate) implementation: ImplementationProof,
    pub(crate) host: HostProof,
    pub(crate) vendor: VendorProof,
    pub(crate) hil: HilProof,
    pub(crate) async_proof: AsyncProof,
    pub(crate) owners: Vec<FileReference>,
    pub(crate) tests: Vec<FileReference>,
    pub(crate) vendor_roots: Vec<VendorRoot>,
    pub(crate) vendor_evidence: Vec<VendorEvidenceRef>,
    pub(crate) vendor_not_applicable: Option<String>,
    pub(crate) vendor_anchors: Vec<FileReference>,
    pub(crate) hil_evidence: Vec<FileReference>,
    pub(crate) dependencies: Vec<String>,
    pub(crate) gaps: Vec<Gap>,
}

impl Capability {
    pub(crate) fn proof_ready(&self) -> bool {
        self.implementation.is_terminal()
            && self.host.is_terminal()
            && self.vendor.is_terminal()
            && self.hil.is_terminal()
            && self.async_proof.is_terminal()
    }

    pub(crate) fn gaps_for(&self, axis: Axis) -> impl Iterator<Item = &Gap> {
        self.gaps.iter().filter(move |gap| gap.axis == axis)
    }
}

#[derive(Default)]
struct CapabilityBuilder {
    id: String,
    title: Option<String>,
    scope: Option<String>,
    implementation: Option<ImplementationProof>,
    host: Option<HostProof>,
    hil: Option<HilProof>,
    async_proof: Option<AsyncProof>,
    owners: Vec<FileReference>,
    tests: Vec<FileReference>,
    vendor_roots: Vec<VendorRoot>,
    vendor_evidence: Vec<VendorEvidenceRef>,
    vendor_not_applicable: Option<String>,
    vendor_anchors: Vec<FileReference>,
    hil_evidence: Vec<FileReference>,
    dependencies: Vec<String>,
    gaps: Vec<Gap>,
    line: usize,
}

impl CapabilityBuilder {
    fn finish(self) -> Result<Capability> {
        fn required<T>(value: Option<T>, id: &str, line: usize, field: &str) -> Result<T> {
            value.ok_or_else(|| {
                format!("capability {id} has no {field} (started at line {line})").into()
            })
        }
        let id = self.id.clone();
        let line = self.line;
        Ok(Capability {
            id: self.id,
            title: required(self.title, &id, line, "title")?,
            scope: required(self.scope, &id, line, "scope")?,
            implementation: required(self.implementation, &id, line, "implementation proof")?,
            host: required(self.host, &id, line, "host proof")?,
            vendor: VendorProof::Unmapped,
            hil: required(self.hil, &id, line, "HIL proof")?,
            async_proof: required(self.async_proof, &id, line, "async proof")?,
            owners: self.owners,
            tests: self.tests,
            vendor_roots: self.vendor_roots,
            vendor_evidence: self.vendor_evidence,
            vendor_not_applicable: self.vendor_not_applicable,
            vendor_anchors: self.vendor_anchors,
            hil_evidence: self.hil_evidence,
            dependencies: self.dependencies,
            gaps: self.gaps,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Ledger {
    pub(crate) target: String,
    pub(crate) verification_project: PathBuf,
    pub(crate) vendor_evidence_index: PathBuf,
    pub(crate) capabilities: BTreeMap<String, Capability>,
}

fn is_slug(value: &str) -> bool {
    value
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_lowercase())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !value.ends_with('-')
        && !value.contains("--")
}

fn slug(value: &str, kind: &str, line: usize) -> Result<String> {
    if is_slug(value) {
        Ok(value.to_owned())
    } else {
        Err(format!("invalid {kind} {value:?} at line {line}").into())
    }
}

fn relative_path(value: &str, line: usize) -> Result<PathBuf> {
    let path = Path::new(value);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!("unsafe repository-relative path {value:?} at line {line}").into());
    }
    Ok(path.to_owned())
}

fn words_exact<'a>(
    value: &'a str,
    count: usize,
    directive: &str,
    line: usize,
) -> Result<Vec<&'a str>> {
    let words = value.split_whitespace().collect::<Vec<_>>();
    if words.len() != count {
        return Err(format!("{directive} expects {count} fields at line {line}").into());
    }
    Ok(words)
}

fn file_reference(value: &str, directive: &str, line: usize) -> Result<FileReference> {
    let words = words_exact(value, 2, directive, line)?;
    if words[1].contains('#') {
        return Err(format!("{directive} token cannot contain # at line {line}").into());
    }
    Ok(FileReference {
        path: relative_path(words[0], line)?,
        token: words[1].to_owned(),
    })
}

fn set_once<T>(slot: &mut Option<T>, value: T, directive: &str, line: usize) -> Result<()> {
    if slot.replace(value).is_some() {
        Err(format!("duplicate {directive} at line {line}").into())
    } else {
        Ok(())
    }
}

impl Ledger {
    pub(crate) fn load(path: &Path) -> Result<Self> {
        Self::parse(&fs::read_to_string(path)?)
    }

    fn parse(input: &str) -> Result<Self> {
        let mut version = None;
        let mut target = None;
        let mut verification_project = None;
        let mut vendor_evidence_index = None;
        let mut required_capabilities = BTreeSet::new();
        let mut capabilities = BTreeMap::new();
        let mut current: Option<CapabilityBuilder> = None;

        let finish = |builder: CapabilityBuilder,
                      capabilities: &mut BTreeMap<String, Capability>|
         -> Result<()> {
            let capability = builder.finish()?;
            let id = capability.id.clone();
            if capabilities.insert(id.clone(), capability).is_some() {
                return Err(format!("duplicate capability {id}").into());
            }
            Ok(())
        };

        for (index, raw_line) in input.lines().enumerate() {
            let line = index + 1;
            let input = raw_line.split('#').next().unwrap_or_default().trim();
            if input.is_empty() {
                continue;
            }
            let (directive, value) = input
                .split_once(char::is_whitespace)
                .map(|(directive, value)| (directive, value.trim()))
                .filter(|(_, value)| !value.is_empty())
                .ok_or_else(|| format!("directive needs a value at line {line}"))?;
            if directive == "capability" {
                if let Some(builder) = current.take() {
                    version.ok_or("ledger-version must precede capabilities")?;
                    finish(builder, &mut capabilities)?;
                }
                current = Some(CapabilityBuilder {
                    id: slug(value, "capability id", line)?,
                    line,
                    ..CapabilityBuilder::default()
                });
                continue;
            }
            if current.is_none() {
                match directive {
                    "ledger-version" => {
                        let parsed = value.parse::<u32>()?;
                        if parsed != 2 {
                            return Err(format!("unsupported ledger version {parsed}").into());
                        }
                        set_once(&mut version, parsed, directive, line)?;
                    }
                    "target" => {
                        set_once(&mut target, slug(value, "target", line)?, directive, line)?
                    }
                    "verification-project" => set_once(
                        &mut verification_project,
                        relative_path(value, line)?,
                        directive,
                        line,
                    )?,
                    "vendor-evidence-index" => set_once(
                        &mut vendor_evidence_index,
                        relative_path(value, line)?,
                        directive,
                        line,
                    )?,
                    "require-capability" => {
                        let id = slug(value, "required capability", line)?;
                        if !required_capabilities.insert(id.clone()) {
                            return Err(format!(
                                "duplicate required capability {id} at line {line}"
                            )
                            .into());
                        }
                    }
                    _ => {
                        return Err(format!(
                            "unknown ledger directive {directive:?} at line {line}"
                        )
                        .into());
                    }
                }
                continue;
            }
            let builder = current.as_mut().expect("checked above");
            match directive {
                "title" => set_once(&mut builder.title, value.to_owned(), directive, line)?,
                "scope" => set_once(&mut builder.scope, value.to_owned(), directive, line)?,
                "implementation" => set_once(
                    &mut builder.implementation,
                    ImplementationProof::parse(value, line)?,
                    directive,
                    line,
                )?,
                "host-proof" => set_once(
                    &mut builder.host,
                    HostProof::parse(value, line)?,
                    directive,
                    line,
                )?,
                "hil-proof" => set_once(
                    &mut builder.hil,
                    HilProof::parse(value, line)?,
                    directive,
                    line,
                )?,
                "async-proof" => set_once(
                    &mut builder.async_proof,
                    AsyncProof::parse(value, line)?,
                    directive,
                    line,
                )?,
                "owner" => builder.owners.push(file_reference(value, directive, line)?),
                "test" => builder.tests.push(file_reference(value, directive, line)?),
                "vendor-anchor" => builder
                    .vendor_anchors
                    .push(file_reference(value, directive, line)?),
                "hil-evidence" => builder
                    .hil_evidence
                    .push(file_reference(value, directive, line)?),
                "vendor-root" => {
                    let words = words_exact(value, 2, directive, line)?;
                    if !matches!(words[0], "rom" | "archive") {
                        return Err(
                            format!("invalid vendor source {:?} at line {line}", words[0]).into(),
                        );
                    }
                    builder.vendor_roots.push(VendorRoot {
                        source: words[0].to_owned(),
                        symbol: words[1].to_owned(),
                    });
                }
                "vendor-evidence" => {
                    let words = words_exact(value, 3, directive, line)?;
                    builder.vendor_evidence.push(VendorEvidenceRef {
                        suite: slug(words[0], "verification suite", line)?,
                        source: words[1].to_owned(),
                        symbol: words[2].to_owned(),
                    });
                }
                "vendor-not-applicable" => set_once(
                    &mut builder.vendor_not_applicable,
                    slug(value, "vendor not-applicable reason", line)?,
                    directive,
                    line,
                )?,
                "depends-on" => builder.dependencies.push(slug(value, "dependency", line)?),
                "gap" => {
                    let words = words_exact(value, 2, directive, line)?;
                    builder.gaps.push(Gap {
                        axis: Axis::parse(words[0], line)?,
                        id: slug(words[1], "gap id", line)?,
                    });
                }
                _ => {
                    return Err(format!(
                        "unknown capability directive {directive:?} at line {line}"
                    )
                    .into());
                }
            }
        }
        if let Some(builder) = current {
            version.ok_or("ledger-version must precede capabilities")?;
            finish(builder, &mut capabilities)?;
        }
        version.ok_or("ledger has no ledger-version")?;
        let target = target.ok_or("ledger has no target")?;
        let verification_project =
            verification_project.ok_or("ledger has no verification-project")?;
        let vendor_evidence_index =
            vendor_evidence_index.ok_or("ledger version 2 has no vendor-evidence-index")?;
        for required in &required_capabilities {
            if !capabilities.contains_key(required) {
                return Err(format!("required capability {required} is missing").into());
            }
        }
        if required_capabilities.len() != capabilities.len() {
            let undeclared = capabilities
                .keys()
                .filter(|id| !required_capabilities.contains(*id))
                .cloned()
                .collect::<Vec<_>>();
            return Err(format!(
                "capabilities not declared by require-capability: {}",
                undeclared.join(", ")
            )
            .into());
        }
        Ok(Self {
            target,
            verification_project,
            vendor_evidence_index,
            capabilities,
        })
    }

    pub(crate) fn validate(&mut self, root: &Path) -> Result<()> {
        if !root.is_dir() {
            return Err(format!("repository root {} is not a directory", root.display()).into());
        }
        let dispositions = DispositionIndex::load_project(&root.join(&self.verification_project))?;
        let index_path = root.join(&self.vendor_evidence_index);
        let project_index = dispositions
            .vendor_evidence_index
            .as_ref()
            .ok_or("ledger version 2 verification project has no evidence-index output")?;
        if fs::canonicalize(&index_path)? != fs::canonicalize(project_index)? {
            return Err(format!(
                "ledger vendor evidence index {} does not match verification project output {}",
                index_path.display(),
                project_index.display()
            )
            .into());
        }
        let index = VendorEvidenceIndex::load(&index_path)?;
        for capability in self.capabilities.values_mut() {
            capability.vendor = derive_vendor_proof(capability, &index, root)?;
        }
        let mut files = BTreeMap::new();
        for capability in self.capabilities.values() {
            validate_capability(capability, root, &dispositions, &mut files)?;
        }
        validate_dependencies(&self.capabilities)
    }

    pub(crate) fn is_ready(&self, id: &str) -> bool {
        fn visit(ledger: &Ledger, id: &str, visiting: &mut BTreeSet<String>) -> bool {
            let capability = &ledger.capabilities[id];
            if !capability.proof_ready() || !visiting.insert(id.to_owned()) {
                return false;
            }
            let ready = capability
                .dependencies
                .iter()
                .all(|dependency| visit(ledger, dependency, visiting));
            visiting.remove(id);
            ready
        }
        visit(self, id, &mut BTreeSet::new())
    }
}

#[derive(Clone, Copy, Debug)]
struct DispositionEntry {
    has_rust_component: bool,
    has_contract: bool,
}

#[derive(Debug)]
struct DispositionIndex {
    entries: BTreeMap<(String, String), DispositionEntry>,
    vendor_evidence_index: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct VendorEvidenceIndex {
    schema_version: u32,
    complete_project_run: bool,
    entries: Vec<VendorEvidenceIndexEntry>,
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

#[derive(Debug, Deserialize)]
struct VendorEvidenceArtifactHash {
    sha256: String,
}

#[derive(Debug, Deserialize)]
struct VendorEvidenceSourceHash {
    path: PathBuf,
    sha256: String,
}

impl VendorEvidenceIndex {
    fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path).map_err(|error| {
            format!(
                "cannot read vendor evidence index {}: {error}",
                path.display()
            )
        })?;
        let index: Self = serde_json::from_str(&input).map_err(|error| {
            format!(
                "cannot parse vendor evidence index {}: {error}",
                path.display()
            )
        })?;
        if index.schema_version != 1 {
            return Err(format!(
                "vendor evidence index {} has unsupported schema {}",
                path.display(),
                index.schema_version
            )
            .into());
        }
        if !index.complete_project_run {
            return Err(format!(
                "vendor evidence index {} was not produced by a complete project run",
                path.display()
            )
            .into());
        }
        let mut identities = BTreeSet::new();
        for entry in &index.entries {
            if !identities.insert((&entry.suite, &entry.source, &entry.symbol)) {
                return Err(format!(
                    "vendor evidence index {} repeats {} {} {}",
                    path.display(),
                    entry.suite,
                    entry.source,
                    entry.symbol
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
}

impl VendorEvidenceIndexEntry {
    fn is_current_release_evidence(&self, root: &Path) -> bool {
        if !self.release_eligible
            || self.evidence_class != "production-trace"
            || !matches!(self.status.as_str(), "match" | "bounded-match")
            || !self.baseline_passed
            || self.rust_component.is_none()
            || self.evidence_digest.is_none()
            || self.artifact_hashes.is_empty()
            || self.source_hashes.is_empty()
            || !self.release_blockers.is_empty()
        {
            return false;
        }
        if !self.evidence_digest.as_deref().is_some_and(valid_sha256)
            || self
                .artifact_hashes
                .iter()
                .any(|artifact| !valid_sha256(&artifact.sha256))
        {
            return false;
        }
        for source in &self.source_hashes {
            if source.path.as_os_str().is_empty()
                || source.path.is_absolute()
                || source
                    .path
                    .components()
                    .any(|component| !matches!(component, Component::Normal(_)))
                || !valid_sha256(&source.sha256)
            {
                return false;
            }
            let contents = match fs::read(root.join(&source.path)) {
                Ok(contents) => contents,
                Err(_) => return false,
            };
            if format!("{:x}", Sha256::digest(contents)) != source.sha256 {
                return false;
            }
        }
        true
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn derive_vendor_proof(
    capability: &Capability,
    index: &VendorEvidenceIndex,
    root: &Path,
) -> Result<VendorProof> {
    if capability.vendor_not_applicable.is_some() {
        if !capability.vendor_roots.is_empty()
            || !capability.vendor_anchors.is_empty()
            || !capability.vendor_evidence.is_empty()
        {
            return Err(format!(
                "vendor-not-applicable capability {} cannot claim vendor roots, anchors or evidence",
                capability.id
            )
            .into());
        }
        return Ok(VendorProof::NotApplicable);
    }
    let roots = capability
        .vendor_roots
        .iter()
        .map(|root| (root.source.as_str(), root.symbol.as_str()))
        .collect::<BTreeSet<_>>();
    let mut references = BTreeSet::new();
    for reference in &capability.vendor_evidence {
        let key = (
            reference.suite.as_str(),
            reference.source.as_str(),
            reference.symbol.as_str(),
        );
        if !references.insert(key) {
            return Err(format!(
                "capability {} repeats vendor evidence {} {} {}",
                capability.id, reference.suite, reference.source, reference.symbol
            )
            .into());
        }
        if !roots.contains(&(reference.source.as_str(), reference.symbol.as_str())) {
            return Err(format!(
                "vendor evidence {} {} {} for {} does not name a declared vendor-root",
                reference.suite, reference.source, reference.symbol, capability.id
            )
            .into());
        }
        if index.get(reference).is_none() {
            return Err(format!(
                "vendor evidence index has no {} {} {} entry required by {}",
                reference.suite, reference.source, reference.symbol, capability.id
            )
            .into());
        }
    }

    let all_roots_release_eligible = !roots.is_empty()
        && roots.iter().all(|(source, symbol)| {
            capability.vendor_evidence.iter().any(|reference| {
                reference.source == *source
                    && reference.symbol == *symbol
                    && index
                        .get(reference)
                        .is_some_and(|entry| entry.is_current_release_evidence(root))
            })
        });
    if all_roots_release_eligible && capability.vendor_anchors.is_empty() {
        Ok(VendorProof::Qualified)
    } else if !capability.vendor_roots.is_empty() || !capability.vendor_anchors.is_empty() {
        Ok(VendorProof::Mapped)
    } else {
        Ok(VendorProof::Unmapped)
    }
}

#[derive(Deserialize)]
struct VerificationProjectDocument {
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

impl DispositionIndex {
    fn load_project(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path).map_err(|error| {
            format!(
                "cannot read verification project {}: {error}",
                path.display()
            )
        })?;
        let project: VerificationProjectDocument =
            toml_edit::de::from_str(&input).map_err(|error| {
                format!(
                    "cannot parse verification project {}: {error}",
                    path.display()
                )
            })?;
        let project_base = path
            .parent()
            .ok_or_else(|| format!("verification project has no parent: {}", path.display()))?;
        if project.verification_addon.is_absolute()
            || project
                .verification_addon
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(format!(
                "unsafe verification add-on path {:?} in {}",
                project.verification_addon,
                path.display()
            )
            .into());
        }
        let addon_path = project_base.join(project.verification_addon);
        let addon_input = fs::read_to_string(&addon_path).map_err(|error| {
            format!(
                "cannot read verification add-on {}: {error}",
                addon_path.display()
            )
        })?;
        let verification: VerificationAddonDocument = toml_edit::de::from_str(&addon_input)
            .map_err(|error| {
                format!(
                    "cannot parse verification add-on {}: {error}",
                    addon_path.display()
                )
            })?;
        let base = addon_path.parent().ok_or_else(|| {
            format!(
                "verification add-on has no parent: {}",
                addon_path.display()
            )
        })?;
        let vendor_evidence_index = verification
            .evidence_index
            .map(|evidence_index| -> Result<PathBuf> {
                if evidence_index.is_absolute()
                    || evidence_index
                        .components()
                        .any(|component| !matches!(component, Component::Normal(_)))
                {
                    return Err(format!(
                        "unsafe vendor evidence index path {:?} in {}",
                        evidence_index,
                        path.display()
                    )
                    .into());
                }
                Ok(base.join(evidence_index))
            })
            .transpose()?;
        let mut entries = BTreeMap::new();
        for suite in verification.suites {
            for relative in suite.dispositions {
                if relative.is_absolute()
                    || relative
                        .components()
                        .any(|component| !matches!(component, Component::Normal(_)))
                {
                    return Err(format!(
                        "unsafe disposition path {:?} in {}",
                        relative,
                        path.display()
                    )
                    .into());
                }
                let disposition_path = base.join(relative);
                let input = fs::read_to_string(&disposition_path).map_err(|error| {
                    format!(
                        "cannot read disposition pack {}: {error}",
                        disposition_path.display()
                    )
                })?;
                let document: DispositionDocument =
                    toml_edit::de::from_str(&input).map_err(|error| {
                        format!(
                            "cannot parse disposition pack {}: {error}",
                            disposition_path.display()
                        )
                    })?;
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
            vendor_evidence_index,
        })
    }

    fn get(&self, root: &VendorRoot) -> Option<&DispositionEntry> {
        self.entries
            .get(&(root.source.clone(), root.symbol.clone()))
    }
}

fn validate_reference(
    reference: &FileReference,
    root: &Path,
    files: &mut BTreeMap<PathBuf, String>,
) -> Result<String> {
    let contents = match files.get(&reference.path) {
        Some(contents) => contents,
        None => {
            let path = root.join(&reference.path);
            let contents = fs::read_to_string(&path).map_err(|error| {
                format!("cannot read referenced file {}: {error}", path.display())
            })?;
            files.entry(reference.path.clone()).or_insert(contents)
        }
    };
    let contents = contents.clone();
    if !contents.contains(&reference.token) {
        return Err(format!(
            "referenced token {:?} is absent from {}",
            reference.token,
            reference.path.display()
        )
        .into());
    }
    Ok(contents)
}

fn contains_owner_declaration(contents: &str, token: &str) -> bool {
    [
        "pub struct ",
        "pub enum ",
        "pub trait ",
        "pub fn ",
        "pub async fn ",
        "pub const fn ",
    ]
    .iter()
    .any(|prefix| contents.contains(&format!("{prefix}{token}")))
}

fn contains_test_declaration(contents: &str, token: &str) -> bool {
    contents.contains(&format!("fn {token}("))
}

fn is_rust_source_under(path: &Path, prefix: &str) -> bool {
    path.starts_with(prefix)
        && path.extension().and_then(|value| value.to_str()) == Some("rs")
        && path
            .components()
            .any(|component| component.as_os_str() == "src")
}

fn axis_terminal(capability: &Capability, axis: Axis) -> bool {
    match axis {
        Axis::Implementation => capability.implementation.is_terminal(),
        Axis::Host => capability.host.is_terminal(),
        Axis::Vendor => capability.vendor.is_terminal(),
        Axis::Hil => capability.hil.is_terminal(),
        Axis::Async => capability.async_proof.is_terminal(),
    }
}

fn validate_gap_contract(capability: &Capability) -> Result<()> {
    let mut gap_keys = BTreeSet::new();
    for gap in &capability.gaps {
        if !gap_keys.insert((gap.axis, gap.id.as_str())) {
            return Err(format!(
                "duplicate {} gap {} for {}",
                gap.axis.label(),
                gap.id,
                capability.id
            )
            .into());
        }
        if axis_terminal(capability, gap.axis) {
            return Err(format!(
                "terminal {} axis for {} cannot retain gap {}",
                gap.axis.label(),
                capability.id,
                gap.id
            )
            .into());
        }
    }
    for axis in [
        Axis::Implementation,
        Axis::Host,
        Axis::Vendor,
        Axis::Hil,
        Axis::Async,
    ] {
        if !axis_terminal(capability, axis) && capability.gaps_for(axis).next().is_none() {
            return Err(format!(
                "non-terminal {} axis for {} has no explicit gap",
                axis.label(),
                capability.id
            )
            .into());
        }
    }
    Ok(())
}

fn validate_capability(
    capability: &Capability,
    root: &Path,
    dispositions: &DispositionIndex,
    files: &mut BTreeMap<PathBuf, String>,
) -> Result<()> {
    validate_gap_contract(capability)?;
    if matches!(capability.implementation, ImplementationProof::Missing)
        && !capability.owners.is_empty()
    {
        return Err(format!(
            "missing capability {} cannot claim production owners",
            capability.id
        )
        .into());
    }
    if !matches!(capability.implementation, ImplementationProof::Missing)
        && capability.owners.is_empty()
    {
        return Err(format!(
            "implemented or partial capability {} has no production owner",
            capability.id
        )
        .into());
    }
    for owner in &capability.owners {
        if !is_rust_source_under(&owner.path, "driver") {
            return Err(format!(
                "owner {} for {} is not production Rust under driver/*/src",
                owner.path.display(),
                capability.id
            )
            .into());
        }
        let contents = validate_reference(owner, root, files)?;
        if !contains_owner_declaration(&contents, &owner.token) {
            return Err(format!(
                "owner token {:?} for {} is not a public Rust declaration in {}",
                owner.token,
                capability.id,
                owner.path.display()
            )
            .into());
        }
    }

    if matches!(capability.host, HostProof::Missing) && !capability.tests.is_empty() {
        return Err(format!(
            "host-missing capability {} cannot claim tests",
            capability.id
        )
        .into());
    }
    if !matches!(capability.host, HostProof::Missing) && capability.tests.is_empty() {
        return Err(format!(
            "host-covered or partial capability {} has no test reference",
            capability.id
        )
        .into());
    }
    for test in &capability.tests {
        if !test.path.starts_with("driver")
            || test.path.extension().and_then(|value| value.to_str()) != Some("rs")
        {
            return Err(format!(
                "test {} for {} is not Rust under driver",
                test.path.display(),
                capability.id
            )
            .into());
        }
        let contents = validate_reference(test, root, files)?;
        if !contains_test_declaration(&contents, &test.token) {
            return Err(format!(
                "test token {:?} for {} is not a Rust function in {}",
                test.token,
                capability.id,
                test.path.display()
            )
            .into());
        }
    }

    match capability.vendor {
        VendorProof::Qualified => {
            if capability.vendor_roots.is_empty() || !capability.vendor_anchors.is_empty() {
                return Err(format!(
                    "vendor-qualified capability {} needs only executable vendor roots",
                    capability.id
                )
                .into());
            }
        }
        VendorProof::Mapped => {
            if capability.vendor_roots.is_empty() && capability.vendor_anchors.is_empty() {
                return Err(format!(
                    "vendor-mapped capability {} has no root or source anchor",
                    capability.id
                )
                .into());
            }
        }
        VendorProof::Unmapped | VendorProof::NotApplicable => {
            if !capability.vendor_roots.is_empty() || !capability.vendor_anchors.is_empty() {
                return Err(format!(
                    "vendor {} capability {} cannot claim vendor references",
                    capability.vendor.label(),
                    capability.id
                )
                .into());
            }
        }
    }
    for vendor_root in &capability.vendor_roots {
        let disposition = dispositions.get(vendor_root).ok_or_else(|| {
            format!(
                "vendor root {} {} for {} has no explicit disposition entry",
                vendor_root.source, vendor_root.symbol, capability.id
            )
        })?;
        if !disposition.has_rust_component {
            return Err(format!(
                "vendor root {} {} for {} has no rust-component",
                vendor_root.source, vendor_root.symbol, capability.id
            )
            .into());
        }
        if matches!(capability.vendor, VendorProof::Qualified) && !disposition.has_contract {
            return Err(format!(
                "vendor-qualified root {} {} for {} has no executable contract",
                vendor_root.source, vendor_root.symbol, capability.id
            )
            .into());
        }
    }
    for anchor in &capability.vendor_anchors {
        validate_reference(anchor, root, files)?;
    }

    if matches!(capability.hil, HilProof::Qualified | HilProof::Partial)
        && capability.hil_evidence.is_empty()
    {
        return Err(format!(
            "HIL {} capability {} has no evidence record",
            capability.hil.label(),
            capability.id
        )
        .into());
    }
    for evidence in &capability.hil_evidence {
        if !evidence
            .path
            .starts_with("qualification/targets/esp32s31/records")
            || evidence.path.extension().and_then(|value| value.to_str()) != Some("md")
        {
            return Err(format!(
                "HIL evidence {} for {} is not a dated qualification record",
                evidence.path.display(),
                capability.id
            )
            .into());
        }
        validate_reference(evidence, root, files)?;
    }

    if matches!(capability.async_proof, AsyncProof::Bounded)
        && (capability.owners.is_empty() || capability.tests.is_empty())
    {
        return Err(format!(
            "async-bounded capability {} needs an owner and host test",
            capability.id
        )
        .into());
    }

    Ok(())
}

fn validate_dependencies(capabilities: &BTreeMap<String, Capability>) -> Result<()> {
    for capability in capabilities.values() {
        let mut unique = BTreeSet::new();
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
            if !unique.insert(dependency) {
                return Err(format!(
                    "capability {} repeats dependency {dependency}",
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

#[cfg(test)]
mod tests {
    use super::*;

    const COMPLETE: &str = r#"
ledger-version 2
target test-radio
verification-project tools/verification-project.toml
vendor-evidence-index evidence/vendor.json
require-capability channel-switch

capability channel-switch
title Channel switch
scope One finite transition
implementation complete
host-proof covered
hil-proof qualified
async-proof bounded
owner driver/radio/src/channel.rs Channel
test driver/radio/src/channel.rs completes_channel
vendor-root archive set_channel
vendor-evidence radio archive set_channel
hil-evidence qualification/targets/esp32s31/records/record.md HIL_CHANNEL
"#;

    #[test]
    fn parses_a_closed_five_axis_capability() {
        let ledger = Ledger::parse(COMPLETE).unwrap();
        let capability = &ledger.capabilities["channel-switch"];
        assert_eq!(capability.vendor, VendorProof::Unmapped);
        assert_eq!(capability.vendor_roots[0].symbol, "set_channel");
    }

    #[test]
    fn non_terminal_axis_requires_a_named_gap() {
        let input = COMPLETE.replace("host-proof covered", "host-proof partial");
        let ledger = Ledger::parse(&input).unwrap();
        let capability = &ledger.capabilities["channel-switch"];
        assert!(!capability.host.is_terminal());
        let error = validate_gap_contract(capability).unwrap_err();
        assert!(error.to_string().contains("host axis"));
    }

    #[test]
    fn required_capability_cannot_silently_disappear() {
        let input = COMPLETE.replace(
            "\ncapability channel-switch\n",
            "\ncapability other-channel\n",
        );
        let error = Ledger::parse(&input).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("required capability channel-switch is missing")
        );
    }

    #[test]
    fn rejects_parent_paths_in_references() {
        let input = COMPLETE.replace(
            "driver/radio/src/channel.rs Channel",
            "../radio/src/channel.rs Channel",
        );
        let error = Ledger::parse(&input).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unsafe repository-relative path")
        );
    }

    #[test]
    fn vendor_status_is_derived_from_release_eligible_index_rows() {
        let ledger = Ledger::parse(COMPLETE).unwrap();
        let capability = &ledger.capabilities["channel-switch"];
        let root = std::env::temp_dir().join(format!(
            "qualification-check-vendor-index-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("channel.rs"), "production source").unwrap();
        let mut index = VendorEvidenceIndex {
            schema_version: 1,
            complete_project_run: true,
            entries: vec![VendorEvidenceIndexEntry {
                suite: "radio".to_owned(),
                source: "archive".to_owned(),
                symbol: "set_channel".to_owned(),
                evidence_class: "production-trace".to_owned(),
                status: "match".to_owned(),
                release_eligible: true,
                rust_component: Some("driver::set_channel".to_owned()),
                evidence_digest: Some("11".repeat(32)),
                baseline_passed: true,
                artifact_hashes: vec![VendorEvidenceArtifactHash {
                    sha256: "22".repeat(32),
                }],
                source_hashes: vec![VendorEvidenceSourceHash {
                    path: PathBuf::from("channel.rs"),
                    sha256: format!(
                        "{:x}",
                        Sha256::digest(std::fs::read(root.join("channel.rs")).unwrap())
                    ),
                }],
                release_blockers: Vec::new(),
            }],
        };
        assert_eq!(
            derive_vendor_proof(capability, &index, &root).unwrap(),
            VendorProof::Qualified
        );
        index.entries[0].release_eligible = false;
        assert_eq!(
            derive_vendor_proof(capability, &index, &root).unwrap(),
            VendorProof::Mapped
        );
        index.entries[0].release_eligible = true;
        std::fs::write(root.join("channel.rs"), "changed production source").unwrap();
        assert_eq!(
            derive_vendor_proof(capability, &index, &root).unwrap(),
            VendorProof::Mapped
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_removed_ledger_version_one() {
        let error =
            Ledger::parse(&COMPLETE.replace("ledger-version 2", "ledger-version 1")).unwrap_err();
        assert!(error.to_string().contains("unsupported ledger version 1"));
    }

    #[test]
    fn rejects_removed_handwritten_vendor_proof() {
        let input = COMPLETE.replace(
            "host-proof covered",
            "host-proof covered\nvendor-proof qualified",
        );
        let error = Ledger::parse(&input).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unknown capability directive \"vendor-proof\"")
        );
    }
}
