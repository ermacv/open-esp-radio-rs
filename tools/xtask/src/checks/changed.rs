//! `cargo xtask check changed`: what this checkout changed against its
//! merge base with a base revision, committed or not, checked by the push
//! gate ([`crate::gate`]); with `--full`, also the tests of the dependents,
//! the Markdown check and the heavier checks CI runs on every pull request.

use std::{collections::BTreeSet, path::PathBuf};

use oer_process as process;

use crate::{Context, Result, doc, gate};

pub fn run(ctx: &Context, base: &str, full: bool) -> Result<()> {
    let merge_base = gate::merge_base(ctx, base)?;
    let mut files: BTreeSet<String> = gate::committed(ctx, &merge_base)?.into_iter().collect();
    files.extend(gate::uncommitted(ctx)?);
    let files: Vec<String> = files.into_iter().collect();
    let tree = gate::Tree::load(&ctx.root)?;
    let mut selection = gate::select(&tree, &files);
    gate::select_locks(ctx, &tree, &files, &merge_base, None, &mut selection)?;
    let affected = gate::affected(ctx, &selection)?;
    println!(
        "check changed: {} files against {base}; {} package(s) changed, {} with dependents",
        files.len(),
        selection.packages.len(),
        affected.len()
    );
    let depth = if full {
        gate::Depth::Full
    } else {
        gate::Depth::Fast
    };
    gate::run(ctx, &tree, &selection, &affected, depth)?;
    if full {
        heavy(ctx, &files, &affected)?;
    }
    crate::ci_status::print(ctx, "check changed");
    println!(
        "check changed passed{}",
        if full {
            ""
        } else {
            "; `--full` adds what CI checks on the pull request"
        }
    );
    Ok(())
}

/// What `--full` adds to the gate, as CI runs it after a push: Clippy of the
/// whole root workspace, API documentation of the affected root packages,
/// a type check of the HIL image classes the change reaches, a link of one
/// example for a platform change, the PHY, network and register audits for
/// their inputs, and the vendor provenance of every chip with pinned
/// artifacts.
fn heavy(ctx: &Context, files: &[String], affected: &BTreeSet<gate::Key>) -> Result<()> {
    let touches = |prefixes: &[&str]| {
        files
            .iter()
            .any(|file| prefixes.iter().any(|prefix| file.starts_with(prefix)))
    };
    let manifest = |file: &String| file.ends_with("Cargo.toml") || file.ends_with("Cargo.lock");
    process::run(ctx.cargo().args([
        "clippy",
        "--workspace",
        "--all-targets",
        "--locked",
        "--",
        "-D",
        "warnings",
    ]))?;
    let root_packages: BTreeSet<&str> = affected
        .iter()
        .filter(|(workspace, _)| workspace == "Cargo.toml")
        .map(|(_, name)| name.as_str())
        .collect();
    if !root_packages.is_empty() {
        let root_manifest = ctx.root.join("Cargo.toml");
        let metadata = crate::cargo::metadata_no_deps(ctx, &root_manifest)?;
        for mut group in doc::groups(&root_manifest, &metadata)? {
            group
                .packages
                .retain(|package| root_packages.contains(package.as_str()));
            group.features.retain(|feature| {
                feature
                    .split_once('/')
                    .is_some_and(|(package, _)| root_packages.contains(package))
            });
            if !group.packages.is_empty() {
                process::run(doc::command(ctx, "doc", &group).arg("--no-deps"))?;
            }
        }
    }
    let paths: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
    if files.iter().any(|file| file == "Cargo.lock") {
        oer_hil_image::ensure_vendor_dependencies_absent(&ctx.root)?;
    }
    let classes = super::firmware::affected(&paths)?;
    if !classes.is_empty() {
        super::firmware::run(
            ctx,
            &classes,
            super::firmware::Depth::TypeCheck,
            super::firmware::default_jobs(),
        )?;
    }
    if touches(&["platform/"]) {
        crate::firmware::build(ctx, "station", &[], false)?;
    }
    if files
        .iter()
        .any(|file| file.starts_with("crates/") && file.ends_with("Cargo.toml"))
    {
        super::phy::run(ctx, super::CHIP)?;
    }
    if files.iter().any(manifest) || touches(&["crates/network", "crates/adapters/embassy-net"]) {
        super::network::run(ctx)?;
    }
    if touches(&["registers/", "tools/registers/"]) {
        for chip in crate::chips::supported(&ctx.root)? {
            let manifest = ctx
                .root
                .join("registers")
                .join(&chip)
                .join("publication/registers.toml");
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
    if files.iter().any(|file| {
        (file.ends_with(".rs")
            && (file.starts_with("crates/") || file.starts_with("verification/")))
            || file.starts_with("registers/")
            || file.starts_with("docs/vendor/")
            || file.split('/').any(|part| part == "facts")
    }) {
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
            if !oer_vendor_artifacts::unfetched(&ctx.root, &chip)?.is_empty() {
                oer_vendor_artifacts::fetch_vendor_sources(&ctx.root, &chip)?;
            }
            crate::vendor_provenance::check(ctx, &chip)?;
        }
    }
    Ok(())
}
