//! The gate `cargo xtask push` runs before it pushes a branch, and that
//! `cargo xtask check changed` runs for the developer: what a set of changed
//! files can break, checked within a minute warm. CI is the full check of
//! every pull request.
//!
//! Selection reads the tree through the repository model (`oer-repo`),
//! without Cargo:
//!
//! - a file inside a package selects that package; a workspace's own
//!   manifest, and the files every build reads (`rust-toolchain.toml`,
//!   `.cargo/config.toml`, `clippy.toml`, `rustfmt.toml`), select every
//!   package of the workspaces they reach;
//! - a package's `open-radio.inputs` patterns name files outside it that its
//!   tests read (the evaluator reads the catalogs, xtask the workflows), so
//!   a change there selects it too;
//! - the model's path-dependency graph adds every package of the same
//!   workspace that depends on a selected one, through any dependency kind.
//!
//! What runs for a selection is the check registry's
//! ([`crate::registry`]): each check's trigger reads the [`Change`].

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    time::Duration,
};

use oer_repo::{Model, Platform};

use crate::{Result, registry::Tier};
use oer_process::Checkout;

/// Files every build of every workspace reads.
const GLOBAL: &[&str] = &[
    "rust-toolchain.toml",
    ".cargo/config.toml",
    "clippy.toml",
    "rustfmt.toml",
];

/// How long the tests of one workspace may run in the gate.
pub const TEST_LIMIT: Duration = Duration::from_secs(20 * 60);

/// One package as the gate sees it.
#[derive(Clone, Debug)]
pub struct Package {
    pub name: String,
    /// Repository path of its directory.
    pub directory: String,
    /// Repository path of its workspace's root manifest.
    pub workspace: String,
    /// Whether the gate builds and tests it on the host: every package of
    /// the root workspace, whose tests run on the host whatever its
    /// platform, and host or portable packages elsewhere. Other packages
    /// build only for a chip, which CI covers.
    pub host: bool,
    /// Whether it is a chip or family package, whose code under the chip
    /// target's `cfg` the host never compiles even when it builds the rest.
    pub chip: bool,
    /// Patterns of repository files outside the package its tests read.
    pub inputs: Vec<String>,
}

/// The packages and workspaces of the tree.
#[derive(Clone, Debug)]
pub struct Tree {
    /// The repository model the gate reads.
    pub model: Model,
    pub packages: Vec<Package>,
    /// Root manifests of every workspace.
    pub workspaces: Vec<String>,
}

impl Tree {
    /// The tree of the checkout at `root`.
    pub fn load(root: &Path) -> Result<Self> {
        Ok(Self::of(Model::load(&oer_repo::Repo::from_git(root)?)?))
    }

    /// The gate's view of `model`.
    pub fn of(model: Model) -> Self {
        let mut packages = Vec::new();
        for package in model.packages() {
            let Some(workspace) = model.workspace_of(package) else {
                continue;
            };
            let class = model.classification(package);
            let platform = class.as_ref().map(|class| &class.platform);
            packages.push(Package {
                name: package.name.clone(),
                directory: package.directory.clone(),
                workspace: workspace.to_owned(),
                host: workspace == "Cargo.toml"
                    || matches!(platform, Ok(Platform::Host | Platform::Portable)),
                chip: matches!(platform, Ok(Platform::Chip(_) | Platform::Family(_))),
                inputs: class.map(|class| class.inputs.clone()).unwrap_or_default(),
            });
        }
        Self {
            workspaces: model.workspaces().to_vec(),
            model,
            packages,
        }
    }

    /// The package a path belongs to ([`Model::owner`]).
    fn owner(&self, path: &str) -> Option<&Package> {
        let owner = self.model.owner(path)?;
        self.packages
            .iter()
            .find(|package| package.directory == owner.directory)
    }
}

/// A package of a workspace: `(workspace manifest, package name)`.
pub type Key = (String, String);

/// What a set of changed files needs.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Selection {
    /// Workspaces whose formatting a changed Rust file can break.
    pub format: BTreeSet<String>,
    /// Packages a changed file belongs to or that declare it an input.
    pub packages: BTreeSet<Key>,
    /// Workspaces whose manifests or lock changed: their lock must still
    /// match their manifests.
    pub locks: BTreeSet<String>,
    /// Markdown, qualification catalogs or programs changed.
    pub docs: bool,
    /// Rust sources or capability catalogs changed: the changed Rust files,
    /// whose capability anchors are listed.
    pub capabilities: Option<BTreeSet<String>>,
}

impl Selection {
    /// Whether nothing needs a check beyond the integrity tier.
    pub fn is_empty(&self) -> bool {
        self.format.is_empty()
            && self.packages.is_empty()
            && self.locks.is_empty()
            && !self.docs
            && self.capabilities.is_none()
    }
}

