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
    /// Build API documentation from each package's `[package.metadata.docs.rs]`
    /// with `RUSTDOCFLAGS=-D warnings`, then run host doctests.
    Doc,
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
    Check {
        #[command(subcommand)]
        check: Check,
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
    BlobrayStandalone,
    /// Run Blobray's RISC-V executor on the pinned architectural tests and
    /// compare every signature with the Sail formal model's.
    IsaConformance {
        /// A clang with the riscv32 target and lld.
        #[arg(long, default_value = "clang")]
        cc: PathBuf,
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
            check: Check::Tier { list: true, .. },
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
        Check::BlobrayStandalone => "blobray-standalone",
        Check::IsaConformance { .. } => "isa-conformance",
    }
}

fn dispatch(ctx: &Checkout, command: Task) -> Result<std::process::ExitCode> {
    let ctx = ctx.clone();
    match command {
        Task::Doc => oer_xtask::doc::run(&ctx),
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
        Task::CiStatus => {
            oer_xtask::ci_status::print(&ctx, "ci");
            Ok(())
        }
        Task::Hooks { check } => oer_xtask::hooks::run(&ctx.root, check),
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
            Check::BlobrayStandalone => checks::standalone::run(&ctx),
            Check::IsaConformance { cc } => checks::isa_conformance::run(&ctx, &cc),
        },
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
        let cli = Cli::try_parse_from(["xtask", "lock", "--check"]).unwrap();
        assert!(matches!(cli.command, Task::Lock { check: true }));
        assert!(Cli::try_parse_from(["xtask", "fetch"]).is_err());
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
