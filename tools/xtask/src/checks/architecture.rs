use crate::{Result, cargo, graph::Graph};
use oer_process as process;
use oer_process::Checkout;
use oer_repo::Layer;

use super::common::*;

mod facade;
mod interrupts;
mod sans_io;
mod unsafe_policy;
mod zeroed;

/// Layers whose packages stay free of the platform runtime: whatever they
/// reach for the chip target names no HAL and no Embassy crate but the
/// executor-independent `embassy-sync`. Runtimes, adapters and compositions
/// bind the platform.
const PLATFORM_FREE: &[Layer] = &[
    Layer::Contract,
    Layer::Protocol,
    Layer::Hardware,
    Layer::Role,
    Layer::Service,
];

fn platform_runtime(name: &str) -> bool {
    name == "esp-hal" || (name.starts_with("embassy-") && name != "embassy-sync")
}

/// Chip-target Clippy of every production package's isolated feature
/// profiles and of the validation probes the chip profiles name: what
/// [`run`]'s whole-tree policies leave out, and most of the job's time. Its
/// inputs are those packages with their dependencies, their workspaces'
/// manifests, locks and build configuration, and the chip profiles.
pub fn clippy(ctx: &Checkout) -> Result<()> {
    let repo = oer_repo::Repo::from_git(&ctx.root)?;
    let model = oer_repo::Model::load(&repo)?;
    let chips = oer_repo::chips::Chips::at(&ctx.root)?;
    let packages = production_packages(&ctx.root, &model)?;
    let configurations = architecture_configurations(&ctx.root, &packages)?;
    // Clippy compiles each isolated profile and applies every crate's own
    // lint policy from its manifest `[lints]` and crate attributes.
    for configuration in &configurations {
        let mut command = oer_toolchain::cargo_in(&ctx.root);
        command.args(["clippy", "--quiet"]);
        configuration.apply(&mut command);
        process::run(&mut command)?;
    }
    eprintln!(
        "driver architecture compilation: {} isolated feature profiles",
        configurations.len()
    );
    // `cargo test` compiles validation probes only under cfg(test), which can
    // conceal invalid validation-only imports in the ordinary host library.
    // The target build is part of the `--all-features` profile above.
    for package in chips
        .profiles()
        .iter()
        .flat_map(|profile| &profile.gate.validation_probes)
    {
        process::run(oer_toolchain::cargo_in(&ctx.root).args([
            "clippy",
            "--quiet",
            "--locked",
            "-p",
            package,
            "--features",
            "validation-probes",
        ]))?;
    }
    Ok(())
}

pub fn run(ctx: &Checkout) -> Result<()> {
    // Classification, the package name rule and the layer, platform and
    // role rules of every dependency are `oer-tidy`'s (`cargo tidy check`).
    let repo = oer_repo::Repo::from_git(&ctx.root)?;
    let model = oer_repo::Model::load(&repo)?;
    let chips = oer_repo::chips::Chips::at(&ctx.root)?;
    registers(ctx, &["check", "pac-transactions", "crates/hardware"])?;
    zeroed::check(&repo)?;
    interrupts::check(&model, &chips)?;
    for profile in chips
        .profiles()
        .iter()
        .filter(|profile| profile.gate.shared_words)
    {
        shared_words(ctx, profile)?;
    }
    let packages = production_packages(&ctx.root, &model)?;
    let policy = unsafe_policy::Policy::load(&ctx.root)?;
    unsafe_policy::check(&ctx.root, &policy, &packages)?;
    // Protocol logic is sans-IO; drivers that wait live in services.
    sans_io::check(&packages)?;
    facade::check(ctx, &chips, &policy)?;
    for target in chip_targets(&chips) {
        let graph = cargo::metadata(ctx, &ctx.root.join("Cargo.toml"), &[], Some(&target), true)?;
        for item in &packages {
            let layer = item.class.layer;
            if !PLATFORM_FREE.contains(&layer) {
                continue;
            }
            let name = item.package.name.as_str();
            for package in closure(&graph, &id_for_name(&graph, name)?)? {
                if platform_runtime(package.name.as_str()) {
                    return Err(format!(
                        "{layer} package {name} depends on platform runtime {}",
                        package.name
                    )
                    .into());
                }
            }
        }
    }
    eprintln!(
        "driver architecture audit passed ({} production packages)",
        packages.len()
    );
    Ok(())
}