/// Maps changed repository paths to what they need.
pub fn select(tree: &Tree, changed: &[String]) -> Selection {
    let mut selection = Selection::default();
    let whole = |selection: &mut Selection, workspace: &str| {
        selection.format.insert(workspace.to_owned());
        selection.locks.insert(workspace.to_owned());
        for package in tree.packages.iter().filter(|p| p.workspace == workspace) {
            selection
                .packages
                .insert((package.workspace.clone(), package.name.clone()));
        }
    };
    for path in changed {
        let name = path.rsplit('/').next().unwrap_or(path);
        let extension = name.rsplit_once('.').map_or("", |(_, extension)| extension);
        if extension == "md" || path.starts_with("qualification/") {
            selection.docs = true;
        }
        if extension == "rs" || path.starts_with("qualification/catalog/") {
            let anchored = selection.capabilities.get_or_insert_default();
            if extension == "rs" {
                anchored.insert(path.clone());
            }
        }
        if GLOBAL.contains(&path.as_str()) {
            for workspace in &tree.workspaces {
                whole(&mut selection, workspace);
            }
            continue;
        }
        if tree.workspaces.contains(path)
            && !tree
                .packages
                .iter()
                .any(|package| format!("{}Cargo.toml", prefix(&package.directory)) == *path)
        {
            // A virtual workspace's manifest: its lints, profiles, patches
            // and shared dependencies reach every member.
            whole(&mut selection, path);
            continue;
        }
        if name == "Cargo.lock" {
            let manifest = format!("{}Cargo.toml", path.trim_end_matches("Cargo.lock"));
            if tree.workspaces.contains(&manifest) {
                selection.locks.insert(manifest);
            }
            continue;
        }
        if let Some(package) = tree.owner(path) {
            selection
                .packages
                .insert((package.workspace.clone(), package.name.clone()));
            if extension == "rs" {
                selection.format.insert(package.workspace.clone());
            }
            if name == "Cargo.toml" {
                // A package's dependencies reach every workspace that builds
                // it through a path dependency: the firmware workspaces lock
                // the production crates too.
                selection.locks.extend(tree.workspaces.iter().cloned());
            }
        }
        for package in &tree.packages {
            if package
                .inputs
                .iter()
                .any(|pattern| oer_repo::classification::input_matches(pattern, path))
            {
                selection
                    .packages
                    .insert((package.workspace.clone(), package.name.clone()));
            }
        }
    }
    selection
}

fn prefix(directory: &str) -> String {
    if directory.is_empty() {
        String::new()
    } else {
        format!("{directory}/")
    }
}

/// The `(name, version, source, dependencies)` of every entry of a lock
/// file; an unreadable or absent lock has none.
type LockEntry = (String, String, Option<String>, Vec<String>);

fn lock_entries(text: &str) -> Vec<LockEntry> {
    oer_repo::lock::parse(text, "Cargo.lock")
        .unwrap_or_default()
        .into_iter()
        .map(|entry| (entry.name, entry.version, entry.source, entry.dependencies))
        .collect()
}

/// The workspace members of the lock `new` whose resolved dependencies
/// differ from the lock `old`: every package that reaches, through the
/// lock's dependency lists, an entry that is new or changed (a different
/// version, source or dependency list). Members are the entries without a
/// source.
pub fn lock_dependents(old: &str, new: &str) -> BTreeSet<String> {
    let old: BTreeSet<LockEntry> = lock_entries(old).into_iter().collect();
    let new = lock_entries(new);
    // A dependency is named `name`, `name version` or `name version (source)`.
    let names = |dependency: &str| -> (String, Option<String>) {
        let mut words = dependency.split(' ');
        (
            words.next().unwrap_or_default().to_owned(),
            words.next().map(str::to_owned),
        )
    };
    let mut changed: BTreeSet<(String, String)> = new
        .iter()
        .filter(|entry| !old.contains(*entry))
        .map(|(name, version, _, _)| (name.clone(), version.clone()))
        .collect();
    loop {
        let before = changed.len();
        for (name, version, _, dependencies) in &new {
            if changed.contains(&(name.clone(), version.clone())) {
                continue;
            }
            let reaches = dependencies.iter().any(|dependency| {
                let (dependency, pinned) = names(dependency);
                changed.iter().any(|(name, version)| {
                    *name == dependency && pinned.as_ref().is_none_or(|pinned| pinned == version)
                })
            });
            if reaches {
                changed.insert((name.clone(), version.clone()));
            }
        }
        if changed.len() == before {
            break;
        }
    }
    new.iter()
        .filter(|(name, version, source, _)| {
            source.is_none() && changed.contains(&(name.clone(), version.clone()))
        })
        .map(|(name, ..)| name.clone())
        .collect()
}

