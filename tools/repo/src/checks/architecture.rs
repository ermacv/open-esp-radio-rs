use crate::{Context, Result, cargo, process};

use super::{TARGET, common::*};

const INTEGRATION: &str = "driver/integration/esp32s31/embassy/ieee80211/Cargo.toml";
const INTEGRATION_PACKAGE: &str = "open-esp-radio-esp32s31-embassy-wifi";
const HIL_RUNTIME: &str = "hil/targets/esp32s31/runtime/Cargo.toml";
const COMMON_HIL: &str =
    "open-radio-hil,upstream-network,psram-task-stack,code-psram,profile-psram-data";

pub fn run(ctx: &Context) -> Result<()> {
    let packages = driver_packages(ctx)?;
    let driver = ctx.root.join("driver").canonicalize()?;
    let mut profile_count = 0;
    for item in &packages {
        for dependency in production_dependencies(&item.package) {
            if let Some(path) = &dependency.path
                && !path.as_std_path().canonicalize()?.starts_with(&driver)
            {
                return Err(format!(
                    "production package {} declares local dependency outside driver/: {path}",
                    item.package.name
                )
                .into());
            }
        }
        let declared = declared_profiles(&item.package)?;
        let modes = if declared.is_empty() {
            vec![
                vec!["--no-default-features".into()],
                vec![],
                vec!["--all-features".into()],
            ]
        } else {
            declared
                .into_iter()
                .map(|features| {
                    vec![
                        "--no-default-features".into(),
                        "--features".into(),
                        features,
                    ]
                })
                .collect()
        };
        for mode in modes {
            process::run(
                ctx.cargo()
                    .args([
                        "check",
                        "--quiet",
                        "--locked",
                        "--offline",
                        "--manifest-path",
                    ])
                    .arg(&item.manifest)
                    .args(["--package", item.package.name.as_str(), "--target", TARGET])
                    .args(mode),
            )?;
            profile_count += 1;
        }
    }
    eprintln!("driver architecture compilation: {profile_count} isolated feature profiles");
    let graph = cargo::metadata(ctx, &ctx.root.join("Cargo.toml"), &[], Some(TARGET), true)?;
    for name in [
        "open-esp-radio-wifi-ap",
        "open-esp-radio-wifi-sta",
        "open-esp-radio-wifi-softmac",
        "open-esp-radio-esp32s31-wifi-ap",
        "open-esp-radio-esp32s31-wifi-sta",
    ] {
        for package in closure(&graph, &id_for_name(&graph, name)?)? {
            for path in [
                "driver/adapters",
                "driver/runtime",
                "driver/network/adapters",
                "driver/network/research",
                "driver/integration",
                "hil",
            ] {
                if package
                    .manifest_path
                    .as_std_path()
                    .starts_with(ctx.root.join(path))
                {
                    return Err(format!(
                        "policy layer {name} depends on upper layer {}",
                        package.name
                    )
                    .into());
                }
            }
            if package.name.as_str() == "esp-hal" || package.name.as_str().starts_with("embassy-") {
                return Err(format!(
                    "policy layer {name} depends on platform runtime {}",
                    package.name
                )
                .into());
            }
        }
    }
    let radio_manifest = ctx.root.join("driver/radio/Cargo.toml");
    let radio = cargo::metadata(ctx, &radio_manifest, &[], Some(TARGET), true)?;
    let root = package_for_manifest(&radio.metadata, &radio_manifest)?
        .id
        .clone();
    for package in closure(&radio, &root)? {
        for path in [
            "driver/chips/esp32s31",
            "driver/adapters/esp-hal",
            "driver/runtime/embassy/esp32s31",
            "driver/integration/esp32s31",
        ] {
            if package
                .manifest_path
                .as_std_path()
                .starts_with(ctx.root.join(path))
            {
                return Err(format!(
                    "generic radio facade depends on concrete platform {}",
                    package.name
                )
                .into());
            }
        }
    }
    check_composition(ctx)?;
    process::run(ctx.cargo().args([
        "test",
        "--quiet",
        "--locked",
        "--offline",
        "--package",
        "open-esp-radio-esp32s31-wifi-embassy",
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

fn check_composition(ctx: &Context) -> Result<()> {
    let manifest = ctx.root.join(INTEGRATION);
    let direct = cargo::metadata_no_deps(ctx, &manifest)?;
    let package = package_for_manifest(&direct, &manifest)?;
    for required in [
        "open-esp-radio-esp32s31-hal",
        "open-esp-radio-esp32s31-phy",
        "open-esp-radio-esp32s31-wifi",
        "open-esp-radio-esp32s31-wifi-mac",
        "open-esp-radio-esp32s31-wifi-ap",
        "open-esp-radio-esp32s31-wifi-sta",
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
        let graph = hil_graph(ctx, overlay)?;
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
        if package_feature(
            &graph,
            "open-esp-radio-esp32s31-phy",
            "registration-diagnostics",
        )? != expected
        {
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
            &hil_graph(ctx, Some(overlay))?,
            INTEGRATION_PACKAGE,
            feature,
        )? != expected
        {
            return Err(format!("incorrect {feature} selection for HIL {overlay}").into());
        }
    }
    Ok(())
}

fn hil_graph(ctx: &Context, overlay: Option<&str>) -> Result<crate::graph::Graph> {
    let features = overlay.map_or_else(
        || COMMON_HIL.to_owned(),
        |overlay| format!("{COMMON_HIL},{overlay}"),
    );
    cargo::metadata(
        ctx,
        &ctx.root.join(HIL_RUNTIME),
        &[
            "--no-default-features".into(),
            "--features".into(),
            features,
        ],
        Some(TARGET),
        true,
    )
}
