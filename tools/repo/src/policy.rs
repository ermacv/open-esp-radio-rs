//! Which package may depend on which: the one statement of the layer,
//! platform and role rules ([layer dependencies](../../../docs/architecture.md#layer-dependencies)).
//!
//! - A production package's normal and build path dependencies are
//!   production packages of a layer its own may use, never the facade, and
//!   of a compatible platform; a build dependency runs on the host.
//! - Only adapters, compositions and the facade bind an executor
//!   (`embassy-executor`) or the time driver (`embassy-time`,
//!   `oer-time-embassy`); the time-driver rule covers dev dependencies too.
//! - A verdict package never depends on a report package, and an
//!   observation package never on HIL orchestration or stand operation.
//! - A qualification package reads run bundles, scenario catalogs and
//!   shards: its normal and build dependencies never reach HIL orchestration
//!   (runner, lab, HIL images) or stand operation; tests may.
//! - A host package depends only on its own host layer and the layers below
//!   it ([`HostLayer`]): entry, orchestration, execution, verification,
//!   stand, build, foundation.

use crate::{
    Model,
    classification::{Classification, Evidence, Hil, HostLayer, Layer, Platform, Scope},
    manifest::{Dependency, Kind},
};

/// Executor crates. Lower layers expose futures that any executor may poll.
pub const EXECUTORS: &[&str] = &["embassy-executor"];

/// Crates that read or wait on the image's one time driver: the driver
/// interface and its `oer-time` binding. Lower layers, runtimes included,
/// take time through the `oer-time` clock and timer ports; their tests use
/// virtual time, so the rule covers dev dependencies too.
pub const TIME_DRIVERS: &[&str] = &["embassy-time", "oer-time-embassy"];

/// Whether packages of `layer` may bind an executor or the time driver.
pub fn binds_runtime(layer: Layer) -> bool {
    matches!(layer, Layer::Adapter | Layer::Composition | Layer::Facade)
}

/// Responsibilities are not a single stack: an adapter may bind a service or
/// runtime interface to an executor, while a runtime may use an adapter for a
/// lower executor contract. Hardware owns chip resources and wire codecs; a
/// role composes portable role protocols with that hardware. Services declare
/// executor-free ports and never depend on the adapters that bind them. None
/// may acquire the final composition or public facade above them.
pub fn layer_allows(source: Layer, target: Layer) -> bool {
    use Layer::*;
    match source {
        Contract | Protocol => matches!(target, Contract | Protocol),
        Hardware => matches!(target, Contract | Protocol | Hardware),
        Role => matches!(target, Contract | Protocol | Hardware | Role),
        Service => matches!(target, Contract | Protocol | Service),
        Adapter | Runtime => matches!(
            target,
            Contract | Protocol | Hardware | Role | Adapter | Runtime | Service
        ),
        Composition | Facade => target != Facade,
        _ => false,
    }
}

/// Whether code of `source` may use code of `target` on its platform;
/// `family` names a chip's family. A build dependency runs on the host.
pub fn platform_allows(
    source: &Classification,
    target: &Platform,
    kind: Kind,
    family: &dyn Fn(&str) -> Option<String>,
) -> bool {
    if kind == Kind::Build {
        return matches!(target, Platform::Host | Platform::Portable);
    }
    match (&source.platform, target) {
        (_, Platform::Portable) => true,
        (Platform::Chip(source), Platform::Chip(target)) => source == target,
        (Platform::Host, Platform::Host) => true,
        // Family code is shared within its family only.
        (Platform::Family(source), Platform::Family(target)) => source == target,
        (Platform::Chip(chip), Platform::Family(target)) => {
            family(chip).is_some_and(|own| own == *target)
        }
        _ => source.layer == Layer::Facade,
    }
}

/// Whether a package with evidence role `source` may depend on one with
/// role `target`: a verdict never depends on a report, so report code can
/// neither decide a verdict nor enter a shard's sources.
pub fn evidence_edge_allowed(source: Option<Evidence>, target: Option<Evidence>) -> bool {
    !(source == Some(Evidence::Verdict) && target == Some(Evidence::Report))
}

/// Whether a package with HIL role `source` may depend on one with role
/// `target`: observation code reaches neither orchestration nor stand
/// operation, so neither can change what a run observes through it.
pub fn hil_edge_allowed(source: Option<Hil>, target: Option<Hil>) -> bool {
    !(source == Some(Hil::Observation)
        && matches!(target, Some(Hil::Orchestration | Hil::Operation)))
}

