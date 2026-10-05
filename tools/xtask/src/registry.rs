//! The one registry of repository checks.
//!
//! Every check the gate, `check changed`, `push` and CI run is one [`Check`]
//! here: its id, the tier it belongs to, the CI job that runs it, the
//! changes that select it and its runner. Nothing else lists what runs:
//!
//! - `cargo xtask check changed` runs the [`Tier::Fast`] checks a change
//!   selects, `--full` also the [`Tier::Full`] ones, each over what the
//!   change reaches ([`Scope::Change`]);
//! - `cargo xtask push` runs the fast checks over what `HEAD` changed;
//! - each CI job runs `cargo xtask check tier TIER --job JOB`: every check of
//!   that job up to that tier, over the whole tree ([`Scope::Tree`]). The
//!   workflows keep jobs for parallelism and the runner setup (caches,
//!   toolchains a check needs), never a list of checks; a test holds them to
//!   the registry.
//!
//! A check without a trigger runs only with its whole tier, in CI: building
//! every image class, or a check that needs tools a workstation lacks.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::PathBuf,
    str::FromStr,
    time::Instant,
};

use oer_process as process;
use oer_process::Checkout;

use crate::{
    Result, checks,
    gate::{self, Change, Key},
};

/// When a check runs.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Tier {
    /// The push gate: every push and `check changed`, within a minute warm.
    Fast,
    /// CI on every push to a branch, and `check changed --full`.
    Full,
    /// Once a night on `main`.
    Nightly,
}

impl Tier {
    pub const ALL: [Self; 3] = [Self::Fast, Self::Full, Self::Nightly];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Full => "full",
            Self::Nightly => "nightly",
        }
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl FromStr for Tier {
    type Err = String;

    fn from_str(text: &str) -> std::result::Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|tier| tier.name() == text)
            .ok_or_else(|| format!("unknown tier `{text}`: fast, full or nightly"))
    }
}

/// What a runner checks.
#[derive(Clone, Copy)]
pub enum Scope<'a> {
    /// Everything of its kind: the tier runs in CI.
    Tree,
    /// What a change reaches.
    Change(&'a Change),
}

