//! Code anchors bind catalog entries to the code that owns them.
//!
//! A `// CAPABILITY: <id>[, <id>...]` line comment placed directly above a
//! Rust item (after its doc comments and attributes) names the catalog
//! inventory items, source facts or catalog capabilities that item owns. The
//! catalog states what is supported; anchors state where. Checking both
//! against each other keeps the feature map from drifting away from the code:
//!
//! - an implemented, partial or fail-closed entry needs an anchor in a
//!   production package;
//! - a diagnostic entry needs an anchor anywhere;
//! - a host-only entry belongs to an upper protocol stack outside the radio
//!   and may be anchored or not;
//! - an absent entry has no anchor;
//! - a capability declared `implementation = "complete"` is anchored itself
//!   or through source facts that are all implemented and anchored;
//! - every anchor names an existing entry, and an inventory item that
//!   projects a source fact is anchored through that fact.
//!
//! Anchors are reference metadata like the catalog's other links. They are
//! never evidence and never change a readiness axis.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::Result;
use crate::model::{CatalogView, ImplementationProof, SourceStatus};

const MARKER: &str = "// CAPABILITY:";

/// Directories never scanned: build output, private inputs and VCS state.
const SKIPPED: &[&str] = &["target", "_oracles"];

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Scope {
    Production,
    Experimental,
    Development,
}

/// The package that owns an anchored file, as its manifest classifies it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Package {
    pub(crate) name: String,
    pub(crate) scope: Scope,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct Location {
    pub(crate) path: PathBuf,
    pub(crate) line: usize,
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.path.display(), self.line)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Anchor {
    pub(crate) id: String,
    pub(crate) location: Location,
    /// The first line of the anchored item.
    pub(crate) item: String,
    pub(crate) package: Package,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum EntryKind {
    Item,
    Fact,
    Capability,
}

impl EntryKind {
    const fn label(self) -> &'static str {
        match self {
            Self::Item => "item",
            Self::Fact => "source fact",
            Self::Capability => "capability",
        }
    }
}

/// What a catalog entry's declared state requires of its anchors.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Expectation {
    /// Implemented, partial or fail-closed: owned by production code.
    Production,
    Diagnostic,
    Absent,
    /// A complete capability: anchored itself or through these facts.
    Complete {
        facts: Vec<String>,
    },
    /// A host-only entry or an incomplete capability may be anchored or not.
    Unconstrained,
}

#[derive(Clone, Debug)]
pub(crate) struct Entry {
    pub(crate) id: String,
    pub(crate) kind: EntryKind,
    /// The inventory domain or owning catalog, so owners can find theirs.
    pub(crate) owner: String,
    pub(crate) expectation: Expectation,
    /// The packages an inventory item declares; every anchor of the item
    /// must sit in one of them. Empty when none are declared.
    pub(crate) packages: Vec<String>,
}

impl fmt::Display for Entry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} `{}`", self.owner, self.kind.label(), self.id)
    }
}

/// The anchoring obligations of a catalog view.
#[derive(Debug, Default)]
pub(crate) struct Ledger {
    pub(crate) entries: Vec<Entry>,
    /// Inventory items that project a source fact of another id.
    pub(crate) projections: BTreeMap<String, String>,
}

impl Ledger {
    pub(crate) fn from_view(view: &CatalogView) -> Self {
        let domains = view
            .sections
            .iter()
            .map(|section| (section.id.as_str(), section.domain.as_str()))
            .collect::<BTreeMap<_, _>>();
        let mut ledger = Self::default();
        let mut fact_owners = BTreeMap::new();
        for item in &view.items {
            let owner = domains
                .get(item.section.as_str())
                .copied()
                .unwrap_or("inventory")
                .to_owned();
            match &item.source_fact {
                Some(fact) => {
                    fact_owners.insert(fact.clone(), owner);
                    if *fact != item.id {
                        ledger.projections.insert(item.id.clone(), fact.clone());
                    }
                }
                None => ledger.entries.push(Entry {
                    id: item.id.clone(),
                    kind: EntryKind::Item,
                    owner,
                    expectation: expectation(item.status),
                    packages: item.packages.clone(),
                }),
            }
        }
        for (id, fact) in &view.source_facts {
            ledger.entries.push(Entry {
                id: id.clone(),
                kind: EntryKind::Fact,
                owner: fact_owners
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| "source-facts".into()),
                expectation: expectation(fact.status),
                packages: Vec::new(),
            });
        }
        for (id, capability) in &view.capabilities {
            ledger.entries.push(Entry {
                id: id.clone(),
                kind: EntryKind::Capability,
                owner: view
                    .capability_owners
                    .get(id)
                    .map_or_else(|| "program".into(), |(catalog, _)| catalog.clone()),
                expectation: match capability.implementation {
                    ImplementationProof::Complete => Expectation::Complete {
                        facts: capability.source_fact_refs.clone(),
                    },
                    ImplementationProof::Incomplete => Expectation::Unconstrained,
                },
                packages: Vec::new(),
            });
        }
        ledger
    }
}

