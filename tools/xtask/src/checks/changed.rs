//! Pre-push checks for the files a checkout changed against its base.
//!
//! Several sessions push to `main` directly, so the full CI run is the
//! checkpoint only after the push. This check runs the subset that catches the
//! usual breakage first: formatting of every workspace a changed file belongs
//! to, Clippy and API documentation of the root workspace, the tests of the
//! changed root packages, the Markdown/catalog check when prose changed, and
//! the metadata check when a manifest or lockfile changed. It prints any CI
//! workflow whose newest run on `main` failed. It is not full
//! repository coverage; the CI jobs remain the source checkpoint.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use crate::{Context, Result, cargo, doc, process};

/// What one set of changed files requires.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct Plan {
    /// Workspace manifests whose formatting must be checked.
    pub format: BTreeSet<PathBuf>,
    /// Changed root-workspace packages: tested and documented.
    pub packages: BTreeSet<String>,
    /// A root-workspace source changed: run Clippy over the whole workspace,
    /// because dependents of a changed package can break too.
    pub clippy: bool,
    /// Markdown, qualification catalogs or programs changed.
    pub docs: bool,
    /// A changed file other than prose could alter a HIL firmware image or
    /// its audits; the firmware check selects the classes it reaches.
    pub firmware: bool,
    /// Shared platform boot, runtime entry or linker placement changed:
    /// link one standalone example, since a type-check never runs the
    /// linker scripts' assertions.
    pub platform_link: bool,
    /// A manifest, lockfile or network owner changed: the network adapter
    /// dependency boundaries may have moved.
    pub network: bool,
    /// A register model, its policy or the register tool changed: the
    /// published SVD, bindings and PAC must still be what the model yields.
    pub registers: bool,
    /// A Cargo manifest or lockfile changed.
    pub metadata: bool,
    /// Code, register models, vendor docs or provenance facts changed: vendor
    /// citations may have changed.
    pub provenance: bool,
    /// A production crate manifest changed: the source-only PHY graph may
    /// have gained a package.
    pub phy_graph: bool,
    /// Rust sources or capability catalogs changed: a code anchor or the
    /// entry it names may have gone. The changed Rust files, to list the
    /// catalog entries anchored in them.
    pub capabilities: Option<BTreeSet<PathBuf>>,
    /// Changed workspaces other than the root, which need their own target
    /// and feature profile to build.
    pub other_workspaces: BTreeSet<PathBuf>,
}

/// One package of the root workspace: its name and directory.
#[derive(Clone, Debug)]
pub struct Member {
    pub name: String,
    pub directory: PathBuf,
}