/// Whether a package of layer `source` may take a `kind` dependency on one
/// with HIL role `target`: qualification evaluates recorded runs through
/// the formats only, so its normal and build dependencies reach neither HIL
/// orchestration (runner, lab, images) nor stand operation.
pub fn qualification_edge_allowed(source: Layer, kind: Kind, target: Option<Hil>) -> bool {
    !(source == Layer::Qualification
        && kind != Kind::Development
        && matches!(target, Some(Hil::Orchestration | Hil::Operation)))
}

/// Whether a package of host layer `source` may depend on one of host layer
/// `target`: only on its own layer and those below it. A package without a
/// host layer (production code, a portable helper) constrains nothing.
pub fn host_edge_allowed(source: Option<HostLayer>, target: Option<HostLayer>) -> bool {
    match (source, target) {
        (Some(source), Some(target)) => target >= source,
        _ => true,
    }
}

/// Every dependency of the model that breaks a rule, as
/// `<manifest>: <problem>`. Unclassified packages are the classification
/// check's; their edges are skipped.
pub fn check(model: &Model) -> Vec<String> {
    let family = |chip: &str| model.chips.family_of(chip).map(str::to_owned);
    let mut problems = Vec::new();
    for package in model.packages() {
        let Ok(source) = model.classification(package) else {
            continue;
        };
        let mut problem = |message: String| {
            problems.push(format!("{}: {message}", package.manifest));
        };
        for dependency in &package.dependencies {
            let target = model
                .target(dependency)
                .map(|target| (target, model.classification(target)));
            if let Some((target, Ok(class))) = target {
                if !evidence_edge_allowed(source.evidence, class.evidence) {
                    problem(format!(
                        "verdict package {} depends on report package {}",
                        package.name, target.name
                    ));
                }
                if !host_edge_allowed(source.host_layer, class.host_layer) {
                    problem(format!(
                        "{} package {} depends on {} package {}; a host package uses only its own host layer and those below it",
                        source.host_layer.map_or("?", HostLayer::name),
                        package.name,
                        class.host_layer.map_or("?", HostLayer::name),
                        target.name
                    ));
                }
                if !qualification_edge_allowed(source.layer, dependency.kind, class.hil) {
                    problem(format!(
                        "qualification package {} depends on HIL {} package {}; qualification reads recorded runs through the formats only",
                        package.name,
                        match class.hil {
                            Some(Hil::Orchestration) => "orchestration",
                            _ => "stand operation",
                        },
                        target.name
                    ));
                }
                if !hil_edge_allowed(source.hil, class.hil) {
                    problem(format!(
                        "observation package {} depends on {} package {}",
                        package.name,
                        match class.hil {
                            Some(Hil::Orchestration) => "orchestration",
                            _ => "stand operation",
                        },
                        target.name
                    ));
                }
            }
            if source.scope != Scope::Production {
                continue;
            }
            if TIME_DRIVERS.contains(&dependency.package.as_str()) && !binds_runtime(source.layer) {
                problem(format!(
                    "{} package {} depends on time driver {}; only adapters, compositions and the facade bind it, and lower layers take time through oer-time",
                    source.layer, package.name, dependency.package
                ));
            }
            if dependency.kind == Kind::Development {
                continue;
            }
            if EXECUTORS.contains(&dependency.package.as_str()) && !binds_runtime(source.layer) {
                problem(format!(
                    "{} package {} depends on executor {}; only adapters and compositions bind an executor",
                    source.layer, package.name, dependency.package
                ));
            }
            production_edge(model, source, package, dependency, &family, &mut problem);
        }
    }
    problems.sort();
    problems.dedup();
    problems
}

