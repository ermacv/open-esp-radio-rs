use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

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
proof_axis!(VendorProof, |value| matches!(value, VendorProof::Qualified | VendorProof::NotApplicable), {
    "qualified" => Qualified,
    "mapped" => Mapped,
    "unmapped" => Unmapped,
    "not-applicable" => NotApplicable,
});
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
    vendor: Option<VendorProof>,
    hil: Option<HilProof>,
    async_proof: Option<AsyncProof>,
    owners: Vec<FileReference>,
    tests: Vec<FileReference>,
    vendor_roots: Vec<VendorRoot>,
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
            vendor: required(self.vendor, &id, line, "vendor proof")?,
            hil: required(self.hil, &id, line, "HIL proof")?,
            async_proof: required(self.async_proof, &id, line, "async proof")?,
            owners: self.owners,
            tests: self.tests,
            vendor_roots: self.vendor_roots,
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
    pub(crate) disposition_manifest: PathBuf,
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
        let mut disposition_manifest = None;
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
                        if parsed != 1 {
                            return Err(format!("unsupported ledger version {parsed}").into());
                        }
                        set_once(&mut version, parsed, directive, line)?;
                    }
                    "target" => {
                        set_once(&mut target, slug(value, "target", line)?, directive, line)?
                    }
                    "disposition-manifest" => set_once(
                        &mut disposition_manifest,
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
                "vendor-proof" => set_once(
                    &mut builder.vendor,
                    VendorProof::parse(value, line)?,
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
            finish(builder, &mut capabilities)?;
        }
        version.ok_or("ledger has no ledger-version")?;
        let target = target.ok_or("ledger has no target")?;
        let disposition_manifest =
            disposition_manifest.ok_or("ledger has no disposition-manifest")?;
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
            disposition_manifest,
            capabilities,
        })
    }

    pub(crate) fn validate(&self, root: &Path) -> Result<()> {
        if !root.is_dir() {
            return Err(format!("repository root {} is not a directory", root.display()).into());
        }
        let dispositions = DispositionIndex::load(&root.join(&self.disposition_manifest))?;
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

#[derive(Debug)]
struct DispositionEntry {
    has_rust_component: bool,
    has_contract: bool,
}

#[derive(Debug)]
struct DispositionIndex(BTreeMap<(String, String), DispositionEntry>);

impl DispositionIndex {
    fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path).map_err(|error| {
            format!(
                "cannot read disposition manifest {}: {error}",
                path.display()
            )
        })?;
        let mut entries = BTreeMap::new();
        let mut current: Option<((String, String), DispositionEntry)> = None;
        let finish = |entry: ((String, String), DispositionEntry),
                      entries: &mut BTreeMap<_, _>|
         -> Result<()> {
            if entries.insert(entry.0.clone(), entry.1).is_some() {
                return Err(
                    format!("duplicate disposition entry {} {}", entry.0.0, entry.0.1).into(),
                );
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
                .ok_or_else(|| format!("invalid disposition line {line}"))?;
            if directive == "function" {
                if let Some(entry) = current.take() {
                    finish(entry, &mut entries)?;
                }
                let words = words_exact(value, 2, directive, line)?;
                current = Some((
                    (words[0].to_owned(), words[1].to_owned()),
                    DispositionEntry {
                        has_rust_component: false,
                        has_contract: false,
                    },
                ));
            } else if let Some((_, entry)) = current.as_mut() {
                match directive {
                    "rust-component" => entry.has_rust_component = true,
                    "semantic-contract" | "effect-contract" => entry.has_contract = true,
                    _ => {}
                }
            }
        }
        if let Some(entry) = current {
            finish(entry, &mut entries)?;
        }
        Ok(Self(entries))
    }

    fn get(&self, root: &VendorRoot) -> Option<&DispositionEntry> {
        self.0.get(&(root.source.clone(), root.symbol.clone()))
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
        if !is_rust_source_under(&owner.path, "crates") {
            return Err(format!(
                "owner {} for {} is not production Rust under crates/*/src",
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
        if !test.path.starts_with("crates")
            || test.path.extension().and_then(|value| value.to_str()) != Some("rs")
        {
            return Err(format!(
                "test {} for {} is not Rust under crates",
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
        if !evidence.path.starts_with("docs/hil")
            || evidence.path.extension().and_then(|value| value.to_str()) != Some("md")
        {
            return Err(format!(
                "HIL evidence {} for {} is not a dated docs/hil record",
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
ledger-version 1
target test-radio
disposition-manifest tools/dispositions
require-capability channel-switch

capability channel-switch
title Channel switch
scope One finite transition
implementation complete
host-proof covered
vendor-proof qualified
hil-proof qualified
async-proof bounded
owner crates/radio/src/channel.rs Channel
test crates/radio/src/channel.rs completes_channel
vendor-root archive set_channel
hil-evidence docs/hil/record.md HIL_CHANNEL
"#;

    #[test]
    fn parses_a_closed_five_axis_capability() {
        let ledger = Ledger::parse(COMPLETE).unwrap();
        let capability = &ledger.capabilities["channel-switch"];
        assert!(capability.proof_ready());
        assert_eq!(capability.vendor_roots[0].symbol, "set_channel");
    }

    #[test]
    fn non_terminal_axis_requires_a_named_gap() {
        let input = COMPLETE.replace("vendor-proof qualified", "vendor-proof mapped");
        let ledger = Ledger::parse(&input).unwrap();
        let capability = &ledger.capabilities["channel-switch"];
        assert!(!capability.vendor.is_terminal());
        let error = validate_gap_contract(capability).unwrap_err();
        assert!(error.to_string().contains("vendor axis"));
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
            "crates/radio/src/channel.rs Channel",
            "../radio/src/channel.rs Channel",
        );
        let error = Ledger::parse(&input).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unsafe repository-relative path")
        );
    }
}