/// One repository check.
pub struct Check {
    /// Its name in output, `--list` and the CI summary.
    pub id: &'static str,
    pub tier: Tier,
    /// The CI job that runs it with its tier.
    pub job: &'static str,
    /// What it checks, one line.
    pub summary: &'static str,
    /// Whether a change needs it; `None` runs it only with its whole tier.
    pub trigger: Option<fn(&Change) -> bool>,
    pub run: fn(&Checkout, Scope<'_>) -> Result<()>,
}

/// Every check, in the order a tier runs them.
pub const CHECKS: &[Check] = &[
    Check {
        id: "tidy",
        tier: Tier::Fast,
        job: "host",
        summary: "the oer-tidy text policy over the whole tree",
        trigger: Some(|_| true),
        run: tidy,
    },
    Check {
        id: "fmt",
        tier: Tier::Fast,
        job: "host",
        summary: "rustfmt of every workspace, or of a change's packages",
        trigger: Some(|change| !change.selection.format.is_empty()),
        run: fmt,
    },
    Check {
        id: "lock",
        tier: Tier::Fast,
        job: "host",
        summary: "every lock matches its manifests (`lock --check`)",
        trigger: Some(|change| !change.selection.locks.is_empty()),
        run: lock,
    },
    Check {
        id: "capabilities",
        tier: Tier::Fast,
        job: "docs",
        summary: "`// CAPABILITY:` anchors against every catalog",
        trigger: Some(|change| change.selection.capabilities.is_some()),
        run: capabilities,
    },
    Check {
        id: "images-type-check",
        tier: Tier::Fast,
        job: "firmware",
        summary: "`cargo check` of the final HIL images and of each class compiling changed chip code",
        trigger: Some(|change| !gate::chip_code(&change.tree, &change.affected).is_empty()),
        run: images_type_check,
    },
    Check {
        id: "clippy",
        tier: Tier::Fast,
        job: "host",
        summary: "Clippy -D warnings of the root workspace, or of a change's host packages and their dependents",
        trigger: Some(|change| !host_packages(change).is_empty()),
        run: clippy,
    },
    Check {
        id: "test",
        tier: Tier::Fast,
        job: "host",
        summary: "tests of the root workspace, or of a change's host packages (with `--full` their dependents)",
        trigger: Some(|change| !tested(change).is_empty()),
        run: test,
    },
    Check {
        id: "feature-sets",
        tier: Tier::Fast,
        job: "host",
        summary: "tests under each declared `open-radio.test-feature-sets`",
        trigger: Some(|change| !tested(change).is_empty()),
        run: feature_sets,
    },
    Check {
        id: "docs",
        tier: Tier::Full,
        job: "docs",
        summary: "Markdown links, documented commands and the static qualification catalogs",
        trigger: Some(|change| change.selection.docs),
        run: |ctx, _| checks::docs::run(ctx),
    },
    Check {
        id: "doc",
        tier: Tier::Full,
        job: "docs",
        summary: "docs.rs-style API documentation (and, for the tree, doctests)",
        trigger: Some(|change| !root_packages(&change.affected).is_empty()),
        run: doc,
    },
    Check {
        id: "metadata",
        tier: Tier::Full,
        job: "host",
        summary: "locked metadata, patches, Git pins and lints of every workspace",
        trigger: Some(|change| change.files.iter().any(|file| manifest(file))),
        run: |ctx, _| checks::metadata::run(ctx).map(drop),
    },
    Check {
        id: "network",
        tier: Tier::Full,
        job: "host",
        summary: "isolated network consumers and their dependency boundaries",
        trigger: Some(|change| {
            change.files.iter().any(|file| manifest(file))
                || touches(change, &["crates/network", "crates/adapters/embassy-net"])
        }),
        run: |ctx, _| checks::network::run(ctx),
    },
    Check {
        id: "architecture",
        tier: Tier::Full,
        job: "architecture",
        summary: "chip-target Clippy of every production feature profile, facade, unsafe and PAC boundaries",
        trigger: Some(|change| touches(change, &["crates/", "platform/", "registers/"])),
        run: |ctx, _| checks::architecture::run(ctx),
    },
    Check {
        id: "chip-doctest",
        tier: Tier::Full,
        job: "architecture",
        summary: "the chip-target doctests of the ESP32-S31 SoC adapter (borrowed DMA memory needs `unsafe`)",
        trigger: Some(|change| reaches(change, CHIP_DOCTEST.0)),
        run: chip_doctest,
    },
    Check {
        id: "examples-type-check",
        tier: Tier::Full,
        job: "firmware",
        summary: "every ESP32-S31 example's runtime type-checked with its image flags",
        trigger: Some(|change| {
            touches(change, &["platform/", crate::firmware::EXAMPLES])
                || !gate::chip_code(&change.tree, &change.affected).is_empty()
        }),
        run: |ctx, _| {
            for example in crate::firmware::NAMES {
                crate::firmware::type_check(ctx, example, &[], false)?;
            }
            Ok(())
        },
    },
    Check {
        id: "example-link",
        tier: Tier::Full,
        job: "images",
        summary: "the station example built, audited and encoded into a bundle",
        trigger: Some(|change| touches(change, &["platform/"])),
        run: |ctx, _| {
            // The interrupt-stack gate bounds the image with the pinned ROM.
            oer_vendor_artifacts::run(&ctx.root, oer_image::staged::CHIP, &[String::from("rom")])?;
            crate::firmware::build(ctx, "station", &[], false).map(drop)
        },
    },
    Check {
        id: "access-point-host-tests",
        tier: Tier::Full,
        job: "firmware",
        summary: "the access-point example's library tests on the host",
        trigger: Some(|change| touches(change, &["examples/esp32s31/access-point/"])),
        run: access_point_host_tests,
    },
    Check {
        id: "esp32c5-agent",
        tier: Tier::Full,
        job: "firmware",
        summary: "Clippy of the ESP32-C5 HIL agent for each of its image classes",
        trigger: Some(|change| touches(change, ESP32C5_AGENT_INPUTS)),
        run: esp32c5_agent,
    },
    Check {
        id: "esp32c5-register-probe",
        tier: Tier::Full,
        job: "firmware",
        summary: "the ESP32-C5 register probe built",
        trigger: Some(|change| {
            touches(
                change,
                &[
                    "verification/esp32c5/hardware/",
                    "crates/hardware/esp32c5/",
                    "platform/esp32c5/",
                    "registers/esp32c5/",
                ],
            )
        }),
        run: |ctx, _| {
            process::run(
                oer_toolchain::cargo_in(
                    &ctx.root
                        .join("verification/esp32c5/hardware/register-probe"),
                )
                .args(["build", "--locked", "--release"]),
            )
        },
    },
    Check {
        id: "esp32c5-pac-hal",
        tier: Tier::Full,
        job: "firmware",
        summary: "the ESP32-C5 PAC and HAL built for the chip target",
        trigger: Some(|change| {
            touches(change, &["crates/hardware/esp32c5/", "registers/esp32c5/"])
        }),
        run: |ctx, _| {
            let target = oer_chip_profile::rust_target(&ctx.root, "esp32c5")?;
            process::run(oer_toolchain::cargo_in(&ctx.root).args([
                "build",
                "--locked",
                "-p",
                "oer-esp32c5-pac",
                "-p",
                "oer-esp32c5-hal",
                "--target",
                &target,
            ]))
        },
    },
    Check {
        id: "phy",
        tier: Tier::Full,
        job: "firmware",
        summary: "the PHY library built for the chip target, its artifact and graph audited",
        trigger: Some(|change| {
            change
                .files
                .iter()
                .any(|file| file.starts_with("crates/") && file.ends_with("Cargo.toml"))
        }),
        run: |ctx, _| checks::phy::run(ctx, oer_image::staged::CHIP),
    },
    Check {
        id: "registers",
        tier: Tier::Full,
        job: "models",
        summary: "every register publication validates and generates what is committed",
        trigger: Some(|change| touches(change, &["registers/", "tools/registers/"])),
        run: registers,
    },
    Check {
        id: "qualification",
        tier: Tier::Full,
        job: "models",
        summary: "every qualification program validates and its committed evidence evaluates",
        trigger: Some(|change| {
            touches(
                change,
                &["qualification/", "hil/scenarios/", "verification/"],
            ) || reaches(change, "oer-qualification")
        }),
        run: qualification,
    },
    Check {
        id: "provenance",
        tier: Tier::Full,
        job: "verification",
        summary: "every cited vendor function is registered with its reviewed, still pinned code",
        trigger: Some(|change| {
            change.files.iter().any(|file| {
                (file.ends_with(".rs")
                    && (file.starts_with("crates/") || file.starts_with("verification/")))
                    || file.starts_with("registers/")
                    || file.starts_with("docs/vendor/")
                    || file.split('/').any(|part| part == "facts")
            })
        }),
        run: provenance,
    },
    // Needs clang, lld and the pinned Sail model.
    Check {
        id: "isa-conformance",
        tier: Tier::Full,
        job: "isa-conformance",
        summary: "Blobray's RISC-V executor against the architectural tests and the Sail model",
        trigger: None,
        run: |ctx, _| checks::isa_conformance::run(ctx, std::path::Path::new("clang")),
    },
    // A change type-checks the classes it reaches instead.
    Check {
        id: "final-images",
        tier: Tier::Full,
        job: "images",
        summary: "both final HIL images built with every gate and Blobray's target audit",
        trigger: None,
        run: |ctx, _| {
            oer_vendor_artifacts::run(&ctx.root, oer_image::staged::CHIP, &[String::from("rom")])?;
            checks::firmware::run(
                ctx,
                &gate::FINAL_IMAGES,
                checks::firmware::Depth::Build,
                checks::firmware::default_jobs(),
            )
        },
    },
    Check {
        id: "image-classes",
        tier: Tier::Nightly,
        job: "classes",
        summary: "every HIL image class built and audited as the runner builds it",
        trigger: None,
        run: |ctx, _| {
            oer_vendor_artifacts::run(&ctx.root, oer_image::staged::CHIP, &[String::from("rom")])?;
            checks::firmware::run(
                ctx,
                &[],
                checks::firmware::Depth::Build,
                checks::firmware::default_jobs(),
            )
        },
    },
    Check {
        id: "examples",
        tier: Tier::Nightly,
        job: "examples",
        summary: "every ESP32-S31 example built and audited, keeping only its outputs",
        trigger: None,
        run: examples,
    },
    Check {
        id: "decoder-reference",
        tier: Tier::Nightly,
        job: "examples",
        summary: "the RV32 decoder's accepted encodings against the toolchain's llvm-objdump",
        trigger: None,
        run: |ctx, _| {
            process::run(oer_toolchain::cargo_in(&ctx.root).args([
                "test",
                "--locked",
                "-p",
                "oer-riscv-decode",
                "--test",
                "llvm_reference",
                "--",
                "--ignored",
            ]))
        },
    },
    Check {
        id: "blobray",
        tier: Tier::Nightly,
        job: "blobray",
        summary: "Clippy and the tests of the whole Blobray workspace",
        trigger: None,
        run: |ctx, _| {
            process::run(oer_toolchain::blobray::cargo(&ctx.root, "clippy").args([
                "--locked",
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ]))?;
            process::run(oer_toolchain::blobray::cargo(&ctx.root, "test").args([
                "--locked",
                "--workspace",
                "--no-fail-fast",
            ]))
        },
    },
    // Needs a GNU RISC-V ld 2.47 or newer for the linker adapter's tests.
    Check {
        id: "blobray-standalone",
        tier: Tier::Nightly,
        job: "blobray",
        summary: "generic Blobray extracted with the crates it takes by path, built and tested on its own",
        trigger: None,
        run: |ctx, _| checks::standalone::run(ctx),
    },
    Check {
        id: "vendor-probes",
        tier: Tier::Nightly,
        job: "verification",
        summary: "each verified chip's Rust comparison probe images",
        trigger: None,
        run: |ctx, _| {
            for chip in verified_chips(ctx)? {
                oer_vendor_evidence::run::probes::run(ctx, &chip, false)?;
            }
            Ok(())
        },
    },
    Check {
        id: "host-stands",
        tier: Tier::Nightly,
        job: "verification",
        summary: "Clippy and the tests of each verified chip's vendor host stands",
        trigger: None,
        run: host_stands,
    },
];

/// The SoC adapter whose chip-target doctests run, and its feature.
const CHIP_DOCTEST: (&str, &str) = ("oer-esp32s31-soc-esp-hal", "axi-gdma-mem2mem");

/// The image classes the ESP32-C5 agent builds, by runtime feature.
const ESP32C5_AGENT_FEATURES: [&str; 2] = ["boot-smoke", "system-watchdog"];

/// What the ESP32-C5 agent compiles besides its own workspace.
const ESP32C5_AGENT_INPUTS: &[&str] = &[
    "hil/targets/esp32c5/",
    "hil/agent/",
    "hil/protocol/",
    "crates/hardware/esp32c5/",
    "platform/esp32c5/",
];

/// The registered check `id`.
pub fn check(id: &str) -> Option<&'static Check> {
    CHECKS.iter().find(|check| check.id == id)
}