const fn expectation(status: SourceStatus) -> Expectation {
    match status {
        SourceStatus::Implemented | SourceStatus::Partial | SourceStatus::FailClosed => {
            Expectation::Production
        }
        SourceStatus::HostOnly => Expectation::Unconstrained,
        SourceStatus::Diagnostic => Expectation::Diagnostic,
        SourceStatus::Absent => Expectation::Absent,
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Problem {
    Malformed {
        location: Location,
        reason: String,
    },
    /// The marker is not directly above a Rust item.
    Detached {
        location: Location,
    },
    Unclassified {
        location: Location,
    },
    Unknown {
        location: Location,
        id: String,
    },
    Ambiguous {
        location: Location,
        id: String,
    },
    Projection {
        location: Location,
        item: String,
        fact: String,
    },
    Missing {
        entry: Entry,
    },
    NotProduction {
        entry: Entry,
        anchors: Vec<Location>,
    },
    Anchored {
        entry: Entry,
        location: Location,
    },
    Unbacked {
        entry: Entry,
    },
    /// The item lists its packages, and this anchor sits in none of them.
    OutsidePackages {
        entry: Entry,
        location: Location,
        package: String,
    },
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed { location, reason } => {
                write!(f, "{location}: malformed CAPABILITY anchor: {reason}")
            }
            Self::Detached { location } => write!(
                f,
                "{location}: CAPABILITY anchor must sit directly above the Rust item it names"
            ),
            Self::Unclassified { location } => write!(
                f,
                "{location}: CAPABILITY anchor outside a package with open-radio scope"
            ),
            Self::Unknown { location, id } => {
                write!(
                    f,
                    "{location}: CAPABILITY anchor names unknown catalog entry `{id}`"
                )
            }
            Self::Ambiguous { location, id } => write!(
                f,
                "{location}: CAPABILITY anchor `{id}` names both an inventory item and a capability"
            ),
            Self::Projection {
                location,
                item,
                fact,
            } => write!(
                f,
                "{location}: `{item}` projects source fact `{fact}`; anchor the fact instead"
            ),
            Self::Missing { entry } => write!(f, "{entry}: no CAPABILITY anchor in code"),
            Self::NotProduction { entry, anchors } => write!(
                f,
                "{entry}: anchored only outside production packages ({})",
                list(anchors)
            ),
            Self::Anchored { entry, location } => {
                write!(f, "{entry}: absent but anchored at {location}")
            }
            Self::OutsidePackages {
                entry,
                location,
                package,
            } => write!(
                f,
                "{entry}: anchored at {location} in `{package}`, which its `packages` does not list"
            ),
            Self::Unbacked { entry } => write!(
                f,
                "{entry}: implementation complete without a production anchor on the capability \
                 or on implemented, anchored source facts"
            ),
        }
    }
}

fn list(locations: &[Location]) -> String {
    locations
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Every anchor under `root`, and every marker that cannot be one.
pub(crate) fn scan(root: &Path) -> Result<(Vec<Anchor>, Vec<Problem>)> {
    let mut files = Vec::new();
    collect(root, root, &mut files)?;
    files.sort();
    let mut packages = Packages::default();
    let mut anchors = Vec::new();
    let mut problems = Vec::new();
    for relative in files {
        let text = fs::read_to_string(root.join(&relative))?;
        let markers = markers(&relative, &text, &mut problems);
        if markers.is_empty() {
            continue;
        }
        let package = packages.of(root, &relative)?;
        for (location, ids, item) in markers {
            let Some(package) = package.clone() else {
                problems.push(Problem::Unclassified { location });
                continue;
            };
            for id in ids {
                anchors.push(Anchor {
                    id,
                    location: location.clone(),
                    item: item.clone(),
                    package: package.clone(),
                });
            }
        }
    }
    Ok((anchors, problems))
}

fn collect(root: &Path, directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            if !name.starts_with('.') && !SKIPPED.contains(&name.as_ref()) {
                collect(root, &entry.path(), files)?;
            }
        } else if kind.is_file() && name.ends_with(".rs") {
            files.push(entry.path().strip_prefix(root)?.to_owned());
        }
    }
    Ok(())
}

