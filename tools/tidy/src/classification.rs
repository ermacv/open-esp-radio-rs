//! Package classification: the `[package.metadata.open-radio]` table every
//! package declares, the scope its layer implies, and the package name rule.
//!
//! This module is the one reader of the table. `oer-xtask` classifies the
//! packages it audits through [`classify`], and the [`check`] here validates
//! every package of the tree as text, so a misclassified or misnamed package
//! fails in seconds instead of in the architecture audit.

use std::collections::{BTreeMap, BTreeSet};

use toml::{Table, Value};

use crate::{Context, chips::Chips, manifest::Package};

/// What a package is for, implied by its layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scope {
    /// Shipping code: the architecture audit compiles and bounds it.
    Production,
    /// Experimental code no production package depends on.
    Experimental,
    /// Tools, HIL, qualification, verification and applications.
    Development,
}

/// Every architecture layer and the scope it implies.
pub const LAYERS: &[(&str, Scope)] = &[
    ("contract", Scope::Production),
    ("protocol", Scope::Production),
    ("hardware", Scope::Production),
    ("role", Scope::Production),
    ("adapter", Scope::Production),
    ("runtime", Scope::Production),
    ("service", Scope::Production),
    ("composition", Scope::Production),
    ("facade", Scope::Production),
    ("experiment", Scope::Experimental),
    ("tool", Scope::Development),
    ("hil", Scope::Development),
    ("qualification", Scope::Development),
    ("verification", Scope::Development),
    ("application", Scope::Development),
    ("platform", Scope::Development),
];

/// The keys `[package.metadata.open-radio]` may hold.
pub const KEYS: &[&str] = &[
    "layer",
    "platform",
    "chip",
    "family",
    "evidence",
    "hil",
    "supported-feature-profiles",
    "test-feature-sets",
    "default-configuration",
    "inputs",
];

/// Where a package's code runs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Platform {
    Portable,
    Host,
    /// One chip, by id.
    Chip(String),
    /// Built for the one chip a feature named after a chip id selects;
    /// written once for every chip (see `oer-chip-cfg`).
    Selected,
    /// Shared by the chips of one family (`family` in `chip.toml`).
    Family(String),
}

/// What a verification package's code does for the evidence shards: decide
/// a verdict or a recorded set, whose sources a shard records, or only
/// render a report, whose sources a shard never records.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Evidence {
    Verdict,
    Report,
}

/// What a HIL package's code does for a run: observe the device under test
/// (protocol, scenarios, the link, fixtures, target firmware, evidence), or
/// operate the stand (arbitration, boards, image builds, recovery) or reach
/// code that does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Hil {
    Observation,
    Operation,
}

/// One package's classification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Classification {
    pub layer: String,
    pub scope: Scope,
    pub platform: Platform,
    /// The role of a verification package in the evidence shards.
    pub evidence: Option<Evidence>,
    /// The role of a HIL package in a run's observation.
    pub hil: Option<Hil>,
}

/// The layer whose packages declare an [`Evidence`] role.
const EVIDENCE_LAYER: &str = "verification";
/// The layer whose packages declare a [`Hil`] role.
const HIL_LAYER: &str = "hil";
/// The one package outside the `oer-` prefix.
pub const FACADE: &str = "open-esp-radio";