/// Map changed paths (relative to the root) to the checks they require.
/// `workspace_of` returns the workspace manifest owning a changed Rust or
/// Cargo file, or `None` when no Cargo package owns it.
pub fn plan(
    root: &Path,
    changed: &[PathBuf],
    members: &[Member],
    workspace_of: &mut dyn FnMut(&Path) -> Result<Option<PathBuf>>,
) -> Result<Plan> {
    let root_manifest = root.join("Cargo.toml");
    let mut plan = Plan::default();
    for path in changed {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if extension == "md" || path.starts_with("qualification") {
            plan.docs = true;
        }
        // The evaluator's tests read the catalogs, programs and reviews, so
        // a change to any of them tests the evaluator.
        if path.starts_with("qualification") && !path.starts_with("qualification/evaluator") {
            plan.packages.insert(String::from("oer-qualification"));
        }
        // The evaluator reads the evidence the runner writes, an edge Cargo's
        // dependency graph does not show, so an evaluator change tests the
        // evidence writer and the runner too.
        if path.starts_with("qualification/evaluator") {
            plan.packages.insert(String::from("oer-hil-evidence"));
            plan.packages.insert(String::from("oer-hil-runner"));
        }
        // The evaluator resolves image source graphs with `cargo tree` over
        // the HIL runtime's features, so a runtime manifest change can break
        // its tests without a Cargo edge from the evaluator.
        if path.starts_with("hil/targets") && name == "Cargo.toml" {
            plan.packages.insert(String::from("oer-qualification"));
        }
        let image_input = [
            "crates",
            "hil",
            "platform",
            "tools/firmware",
            "tools/memory-report",
            ".cargo",
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain",
        ]
        .iter()
        .any(|prefix| path.starts_with(prefix))
            // Host code under `hil/host` never reaches an image; the image
            // builder, the image classes and the source snapshot do.
            && (!path.starts_with("hil/host")
                || IMAGE_BUILDER.iter().any(|package| path.starts_with(package)));
        if image_input && path.extension().is_none_or(|extension| extension != "md") {
            plan.firmware = true;
        }
        if path.starts_with("platform") {
            plan.platform_link = true;
        }
        if path.starts_with("registers") || path.starts_with("tools/registers") {
            plan.registers = true;
        }
        if name == "Cargo.toml"
            || name == "Cargo.lock"
            || [
                "crates/network",
                "crates/adapters/embassy-net",
                "experiments/network-engine",
            ]
            .iter()
            .any(|owner| path.starts_with(owner))
        {
            plan.network = true;
        }
        if name == "Cargo.toml" || name == "Cargo.lock" {
            plan.metadata = true;
        }
        if name == "Cargo.toml" && path.starts_with("crates") {
            plan.phy_graph = true;
        }
        if extension == "rs" || path.starts_with("qualification/catalog") {
            let anchored = plan.capabilities.get_or_insert_default();
            if extension == "rs" {
                anchored.insert(path.clone());
            }
        }
        if (extension == "rs" && (path.starts_with("crates") || path.starts_with("verification")))
            || path.starts_with("registers")
            || path.starts_with("docs/vendor")
            || path.components().any(|part| part.as_os_str() == "facts")
        {
            plan.provenance = true;
        }
        if extension != "rs" && name != "Cargo.toml" && name != "Cargo.lock" {
            continue;
        }
        let Some(workspace) = workspace_of(&root.join(path))? else {
            continue;
        };
        plan.format.insert(workspace.clone());
        if workspace != root_manifest {
            plan.other_workspaces.insert(workspace);
            continue;
        }
        plan.clippy = true;
        let absolute = root.join(path);
        if let Some(member) = members
            .iter()
            .filter(|member| absolute.starts_with(&member.directory))
            .max_by_key(|member| member.directory.components().count())
        {
            plan.packages.insert(member.name.clone());
        }
    }
    Ok(plan)
}

/// The host packages that decide what an image build produces.
const IMAGE_BUILDER: [&str; 3] = [
    "hil/host/image",
    "hil/host/image-class",
    "hil/host/source-snapshot",
];

fn git(ctx: &Context, arguments: &[&str]) -> Result<Vec<PathBuf>> {
    let output = process::capture(ctx.command("git").args(arguments))?;
    Ok(String::from_utf8(output.stdout)?
        .lines()
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect())
}

/// Committed, staged, unstaged and untracked changes against the merge base
/// of `HEAD` and `base`. Deleted files are kept: their workspace still needs
/// its checks.
fn changed_files(ctx: &Context, base: &str) -> Result<Vec<PathBuf>> {
    let merge_base = String::from_utf8(
        process::capture(ctx.command("git").args(["merge-base", "HEAD", base]))?.stdout,
    )?;
    let merge_base = merge_base.trim();
    let mut files = BTreeSet::new();
    files.extend(git(ctx, &["diff", "--name-only", merge_base])?);
    files.extend(git(ctx, &["ls-files", "--others", "--exclude-standard"])?);
    Ok(files.into_iter().collect())
}

