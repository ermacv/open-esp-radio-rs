use std::path::PathBuf;

use clap::{Parser, Subcommand};
use oer_xtask::{Context, Result, checks, process};

#[derive(Parser)]
#[command(about = "Repository checks and build orchestration")]
struct Cli {
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    #[command(subcommand)]
    command: Task,
}

#[derive(Subcommand)]
enum Task {
    /// Build an observer with its Cargo receipt, then forward HIL arguments.
    #[command(disable_help_flag = true)]
    Hil {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<std::ffi::OsString>,
    },
    /// Prepare the current observer configuration without running HIL.
    HilObserver,
    /// Build Blobray and the typed vendor scenarios, then run one scenario with
    /// the forwarded arguments (for example `gain --library ...`).
    #[command(disable_help_flag = true)]
    VendorScenario {
        #[arg(long)]
        chip: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<std::ffi::OsString>,
    },
    /// Download the chip's pinned vendor artifacts into `target/vendor` and
    /// verify every artifact against `artifacts.toml`.
    VendorFetch {
        #[arg()]
        chip: String,
    },
    /// Build the chip's tracked vendor firmware against the pinned ESP-IDF
    /// and pinned vendor archives into `target/vendor-firmware`.
    VendorFirmware {
        #[arg(long)]
        chip: String,
        /// One project of `verification/<chip>/hil-vendor`; all when omitted.
        project: Option<String>,
    },
    /// Compare two revisions of a vendor archive function by function, or
    /// every pinned artifact with its namesake in `--baseline`.
    VendorDiff {
        #[arg(long)]
        chip: String,
        #[arg(long, requires = "new", conflicts_with = "baseline")]
        old: Option<PathBuf>,
        #[arg(long, requires = "old")]
        new: Option<PathBuf>,
        #[arg(long)]
        baseline: Option<PathBuf>,
        /// Also list unchanged functions.
        #[arg(long)]
        all: bool,
    },
    /// Rewrite the vendor evidence shards whose recorded sources changed, or
    /// the named scenarios' shards. Resolves conflicting shards after a merge.
    Evidence {
        #[arg(long)]
        chip: String,
        /// Linker the scenarios prepare images with.
        #[arg(long, default_value = "ld.lld")]
        linker: std::path::PathBuf,
        /// Ignored output root of the scenario runs.
        #[arg(long, default_value = "target/blobray-research/evidence")]
        output: std::path::PathBuf,
        /// Rerun the named scenarios, or every one, into the output root and
        /// fail unless each committed shard equals its rerun, without
        /// rewriting any: catches probe data edits the source digests cannot
        /// see. A differing shard reports its changed claims and sources.
        #[arg(long)]
        check: bool,
        /// With `--check`, rerun only shards that record a file changed
        /// since this revision, in the worktree or untracked; a shard that
        /// does not parse is always rerun.
        #[arg(long, requires = "check")]
        changed_since: Option<String>,
        /// Print every vendor location the committed shards leave untriaged
        /// that no other scenario covers, without running a scenario.
        #[arg(long, conflicts_with = "check")]
        untriaged: bool,
        /// Scenarios to rewrite, or with `--check` to rerun; every stale shard,
        /// or with `--check` every shard, when empty.
        scenarios: Vec<String>,
    },
    /// Record reviewed code fingerprints of cited vendor functions.
    VendorProvenance {
        #[arg(long)]
        chip: String,
        /// Functions whose pinned code was reviewed: symbols, or the
        /// `artifact[member]::symbol` form `check provenance` prints.
        #[arg(long, value_delimiter = ',')]
        accept: Vec<String>,
        /// Recompute the registry from the current citations.
        #[arg(long)]
        rebuild: bool,
        /// Directory of the revision the facts were observed in, for
        /// `--rebuild`.
        #[arg(long, requires = "rebuild")]
        baseline: Option<PathBuf>,
        /// Print each accepted function's annotated pinned code before
        /// recording it.
        #[arg(long, requires = "accept")]
        show: bool,
    },
    /// Inventory the vendor's radio MMIO accesses against the register model.
    RegisterInventory {
        #[arg(long)]
        chip: String,
        /// Report directory; `target/register-inventory/<chip>` by default.
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Build API documentation from each package's `[package.metadata.docs.rs]`
    /// with `RUSTDOCFLAGS=-D warnings`, then run host doctests.
    Doc,
    /// Compare linked images function by function, modulo placement.
    Compare {
        #[command(subcommand)]
        compare: Compare,
    },
    /// Rebase onto origin/main, run `check changed` there, and push to main
    /// only a revision that passed; reinstall `oer-stand` when its tooling
    /// changed.
    Push,
    /// Update every workspace's Cargo.lock to its manifests after a
    /// dependency or pin change.
    Lock,
    /// Download the dependencies every workspace's lock file names and the
    /// local cache lacks; Cargo otherwise runs offline in this repository.
    Fetch,
    /// Add or remove a Git worktree whose target/ starts as a copy-on-write
    /// clone of this checkout's build outputs.
    Worktree {
        #[command(subcommand)]
        worktree: Worktree,
    },
    /// Build the xtask of origin/main once and install `oer-stand`, which
    /// runs the operational HIL stand commands without building this tree.
    StandInstall,
    /// List, or with --apply remove, rebuildable build caches unused for a
    /// while (incremental data, HIL image caches); running builds are skipped.
    Sweep {
        /// Every sibling open-esp-radio-rs checkout, not only this one.
        #[arg(long)]
        all_checkouts: bool,
        #[arg(long)]
        apply: bool,
        /// The daily sweep of every checkout that `check changed` starts in
        /// the background: skipped when a recent one ran and space is ample.
        #[arg(long, conflicts_with_all = ["all_checkouts", "apply"])]
        automatic: bool,
    },
    Check {
        #[command(subcommand)]
        check: Check,
    },
    Build {
        #[command(subcommand)]
        build: Build,
    },
}

#[derive(Subcommand)]
enum Compare {
    /// Two ELF files.
    Elf {
        old: PathBuf,
        new: PathBuf,
        /// Reviewed rename applied to both images, FROM=TO (e.g. a moved path).
        #[arg(long = "alias")]
        aliases: Vec<String>,
        /// Reviewed function whose code may differ (a scheduling tie).
        #[arg(long = "allow")]
        allowed: Vec<String>,
        /// Print the instruction diff of differing functions whose name
        /// contains this.
        #[arg(long)]
        show: Vec<String>,
    },
    /// HIL image classes built at BASE and in this checkout.
    Images {
        #[arg(long)]
        base: String,
        #[arg(long = "class", default_values_t = [String::from("performance"), String::from("correctness")])]
        classes: Vec<String>,
        #[arg(long = "alias")]
        aliases: Vec<String>,
        #[arg(long = "allow")]
        allowed: Vec<String>,
        #[arg(long)]
        show: Vec<String>,
    },
}

#[derive(Subcommand)]
enum Worktree {
    /// Create PATH on a new BRANCH from FROM and seed its target/.
    Add {
        path: std::path::PathBuf,
        #[arg(long)]
        branch: String,
        #[arg(long, default_value = "origin/main")]
        from: String,
    },
    /// Remove a worktree and its private target/.
    Remove { path: std::path::PathBuf },
    /// Turn this checkout's target/ into a btrfs subvolume once, so `add`
    /// snapshots it instantly; run it while no build uses target/.
    Prepare,
}

#[derive(Subcommand)]
enum Check {
    /// Before a push: format, Clippy, tests and API documentation for what
    /// this checkout changed against the merge base with BASE.
    Changed {
        #[arg(long, default_value = "origin/main")]
        base: String,
    },
    Metadata,
    /// Test every root-workspace package with the feature sets its
    /// `open-radio.test-feature-sets` declares.
    FeatureSets,
    Architecture,
    /// Audit the resolved dependency graph of every network profile.
    Network,
    /// Check local Markdown links and the static qualification catalogs.
    Docs,
    /// Check the `// CAPABILITY: <id>` anchors in code against every catalog
    /// entry, and list the entries anchored in the given changed files.
    Capabilities {
        #[arg(long)]
        changed: Vec<std::path::PathBuf>,
    },
    /// Build the PHY library for the chip target and audit its artifact and graph.
    Phy {
        #[arg(long)]
        chip: String,
    },
    /// Build both final HIL application images and run their target audits.
    Images,
    /// Build HIL image classes with their link-time audits, reporting every
    /// class: all of them with `--all`, or each `--class`.
    #[command(group(clap::ArgGroup::new("selection").required(true).args(["all", "classes"])))]
    Firmware {
        #[arg(long)]
        all: bool,
        #[arg(long = "class")]
        classes: Vec<oer_hil_image_class::ImageClass>,
        /// Only `cargo check` each runtime, without code generation or audits.
        #[arg(long)]
        type_check: bool,
        /// Classes built at once; defaults to a fifth of the cores, 1 to 4.
        #[arg(long)]
        jobs: Option<usize>,
    },
    BlobrayStandalone,
    /// Check that every vendor function production and the register model
    /// cite is registered with its reviewed, still pinned code.
    Provenance {
        #[arg(long)]
        chip: String,
    },
}

#[derive(Subcommand)]
enum Build {
    /// Build and audit a bootable example with the shared ESP32-S31 bootstrap.
    Firmware {
        #[arg(value_parser = ["station", "access-point", "monitor", "thread"])]
        example: String,
        #[arg(long)]
        flash: bool,
        #[arg(long, requires = "flash")]
        port: Option<PathBuf>,
        #[arg(long, requires_all = ["flash", "port"])]
        monitor: bool,
        #[arg(long, value_delimiter = ',')]
        features: Vec<String>,
        #[arg(long)]
        no_default_features: bool,
        /// Network implementation: owned-xarxa, the only one.
        #[arg(long)]
        network: Option<oer_esp32s31_firmware::network::Integration>,
    },
    VendorProbes {
        #[arg(long)]
        chip: String,
        #[arg(long)]
        list_roles: bool,
    },
}

fn run() -> Result<std::process::ExitCode> {
    let cli = Cli::parse();
    let ctx = match cli.root {
        Some(root) => Context::new(root)?,
        None => Context::discover()?,
    };
    let _signals = process::install_signal_handlers()?;
    match cli.command {
        Task::HilObserver => {
            oer_xtask::hil::prepare(&ctx)?;
            println!(
                "{}",
                ctx.root.join("target/hil/current-observer.json").display()
            );
            Ok(())
        }
        Task::Hil { args } => return oer_xtask::hil::run(&ctx, &args),
        Task::VendorScenario { chip, args } => {
            return oer_xtask::vendor_scenario::run(&ctx, &chip, &args);
        }
        Task::VendorFetch { chip } => oer_xtask::vendor_fetch::run(&ctx, &chip),
        Task::VendorFirmware { chip, project } => {
            oer_xtask::vendor_firmware::run(&ctx, &chip, project.as_deref())
        }
        Task::VendorDiff {
            chip,
            old,
            new,
            baseline,
            all,
        } => oer_xtask::vendor_diff::run(&ctx, &chip, old, new, baseline, all),
        Task::Evidence {
            chip,
            linker,
            output,
            check,
            changed_since,
            untriaged,
            scenarios,
        } => {
            if untriaged {
                return oer_xtask::evidence::untriaged(&ctx, &chip);
            }
            if check {
                return oer_xtask::evidence::check(
                    &ctx,
                    &chip,
                    scenarios,
                    changed_since,
                    linker,
                    output,
                );
            }
            return oer_xtask::evidence::run(&ctx, &chip, scenarios, linker, output);
        }
        Task::VendorProvenance {
            chip,
            accept,
            rebuild,
            baseline,
            show,
        } => oer_xtask::vendor_provenance::update(&ctx, &chip, &accept, rebuild, baseline, show),
        Task::RegisterInventory { chip, output } => {
            oer_xtask::register_inventory::run(&ctx, &chip, output)
        }
        Task::Doc => oer_xtask::doc::run(&ctx),
        Task::Compare { compare } => {
            let review = |aliases: Vec<String>,
                          allowed: Vec<String>,
                          show: Vec<String>|
             -> oer_xtask::Result<oer_xtask::compare_images::Review> {
                let aliases = aliases
                    .into_iter()
                    .map(|alias| {
                        alias
                            .split_once('=')
                            .map(|(a, b)| (a.to_owned(), b.to_owned()))
                            .ok_or_else(|| format!("alias `{alias}` is not FROM=TO").into())
                    })
                    .collect::<oer_xtask::Result<Vec<_>>>()?;
                Ok(oer_xtask::compare_images::Review {
                    aliases: oer_xtask::compare_images::Aliases(aliases),
                    allowed: allowed.into_iter().collect(),
                    show,
                })
            };
            match compare {
                Compare::Elf {
                    old,
                    new,
                    aliases,
                    allowed,
                    show,
                } => {
                    let review = review(aliases, allowed, show)?;
                    let comparison =
                        oer_xtask::compare_images::compare_elf(&ctx, &old, &new, &review)?;
                    if comparison.equivalent(&review.allowed) {
                        Ok(())
                    } else {
                        Err("images differ beyond placement".into())
                    }
                }
                Compare::Images {
                    base,
                    classes,
                    aliases,
                    allowed,
                    show,
                } => oer_xtask::compare_images::compare_images(
                    &ctx,
                    &base,
                    &classes,
                    &review(aliases, allowed, show)?,
                ),
            }
        }
        Task::Push => oer_xtask::push::run(&ctx),
        Task::Lock => checks::metadata::update_locks(&ctx),
        Task::Fetch => checks::metadata::fetch(&ctx),
        Task::Worktree { worktree } => match worktree {
            Worktree::Add { path, branch, from } => {
                oer_xtask::worktree::add(&ctx, &path, &branch, &from)
            }
            Worktree::Remove { path } => oer_xtask::worktree::remove(&ctx, &path),
            Worktree::Prepare => oer_xtask::worktree::prepare(&ctx),
        },
        Task::StandInstall => oer_xtask::stand_install::run(&ctx),
        Task::Sweep {
            automatic: true, ..
        } => oer_xtask::sweep::automatically(&ctx.root),
        Task::Sweep {
            all_checkouts,
            apply,
            ..
        } => {
            let roots = if all_checkouts {
                oer_xtask::sweep::checkouts(&ctx.root)
            } else {
                vec![ctx.root.clone()]
            };
            oer_xtask::sweep::run(&roots, oer_xtask::sweep::Policy::default(), apply).map(|_| ())
        }
        Task::Check { check } => match check {
            Check::Changed { base } => checks::changed::run(&ctx, &base),
            Check::Metadata => checks::metadata::run(&ctx).map(|_| ()),
            Check::FeatureSets => checks::feature_sets::run(&ctx),
            Check::Architecture => checks::architecture::run(&ctx),
            Check::Network => checks::network::run(&ctx),
            Check::Docs => checks::docs::run(&ctx),
            Check::Capabilities { changed } => checks::docs::capabilities(&ctx, &changed),
            Check::Phy { chip } => checks::phy::run(&ctx, &chip),
            Check::Images => checks::images::run(&ctx),
            Check::Firmware {
                all: _,
                classes,
                type_check,
                jobs,
            } => checks::firmware::run(
                &ctx,
                &classes,
                if type_check {
                    checks::firmware::Depth::TypeCheck
                } else {
                    checks::firmware::Depth::Build
                },
                jobs.unwrap_or_else(checks::firmware::default_jobs),
            ),
            Check::BlobrayStandalone => checks::standalone::run(&ctx),
            Check::Provenance { chip } => oer_xtask::vendor_provenance::check(&ctx, &chip),
        },
        Task::Build {
            build:
                Build::Firmware {
                    example,
                    flash,
                    port,
                    monitor,
                    features,
                    no_default_features,
                    network,
                },
        } => {
            let output = oer_xtask::firmware::build(
                &ctx,
                &example,
                &features,
                no_default_features,
                network,
            )?;
            if flash {
                oer_xtask::firmware::flash(&output, &example, port.as_deref(), monitor)?;
            }
            Ok(())
        }
        Task::Build {
            build: Build::VendorProbes { chip, list_roles },
        } => checks::vendor::run(&ctx, &chip, list_roles),
    }?;
    Ok(std::process::ExitCode::SUCCESS)
}

fn main() -> std::process::ExitCode {
    if oer_command_tree::requested() {
        use clap::CommandFactory as _;
        let tree = oer_command_tree::command_tree(&Cli::command(), &[String::from("xtask")]);
        println!("{}", oer_command_tree::json(&tree));
        return std::process::ExitCode::SUCCESS;
    }
    match run() {
        Ok(status) => status,
        Err(error) => {
            eprintln!("xtask: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documentation_check_has_no_api_matrix_modes() {
        assert!(matches!(
            Cli::try_parse_from(["xtask", "check", "docs"])
                .unwrap()
                .command,
            Task::Check { check: Check::Docs }
        ));
        assert!(Cli::try_parse_from(["xtask", "check", "docs", "--full"]).is_err());
    }
}