/// The CI jobs, each with the tiers its checks belong to.
pub fn jobs() -> BTreeMap<&'static str, BTreeSet<Tier>> {
    let mut jobs: BTreeMap<&str, BTreeSet<Tier>> = BTreeMap::new();
    for check in CHECKS {
        jobs.entry(check.job).or_default().insert(check.tier);
    }
    jobs
}

/// The checks `job` runs at `tier`: its checks of that tier and below.
pub fn of_job(tier: Tier, job: &str) -> Vec<&'static Check> {
    CHECKS
        .iter()
        .filter(|check| check.job == job && check.tier <= tier)
        .collect()
}

/// The checks a change at `tier` runs: those of that tier and below whose
/// trigger the change fires.
pub fn of_change(change: &Change) -> Vec<&'static Check> {
    CHECKS
        .iter()
        .filter(|check| {
            check.tier <= change.tier && check.trigger.is_some_and(|trigger| trigger(change))
        })
        .collect()
}

/// One check's outcome, printed as it ends.
fn step(label: &str, check: &Check, work: impl FnOnce() -> Result<()>) -> Result<()> {
    let started = Instant::now();
    let result = work();
    println!(
        "{label}: {} {} ({:.1} s)",
        if result.is_ok() { "PASS" } else { "FAIL" },
        check.id,
        started.elapsed().as_secs_f64()
    );
    result.map_err(|error| format!("{}: {error}", check.id).into())
}