/// Selects the members whose resolved dependencies a lock change among
/// `changed` alters, comparing each lock at `base` with the lock at `to`,
/// or in the working tree when `to` is `None`.
pub fn select_locks(
    ctx: &Checkout,
    tree: &Tree,
    changed: &[String],
    base: &str,
    to: Option<&str>,
    selection: &mut Selection,
) -> Result<()> {
    for lock in changed
        .iter()
        .filter(|path| path.rsplit('/').next() == Some("Cargo.lock"))
    {
        let workspace = format!("{}Cargo.toml", lock.trim_end_matches("Cargo.lock"));
        let at = |revision: &str| {
            oer_process::git::output(&ctx.root, ["show", &format!("{revision}:{lock}")])
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .unwrap_or_default()
        };
        let old = at(base);
        let new = match to {
            Some(revision) => at(revision),
            None => std::fs::read_to_string(ctx.root.join(lock)).unwrap_or_default(),
        };
        for name in lock_dependents(&old, &new) {
            if tree
                .packages
                .iter()
                .any(|package| package.workspace == workspace && package.name == name)
            {
                selection.packages.insert((workspace.clone(), name));
            }
        }
    }
    Ok(())
}

/// The selected packages and every package of the same workspace that
/// depends on one of them, through any dependency kind.
pub fn affected(tree: &Tree, selection: &Selection) -> BTreeSet<Key> {
    let mut by_workspace: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (workspace, package) in &selection.packages {
        by_workspace.entry(workspace).or_default().insert(package);
    }
    let mut affected = BTreeSet::new();
    for (workspace, selected) in by_workspace {
        let members: Vec<&oer_repo::Package> = tree.model.members(workspace).collect();
        let chosen: Vec<&oer_repo::Package> = members
            .iter()
            .copied()
            .filter(|package| selected.contains(package.name.as_str()))
            .collect();
        for package in tree.model.dependents(&members, &chosen) {
            affected.insert((workspace.to_owned(), package.name.clone()));
        }
    }
    affected
}

/// The packages whose tests a change at `tier` runs: the selected ones,
/// and from [`Tier::Full`] on every affected one.
pub fn tested<'a>(
    tier: Tier,
    selection: &Selection,
    affected: &'a BTreeSet<Key>,
) -> BTreeSet<&'a Key> {
    affected
        .iter()
        .filter(|key| tier >= Tier::Full || selection.packages.contains(*key))
        .collect()
}

/// The packages of `affected` with code the host checks of the gate never
/// compile: those built only for a chip, and chip or family packages the
/// host builds without their chip-target `cfg` code.
pub fn chip_code<'a>(tree: &Tree, affected: &'a BTreeSet<Key>) -> BTreeSet<&'a Key> {
    affected
        .iter()
        .filter(|(workspace, name)| {
            !tree
                .packages
                .iter()
                .any(|p| &p.workspace == workspace && &p.name == name && p.host && !p.chip)
        })
        .collect()
}

/// A change as the checks see it: its files, the tree, what it selects and
/// reaches, and the tier it is checked at.
#[derive(Clone, Debug)]
pub struct Change {
    pub files: Vec<String>,
    pub tree: Tree,
    pub selection: Selection,
    pub affected: BTreeSet<Key>,
    pub tier: Tier,
}

impl Change {
    /// The change of `files` against `base`, whose locks are compared with
    /// the lock at `to`, or in the working tree when `to` is `None`.
    pub fn of(
        ctx: &Checkout,
        files: Vec<String>,
        base: &str,
        to: Option<&str>,
        tier: Tier,
    ) -> Result<Self> {
        let tree = Tree::load(&ctx.root)?;
        let mut selection = select(&tree, &files);
        select_locks(ctx, &tree, &files, base, to, &mut selection)?;
        let affected = affected(&tree, &selection);
        Ok(Self {
            files,
            tree,
            selection,
            affected,
            tier,
        })
    }
}

/// The merge base of `HEAD` and `base`.
pub fn merge_base(ctx: &Checkout, base: &str) -> Result<String> {
    oer_process::git::text(&ctx.root, ["merge-base", "HEAD", base])
}

/// The files `HEAD` changed against `merge_base`.
pub fn committed(ctx: &Checkout, merge_base: &str) -> Result<Vec<String>> {
    oer_process::git::lines(&ctx.root, ["diff", "--name-only", merge_base, "HEAD"])
}

/// The files of the working tree that differ from `HEAD`: modified, staged
/// and untracked but not ignored.
pub fn uncommitted(ctx: &Checkout) -> Result<Vec<String>> {
    let mut files: BTreeSet<String> =
        oer_process::git::lines(&ctx.root, ["diff", "--name-only", "HEAD"])?
            .into_iter()
            .collect();
    files.extend(oer_process::git::lines(
        &ctx.root,
        ["ls-files", "--others", "--exclude-standard"],
    )?);
    Ok(files.into_iter().collect())
}

#[cfg(test)]
mod tests;