/// Classifies the package `name` from its `[package.metadata.open-radio]`
/// fields, which `field` returns as strings.
pub fn classify(
    name: &str,
    field: &dyn Fn(&str) -> Option<String>,
) -> Result<Classification, String> {
    let required =
        |key: &str| field(key).ok_or_else(|| format!("package {name} lacks open-radio.{key}"));
    let layer = required("layer")?;
    let scope = LAYERS
        .iter()
        .find(|(known, _)| *known == layer)
        .map(|(_, scope)| *scope)
        .ok_or_else(|| format!("package {name} has unknown architecture layer `{layer}`"))?;
    let identifier = |key: &str| -> Result<Option<String>, String> {
        match field(key) {
            None => Ok(None),
            Some(id) if oer_chip_profile::valid_identifier(&id) => Ok(Some(id)),
            Some(_) => Err(format!(
                "package {name} has invalid open-radio.{key} identifier"
            )),
        }
    };
    let platform = match (
        required("platform")?.as_str(),
        identifier("chip")?,
        identifier("family")?,
    ) {
        ("portable", None, None) => Platform::Portable,
        ("host", None, None) => Platform::Host,
        ("selected", None, None) => Platform::Selected,
        ("chip", Some(chip), None) => Platform::Chip(chip),
        ("family", None, Some(family)) => Platform::Family(family),
        _ => {
            return Err(format!(
                "package {name} has inconsistent platform/chip classification"
            ));
        }
    };
    let evidence = match (layer == EVIDENCE_LAYER, field("evidence").as_deref()) {
        (true, Some("verdict")) => Some(Evidence::Verdict),
        (true, Some("report")) => Some(Evidence::Report),
        (true, _) => {
            return Err(format!(
                "package {name} of the verification layer needs open-radio.evidence = \"verdict\" or \"report\""
            ));
        }
        (false, None) => None,
        (false, Some(_)) => {
            return Err(format!(
                "package {name} outside the verification layer declares open-radio.evidence"
            ));
        }
    };
    let hil = match (layer == HIL_LAYER, field("hil").as_deref()) {
        (true, Some("observation")) => Some(Hil::Observation),
        (true, Some("operation")) => Some(Hil::Operation),
        (true, _) => {
            return Err(format!(
                "package {name} of the hil layer needs open-radio.hil = \"observation\" or \"operation\""
            ));
        }
        (false, None) => None,
        (false, Some(_)) => {
            return Err(format!(
                "package {name} outside the hil layer declares open-radio.hil"
            ));
        }
    };
    Ok(Classification {
        layer,
        scope,
        platform,
        evidence,
        hil,
    })
}

/// Every package is `oer-<tokens>` of lowercase letters and digits; the
/// public facade alone is `open-esp-radio`.
pub fn check_name(name: &str, layer: &str) -> Result<(), String> {
    let valid = if layer == "facade" {
        name == FACADE
    } else {
        name.strip_prefix("oer-").is_some_and(|tokens| {
            tokens.split('-').all(|token| {
                !token.is_empty()
                    && token
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            })
        })
    };
    if valid {
        Ok(())
    } else {
        Err(format!(
            "package {name} does not follow the `oer-<tokens>` naming rule (docs/architecture.md)"
        ))
    }
}

/// Whether a package with evidence role `source` may depend on one with
/// role `target`: a verdict never depends on a report, so report code can
/// neither decide a verdict nor enter a shard's sources.
pub fn evidence_edge_allowed(source: Option<Evidence>, target: Option<Evidence>) -> bool {
    !(source == Some(Evidence::Verdict) && target == Some(Evidence::Report))
}

/// Whether a package with HIL role `source` may depend on one with role
/// `target`: observation never depends on operation, so stand code can
/// neither change what a run observes nor enter its evidence.
pub fn hil_edge_allowed(source: Option<Hil>, target: Option<Hil>) -> bool {
    !(source == Some(Hil::Observation) && target == Some(Hil::Operation))
}

/// The classification of a package read as text.
pub fn of(package: &Package) -> Result<Classification, String> {
    let table = package.open_radio.as_ref().ok_or_else(|| {
        format!(
            "package {} lacks [package.metadata.open-radio]",
            package.name
        )
    })?;
    classify(&package.name, &|key| {
        table.get(key).and_then(Value::as_str).map(str::to_owned)
    })
}

/// Whether the repository `path` matches the input `pattern` of a
/// package's `open-radio.inputs`: `/`-separated components where `*`
/// matches within one component and `**` any number of components; a
/// pattern that ends before the path matches everything below it.
pub fn input_matches(pattern: &str, path: &str) -> bool {
    fn component(pattern: &str, text: &str) -> bool {
        match pattern.split_once('*') {
            None => pattern == text,
            Some((head, tail)) => {
                text.starts_with(head)
                    && (head.len()..=text.len()).any(|start| component(tail, &text[start..]))
            }
        }
    }
    fn components(pattern: &[&str], path: &[&str]) -> bool {
        match (pattern.split_first(), path.split_first()) {
            (None, _) => true,
            (Some((&"**", rest)), _) => {
                (0..=path.len()).any(|skip| components(rest, &path[skip..]))
            }
            (Some((first, rest)), Some((name, others))) => {
                component(first, name) && components(rest, others)
            }
            (Some(_), None) => false,
        }
    }
    let pattern: Vec<&str> = pattern.trim_end_matches('/').split('/').collect();
    let path: Vec<&str> = path.split('/').collect();
    components(&pattern, &path)
}