/// Run what `change` needs, stopping at the first failure.
pub fn run_change(ctx: &Checkout, change: &Change) -> Result<()> {
    for check in of_change(change) {
        step("gate", check, || (check.run)(ctx, Scope::Change(change)))?;
    }
    if change.tier == Tier::Fast {
        let left: Vec<&str> = CHECKS
            .iter()
            .filter(|check| {
                check.tier == Tier::Full && check.trigger.is_some_and(|trigger| trigger(change))
            })
            .map(|check| check.id)
            .collect();
        if !left.is_empty() {
            println!("gate: left to CI (or `--full`): {}", left.join(", "));
        }
    }
    Ok(())
}

/// Run every check `job` has up to `tier` over the whole tree; a failure
/// does not stop the remaining checks, and any failure fails the job.
pub fn run_tier(ctx: &Checkout, tier: Tier, job: &str) -> Result<()> {
    let checks = of_job(tier, job);
    if checks.is_empty() {
        return Err(format!(
            "no check of job `{job}` up to tier {tier}; jobs: {}",
            jobs().into_keys().collect::<Vec<_>>().join(", ")
        )
        .into());
    }
    let mut failed = Vec::new();
    for check in checks {
        if let Err(error) = step(&format!("{tier} {job}"), check, || {
            (check.run)(ctx, Scope::Tree)
        }) {
            eprintln!("{error}");
            failed.push(check.id);
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("failed: {}", failed.join(", ")).into())
    }
}

