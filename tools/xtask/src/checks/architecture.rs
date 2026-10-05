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
    for profile in chips.profiles() {
        let Some(integration) = &profile.gate.integration else {
            continue;
        };
        check_composition(ctx, profile, integration)?;
        // The test job runs the workspace tests with default features; the
        // composition's tests also hold without its default network.
        process::run(
            oer_toolchain::cargo_in(&ctx.root)
                .args(["test", "--quiet", "--locked", "--manifest-path"])
                .arg(ctx.root.join(&integration.manifest))
                .arg("--no-default-features"),
        )?;
    }
    eprintln!(
        "driver architecture audit passed ({} production packages)",
        packages.len()
    );
    Ok(())
}

/// Hold a chip's Wi-Fi composition and its HIL runtime to their diagnostics
/// selection: no telemetry without its overlay, diagnostics (and the PHY's
/// registration diagnostics) exactly under driver observation.
fn check_composition(
    ctx: &Checkout,
    profile: &oer_repo::chips::profile::Profile,
    integration: &oer_repo::chips::profile::Integration,
) -> Result<()> {
    let manifest = ctx.root.join(&integration.manifest);
    let target = &profile.rust_target;
    for features in [
        vec!["--no-default-features".into()],
        vec![
            "--no-default-features".into(),
            "--features".into(),
            "diagnostics".into(),
        ],
    ] {
        let graph = cargo::metadata(ctx, &manifest, &features, Some(target), true)?;
        forbid_features(&graph, &["cooperative-scheduler-telemetry"])?;
    }
    let runtime = profile.hil_agent_manifest(&ctx.root);
    for (overlay, expected) in [(None, false), (Some("driver-observation"), true)] {
        let graph = hil_wifi_graph(ctx, profile, overlay)?;
        forbid_features(
            &graph,
            &[
                "cooperative-scheduler-telemetry",
                "network-scheduler-observation",
                "task-poll-telemetry",
                "mac-irq-diagnostics",
            ],
        )?;
        if package_feature(&graph, &integration.package, "diagnostics")? != expected {
            return Err(format!(
                "incorrect integration diagnostics selection for HIL overlay {overlay:?}"
            )
            .into());
        }
        if package_feature(&graph, &integration.phy, "registration-diagnostics")? != expected {
            return Err(format!(
                "incorrect PHY registration diagnostics selection for HIL overlay {overlay:?}"
            )
            .into());
        }
        package_for_manifest(&graph.metadata, &runtime)?;
    }
    for (overlay, feature, expected) in [
        ("task-residence-telemetry", "task-poll-telemetry", false),
        ("core0-rx-cycle-telemetry", "task-poll-telemetry", true),
        ("mac-irq-telemetry", "mac-irq-diagnostics", true),
    ] {
        if package_feature(
            &hil_wifi_graph(ctx, profile, Some(overlay))?,
            &integration.package,
            feature,
        )? != expected
        {
            return Err(format!("incorrect {feature} selection for HIL {overlay}").into());
        }
    }
    Ok(())
}

fn hil_wifi_graph(
    ctx: &Checkout,
    profile: &oer_repo::chips::profile::Profile,
    overlay: Option<&str>,
) -> Result<crate::graph::Graph> {
    let manifest = profile.hil_agent_manifest(&ctx.root);
    let relative = manifest
        .strip_prefix(&ctx.root)?
        .to_string_lossy()
        .into_owned();
    let model = model(ctx)?;
    let package = model
        .owner(&relative)
        .ok_or("the HIL runtime manifest has no package")?;
    let profiles = &model.classification(package)?.supported_feature_profiles;
    let base = hil_wifi_profile(profiles)?;
    let features = overlay.map_or_else(|| base.to_owned(), |overlay| format!("{base},{overlay}"));
    cargo::metadata(
        ctx,
        &manifest,
        &[
            "--no-default-features".into(),
            "--features".into(),
            features,
        ],
        Some(&profile.rust_target),
        true,
    )
}

// Wi-Fi overlays apply only to the shared Wi-Fi runtime, not the separately
// compiled Bluetooth applications declared by the same firmware package.
fn hil_wifi_profile(profiles: &[String]) -> Result<&str> {
    let mut matching = profiles.iter().filter(|profile| {
        profile
            .split(',')
            .any(|feature| feature == "open-radio-hil")
    });
    match (matching.next(), matching.next()) {
        (Some(profile), None) => Ok(profile),
        _ => Err("HIL runtime must declare exactly one open-radio-hil feature profile".into()),
    }
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
    use super::hil_wifi_profile;

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

    #[test]
    fn wifi_overlays_select_the_explicit_profile_independently_of_order() {
        let profiles = vec![
            "bluetooth-secure-gatt".into(),
            "open-radio-hil,owned-network".into(),
            "bluetooth-gatt".into(),
        ];
        assert_eq!(hil_wifi_profile(&profiles).unwrap(), profiles[1]);
        assert!(hil_wifi_profile(&profiles[..1]).is_err());
        assert!(hil_wifi_profile(&["not-open-radio-hil".into()]).is_err());
        let mut ambiguous = profiles;
        ambiguous.push("open-radio-hil,another-profile".into());
        assert!(hil_wifi_profile(&ambiguous).is_err());
    }
}
