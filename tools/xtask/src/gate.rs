//! The gate `cargo xtask push` runs before it pushes a branch, and that
//! `cargo xtask check changed` runs for the developer: what a set of changed
//! files can break, checked within a minute warm. CI is the full check of
//! every pull request.
//!
//! Selection reads the tree through `oer-tidy`'s model, without Cargo:
//!
//! - a file inside a package selects that package; a workspace's own
//!   manifest, and the files every build reads (`rust-toolchain.toml`,
//!   `.cargo/config.toml`, `clippy.toml`, `rustfmt.toml`), select every
//!   package of the workspaces they reach;
//! - a package's `open-radio.inputs` patterns name files outside it that its
//!   tests read (the evaluator reads the catalogs, xtask the workflows), so
//!   a change there selects it too;
//! - one `cargo metadata --no-deps` per workspace adds every package that
//!   depends on a selected one, through any dependency kind.
//!
//! The gate then runs the integrity tier over the whole tree, `cargo fmt`
//! of every workspace a Rust file changed in, the lock check of every
//! workspace whose manifests changed, the capability check when catalogs or
//! code changed, Clippy of the selected host packages and their dependents,
//! so a changed interface fails where it is used, and the tests of the
//! selected host packages with a time limit ([`Depth::Fast`]). The tests of
//! the dependents, the Markdown check, firmware, images, the PHY audit,
//! registers, provenance and API documentation are CI's; `check changed
//! --full` runs them locally ([`Depth::Full`]).

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use oer_process as process;
use oer_tidy::classification::Platform;

use crate::{Context, Result, cargo};

/// Files every build of every workspace reads.
const GLOBAL: &[&str] = &[
    "rust-toolchain.toml",
    ".cargo/config.toml",
    "clippy.toml",
    "rustfmt.toml",
];