/// The markers of one file: location, named ids and the anchored item.
fn markers(
    path: &Path,
    text: &str,
    problems: &mut Vec<Problem>,
) -> Vec<(Location, Vec<String>, String)> {
    let lines = text.lines().collect::<Vec<_>>();
    let mut found = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let Some(rest) = line.trim_start().strip_prefix(MARKER) else {
            continue;
        };
        let location = Location {
            path: path.to_owned(),
            line: index + 1,
        };
        let ids = match parse_ids(rest) {
            Ok(ids) => ids,
            Err(reason) => {
                problems.push(Problem::Malformed { location, reason });
                continue;
            }
        };
        match anchored_item(&lines[index + 1..]) {
            Some(item) => found.push((location, ids, item)),
            None => problems.push(Problem::Detached { location }),
        }
    }
    found
}

fn parse_ids(rest: &str) -> std::result::Result<Vec<String>, String> {
    let mut ids = Vec::new();
    for id in rest.split(',').map(str::trim) {
        let valid = id.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
            && id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !id.ends_with('-')
            && !id.contains("--");
        if !valid {
            return Err(format!("`{id}` is not a catalog id"));
        }
        if ids.iter().any(|known| known == id) {
            return Err(format!("`{id}` is repeated"));
        }
        ids.push(id.to_owned());
    }
    Ok(ids)
}

/// The item the lines after a marker declare, skipping comments and
/// attributes; `None` when anything else comes first.
fn anchored_item(lines: &[&str]) -> Option<String> {
    let mut attribute_depth = 0i32;
    for line in lines {
        let trimmed = line.trim();
        if attribute_depth > 0 || trimmed.starts_with("#[") {
            attribute_depth += bracket_balance(trimmed);
            continue;
        }
        if trimmed.starts_with("//") {
            continue;
        }
        return is_item(trimmed).then(|| {
            trimmed
                .trim_end_matches('{')
                .trim_end()
                .chars()
                .take(100)
                .collect()
        });
    }
    None
}

fn bracket_balance(line: &str) -> i32 {
    line.chars().fold(0, |depth, c| match c {
        '[' => depth + 1,
        ']' => depth - 1,
        _ => depth,
    })
}

fn is_item(line: &str) -> bool {
    const QUALIFIERS: &[&str] = &["pub", "unsafe", "async", "default", "extern", "\"C\""];
    const ITEMS: &[&str] = &[
        "fn", "struct", "enum", "trait", "impl", "mod", "type", "const", "static", "union",
    ];
    let first = line
        .split_whitespace()
        .find(|token| !QUALIFIERS.contains(token) && !token.starts_with("pub("));
    first.is_some_and(|token| {
        ITEMS.contains(&token.trim_end_matches(|c: char| !c.is_ascii_alphanumeric()))
            || token.starts_with("impl<")
            || token == "macro_rules!"
    })
}

/// Package classification of each directory, read once.
#[derive(Default)]
struct Packages(BTreeMap<PathBuf, Option<Package>>);

impl Packages {
    fn of(&mut self, root: &Path, file: &Path) -> Result<Option<Package>> {
        let mut directory = file.parent();
        while let Some(current) = directory {
            if let Some(known) = self.0.get(current) {
                return Ok(known.clone());
            }
            let manifest = root.join(current).join("Cargo.toml");
            if manifest.is_file()
                && let Some(package) = classify(&fs::read_to_string(&manifest)?, &manifest)?
            {
                self.0.insert(current.to_owned(), package.clone());
                return Ok(package);
            }
            directory = current.parent();
        }
        Ok(None)
    }
}

#[derive(Deserialize)]
struct Manifest {
    package: Option<ManifestPackage>,
}

#[derive(Deserialize)]
struct ManifestPackage {
    name: String,
    #[serde(default)]
    metadata: Option<ManifestMetadata>,
}

#[derive(Deserialize)]
struct ManifestMetadata {
    #[serde(rename = "open-radio")]
    open_radio: Option<OpenRadio>,
}

#[derive(Deserialize)]
struct OpenRadio {
    scope: Scope,
}

/// `None` when the manifest is a virtual workspace; `Some(None)` for a
/// package without classification.
fn classify(text: &str, path: &Path) -> Result<Option<Option<Package>>> {
    let manifest: Manifest =
        toml_edit::de::from_str(text).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(manifest.package.map(|package| {
        package
            .metadata
            .and_then(|metadata| metadata.open_radio)
            .map(|open_radio| Package {
                name: package.name,
                scope: open_radio.scope,
            })
    }))
}