/// The registry as a table: one line per check.
pub fn list() -> String {
    let mut text = String::new();
    for check in CHECKS {
        text += &format!(
            "{:<24} {:<8} {:<16} {}{}\n",
            check.id,
            check.tier.name(),
            check.job,
            check.summary,
            if check.trigger.is_none() {
                " [whole tier only]"
            } else {
                ""
            }
        );
    }
    text
}

fn touches(change: &Change, prefixes: &[&str]) -> bool {
    change
        .files
        .iter()
        .any(|file| prefixes.iter().any(|prefix| file.starts_with(prefix)))
}

fn manifest(file: &str) -> bool {
    file.ends_with("Cargo.toml") || file.ends_with("Cargo.lock")
}

/// Whether the change reaches the package `name` of any workspace.
fn reaches(change: &Change, name: &str) -> bool {
    change.affected.iter().any(|(_, package)| package == name)
}

/// The affected packages the host builds.
fn host_packages(change: &Change) -> BTreeSet<&Key> {
    change
        .affected
        .iter()
        .filter(|(workspace, name)| {
            change
                .tree
                .packages
                .iter()
                .any(|p| &p.workspace == workspace && &p.name == name && p.host)
        })
        .collect()
}

/// The host packages whose tests the change runs.
fn tested(change: &Change) -> BTreeSet<&Key> {
    let tested = gate::tested(change.tier, &change.selection, &change.affected);
    host_packages(change)
        .into_iter()
        .filter(|key| tested.contains(key))
        .collect()
}

/// Root-workspace packages of `keys`.
fn root_packages<'a>(keys: impl IntoIterator<Item = &'a Key>) -> BTreeSet<&'a str> {
    keys.into_iter()
        .filter(|(workspace, _)| workspace == "Cargo.toml")
        .map(|(_, name)| name.as_str())
        .collect()
}

/// `keys` grouped by workspace manifest.
fn by_workspace<'a>(keys: impl IntoIterator<Item = &'a Key>) -> BTreeMap<&'a str, Vec<&'a str>> {
    let mut groups: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (workspace, name) in keys {
        groups.entry(workspace).or_default().push(name);
    }
    groups
}

fn label(packages: &[&str]) -> String {
    if packages.len() <= 4 {
        packages.join(", ")
    } else {
        format!("{} packages", packages.len())
    }
}

