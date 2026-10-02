//! Tests of each package under the feature sets it declares.
//!
//! A package whose tests exercise code behind optional features declares the
//! combinations in `open-radio.test-feature-sets`; `check changed` tests a
//! changed package with each and CI tests every package with each, so the
//! list lives next to the package rather than in a workflow.

use crate::{Context, Result, cargo};
use oer_process as process;

/// Test `package` with each of its declared test feature sets.
pub fn test(ctx: &Context, package: &cargo_metadata::Package) -> Result<()> {
    for features in super::common::test_feature_sets(package)? {
        println!("feature sets: testing {} with {features}", package.name);
        process::run(ctx.cargo().args([
            "test",
            "--locked",
            "--no-fail-fast",
            "-p",
            package.name.as_str(),
            "--features",
            &features,
        ]))?;
    }
    Ok(())
}

/// Test every package of the root workspace with its declared feature sets.
pub fn run(ctx: &Context) -> Result<()> {
    let metadata = cargo::metadata_no_deps(ctx, &ctx.root.join("Cargo.toml"))?;
    let members = metadata.workspace_members.clone();
    for package in metadata
        .packages
        .iter()
        .filter(|package| members.contains(&package.id))
    {
        test(ctx, package)?;
    }
    Ok(())
}