/// Every violation of the anchoring rules.
pub(crate) fn check(ledger: &Ledger, anchors: &[Anchor]) -> Vec<Problem> {
    let mut problems = Vec::new();
    let mut by_entry: BTreeMap<(EntryKind, &str), Vec<&Anchor>> = BTreeMap::new();
    let kinds = ledger.entries.iter().fold(
        BTreeMap::<&str, BTreeSet<EntryKind>>::new(),
        |mut kinds, entry| {
            kinds.entry(&entry.id).or_default().insert(entry.kind);
            kinds
        },
    );
    for anchor in anchors {
        if let Some(fact) = ledger.projections.get(&anchor.id) {
            problems.push(Problem::Projection {
                location: anchor.location.clone(),
                item: anchor.id.clone(),
                fact: fact.clone(),
            });
            continue;
        }
        let Some(found) = kinds.get(anchor.id.as_str()) else {
            problems.push(Problem::Unknown {
                location: anchor.location.clone(),
                id: anchor.id.clone(),
            });
            continue;
        };
        // A projecting item may share its fact's id; the fact owns it.
        let kind = if found.contains(&EntryKind::Fact) {
            EntryKind::Fact
        } else if found.len() > 1 {
            problems.push(Problem::Ambiguous {
                location: anchor.location.clone(),
                id: anchor.id.clone(),
            });
            continue;
        } else {
            *found.first().expect("an entry kind")
        };
        by_entry.entry((kind, &anchor.id)).or_default().push(anchor);
    }
    let anchored = |kind: EntryKind, id: &str| -> Vec<&Anchor> {
        by_entry.get(&(kind, id)).cloned().unwrap_or_default()
    };
    let production = |anchors: &[&Anchor]| {
        anchors
            .iter()
            .any(|anchor| anchor.package.scope == Scope::Production)
    };
    let facts = ledger
        .entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::Fact)
        .map(|entry| (entry.id.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    for entry in &ledger.entries {
        let own = anchored(entry.kind, &entry.id);
        if !entry.packages.is_empty() {
            for anchor in &own {
                if !entry.packages.contains(&anchor.package.name) {
                    problems.push(Problem::OutsidePackages {
                        entry: entry.clone(),
                        location: anchor.location.clone(),
                        package: anchor.package.name.clone(),
                    });
                }
            }
        }
        let locations = || own.iter().map(|a| a.location.clone()).collect::<Vec<_>>();
        match &entry.expectation {
            Expectation::Production if own.is_empty() => {
                problems.push(Problem::Missing {
                    entry: entry.clone(),
                });
            }
            Expectation::Production if !production(&own) => {
                problems.push(Problem::NotProduction {
                    entry: entry.clone(),
                    anchors: locations(),
                });
            }
            Expectation::Diagnostic if own.is_empty() => {
                problems.push(Problem::Missing {
                    entry: entry.clone(),
                });
            }
            Expectation::Absent => {
                if let Some(anchor) = own.first() {
                    problems.push(Problem::Anchored {
                        entry: entry.clone(),
                        location: anchor.location.clone(),
                    });
                }
            }
            Expectation::Complete { facts: refs } => {
                let through_facts = !refs.is_empty()
                    && refs.iter().all(|fact| {
                        facts.get(fact.as_str()).is_some_and(|fact| {
                            fact.expectation == Expectation::Production
                                && production(&anchored(EntryKind::Fact, &fact.id))
                        })
                    });
                if !production(&own) && !through_facts {
                    problems.push(Problem::Unbacked {
                        entry: entry.clone(),
                    });
                }
            }
            Expectation::Production | Expectation::Diagnostic | Expectation::Unconstrained => {}
        }
    }
    problems
}

/// The catalog entries anchored in `changed` files, for review after an edit.
pub(crate) fn touched<'a>(
    anchors: &'a [Anchor],
    changed: &BTreeSet<PathBuf>,
) -> BTreeMap<&'a str, Vec<&'a Anchor>> {
    let mut touched: BTreeMap<&str, Vec<&Anchor>> = BTreeMap::new();
    for anchor in anchors {
        if changed.contains(&anchor.location.path) {
            touched.entry(&anchor.id).or_default().push(anchor);
        }
    }
    touched
}

/// `cargo qualification catalog anchors`: check the view's anchors, list the
/// entries anchored in `changed` files, and fail on any problem.
pub(crate) fn run(view: &CatalogView, root: &Path, changed: &[PathBuf]) -> Result<()> {
    let (anchors, mut problems) = scan(root)?;
    let ledger = Ledger::from_view(view);
    problems.extend(check(&ledger, &anchors));
    let changed = changed.iter().cloned().collect::<BTreeSet<_>>();
    for (id, anchors) in touched(&anchors, &changed) {
        for anchor in anchors {
            println!(
                "CAPABILITY-CHANGED\t{id}\t{}\t{}",
                anchor.location, anchor.item
            );
        }
    }
    let mut lines = problems.iter().map(ToString::to_string).collect::<Vec<_>>();
    lines.sort();
    for line in &lines {
        eprintln!("{line}");
    }
    println!(
        "CAPABILITY-ANCHORS\tanchors={}\tentries={}\tproblems={}",
        anchors.len(),
        ledger.entries.len(),
        lines.len()
    );
    if lines.is_empty() {
        Ok(())
    } else {
        Err(format!("{} capability anchor problems", lines.len()).into())
    }
}

#[cfg(test)]
mod tests;
