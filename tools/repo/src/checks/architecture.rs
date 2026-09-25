use crate::{Context, Result, cargo, graph::Graph, process};

use super::{TARGET, common::*};

mod facade;

const INTEGRATION: &str = "crates/composition/esp32s31/embassy/ieee80211/Cargo.toml";
const INTEGRATION_PACKAGE: &str = "oer-esp32s31-embassy-wifi";
const HIL_RUNTIME: &str = "hil/targets/esp32s31/runtime/Cargo.toml";

pub fn run(ctx: &Context) -> Result<()> {
    let packages = production_packages(ctx)?;
    validate_production_edges(&packages)?;
    let configurations = architecture_configurations(&packages, TARGET)?;
    // Clippy compiles each isolated profile and applies every crate's own
    // lint policy from its manifest `[lints]` and crate attributes.
    for configuration in &configurations {
        let mut command = ctx.cargo();
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
    process::run(ctx.cargo().args([
        "clippy",
        "--quiet",
        "--locked",
        "--offline",
        "-p",
        "oer-esp32s31-bluetooth",
        "--features",
        "validation-probes",
    ]))?;
    facade::check(ctx)?;
    let graph = cargo::metadata(ctx, &ctx.root.join("Cargo.toml"), &[], Some(TARGET), true)?;
    for name in [
        "oer-wifi-ap",
        "oer-wifi-sta",
        "oer-wifi-softmac",
        "oer-esp32s31-wifi-ap",
        "oer-esp32s31-wifi-sta",
    ] {
        for package in closure(&graph, &id_for_name(&graph, name)?)? {
            if package.name.as_str() == "esp-hal" || package.name.as_str().starts_with("embassy-") {
                return Err(format!(
                    "policy layer {name} depends on platform runtime {}",
                    package.name
                )
                .into());
            }
        }
    }
    check_esp32s31_composition(ctx)?;
    process::run(ctx.cargo().args([
        "test",
        "--quiet",
        "--locked",
        "--offline",
        "--package",
        "oer-esp32s31-wifi-embassy",
    ]))?;
    process::run(
        ctx.cargo()
            .args([
                "test",
                "--quiet",
                "--locked",
                "--offline",
                "--manifest-path",
            ])
            .arg(ctx.root.join(INTEGRATION))
            .arg("--no-default-features"),
    )?;
    eprintln!(
        "driver architecture audit passed ({} production packages)",
        packages.len()
    );
    Ok(())
}

fn check_esp32s31_composition(ctx: &Context) -> Result<()> {
    let manifest = ctx.root.join(INTEGRATION);
    let direct = cargo::metadata_no_deps(ctx, &manifest)?;
    let package = package_for_manifest(&direct, &manifest)?;
    for required in [
        "oer-esp32s31-hal",
        "oer-esp32s31-phy",
        "oer-esp32s31-wifi",
        "oer-esp32s31-wifi-mac",
        "oer-esp32s31-wifi-ap",
        "oer-esp32s31-wifi-sta",
    ] {
        if !package.dependencies.iter().any(|d| d.name == required) {
            return Err(format!("integration lacks required direct dependency {required}").into());
        }
    }
    for features in [
        vec!["--no-default-features".into()],
        vec![
            "--no-default-features".into(),
            "--features".into(),
            "diagnostics".into(),
        ],
    ] {
        let graph = cargo::metadata(ctx, &manifest, &features, Some(TARGET), true)?;
        forbid_features(&graph, &["cooperative-scheduler-telemetry"])?;
    }
    for (overlay, expected) in [(None, false), (Some("driver-observation"), true)] {
        let graph = hil_wifi_graph(ctx, overlay)?;
        forbid_features(
            &graph,
            &[
                "cooperative-scheduler-telemetry",
                "network-scheduler-observation",
                "task-poll-telemetry",
                "mac-irq-diagnostics",
            ],
        )?;
        if package_feature(&graph, INTEGRATION_PACKAGE, "diagnostics")? != expected {
            return Err(format!(
                "incorrect integration diagnostics selection for HIL overlay {overlay:?}"
            )
            .into());
        }
        if package_feature(&graph, "oer-esp32s31-phy", "registration-diagnostics")? != expected {
            return Err(format!(
                "incorrect PHY registration diagnostics selection for HIL overlay {overlay:?}"
            )
            .into());
        }
        package_for_manifest(&graph.metadata, &ctx.root.join(HIL_RUNTIME))?;
    }
    for (overlay, feature, expected) in [
        ("task-residence-telemetry", "task-poll-telemetry", false),
        ("core0-rx-cycle-telemetry", "task-poll-telemetry", true),
        ("mac-irq-telemetry", "mac-irq-diagnostics", true),
    ] {
        if package_feature(
            &hil_wifi_graph(ctx, Some(overlay))?,
            INTEGRATION_PACKAGE,
            feature,
        )? != expected
        {
            return Err(format!("incorrect {feature} selection for HIL {overlay}").into());
        }
    }
    Ok(())
}

fn hil_wifi_graph(ctx: &Context, overlay: Option<&str>) -> Result<crate::graph::Graph> {
    let manifest = ctx.root.join(HIL_RUNTIME);
    let direct = cargo::metadata_no_deps(ctx, &manifest)?;
    let package = package_for_manifest(&direct, &manifest)?;
    let profiles = declared_profiles(package)?;
    let base = hil_wifi_profile(&profiles)?;
    let features = overlay.map_or_else(|| base.to_owned(), |overlay| format!("{base},{overlay}"));
    cargo::metadata(
        ctx,
        &manifest,
        &[
            "--no-default-features".into(),
            "--features".into(),
            features,
        ],
        Some(TARGET),
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
pub fn reject_wifi_in_bluetooth(graph: &Graph, manifest: &std::path::Path) -> Result<()> {
    let root = graph.root(manifest)?;
    for id in graph.reachable(&root).keys() {
        let name = graph.package(id).name.as_str();
        if name.starts_with("oer-wifi")
            || name.starts_with("oer-ieee80211")
            || name.starts_with("oer-esp32s31-wifi")
            || name == "oer-esp32s31-embassy-wifi"
            || name.starts_with("embassy-net")
        {
            return Err(format!("Bluetooth consumer depends on Wi-Fi package {name}").into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::hil_wifi_profile;

    #[test]
    fn wifi_overlays_select_the_explicit_profile_independently_of_order() {
        let profiles = vec![
            "bluetooth-secure-gatt,code-psram".into(),
            "open-radio-hil,upstream-network,code-psram".into(),
            "bluetooth-gatt,code-psram".into(),
        ];
        assert_eq!(hil_wifi_profile(&profiles).unwrap(), profiles[1]);
        assert!(hil_wifi_profile(&profiles[..1]).is_err());
        assert!(hil_wifi_profile(&["not-open-radio-hil".into()]).is_err());
        let mut ambiguous = profiles;
        ambiguous.push("open-radio-hil,another-profile".into());
        assert!(hil_wifi_profile(&ambiguous).is_err());
    }
}