/// The image classes CI type-checks on every pull request; the gate
/// type-checks them whenever chip code changes.
const FINAL_IMAGES: [oer_hil_image_class::ImageClass; 2] = [
    oer_hil_image_class::ImageClass::Performance,
    oer_hil_image_class::ImageClass::Correctness,
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
#[derive(Clone, Debug, Default)]
pub struct Tree {
    pub packages: Vec<Package>,
    /// Root manifests of every workspace.
    pub workspaces: Vec<String>,
}

impl Tree {
    /// The tree of the checkout at `root`, as `oer-tidy` reads it.
    pub fn load(root: &Path) -> Result<Self> {
        let repo = oer_tidy::repo::Repo::from_git(root)?;
        let manifests = oer_tidy::manifest::Manifests::load(&repo)?;
        let owners = oer_tidy::workspaces::owners(&manifests);
        let mut packages = Vec::new();
        for package in &manifests.packages {
            let Some(workspace) = owners.get(&package.manifest) else {
                continue;
            };
            let platform = oer_tidy::classification::of(package).map(|class| class.platform);
            let inputs = package
                .open_radio
                .as_ref()
                .and_then(|table| table.get("inputs"))
                .and_then(toml::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(toml::Value::as_str)
                .map(str::to_owned)
                .collect();
            packages.push(Package {
                name: package.name.clone(),
                directory: package.directory.clone(),
                workspace: workspace.clone(),
                host: workspace == "Cargo.toml"
                    || matches!(platform, Ok(Platform::Host | Platform::Portable)),
                chip: matches!(platform, Ok(Platform::Chip(_) | Platform::Family(_))),
                inputs,
            });
        }
        Ok(Self {
            packages,
            workspaces: oer_tidy::workspaces::discover(&manifests),
        })
    }

    /// The package whose directory holds `path`, the innermost one.
    fn owner(&self, path: &str) -> Option<&Package> {
        self.packages
            .iter()
            .filter(|package| {
                package.directory.is_empty() || path.starts_with(&format!("{}/", package.directory))
            })
            .max_by_key(|package| package.directory.len())
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
                .any(|pattern| oer_tidy::classification::input_matches(pattern, path))
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

/// One `[[package]]` entry of a lock file: its identity and dependencies.
type LockEntry = (String, String, Option<String>, Vec<String>);

fn lock_entries(text: &str) -> Vec<LockEntry> {
    let Ok(lock) = text.parse::<toml::Table>() else {
        return Vec::new();
    };
    lock.get("package")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let field = |key: &str| {
                entry
                    .get(key)
                    .and_then(toml::Value::as_str)
                    .map(str::to_owned)
            };
            Some((
                field("name")?,
                field("version")?,
                field("source"),
                entry
                    .get("dependencies")
                    .and_then(toml::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(toml::Value::as_str)
                    .map(str::to_owned)
                    .collect(),
            ))
        })
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
    ctx: &Context,
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
        let at =
            |revision: &str| git(ctx, &["show", &format!("{revision}:{lock}")]).unwrap_or_default();
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

/// The path dependencies of every member of one workspace, by name.
pub type Edges = BTreeMap<String, BTreeSet<String>>;

/// `selected` and, transitively, every package of `edges` that depends on
/// one of them.
pub fn dependents(edges: &Edges, selected: &BTreeSet<String>) -> BTreeSet<String> {
    let mut found = selected.clone();
    loop {
        let before = found.len();
        for (package, dependencies) in edges {
            if !found.contains(package) && dependencies.iter().any(|d| found.contains(d)) {
                found.insert(package.clone());
            }
        }
        if found.len() == before {
            return found;
        }
    }
}

/// Every member of `workspace` with the members it depends on by path,
/// through every dependency kind: one `cargo metadata --no-deps`.
pub fn edges(ctx: &Context, workspace: &str) -> Result<Edges> {
    let metadata = cargo::metadata_no_deps(ctx, &ctx.root.join(workspace))?;
    let members: Vec<_> = metadata
        .packages
        .iter()
        .filter(|package| metadata.workspace_members.contains(&package.id))
        .collect();
    let by_directory: BTreeMap<_, _> = members
        .iter()
        .filter_map(|package| Some((package.manifest_path.parent()?, package.name.to_string())))
        .collect();
    Ok(members
        .iter()
        .map(|package| {
            let dependencies = package
                .dependencies
                .iter()
                .filter_map(|dependency| by_directory.get(&dependency.path.as_deref()?).cloned())
                .collect();
            (package.name.to_string(), dependencies)
        })
        .collect())
}

/// The selected packages and every package depending on one of them.
pub fn affected(ctx: &Context, selection: &Selection) -> Result<BTreeSet<Key>> {
    let mut by_workspace: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for (workspace, package) in &selection.packages {
        by_workspace
            .entry(workspace)
            .or_default()
            .insert(package.clone());
    }
    let mut affected = BTreeSet::new();
    for (workspace, selected) in by_workspace {
        for package in dependents(&edges(ctx, workspace)?, &selected) {
            affected.insert((workspace.to_owned(), package));
        }
    }
    Ok(affected)
}

/// One step's outcome, printed as it ends.
fn step(name: &str, work: impl FnOnce() -> Result<()>) -> Result<()> {
    let started = Instant::now();
    let result = work();
    println!(
        "gate: {} {name} ({:.1} s)",
        if result.is_ok() { "PASS" } else { "FAIL" },
        started.elapsed().as_secs_f64()
    );
    result.map_err(|error| format!("{name}: {error}").into())
}

/// How much of a change the gate checks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Depth {
    /// Clippy of the affected packages, the tests of the selected ones; the
    /// Markdown check is CI's.
    Fast,
    /// Clippy and the tests of every affected package, and the Markdown check.
    Full,
}

/// The packages whose tests the gate runs at `depth`.
pub fn tested<'a>(
    depth: Depth,
    selection: &Selection,
    affected: &'a BTreeSet<Key>,
) -> BTreeSet<&'a Key> {
    affected
        .iter()
        .filter(|key| depth == Depth::Full || selection.packages.contains(*key))
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

/// The image classes to type-check for chip code in `changed` packages:
/// the final images, and every class whose image's package graph (`graphs`)
/// compiles one of them, in catalog order.
pub fn image_classes(
    changed: &BTreeSet<&str>,
    graphs: &[(oer_hil_image_class::ImageClass, BTreeSet<String>)],
) -> Vec<oer_hil_image_class::ImageClass> {
    graphs
        .iter()
        .filter(|(class, packages)| {
            FINAL_IMAGES.contains(class)
                || packages
                    .iter()
                    .any(|package| changed.contains(package.as_str()))
        })
        .map(|(class, _)| *class)
        .collect()
}

/// Runs the gate for `selection` at `depth`, with Clippy of the packages of
/// `affected` the host builds and the tests [`tested`] names.
pub fn run(
    ctx: &Context,
    tree: &Tree,
    selection: &Selection,
    affected: &BTreeSet<Key>,
    depth: Depth,
) -> Result<()> {
    step("tidy", || crate::checks::tidy::run(ctx))?;
    for workspace in &selection.format {
        // Only the selected packages: formatting the whole root workspace
        // takes 15 s, its changed packages a fraction of that.
        let packages: Vec<&str> = selection
            .packages
            .iter()
            .filter(|(owner, _)| owner == workspace)
            .map(|(_, name)| name.as_str())
            .collect();
        step(&format!("fmt {workspace}"), || {
            let mut command = ctx.cargo();
            command
                .args(["fmt", "--manifest-path"])
                .arg(ctx.root.join(workspace));
            for package in &packages {
                command.args(["-p", package]);
            }
            process::run(command.args(["--", "--check"]))
        })?;
    }
    if !selection.locks.is_empty() {
        let manifests: Vec<PathBuf> = selection
            .locks
            .iter()
            .map(|workspace| ctx.root.join(workspace))
            .collect();
        step("lock --check", || {
            crate::checks::metadata::check_locks(ctx, &manifests)
        })?;
    }
    if selection.docs {
        match depth {
            Depth::Full => step("check docs", || crate::checks::docs::run(ctx))?,
            Depth::Fast => println!("gate: check docs left to CI"),
        }
    }
    if let Some(anchored) = &selection.capabilities {
        let files: Vec<PathBuf> = anchored.iter().map(PathBuf::from).collect();
        step("check capabilities", || {
            crate::checks::docs::capabilities(ctx, &files)
        })?;
    }
    let host: BTreeSet<&Key> = affected
        .iter()
        .filter(|(workspace, name)| {
            tree.packages
                .iter()
                .any(|p| &p.workspace == workspace && &p.name == name && p.host)
        })
        .collect();
    let chip = chip_code(tree, affected);
    if !chip.is_empty() {
        // Chip-target code is invisible to the host checks below:
        // type-check the final images and every class whose image compiles
        // a package with that code, so an interface change it still uses
        // fails here. Full builds and the examples remain CI's.
        let changed: BTreeSet<&str> = chip.iter().map(|(_, name)| name.as_str()).collect();
        let mut graphs = Vec::new();
        for class in oer_hil_image_class::ImageClass::ALL {
            graphs.push((class, oer_hil_image::packages(&ctx.root, class)?));
        }
        let classes = image_classes(&changed, &graphs);
        println!(
            "gate: {} package(s) with chip code; type-checking {} image class(es)",
            chip.len(),
            classes.len()
        );
        step("type-check affected images", || {
            crate::checks::firmware::run(
                ctx,
                &classes,
                crate::checks::firmware::Depth::TypeCheck,
                crate::checks::firmware::default_jobs(),
            )
        })?;
    }
    let tested = tested(depth, selection, affected);
    let mut by_workspace: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (workspace, name) in &host {
        by_workspace.entry(workspace).or_default().push(name);
    }
    let label = |packages: &[&str]| {
        if packages.len() <= 4 {
            packages.join(", ")
        } else {
            format!("{} packages", packages.len())
        }
    };
    for (workspace, packages) in by_workspace {
        let manifest = ctx.root.join(workspace);
        step(&format!("clippy {}", label(&packages)), || {
            let mut command = ctx.cargo();
            command
                .args(["clippy", "--locked", "--all-targets", "--manifest-path"])
                .arg(&manifest);
            for package in &packages {
                command.args(["-p", package]);
            }
            process::run(command.args(["--", "-D", "warnings"]))
        })?;
        let packages: Vec<&str> = packages
            .into_iter()
            .filter(|name| tested.contains(&(workspace.to_owned(), (*name).to_owned())))
            .collect();
        if packages.is_empty() {
            continue;
        }
        step(&format!("test {}", label(&packages)), || {
            let mut command = ctx.cargo();
            command
                .args(["test", "--locked", "--no-fail-fast", "--manifest-path"])
                .arg(&manifest);
            for package in &packages {
                command.args(["-p", package]);
            }
            process::run_with_timeout(&mut command, TEST_LIMIT)
        })?;
        if workspace == "Cargo.toml" {
            let metadata = cargo::metadata_no_deps(ctx, &manifest)?;
            for package in metadata
                .packages
                .iter()
                .filter(|package| packages.contains(&package.name.as_str()))
            {
                if crate::checks::common::test_feature_sets(package)?.is_empty() {
                    continue;
                }
                step(&format!("feature sets {}", package.name), || {
                    crate::checks::feature_sets::test(ctx, package)
                })?;
            }
        }
    }
    Ok(())
}

/// The merge base of `HEAD` and `base`.
pub fn merge_base(ctx: &Context, base: &str) -> Result<String> {
    Ok(git(ctx, &["merge-base", "HEAD", base])?.trim().to_owned())
}

/// The files `HEAD` changed against `merge_base`.
pub fn committed(ctx: &Context, merge_base: &str) -> Result<Vec<String>> {
    lines(ctx, &["diff", "--name-only", merge_base, "HEAD"])
}

/// The files of the working tree that differ from `HEAD`: modified, staged
/// and untracked but not ignored.
pub fn uncommitted(ctx: &Context) -> Result<Vec<String>> {
    let mut files: BTreeSet<String> = lines(ctx, &["diff", "--name-only", "HEAD"])?
        .into_iter()
        .collect();
    files.extend(lines(ctx, &["ls-files", "--others", "--exclude-standard"])?);
    Ok(files.into_iter().collect())
}

pub fn git(ctx: &Context, arguments: &[&str]) -> Result<String> {
    Ok(String::from_utf8(
        process::capture(ctx.command("git").args(arguments))?.stdout,
    )?)
}

fn lines(ctx: &Context, arguments: &[&str]) -> Result<Vec<String>> {
    Ok(git(ctx, arguments)?
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

#[cfg(test)]
mod tests;