/// Every `oer-tidy` check over this checkout, in-process (`cargo tidy
/// check` from the command line); fails listing every problem.
fn tidy(ctx: &Checkout, _: Scope<'_>) -> Result<()> {
    let repo = oer_repo::Repo::from_git(&ctx.root)?;
    let mut problems = Vec::new();
    for outcome in oer_tidy::run(&repo)? {
        for problem in outcome.problems {
            problems.push(format!("tidy {}: {problem}", outcome.check));
        }
    }
    if problems.is_empty() {
        return Ok(());
    }
    Err(format!(
        "{} problem(s) (exceptions: tools/tidy/allowlist.toml):\n{}",
        problems.len(),
        problems.join("\n")
    )
    .into())
}

fn fmt(ctx: &Checkout, scope: Scope<'_>) -> Result<()> {
    let runs: Vec<(String, Option<Vec<&str>>)> = match scope {
        Scope::Tree => checks::common::model(ctx)?
            .workspaces()
            .iter()
            .map(|workspace| (workspace.clone(), None))
            .collect(),
        // Only the selected packages: formatting the whole root workspace
        // takes 15 s, its changed packages a fraction of that.
        Scope::Change(change) => change
            .selection
            .format
            .iter()
            .map(|workspace| {
                let packages = change
                    .selection
                    .packages
                    .iter()
                    .filter(|(owner, _)| owner == workspace)
                    .map(|(_, name)| name.as_str())
                    .collect();
                (workspace.clone(), Some(packages))
            })
            .collect(),
    };
    for (workspace, packages) in runs {
        println!("fmt: {workspace}");
        let mut command = oer_toolchain::cargo_in(&ctx.root);
        command
            .args(["fmt", "--manifest-path"])
            .arg(ctx.root.join(&workspace));
        match packages {
            None => {
                command.arg("--all");
            }
            Some(packages) => {
                for package in packages {
                    command.args(["-p", package]);
                }
            }
        }
        process::run(command.args(["--", "--check"]))?;
    }
    Ok(())
}

fn lock(ctx: &Checkout, scope: Scope<'_>) -> Result<()> {
    let manifests: Vec<PathBuf> = match scope {
        Scope::Tree => checks::metadata::workspaces(ctx)?.into_iter().collect(),
        Scope::Change(change) => change
            .selection
            .locks
            .iter()
            .map(|workspace| ctx.root.join(workspace))
            .collect(),
    };
    checks::metadata::check_locks(ctx, &manifests)
}

fn capabilities(ctx: &Checkout, scope: Scope<'_>) -> Result<()> {
    let files: Vec<PathBuf> = match scope {
        Scope::Tree => Vec::new(),
        Scope::Change(change) => change
            .selection
            .capabilities
            .iter()
            .flatten()
            .map(PathBuf::from)
            .collect(),
    };
    checks::docs::capabilities(ctx, &files)
}

fn images_type_check(ctx: &Checkout, scope: Scope<'_>) -> Result<()> {
    let classes = match scope {
        Scope::Tree => gate::FINAL_IMAGES.to_vec(),
        Scope::Change(change) => {
            // Chip-target code is invisible to the host checks: type-check
            // the final images and every class whose image compiles a
            // package with that code, so an interface change it still uses
            // fails here.
            let chip = gate::chip_code(&change.tree, &change.affected);
            let changed: BTreeSet<&str> = chip.iter().map(|(_, name)| name.as_str()).collect();
            let mut graphs = Vec::new();
            for class in oer_hil_image_class::ImageClass::ALL {
                graphs.push((class, oer_hil_image::packages(&ctx.root, class)?));
            }
            let mut classes = gate::image_classes(&changed, &graphs);
            if change.tier >= Tier::Full {
                // And every class whose last build read a changed file.
                let paths: Vec<PathBuf> = change.files.iter().map(PathBuf::from).collect();
                for class in checks::firmware::affected(&paths)? {
                    if !classes.contains(&class) {
                        classes.push(class);
                    }
                }
            }
            println!(
                "gate: {} package(s) with chip code; type-checking {} image class(es)",
                chip.len(),
                classes.len()
            );
            classes
        }
    };
    if let Scope::Change(change) = scope
        && change.files.iter().any(|file| file == "Cargo.lock")
    {
        oer_hil_image::ensure_vendor_dependencies_absent(&ctx.root)?;
    }
    checks::firmware::run(
        ctx,
        &classes,
        checks::firmware::Depth::TypeCheck,
        checks::firmware::default_jobs(),
    )
}

