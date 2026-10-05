use std::path::PathBuf;

use clap::{Parser, Subcommand};
use oer_process as process;
use oer_process::Checkout;
use oer_xtask::{Result, checks};

#[derive(Parser)]
#[command(about = "Repository checks and build orchestration")]
struct Cli {
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    /// Stream the output of every command a check runs instead of logging
    /// it under target/xtask/logs and showing only a failure's diagnostics.
    #[arg(long, global = true)]
    verbose: bool,
    #[command(subcommand)]
    command: Task,
}

#[derive(Subcommand)]
enum Task {
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
        /// Only these artifacts of `artifacts.toml`, by id (such as `rom`).
        #[arg(long = "artifact", value_name = "ID")]
        artifacts: Vec<String>,
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
    /// Run the fast gate on exactly the committed tree, push this branch,
    /// open its pull request when it has none, and let it merge itself once
    /// CI passes. Refused on main.
    Push {
        /// Open the pull request as a draft and leave merging to a person.
        #[arg(long)]
        draft: bool,
    },
    /// Update every workspace's Cargo.lock to its manifests after a
    /// dependency or pin change.
    Lock {
        /// Only verify, offline, that every lock matches its manifests.
        #[arg(long)]
        check: bool,
    },
    /// Add or remove a Git worktree whose target/ starts as a copy-on-write
    /// clone of this checkout's build outputs.
    Worktree {
        #[command(subcommand)]
        worktree: Worktree,
    },
    /// Build the `cargo hil` binary of origin/main once and install
    /// `oer-stand`, which runs the HIL stand commands without building this
    /// tree.
    StandInstall,
    /// Print the state of GitHub CI on main: each workflow whose newest
    /// finished run failed, or that main is green.
    CiStatus,
    /// Write the heavy-command list the Claude Code hooks read
    /// (`.claude/hooks/heavy-commands.json`).
    Hooks {
        /// Only fail when the committed list is stale.
        #[arg(long)]
        check: bool,
    },
    /// List, or with --apply remove, this checkout's rebuildable build caches
    /// unused for a while (incremental data, HIL image caches); running
    /// builds are skipped.
    Sweep {
        #[arg(long)]
        apply: bool,
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
    /// Run every check of the registry that `--job` has up to TIER over the
    /// whole tree, as that CI job does; `--list` prints the registry.
    #[command(group(clap::ArgGroup::new("selection").required(true).args(["tier", "list"])))]
    Tier {
        #[arg(requires = "job")]
        tier: Option<oer_xtask::registry::Tier>,
        #[arg(long)]
        job: Option<String>,
        #[arg(long)]
        list: bool,
    },
    /// The push gate over what this checkout changed against the merge base
    /// with BASE, committed or not: tidy, formatting, lock, capabilities,
    /// Clippy of the changed packages and their dependents, and the tests of
    /// the changed packages.
    Changed {
        #[arg(long, default_value = "origin/main")]
        base: String,
        /// Also what CI checks on the pull request: the tests of the
        /// dependents, the Markdown check, workspace Clippy, API docs, HIL
        /// image type checks, PHY, network, register and provenance audits.
        #[arg(long)]
        full: bool,
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
    /// Build HIL image classes with their link-time audits, reporting every
    /// class: all of them with `--all`, or each `--class`; `--list` prints
    /// every class with the runtime features it builds with. A built final
    /// image (`performance`, `correctness`) also passes Blobray's target
    /// audit.
    #[command(group(clap::ArgGroup::new("selection").required(true).args(["all", "classes", "list"])))]
    Firmware {
        #[arg(long)]
        all: bool,
        #[arg(long)]
        list: bool,
        #[arg(long = "class")]
        classes: Vec<oer_hil_image_class::ImageClass>,
        /// Only `cargo check` each runtime, without code generation or audits.
        #[arg(long)]
        type_check: bool,
        /// Classes built at once; defaults to half the cores, at most 8.
        #[arg(long)]
        jobs: Option<usize>,
    },
    BlobrayStandalone,
    /// Run Blobray's RISC-V executor on the pinned architectural tests and
    /// compare every signature with the Sail formal model's.
    IsaConformance {
        /// A clang with the riscv32 target and lld.
        #[arg(long, default_value = "clang")]
        cc: PathBuf,
    },
    /// Check that every vendor function production and the register model
    /// cite is registered with its reviewed, still pinned code.
    Provenance {
        #[arg(long)]
        chip: String,
    },
}

#[derive(Subcommand)]
enum Build {
    /// Build and audit a bootable example with the shared ESP32-S31
    /// bootstrap into an image bundle; `cargo hil flash` writes it.
    Firmware {
        #[arg(value_parser = oer_xtask::firmware::NAMES)]
        example: String,
        #[arg(long, value_delimiter = ',')]
        features: Vec<String>,
        #[arg(long)]
        no_default_features: bool,
        /// Only `cargo check` the runtime with the image's target, features
        /// and compiler flags: no image or audit.
        #[arg(long)]
        type_check: bool,
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
        Some(root) => Checkout::new(root)?,
        None => Checkout::discover("xtask")?,
    };
    let _signals = process::install_signal_handlers()?;
    let log = match log_name(&cli.command) {
        Some(name) if !cli.verbose => Some(oer_xtask::report::Log::start(&ctx.root, &name)?),
        _ => None,
    };
    let result = dispatch(&ctx, cli.command);
    if let Some(log) = &log {
        match &result {
            Ok(_) => println!("log: {}", log.path.display()),
            Err(_) => {
                let digest = log.digest();
                if !digest.is_empty() {
                    eprintln!("{digest}");
                }
                eprintln!("full log: {}", log.path.display());
            }
        }
    }
    result
}

/// The log of a command whose child output is logged by default: the
/// checks, `doc`, `lock` and `push`.
fn log_name(command: &Task) -> Option<String> {
    let name = match command {
        Task::Check {
            check: Check::Firmware { list: true, .. } | Check::Tier { list: true, .. },
        } => return None,
        Task::Check { check } => format!("check-{}", check_name(check)),
        Task::Doc => String::from("doc"),
        Task::Lock { .. } => String::from("lock"),
        Task::Push { .. } => String::from("push"),
        _ => return None,
    };
    Some(name)
}

fn check_name(check: &Check) -> &'static str {
    match check {
        Check::Tier { .. } => "tier",
        Check::Changed { .. } => "changed",
        Check::Metadata => "metadata",
        Check::FeatureSets => "feature-sets",
        Check::Architecture => "architecture",
        Check::Network => "network",
        Check::Docs => "docs",
        Check::Capabilities { .. } => "capabilities",
        Check::Phy { .. } => "phy",
        Check::Firmware { .. } => "firmware",
        Check::BlobrayStandalone => "blobray-standalone",
        Check::IsaConformance { .. } => "isa-conformance",
        Check::Provenance { .. } => "provenance",
    }
}

fn dispatch(ctx: &Checkout, command: Task) -> Result<std::process::ExitCode> {
    let ctx = ctx.clone();
    match command {
        Task::HilObserver => {
            oer_hil_observer::prepare::prepare(&ctx.root)?;
            println!(
                "{}",
                ctx.root.join(oer_hil_observer::receipt::CURRENT).display()
            );
            Ok(())
        }
        Task::VendorScenario { chip, args } => {
            return oer_vendor_evidence::run::scenario::run(&ctx, &chip, &args);
        }
        Task::VendorFetch { chip, artifacts } => {
            oer_vendor_artifacts::run(&ctx.root, &chip, &artifacts)
        }
        Task::VendorDiff {
            chip,
            old,
            new,
            baseline,
            all,
        } => oer_vendor_provenance::diff::run(&ctx.root, &chip, old, new, baseline, all),
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
                return oer_vendor_evidence::run::regenerate::untriaged(&ctx, &chip);
            }
            if check {
                return oer_vendor_evidence::run::regenerate::check(
                    &ctx,
                    &chip,
                    scenarios,
                    changed_since,
                    linker,
                    output,
                );
            }
            return oer_vendor_evidence::run::regenerate::run(
                &ctx, &chip, scenarios, linker, output,
            );
        }
        Task::VendorProvenance {
            chip,
            accept,
            rebuild,
            baseline,
            show,
        } => {
            let show_code = |symbol: &str| -> oer_xtask::Result<()> {
                let code = oer_vendor_evidence::run::scenario::run(
                    &ctx,
                    &chip,
                    &["show".into(), symbol.into()],
                )?;
                if code == std::process::ExitCode::SUCCESS {
                    Ok(())
                } else {
                    Err(format!("vendor-scenarios show {symbol} failed").into())
                }
            };
            oer_vendor_provenance::registry::update(
                &ctx.root,
                &chip,
                &accept,
                rebuild,
                baseline,
                show.then_some(&show_code as &oer_vendor_provenance::registry::Show<'_>),
            )
        }
        Task::RegisterInventory { chip, output } => {
            oer_register_tool::checks::inventory::run(&ctx, &chip, output)
        }
        Task::Doc => oer_xtask::doc::run(&ctx),
        Task::Compare { compare } => {
            let review = |aliases: Vec<String>,
                          allowed: Vec<String>,
                          show: Vec<String>|
             -> oer_xtask::Result<oer_image::compare::Review> {
                let aliases = aliases
                    .into_iter()
                    .map(|alias| {
                        alias
                            .split_once('=')
                            .map(|(a, b)| (a.to_owned(), b.to_owned()))
                            .ok_or_else(|| format!("alias `{alias}` is not FROM=TO").into())
                    })
                    .collect::<oer_xtask::Result<Vec<_>>>()?;
                Ok(oer_image::compare::Review {
                    aliases: oer_image::compare::Aliases(aliases),
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
                    let comparison = oer_image::compare::compare_elf(&old, &new, &review)?;
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
                } => oer_hil_image::compare_images(
                    &ctx.root,
                    &base,
                    &classes,
                    &review(aliases, allowed, show)?,
                ),
            }
        }
        Task::Push { draft } => oer_xtask::push::run(&ctx, draft),
        Task::Lock { check: false } => checks::metadata::update_locks(&ctx),
        Task::Lock { check: true } => checks::metadata::check_locks(
            &ctx,
            &checks::metadata::workspaces(&ctx)?
                .into_iter()
                .collect::<Vec<_>>(),
        ),
        Task::Worktree { worktree } => match worktree {
            Worktree::Add { path, branch, from } => {
                oer_xtask::worktree::add(&ctx, &path, &branch, &from)
            }
            Worktree::Remove { path } => oer_xtask::worktree::remove(&ctx, &path),
            Worktree::Prepare => oer_xtask::worktree::prepare(&ctx),
        },
        Task::StandInstall => oer_xtask::stand_install::run(&ctx),
        Task::CiStatus => {
            oer_xtask::ci_status::print(&ctx, "ci");
            Ok(())
        }
        Task::Hooks { check } => oer_xtask::hooks::run(&ctx.root, check),
        Task::Sweep { apply } => {
            oer_xtask::sweep::run(&ctx.root, oer_xtask::sweep::Policy::default(), apply).map(|_| ())
        }
        Task::Check { check } => match check {
            Check::Tier { list: true, .. } => {
                print!("{}", oer_xtask::registry::list());
                Ok(())
            }
            Check::Tier {
                tier: Some(tier),
                job: Some(job),
                ..
            } => oer_xtask::registry::run_tier(&ctx, tier, &job),
            Check::Tier { .. } => Err("select a tier and a --job, or --list".into()),
            Check::Changed { base, full } => checks::changed::run(&ctx, &base, full),
            Check::Metadata => checks::metadata::run(&ctx).map(|_| ()),
            Check::FeatureSets => checks::feature_sets::run(&ctx),
            Check::Architecture => checks::architecture::run(&ctx),
            Check::Network => checks::network::run(&ctx),
            Check::Docs => checks::docs::run(&ctx),
            Check::Capabilities { changed } => checks::docs::capabilities(&ctx, &changed),
            Check::Phy { chip } => checks::phy::run(&ctx, &chip),
            Check::Firmware { list: true, .. } => {
                print!("{}", checks::firmware::list());
                Ok(())
            }
            Check::Firmware {
                all: _,
                list: false,
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
            Check::IsaConformance { cc } => checks::isa_conformance::run(&ctx, &cc),
            Check::Provenance { chip } => oer_vendor_provenance::registry::check(&ctx.root, &chip),
        },
        Task::Build {
            build:
                Build::Firmware {
                    example,
                    features,
                    no_default_features,
                    type_check: true,
                    ..
                },
        } => oer_xtask::firmware::type_check(&ctx, &example, &features, no_default_features),
        Task::Build {
            build:
                Build::Firmware {
                    example,
                    features,
                    no_default_features,
                    type_check: false,
                },
        } => oer_xtask::firmware::build(&ctx, &example, &features, no_default_features).map(drop),
        Task::Build {
            build: Build::VendorProbes { chip, list_roles },
        } => oer_vendor_evidence::run::probes::run(&ctx, &chip, list_roles),
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
    fn checks_log_by_default_and_stream_with_verbose() {
        let cli = Cli::try_parse_from(["xtask", "check", "changed", "--full"]).unwrap();
        assert!(!cli.verbose);
        assert_eq!(log_name(&cli.command).as_deref(), Some("check-changed"));
        let cli = Cli::try_parse_from(["xtask", "--verbose", "push"]).unwrap();
        assert!(cli.verbose);
        assert_eq!(log_name(&cli.command).as_deref(), Some("push"));
        let cli = Cli::try_parse_from(["xtask", "check", "firmware", "--list"]).unwrap();
        assert_eq!(log_name(&cli.command), None);
        let cli = Cli::try_parse_from(["xtask", "lock", "--check"]).unwrap();
        assert!(matches!(cli.command, Task::Lock { check: true }));
        assert!(Cli::try_parse_from(["xtask", "fetch"]).is_err());
        assert!(Cli::try_parse_from(["xtask", "sweep", "--all-checkouts"]).is_err());
    }

    #[test]
    fn a_tier_runs_for_a_job_or_lists_the_registry() {
        let cli = Cli::try_parse_from(["xtask", "check", "tier", "full", "--job", "host"]).unwrap();
        assert_eq!(log_name(&cli.command).as_deref(), Some("check-tier"));
        let cli = Cli::try_parse_from(["xtask", "check", "tier", "--list"]).unwrap();
        assert_eq!(log_name(&cli.command), None);
        assert!(Cli::try_parse_from(["xtask", "check", "tier", "full"]).is_err());
        assert!(
            Cli::try_parse_from(["xtask", "check", "tier", "weekly", "--job", "host"]).is_err()
        );
    }

    #[test]
    fn every_heavy_xtask_command_the_hooks_name_exists() {
        use clap::CommandFactory as _;
        let command = Cli::command();
        let names: Vec<&str> = command.get_subcommands().map(|c| c.get_name()).collect();
        for heavy in oer_xtask::hooks::XTASK {
            assert!(
                names.iter().any(|name| name.starts_with(heavy)),
                "the hooks name `cargo xtask {heavy}`, which does not exist"
            );
        }
    }

    #[test]
    fn the_integrity_tier_is_cargo_tidy_alone() {
        assert!(Cli::try_parse_from(["xtask", "check", "tidy"]).is_err());
    }
}