/// The workspace directory whose packages name themselves: Blobray's.
const OWN_NAMES: &str = "tools/blobray";

/// Every package is classified with known keys only, names an existing chip
/// or family, follows the name rule, and no verdict package depends on a
/// report package and no observation package on stand operation.
pub fn check(context: &Context<'_>, chips: &Chips) -> Vec<String> {
    let mut problems = vec![];
    let mut classes = BTreeMap::new();
    for package in &context.manifests.packages {
        let unknown: BTreeSet<&String> = package
            .open_radio
            .iter()
            .flat_map(Table::keys)
            .filter(|key| !KEYS.contains(&key.as_str()))
            .collect();
        for key in unknown {
            problems.push(format!(
                "{}: unknown key open-radio.{key}{}",
                package.manifest,
                if key == "scope" {
                    " (the layer implies the scope)"
                } else {
                    ""
                }
            ));
        }
        if let Some(inputs) = package
            .open_radio
            .as_ref()
            .and_then(|table| table.get("inputs"))
        {
            let patterns: Option<Vec<&str>> = inputs
                .as_array()
                .and_then(|items| items.iter().map(Value::as_str).collect());
            match patterns {
                Some(patterns) => {
                    for pattern in patterns {
                        if !context
                            .repo
                            .files()
                            .any(|file| input_matches(pattern, file))
                        {
                            problems.push(format!(
                                "{}: open-radio.inputs pattern `{pattern}` matches no file",
                                package.manifest
                            ));
                        }
                    }
                }
                None => problems.push(format!(
                    "{}: open-radio.inputs must be an array of path patterns",
                    package.manifest
                )),
            }
        }
        let class = match of(package) {
            Ok(class) => class,
            Err(error) => {
                problems.push(format!("{}: {error}", package.manifest));
                continue;
            }
        };
        match &class.platform {
            Platform::Chip(chip) if !chips.ids().any(|id| id == chip) => problems.push(format!(
                "{}: open-radio.chip `{chip}` has no platform/{chip}/chip.toml",
                package.manifest
            )),
            Platform::Family(family) if !chips.families().any(|id| id == family) => {
                problems.push(format!(
                    "{}: open-radio.family `{family}` is the family of no chip",
                    package.manifest
                ));
            }
            _ => {}
        }
        if !package.directory.starts_with(OWN_NAMES)
            && let Err(error) = check_name(&package.name, &class.layer)
        {
            problems.push(format!("{}: {error}", package.manifest));
        }
        classes.insert(package.directory.as_str(), class);
    }
    for package in &context.manifests.packages {
        let Some(source) = classes.get(package.directory.as_str()) else {
            continue;
        };
        for dependency in &package.dependencies {
            let Some(target) = dependency
                .path
                .as_deref()
                .and_then(|path| classes.get(path))
            else {
                continue;
            };
            if !evidence_edge_allowed(source.evidence, target.evidence) {
                problems.push(format!(
                    "{}: verdict package {} depends on report package {}",
                    package.manifest, package.name, dependency.key
                ));
            }
            if !hil_edge_allowed(source.hil, target.hil) {
                problems.push(format!(
                    "{}: observation package {} depends on stand operation package {}",
                    package.manifest, package.name, dependency.key
                ));
            }
        }
    }
    problems.sort();
    problems.dedup();
    problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{repo::Repo, testing::tree};

    fn classified(fields: &[(&str, &str)]) -> Result<Classification, String> {
        classify("oer-x", &|key| {
            fields
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).to_owned())
        })
    }

    #[test]
    fn the_layer_implies_the_scope() {
        let class = classified(&[("layer", "role"), ("platform", "portable")]).unwrap();
        assert_eq!(class.scope, Scope::Production);
        let class = classified(&[("layer", "experiment"), ("platform", "host")]).unwrap();
        assert_eq!(class.scope, Scope::Experimental);
        let class = classified(&[("layer", "tool"), ("platform", "host")]).unwrap();
        assert_eq!(class.scope, Scope::Development);
        assert!(classified(&[("layer", "misc"), ("platform", "host")]).is_err());
        assert!(classified(&[("platform", "host")]).is_err());
    }

    #[test]
    fn chip_and_family_identifiers_follow_their_platform() {
        let class = classified(&[
            ("layer", "hardware"),
            ("platform", "chip"),
            ("chip", "esp32s31"),
        ])
        .unwrap();
        assert_eq!(class.platform, Platform::Chip("esp32s31".into()));
        for invalid in [
            &[("layer", "hardware"), ("platform", "chip")][..],
            &[
                ("layer", "hardware"),
                ("platform", "portable"),
                ("chip", "esp32s31"),
            ],
            &[
                ("layer", "hardware"),
                ("platform", "chip"),
                ("chip", "ESP32"),
            ],
            &[
                ("layer", "hardware"),
                ("platform", "family"),
                ("chip", "esp32s31"),
            ],
        ] {
            assert!(classified(invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn only_verification_and_hil_packages_declare_their_roles() {
        let verdict = classified(&[
            ("layer", "verification"),
            ("platform", "host"),
            ("evidence", "verdict"),
        ])
        .unwrap();
        assert_eq!(verdict.evidence, Some(Evidence::Verdict));
        assert!(classified(&[("layer", "verification"), ("platform", "host")]).is_err());
        assert!(
            classified(&[
                ("layer", "tool"),
                ("platform", "host"),
                ("evidence", "report")
            ])
            .is_err()
        );
        let observation = classified(&[
            ("layer", "hil"),
            ("platform", "host"),
            ("hil", "observation"),
        ])
        .unwrap();
        assert_eq!(observation.hil, Some(Hil::Observation));
        assert!(classified(&[("layer", "hil"), ("platform", "host"), ("hil", "x")]).is_err());
        assert!(
            classified(&[
                ("layer", "tool"),
                ("platform", "host"),
                ("hil", "operation")
            ])
            .is_err()
        );
    }

    #[test]
    fn role_edges_point_away_from_reports_and_stand_operation() {
        let (verdict, report) = (Some(Evidence::Verdict), Some(Evidence::Report));
        assert!(!evidence_edge_allowed(verdict, report));
        assert!(evidence_edge_allowed(report, verdict));
        assert!(evidence_edge_allowed(None, report));
        let (observation, operation) = (Some(Hil::Observation), Some(Hil::Operation));
        assert!(!hil_edge_allowed(observation, operation));
        assert!(hil_edge_allowed(operation, observation));
    }

    #[test]
    fn input_patterns_match_prefixes_and_globs() {
        assert!(input_matches(
            "qualification",
            "qualification/catalog/a.toml"
        ));
        assert!(input_matches(
            ".github/workflows/",
            ".github/workflows/ci.yml"
        ));
        assert!(!input_matches("qualification", "qualifications/a.toml"));
        let manifests = "hil/targets/**/Cargo.toml";
        assert!(input_matches(
            manifests,
            "hil/targets/esp32s31/agent/Cargo.toml"
        ));
        assert!(input_matches(manifests, "hil/targets/Cargo.toml"));
        assert!(!input_matches(
            manifests,
            "hil/targets/esp32s31/agent/src/main.rs"
        ));
        assert!(input_matches("docs/*.md", "docs/guide.md"));
        assert!(input_matches("a/*-x*/b", "a/one-x2/b"));
        assert!(!input_matches("docs/*.md", "docs/guide.txt"));
    }

    #[test]
    fn package_names_share_one_prefix_and_only_the_facade_is_branded() {
        for (name, layer) in [
            ("oer-ieee80211-sta", "protocol"),
            ("oer-esp32s31-ieee80211-embassy-net-owned", "adapter"),
            ("oer-hil-runner", "hil"),
            ("open-esp-radio", "facade"),
        ] {
            check_name(name, layer).unwrap();
        }
        for (name, layer) in [
            ("open-esp-radio-hil-runner", "hil"),
            ("oer-Wifi", "protocol"),
            ("oer--mac", "protocol"),
            ("oer-", "protocol"),
            ("oer-radio", "facade"),
        ] {
            assert!(check_name(name, layer).is_err(), "{name}");
        }
    }

    const CHIP: (&str, &str) = (
        "platform/esp32s31/chip.toml",
        "schema = 1\nid = \"esp32s31\"\nfamily = \"espressif\"\nrust-target = \"riscv32imafc-unknown-none-elf\"\nboot = \"staged\"\nespflash-chip = \"esp32s31\"\nrevisions = [\"rev0\"]\n[properties]\nwifi-bands = [\"2g4\"]\nbluetooth = [\"le\"]\nieee802154 = true\ncores = 2\n",
    );

    fn problems(files: &[(&str, &str)]) -> Vec<String> {
        let dir = tree(files);
        let repo = Repo::from_dir(dir.path()).unwrap();
        let context = Context::load(&repo).unwrap();
        check(&context, &Chips::load(&repo).unwrap())
    }

    fn manifest(name: &str, table: &str, dependencies: &str) -> String {
        format!(
            "[package]\nname = \"{name}\"\n[dependencies]\n{dependencies}\n[package.metadata.open-radio]\n{table}\n"
        )
    }

    #[test]
    fn the_tree_check_reports_every_misclassified_package() {
        let files = [
            CHIP,
            (
                "a/Cargo.toml",
                &manifest(
                    "oer-a",
                    "scope = \"production\"\nlayer = \"contract\"\nplatform = \"portable\"",
                    "",
                ),
            ),
            (
                "b/Cargo.toml",
                &manifest(
                    "oer-b",
                    "layer = \"hardware\"\nplatform = \"chip\"\nchip = \"esp32c9\"",
                    "",
                ),
            ),
            (
                "c/Cargo.toml",
                &manifest("open-radio-c", "layer = \"tool\"\nplatform = \"host\"", ""),
            ),
            ("d/Cargo.toml", "[package]\nname = \"oer-d\"\n"),
            (
                "e/Cargo.toml",
                &manifest(
                    "oer-e",
                    "layer = \"hil\"\nplatform = \"host\"\nhil = \"observation\"",
                    "oer-f = { path = \"../f\" }",
                ),
            ),
            (
                "f/Cargo.toml",
                &manifest(
                    "oer-f",
                    "layer = \"hil\"\nplatform = \"host\"\nhil = \"operation\"",
                    "",
                ),
            ),
            (
                "g/Cargo.toml",
                &manifest(
                    "oer-g",
                    "layer = \"hardware\"\nplatform = \"family\"\nfamily = \"espressif\"",
                    "",
                ),
            ),
            (
                "h/Cargo.toml",
                &manifest(
                    "oer-h",
                    "layer = \"tool\"\nplatform = \"host\"\ninputs = [\"a\", \"gone/**/x\"]",
                    "",
                ),
            ),
            (
                "tools/blobray/x/Cargo.toml",
                &manifest("blobray-x", "layer = \"tool\"\nplatform = \"host\"", ""),
            ),
        ];
        assert_eq!(
            problems(&files),
            [
                "a/Cargo.toml: unknown key open-radio.scope (the layer implies the scope)",
                "b/Cargo.toml: open-radio.chip `esp32c9` has no platform/esp32c9/chip.toml",
                "c/Cargo.toml: package open-radio-c does not follow the `oer-<tokens>` naming rule (docs/architecture.md)",
                "d/Cargo.toml: package oer-d lacks [package.metadata.open-radio]",
                "e/Cargo.toml: observation package oer-e depends on stand operation package oer-f",
                "h/Cargo.toml: open-radio.inputs pattern `gone/**/x` matches no file",
            ]
        );
    }
}