fn clippy(ctx: &Checkout, scope: Scope<'_>) -> Result<()> {
    let runs: BTreeMap<&str, Option<Vec<&str>>> = match scope {
        Scope::Tree => BTreeMap::from([("Cargo.toml", None)]),
        Scope::Change(change) => by_workspace(host_packages(change))
            .into_iter()
            .map(|(workspace, packages)| {
                // `--full` lints the whole root workspace, as CI does.
                let whole = change.tier >= Tier::Full && workspace == "Cargo.toml";
                (workspace, (!whole).then_some(packages))
            })
            .collect(),
    };
    for (workspace, packages) in runs {
        let mut command = oer_toolchain::cargo_in(&ctx.root);
        command
            .args(["clippy", "--locked", "--all-targets", "--manifest-path"])
            .arg(ctx.root.join(workspace));
        match &packages {
            None => {
                command.arg("--workspace");
            }
            Some(packages) => {
                for package in packages {
                    command.args(["-p", package]);
                }
            }
        }
        println!(
            "clippy: {}",
            packages
                .as_deref()
                .map_or_else(|| workspace.to_owned(), label)
        );
        process::run(command.args(["--", "-D", "warnings"]))?;
    }
    Ok(())
}

fn test(ctx: &Checkout, scope: Scope<'_>) -> Result<()> {
    let runs: BTreeMap<&str, Option<Vec<&str>>> = match scope {
        Scope::Tree => BTreeMap::from([("Cargo.toml", None)]),
        Scope::Change(change) => by_workspace(tested(change))
            .into_iter()
            .map(|(workspace, packages)| (workspace, Some(packages)))
            .collect(),
    };
    for (workspace, packages) in runs {
        let mut command = oer_toolchain::cargo_in(&ctx.root);
        command
            .args(["test", "--locked", "--no-fail-fast", "--manifest-path"])
            .arg(ctx.root.join(workspace));
        match &packages {
            None => {
                command.arg("--workspace");
            }
            Some(packages) => {
                for package in packages {
                    command.args(["-p", package]);
                }
            }
        }
        println!(
            "test: {}",
            packages
                .as_deref()
                .map_or_else(|| workspace.to_owned(), label)
        );
        // A change's tests are bounded; the whole tree's are the CI job's.
        match packages {
            Some(_) => process::run_with_timeout(&mut command, gate::TEST_LIMIT)?,
            None => process::run(&mut command)?,
        }
    }
    Ok(())
}

fn feature_sets(ctx: &Checkout, scope: Scope<'_>) -> Result<()> {
    let Scope::Change(change) = scope else {
        return checks::feature_sets::run(ctx);
    };
    let tested = root_packages(tested(change));
    for package in change
        .tree
        .model
        .members("Cargo.toml")
        .filter(|package| tested.contains(package.name.as_str()))
    {
        let sets = &change.tree.model.classification(package)?.test_feature_sets;
        checks::feature_sets::test(ctx, &package.name, sets)?;
    }
    Ok(())
}

fn doc(ctx: &Checkout, scope: Scope<'_>) -> Result<()> {
    let Scope::Change(change) = scope else {
        return crate::doc::run(ctx);
    };
    let packages = root_packages(&change.affected);
    let manifest = ctx.root.join("Cargo.toml");
    let metadata = crate::cargo::metadata_no_deps(ctx, &manifest)?;
    for mut group in crate::doc::groups(&manifest, &metadata)? {
        group
            .packages
            .retain(|package| packages.contains(package.as_str()));
        group.features.retain(|feature| {
            feature
                .split_once('/')
                .is_some_and(|(package, _)| packages.contains(package))
        });
        if !group.packages.is_empty() {
            process::run(crate::doc::command(ctx, "doc", &group).arg("--no-deps"))?;
        }
    }
    Ok(())
}

