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
    /// Record reviewed code fingerprints of cited vendor functions.
    /// Rewrite the vendor evidence shards whose recorded sources changed, or
    /// the named scenarios' shards. Resolves conflicting shards after a merge.
    Evidence {
        #[arg(long)]
        chip: String,
        /// Linker the scenarios prepare images with.
        #[arg(long, default_value = "ld.lld")]
        linker: std::path::PathBuf,
        /// Resource-limit mode of the scenario runs.
        #[arg(long, default_value = "watchdog")]
        limit_mode: String,
        /// Ignored output root of the scenario runs.
        #[arg(long, default_value = "target/blobray-research/evidence")]
        output: std::path::PathBuf,
        /// Rerun every scenario into the output root and fail unless each
        /// committed shard equals its rerun, without rewriting any: catches
        /// probe data edits the source digests cannot see.
        #[arg(long)]
        check: bool,
        /// Scenarios to rewrite; every stale shard when empty.
        scenarios: Vec<String>,
    },
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
    },
    /// Build API documentation from each package's `[package.metadata.docs.rs]`
    /// with `RUSTDOCFLAGS=-D warnings`, then run host doctests.
    Doc,
    /// Compare linked images function by function, modulo placement.
    Compare {
        #[command(subcommand)]
        compare: Compare,
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
enum Check {
    /// Before a push: format, Clippy, tests and API documentation for what
    /// this checkout changed against the merge base with BASE.
    Changed {
        #[arg(long, default_value = "origin/main")]
        base: String,
    },
    Metadata,
    Architecture,
    /// Audit the resolved dependency graph of every network profile.
    Network,
    /// Check local Markdown links and the static qualification catalogs.
    Docs,
    /// Build the PHY library for the chip target and audit its artifact and graph.
    Phy {
        #[arg(long)]
        chip: String,
    },
    /// Build both final HIL application images and run their target audits.
    Images,
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
            limit_mode,
            output,
            check,
            scenarios,
        } => {
            if check {
                return oer_xtask::evidence::check(&ctx, &chip, linker, limit_mode, output);
            }
            return oer_xtask::evidence::run(&ctx, &chip, scenarios, linker, limit_mode, output);
        }
        Task::VendorProvenance {
            chip,
            accept,
            rebuild,
            baseline,
        } => oer_xtask::vendor_provenance::update(&ctx, &chip, &accept, rebuild, baseline),
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
        Task::StandInstall => oer_xtask::stand_install::run(&ctx),
        Task::Sweep {
            all_checkouts,
            apply,
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
            Check::Architecture => checks::architecture::run(&ctx),
            Check::Network => checks::network::run(&ctx),
            Check::Docs => checks::docs::run(&ctx),
            Check::Phy { chip } => checks::phy::run(&ctx, &chip),
            Check::Images => checks::images::run(&ctx),
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
                oer_xtask::firmware::flash(&ctx, &output, &example, port.as_deref(), monitor)?;
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