/// The rules of one normal or build path dependency of a production package.
fn production_edge(
    model: &Model,
    source: &Classification,
    package: &crate::Package,
    dependency: &Dependency,
    family: &dyn Fn(&str) -> Option<String>,
    problem: &mut dyn FnMut(String),
) {
    let Some(path) = &dependency.path else {
        return;
    };
    let target = model.package_at(path);
    let Some(class) = target
        .and_then(|target| model.classification(target).ok())
        .filter(|class| class.scope == Scope::Production)
    else {
        problem(format!(
            "production package {} depends on non-production package {}",
            package.name, dependency.package
        ));
        return;
    };
    if class.layer == Layer::Facade {
        problem(format!(
            "internal package {} depends on public facade {}",
            package.name, dependency.package
        ));
    } else if !layer_allows(source.layer, class.layer) {
        problem(format!(
            "forbidden architecture edge {} -> {}: {} depends on {}",
            source.layer, class.layer, package.name, dependency.package
        ));
    }
    if !platform_allows(source, &class.platform, dependency.kind, family) {
        problem(format!(
            "incompatible platform edge {:?} -> {:?}: {} depends on {}",
            source.platform, class.platform, package.name, dependency.package
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Model, files::Repo, testing::tree};

    const CHIP: (&str, &str) = (
        "platform/chip-a/chip.toml",
        "schema = 1\nid = \"chip-a\"\nfamily = \"espressif\"\nrust-target = \"riscv32imafc-unknown-none-elf\"\nboot = \"staged\"\nespflash-chip = \"chip-a\"\nrevisions = [\"rev0\"]\n[properties]\nwifi-bands = [\"2g4\"]\nbluetooth = [\"le\"]\nieee802154 = true\ncores = 2\n",
    );

    fn manifest(name: &str, class: &str, dependencies: &str) -> String {
        format!(
            "[package]\nname = \"{name}\"\n{dependencies}\n[package.metadata.open-radio]\n{class}\n"
        )
    }

    fn problems(files: &[(&str, String)]) -> Vec<String> {
        let mut all: Vec<(&str, &str)> = vec![CHIP];
        all.extend(files.iter().map(|(path, text)| (*path, text.as_str())));
        let dir = tree(&all);
        check(&Model::load(&Repo::from_dir(dir.path()).unwrap()).unwrap())
    }

    fn edge(source: &str, target: &str, section: &str) -> Vec<String> {
        problems(&[
            (
                "s/Cargo.toml",
                manifest(
                    "oer-s",
                    source,
                    &format!("[{section}]\noer-t = {{ path = \"../t\" }}"),
                ),
            ),
            ("t/Cargo.toml", manifest("oer-t", target, "")),
        ])
    }

    const PORTABLE: &str = "platform = \"portable\"";

    #[test]
    fn layers_follow_the_dependency_table() {
        let layer = |layer: &str| format!("layer = \"{layer}\"\n{PORTABLE}");
        assert!(edge(&layer("service"), &layer("protocol"), "dependencies").is_empty());
        assert_eq!(
            edge(&layer("service"), &layer("adapter"), "dependencies"),
            [
                "s/Cargo.toml: forbidden architecture edge service -> adapter: oer-s depends on oer-t"
            ]
        );
        // Optional and build edges count; dev edges may compose anything.
        assert_eq!(
            edge(&layer("protocol"), &layer("hardware"), "build-dependencies").len(),
            1
        );
        assert!(edge(&layer("protocol"), &layer("hardware"), "dev-dependencies").is_empty());
        assert_eq!(
            edge(&layer("composition"), &layer("facade"), "dependencies"),
            ["s/Cargo.toml: internal package oer-s depends on public facade oer-t"]
        );
        assert_eq!(
            edge(
                &layer("role"),
                "layer = \"tool\"\nplatform = \"host\"",
                "dependencies"
            ),
            ["s/Cargo.toml: production package oer-s depends on non-production package oer-t"]
        );
    }

    #[test]
    fn platforms_stay_within_their_chip_and_family() {
        let class = |platform: &str| format!("layer = \"hardware\"\n{platform}");
        let chip = class("platform = \"chip\"\nchip = \"chip-a\"");
        let family = class("platform = \"family\"\nfamily = \"espressif\"");
        let other = class("platform = \"family\"\nfamily = \"other\"");
        assert!(edge(&chip, &family, "dependencies").is_empty());
        assert_eq!(edge(&family, &chip, "dependencies").len(), 1);
        assert_eq!(edge(&chip, &other, "dependencies").len(), 1);
        assert_eq!(edge(&class(PORTABLE), &family, "dependencies").len(), 1);
        // A build script runs on the host.
        let message = edge(&chip, &chip, "build-dependencies");
        assert_eq!(message.len(), 1);
        assert!(
            message[0].contains("incompatible platform edge"),
            "{message:?}"
        );
    }

    #[test]
    fn executors_and_the_time_driver_belong_to_bindings() {
        let service = format!("layer = \"service\"\n{PORTABLE}");
        let found = problems(&[(
            "s/Cargo.toml",
            manifest(
                "oer-s",
                &service,
                "[dependencies]\nexec = { package = \"embassy-executor\", version = \"1\" }\n[dev-dependencies]\nembassy-time = \"1\"",
            ),
        )]);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].contains("depends on executor embassy-executor"));
        assert!(found[1].contains("depends on time driver embassy-time"));
        let adapter = format!("layer = \"adapter\"\n{PORTABLE}");
        assert!(
            problems(&[(
                "s/Cargo.toml",
                manifest(
                    "oer-s",
                    &adapter,
                    "[dependencies]\nembassy-executor = \"1\"\nembassy-time = \"1\""
                ),
            )])
            .is_empty()
        );
    }

    #[test]
    fn role_edges_point_away_from_reports_orchestration_and_operation() {
        let hil = |role: &str| {
            format!(
                "layer = \"hil\"\nplatform = \"host\"\nhil = \"{role}\"\nhost-layer = \"execution\""
            )
        };
        assert_eq!(
            edge(&hil("observation"), &hil("operation"), "dependencies"),
            ["s/Cargo.toml: observation package oer-s depends on stand operation package oer-t"]
        );
        assert_eq!(
            edge(
                &hil("observation"),
                &hil("orchestration"),
                "dev-dependencies"
            ),
            ["s/Cargo.toml: observation package oer-s depends on orchestration package oer-t"]
        );
        assert!(edge(&hil("orchestration"), &hil("operation"), "dependencies").is_empty());
        assert!(edge(&hil("operation"), &hil("observation"), "dependencies").is_empty());
        let evidence = |role: &str| {
            format!(
                "layer = \"verification\"\nplatform = \"host\"\nevidence = \"{role}\"\nhost-layer = \"verification\""
            )
        };
        assert_eq!(
            edge(&evidence("verdict"), &evidence("report"), "dependencies").len(),
            1
        );
        assert!(edge(&evidence("report"), &evidence("verdict"), "dependencies").is_empty());
    }

    #[test]
    fn qualification_never_links_hil_orchestration_or_stand_operation() {
        let qualification =
            "layer = \"qualification\"\nplatform = \"host\"\nhost-layer = \"entry\"";
        let hil = |role: &str| {
            format!(
                "layer = \"hil\"\nplatform = \"host\"\nhil = \"{role}\"\nhost-layer = \"execution\""
            )
        };
        assert_eq!(
            edge(qualification, &hil("orchestration"), "dependencies"),
            [
                "s/Cargo.toml: qualification package oer-s depends on HIL orchestration package oer-t; qualification reads recorded runs through the formats only"
            ]
        );
        assert_eq!(
            edge(qualification, &hil("operation"), "build-dependencies").len(),
            1
        );
        // Tests may drive a runner; formats (observation) are always fine.
        assert!(edge(qualification, &hil("orchestration"), "dev-dependencies").is_empty());
        assert!(edge(qualification, &hil("observation"), "dependencies").is_empty());
    }

    #[test]
    fn host_packages_depend_only_down_their_layers() {
        let host = |layer: &str| {
            format!("layer = \"tool\"\nplatform = \"host\"\nhost-layer = \"{layer}\"")
        };
        assert!(edge(&host("entry"), &host("orchestration"), "dependencies").is_empty());
        assert!(edge(&host("entry"), &host("entry"), "dependencies").is_empty());
        assert!(edge(&host("stand"), &host("build"), "dependencies").is_empty());
        for (source, target) in [
            ("stand", "execution"),
            ("build", "stand"),
            ("verification", "execution"),
            ("foundation", "build"),
            ("orchestration", "entry"),
        ] {
            assert_eq!(
                edge(&host(source), &host(target), "dependencies"),
                [format!(
                    "s/Cargo.toml: {source} package oer-s depends on {target} package oer-t; a host package uses only its own host layer and those below it"
                )],
                "{source} -> {target}"
            );
        }
        // Production code has no host layer and constrains nothing.
        assert!(
            edge(
                &host("foundation"),
                &format!("layer = \"role\"\n{PORTABLE}"),
                "dependencies"
            )
            .is_empty()
        );
    }
}
