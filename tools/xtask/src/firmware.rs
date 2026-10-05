//! `cargo xtask build firmware`: the standalone examples' images, built by
//! the image pipeline into a retained bundle each.

use crate::Result;
use oer_process::Checkout;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// The examples' workspace, relative to the checkout.
pub const EXAMPLES: &str = "examples/esp32s31";

/// Every bootable example of [`EXAMPLES`], by directory.
pub const NAMES: [&str; 4] = ["access-point", "monitor", "station", "thread"];

/// The image spec of `example` with `features`, building into `output`
/// with the example's shared compile cache.
fn spec(
    ctx: &Checkout,
    example: &str,
    (features, no_default_features): (&[String], bool),
    output: PathBuf,
) -> Result<oer_image::ImageSpec> {
    let manifest = ctx.root.join(EXAMPLES).join(example).join("Cargo.toml");
    let data: toml::Table = toml::from_str(&fs::read_to_string(&manifest)?)?;
    let package = data
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(toml::Value::as_str)
        .ok_or("example has no package name")?
        .to_owned();
    let chip = oer_image::staged::CHIP;
    Ok(oer_image::ImageSpec {
        root: ctx.root.clone(),
        chip: chip.to_owned(),
        application: oer_image::Application {
            workspace: PathBuf::from(EXAMPLES),
            binary: package.clone(),
            package,
            features: features.to_vec(),
            default_features: !no_default_features,
        },
        stack_policy: Path::new("platform").join(chip).join("stack.toml"),
        interrupts: oer_image::Required::Proven,
        layout_seed: None,
        overrides: oer_image::Overrides::default(),
        builders: Vec::new(),
        reads: Vec::new(),
        output,
        cache: cache(ctx, example),
        audit: None,
    })
}

/// The directory of `example`'s compile cache and bundles.
fn directory(ctx: &Checkout, example: &str) -> PathBuf {
    ctx.root
        .join("target/firmware")
        .join(format!("{}-{example}", oer_image::staged::CHIP))
}

/// The compile cache of `example`'s builds.
pub fn cache(ctx: &Checkout, example: &str) -> PathBuf {
    directory(ctx, example).join("cargo")
}

/// Type-check an example's runtime exactly as its image build compiles it
/// (target, features and image compiler flags), without code generation.
pub fn type_check(
    ctx: &Checkout,
    example: &str,
    features: &[String],
    no_default_features: bool,
) -> Result<()> {
    let spec = spec(
        ctx,
        example,
        (features, no_default_features),
        directory(ctx, example).join("check"),
    )?;
    oer_image::type_check(&spec)?;
    println!("{example}: the runtime type-checks with the image flags");
    Ok(())
}

/// Build `example` into a new bundle below its directory and keep it: a
/// later build never replaces the files of an earlier one.
pub fn build(
    ctx: &Checkout,
    example: &str,
    features: &[String],
    no_default_features: bool,
) -> Result<oer_image::ImageBundle> {
    let directory = directory(ctx, example);
    fs::create_dir_all(&directory)?;
    let output = tempfile::Builder::new()
        .prefix("build-")
        .tempdir_in(&directory)?
        .keep();
    let bundle = oer_image::build(&spec(
        ctx,
        example,
        (features, no_default_features),
        output,
    )?)?;
    for warning in &bundle.warnings {
        println!("warning: {warning}");
    }
    println!("image bundle: {}", bundle.directory.display());
    println!("application image: {}", bundle.application().display());
    Ok(bundle)
}