/// Reject production Wi-Fi packages in a resolved Bluetooth consumer. Name
/// prefixes also catch Wi-Fi packages that no explicit list names yet.
///
/// Closed register PACs are not Wi-Fi software: a chip's closed PAC carries
/// its Wi-Fi MAC registers into every image that uses it, including the
/// shared Wi-Fi MAC register crates, whose `ieee80211` token names their
/// registers.
pub fn reject_wifi_in_bluetooth(
    graph: &Graph,
    manifest: &std::path::Path,
    closed_pacs: &[String],
) -> Result<()> {
    let root = graph.root(manifest)?;
    for id in graph.reachable(&root).keys() {
        let name = graph.package(id).name.as_str();
        // The package naming rule gives every Wi-Fi package the `ieee80211`
        // domain token.
        let wifi = !closed_pacs.iter().any(|pac| pac == name)
            && name
                .strip_prefix("oer-")
                .is_some_and(|tokens| tokens.split('-').any(|token| token == "ieee80211"));
        if wifi || name.starts_with("embassy-net") {
            return Err(format!("Bluetooth consumer depends on Wi-Fi package {name}").into());
        }
    }
    Ok(())
}

/// Run the register tool, `cargo registers ARGS`, in the checkout.
fn registers(ctx: &Checkout, args: &[&str]) -> Result<()> {
    process::run(
        oer_toolchain::cargo_in(&ctx.root)
            .arg("registers")
            .args(args),
    )
}

/// Runs the register tool's shared-word check of `profile`'s chip against
/// the esp-hal and PAC sources its HIL agent workspace resolves.
fn shared_words(ctx: &Checkout, profile: &oer_repo::chips::profile::Profile) -> Result<()> {
    let chip = &profile.id;
    let graph = cargo::metadata(
        ctx,
        &profile.hil_agent_workspace(&ctx.root).join("Cargo.toml"),
        &[],
        None,
        true,
    )?;
    let directory = |name: &str| -> Result<std::path::PathBuf> {
        graph
            .metadata
            .packages
            .iter()
            .find(|package| package.name.as_str() == name)
            .and_then(|package| package.manifest_path.parent())
            .map(|directory| directory.as_std_path().join("src"))
            .ok_or_else(|| format!("the HIL target workspace resolves no {name} package").into())
    };
    let (esp_hal, pac) = (directory("esp-hal")?, directory(chip)?);
    process::run(
        oer_toolchain::cargo_in(&ctx.root)
            .args([
                "registers",
                "check",
                "shared-words",
                "--chip",
                chip,
                "--esp-hal-src",
            ])
            .arg(esp_hal)
            .arg("--pac-src")
            .arg(pac),
    )
}

#[cfg(test)]
mod tests {
    /// A Bluetooth application package depending on `dependency`.
    fn bluetooth_graph(
        dependency: &str,
    ) -> (tempfile::TempDir, std::path::PathBuf, crate::graph::Graph) {
        let repository = tempfile::tempdir().unwrap();
        std::fs::write(
            repository.path().join("Cargo.toml"),
            "[workspace]\nresolver = '3'\nmembers = ['app', 'dependency']\n",
        )
        .unwrap();
        for (directory, name, dependencies) in [
            (
                "app",
                "bluetooth-app",
                format!("{dependency} = {{ path = '../dependency' }}\n"),
            ),
            ("dependency", dependency, String::new()),
        ] {
            let path = repository.path().join(directory);
            std::fs::create_dir_all(path.join("src")).unwrap();
            std::fs::write(
                path.join("Cargo.toml"),
                format!("[package]\nname = '{name}'\nversion = '0.1.0'\nedition = '2024'\n[dependencies]\n{dependencies}"),
            )
            .unwrap();
            std::fs::write(path.join("src/lib.rs"), "").unwrap();
        }
        let context = oer_process::Checkout::new(repository.path()).unwrap();
        let manifest = repository.path().join("app/Cargo.toml");
        let graph = crate::cargo::metadata(&context, &manifest, &[], None, false).unwrap();
        (repository, manifest, graph)
    }

    #[test]
    fn bluetooth_graphs_admit_closed_register_pacs_but_not_wifi_software() {
        let (_repository, manifest, graph) = bluetooth_graph("oer-ieee80211-pac");
        let closed = ["oer-ieee80211-pac".to_owned()];
        super::reject_wifi_in_bluetooth(&graph, &manifest, &closed).unwrap();
        let (_repository, manifest, graph) = bluetooth_graph("oer-ieee80211-sta");
        let error = super::reject_wifi_in_bluetooth(&graph, &manifest, &closed).unwrap_err();
        assert!(error.to_string().contains("oer-ieee80211-sta"), "{error}");
    }
}