fn chip_doctest(ctx: &Checkout, _: Scope<'_>) -> Result<()> {
    let (package, feature) = CHIP_DOCTEST;
    let target = oer_chip_profile::rust_target(&ctx.root, oer_image::staged::CHIP)?;
    process::run(oer_toolchain::cargo_in(&ctx.root).args([
        "test",
        "--doc",
        "--locked",
        "-p",
        package,
        "--features",
        feature,
        "--target",
        &target,
    ]))
}

fn access_point_host_tests(ctx: &Checkout, _: Scope<'_>) -> Result<()> {
    let directory = ctx
        .root
        .join(crate::firmware::EXAMPLES)
        .join("access-point");
    process::run(oer_toolchain::cargo_in(&directory).args([
        "test",
        "--lib",
        "--locked",
        "--target",
        &oer_toolchain::host_target()?,
    ]))
}

fn esp32c5_agent(ctx: &Checkout, _: Scope<'_>) -> Result<()> {
    let directory = ctx.root.join("hil/targets/esp32c5");
    for features in ESP32C5_AGENT_FEATURES {
        process::run(oer_toolchain::cargo_in(&directory).args([
            "clippy",
            "--locked",
            "--release",
            "-p",
            "oer-esp32c5-hil-agent",
            "--no-default-features",
            "--features",
            features,
            "--",
            "-D",
            "warnings",
        ]))?;
    }
    Ok(())
}

fn registers(ctx: &Checkout, _: Scope<'_>) -> Result<()> {
    for chip in oer_vendor_artifacts::project::supported(&ctx.root)? {
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
                oer_toolchain::cargo_in(&ctx.root)
                    .arg("registers")
                    .args(arguments)
                    .arg("--manifest")
                    .arg(&manifest),
            )?;
        }
    }
    Ok(())
}

fn qualification(ctx: &Checkout, _: Scope<'_>) -> Result<()> {
    checks::docs::programs(ctx)
}

/// The chips with pinned vendor artifacts.
fn verified_chips(ctx: &Checkout) -> Result<Vec<String>> {
    Ok(oer_vendor_artifacts::project::supported(&ctx.root)?
        .into_iter()
        .filter(|chip| {
            ctx.root
                .join("verification")
                .join(chip)
                .join("artifacts.toml")
                .is_file()
        })
        .collect())
}

fn provenance(ctx: &Checkout, _: Scope<'_>) -> Result<()> {
    for chip in verified_chips(ctx)? {
        if !oer_vendor_artifacts::unfetched(&ctx.root, &chip)?.is_empty() {
            oer_vendor_artifacts::fetch_vendor_sources(&ctx.root, &chip)?;
        }
        oer_vendor_provenance::registry::check(&ctx.root, &chip)?;
    }
    Ok(())
}

fn examples(ctx: &Checkout, _: Scope<'_>) -> Result<()> {
    let mut failed = Vec::new();
    for example in crate::firmware::NAMES {
        if let Err(error) = crate::firmware::build(ctx, example, &[], false) {
            eprintln!("{example}: {error}");
            failed.push(example);
        }
        // Keep the example's audit outputs, drop its compile cache: a
        // runner's disk holds one example's cache at a time.
        let cache = crate::firmware::cache(ctx, example);
        if cache.exists() {
            std::fs::remove_dir_all(cache)?;
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("examples failed: {}", failed.join(", ")).into())
    }
}

fn host_stands(ctx: &Checkout, _: Scope<'_>) -> Result<()> {
    let model = checks::common::model(ctx)?;
    for chip in verified_chips(ctx)? {
        for manifest in
            oer_vendor_evidence::producer::host_stand::stands(&model, &chip)?.into_values()
        {
            let manifest = ctx.root.join(manifest);
            process::run(
                oer_toolchain::cargo_in(&ctx.root)
                    .args(["clippy", "--locked", "--manifest-path"])
                    .arg(&manifest)
                    .args(["--all-targets", "--", "-D", "warnings"]),
            )?;
            process::run(
                oer_toolchain::cargo_in(&ctx.root)
                    .args(["test", "--locked", "--no-fail-fast", "--manifest-path"])
                    .arg(&manifest),
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