/// The workspace manifest owning `path`: the nearest `Cargo.toml` above it,
/// resolved by Cargo. Paths under an ignored build directory or without a
/// package have none.
fn workspace_of(
    ctx: &Context,
    cache: &mut BTreeMap<PathBuf, PathBuf>,
    path: &Path,
) -> Result<Option<PathBuf>> {
    let mut directory = path.parent();
    while let Some(current) = directory {
        if !current.starts_with(&ctx.root) {
            return Ok(None);
        }
        let manifest = current.join("Cargo.toml");
        if manifest.is_file() {
            if let Some(workspace) = cache.get(&manifest) {
                return Ok(Some(workspace.clone()));
            }
            let workspace = cargo::workspace_manifest(ctx, &manifest)?;
            cache.insert(manifest, workspace.clone());
            return Ok(Some(workspace));
        }
        directory = current.parent();
    }
    Ok(None)
}

pub fn run(ctx: &Context, base: &str) -> Result<()> {
    if let Err(error) = crate::sweep::in_background(&ctx.root) {
        eprintln!("check changed: the automatic cache sweep did not start: {error}");
    }
    crate::sweep::ensure_space(&ctx.root)?;
    // Cargo runs offline here: download what a changed lock file added.
    super::metadata::fetch(ctx)?;
    let changed = changed_files(ctx, base)?;
    let root_manifest = ctx.root.join("Cargo.toml");
    let metadata = cargo::metadata_no_deps(ctx, &root_manifest)?;
    let members: Vec<Member> = metadata
        .packages
        .iter()
        .filter(|package| metadata.workspace_members.contains(&package.id))
        .filter_map(|package| {
            let directory = package
                .manifest_path
                .parent()?
                .as_std_path()
                .canonicalize()
                .ok()?;
            Some(Member {
                name: package.name.to_string(),
                directory,
            })
        })
        .collect();
    let mut cache = BTreeMap::new();
    let plan = plan(&ctx.root, &changed, &members, &mut |path| {
        workspace_of(ctx, &mut cache, path)
    })?;
    println!(
        "check changed: {} files against {base}; packages: {}",
        changed.len(),
        if plan.packages.is_empty() {
            "none".to_owned()
        } else {
            plan.packages.iter().cloned().collect::<Vec<_>>().join(", ")
        }
    );
    for manifest in &plan.format {
        process::run(
            ctx.cargo()
                .args(["fmt", "--all", "--manifest-path"])
                .arg(manifest)
                .args(["--", "--check"]),
        )?;
    }
    if plan.metadata {
        super::metadata::run(ctx).map(|_| ())?;
    }
    // Checks that take seconds run before Clippy, the tests and the
    // firmware, so a stale citation or document fails the push at once.
    if plan.docs {
        super::docs::run(ctx)?;
    }
    if let Some(anchored) = &plan.capabilities {
        super::docs::capabilities(ctx, &anchored.iter().cloned().collect::<Vec<_>>())?;
    }
    if plan.provenance {
        for chip in crate::chips::supported(&ctx.root)? {
            if !ctx
                .root
                .join("verification")
                .join(&chip)
                .join("artifacts.toml")
                .is_file()
            {
                continue;
            }
            // Citations are checked against the pinned artifacts, never
            // skipped: fetch what this checkout lacks into the shared store
            // (a no-op once any checkout of the host fetched them).
            let unfetched = crate::vendor_fetch::unfetched(ctx, &chip)?;
            if !unfetched.is_empty() {
                println!(
                    "check changed: fetching {} pinned vendor artifacts of {chip} for provenance",
                    unfetched.len()
                );
                crate::vendor_fetch::fetch_vendor_sources(ctx, &chip)?;
            }
            crate::vendor_provenance::check(ctx, &chip)?;
        }
    }
    if plan.clippy {
        process::run(ctx.cargo().args([
            "clippy",
            "--workspace",
            "--all-targets",
            "--locked",
            "--offline",
            "--",
            "-D",
            "warnings",
        ]))?;
    }
    if !plan.packages.is_empty() {
        let mut test = ctx.cargo();
        test.args(["test", "--locked", "--offline", "--no-fail-fast"]);
        for package in &plan.packages {
            test.args(["-p", package]);
        }
        process::run(&mut test)?;
        for package in metadata
            .packages
            .iter()
            .filter(|package| plan.packages.contains(package.name.as_str()))
        {
            super::feature_sets::test(ctx, package)?;
        }
        for mut group in doc::groups(&root_manifest, &metadata)? {
            group
                .packages
                .retain(|package| plan.packages.contains(package));
            group.features.retain(|feature| {
                feature
                    .split_once('/')
                    .is_some_and(|(package, _)| plan.packages.contains(package))
            });
            if !group.packages.is_empty() {
                process::run(doc::command(ctx, "doc", &group).arg("--no-deps"))?;
            }
        }
    }
    if plan.firmware {
        // Type-check the images the change can alter. CI builds and audits
        // the images (stack, placement) after the push: see `ci_status`.
        if changed.iter().any(|path| path == Path::new("Cargo.lock")) {
            oer_hil_image::ensure_vendor_dependencies_absent(&ctx.root)?;
        }
        let affected = super::firmware::affected(&changed)?;
        if !affected.is_empty() {
            println!(
                "check changed: type-checking {} HIL image classes the change reaches",
                affected.len()
            );
            super::firmware::run(
                ctx,
                &affected,
                super::firmware::Depth::TypeCheck,
                super::firmware::default_jobs(),
            )?;
        }
    }
    if plan.phy_graph {
        super::phy::run(ctx, "esp32s31")?;
    }
    if plan.network {
        super::network::run(ctx)?;
    }
    if plan.registers {
        for entry in std::fs::read_dir(ctx.root.join("registers"))? {
            let manifest = entry?.path().join("publication/registers.toml");
            if !manifest.is_file() {
                continue;
            }
            for arguments in [&["validate"][..], &["generate", "--check"]] {
                process::run(
                    ctx.cargo()
                        .arg("registers")
                        .args(arguments)
                        .arg("--manifest")
                        .arg(&manifest),
                )?;
            }
        }
    }
    if plan.platform_link {
        println!("check changed: linking the station example for the platform change");
        crate::firmware::build(ctx, "station", &[], false, None)?;
    }
    for workspace in &plan.other_workspaces {
        println!(
            "check changed: formatted {}; build it with its own target and feature profile",
            workspace.display()
        );
    }
    for failure in crate::ci_status::report(ctx) {
        println!("check changed: {failure}");
    }
    println!("check changed passed; the CI jobs remain the full checkpoint");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn members() -> Vec<Member> {
        vec![
            Member {
                name: "hal".into(),
                directory: "/r/crates/hal".into(),
            },
            Member {
                name: "hal-nested".into(),
                directory: "/r/crates/hal/nested".into(),
            },
        ]
    }

    fn run(changed: &[&str]) -> Plan {
        let changed: Vec<PathBuf> = changed.iter().map(PathBuf::from).collect();
        plan(Path::new("/r"), &changed, &members(), &mut |path| {
            Ok(if path.starts_with("/r/hil/targets") {
                Some("/r/hil/targets/Cargo.toml".into())
            } else if path.starts_with("/r/crates") {
                Some("/r/Cargo.toml".into())
            } else {
                None
            })
        })
        .unwrap()
    }

    #[test]
    fn a_root_source_selects_its_innermost_package_and_workspace_clippy() {
        let plan = run(&["crates/hal/nested/src/lib.rs"]);
        assert_eq!(plan.packages, BTreeSet::from(["hal-nested".to_owned()]));
        assert!(plan.clippy);
        assert_eq!(
            plan.format,
            BTreeSet::from([PathBuf::from("/r/Cargo.toml")])
        );
        assert!(!plan.docs && !plan.metadata);
    }

    #[test]
    fn rust_sources_and_catalogs_check_capability_anchors() {
        let plan = run(&["crates/hal/nested/src/lib.rs", "docs/guide.md"]);
        assert_eq!(
            plan.capabilities,
            Some(BTreeSet::from([PathBuf::from(
                "crates/hal/nested/src/lib.rs"
            )]))
        );
        let plan = run(&["qualification/catalog/esp32s31/coex.toml"]);
        assert_eq!(plan.capabilities, Some(BTreeSet::new()));
        assert!(run(&["platform/esp32s31/linker/runtime/sections.x"]).platform_link);
        assert!(run(&["crates/adapters/embassy-net/owned/Cargo.toml"]).network);
        assert!(run(&["crates/network/interface/src/lib.rs"]).network);
        assert!(!run(&["docs/guide.md"]).network);
        assert!(run(&["registers/esp32s31/model/wifi.toml"]).registers);
        assert!(run(&["tools/registers/model/src/lib.rs"]).registers);
        assert!(!run(&["docs/guide.md"]).registers);
        assert!(!run(&["docs/guide.md"]).platform_link);
        assert!(
            run(&["qualification/evaluator/src/main.rs"])
                .packages
                .contains("oer-hil-evidence")
        );
        assert!(
            run(&["hil/targets/esp32s31/runtime/Cargo.toml"])
                .packages
                .contains("oer-qualification")
        );
        assert_eq!(run(&["docs/guide.md"]).capabilities, None);
    }

    #[test]
    fn another_workspace_is_formatted_but_not_built_as_root() {
        let plan = run(&["hil/targets/runtime/src/main.rs"]);
        assert!(plan.packages.is_empty());
        assert!(!plan.clippy);
        assert_eq!(
            plan.other_workspaces,
            BTreeSet::from([PathBuf::from("/r/hil/targets/Cargo.toml")])
        );
    }

    #[test]
    fn source_changes_select_the_firmware_check_and_prose_does_not() {
        for path in [
            "hil/targets/esp32s31/runtime/src/console.rs",
            "hil/protocol/src/system.rs",
            "crates/hardware/esp32s31/driver/ieee80211/mac/src/lib.rs",
        ] {
            assert!(run(&[path]).firmware, "{path}");
        }
        assert!(!run(&["docs/guide.md"]).firmware);
        assert!(!run(&["hil/host/runner/src/main.rs"]).firmware);
        for builder in [
            "hil/host/image/src/lib.rs",
            "hil/host/image-class/src/lib.rs",
            "hil/host/source-snapshot/src/lib.rs",
        ] {
            assert!(run(&[builder]).firmware, "{builder}");
        }
        assert!(!run(&["hil/host/stand/src/lib.rs"]).firmware);
    }

    #[test]
    fn a_catalog_change_tests_the_evaluator() {
        let plan = run(&["qualification/catalog/esp32s31/bluetooth-products.toml"]);
        assert!(plan.docs);
        assert!(plan.packages.contains("oer-qualification"));
    }

    #[test]
    fn prose_and_manifests_select_their_checks_only() {
        let plan = run(&["docs/architecture.md", "crates/hal/Cargo.toml"]);
        assert!(plan.docs);
        assert!(plan.metadata);
        assert!(plan.phy_graph);
        assert_eq!(plan.packages, BTreeSet::from(["hal".to_owned()]));
        let plan = run(&[".github/workflows/ci.yml"]);
        assert_eq!(plan, Plan::default());
    }

    #[test]
    fn citations_in_code_models_or_vendor_docs_select_provenance() {
        assert!(run(&["crates/hal/src/lib.rs"]).provenance);
        assert!(run(&["docs/vendor/esp32s31/wifi.md"]).provenance);
        assert!(run(&["registers/esp32s31/model/wifi.toml"]).provenance);
        assert!(!run(&["docs/architecture.md"]).provenance);
    }
}
