//! Build and launch the HIL observer with a receipt from Cargo's actual artifacts.
use crate::{Context, Result};
use oer_hil_schema::{artifacts, compile::compile};
use sha2::{Digest, Sha256};
use std::{ffi::OsString, fs, process::Command};

pub fn prepare(ctx: &Context) -> Result<(std::path::PathBuf, std::path::PathBuf)> {
    let compilation = compile(&ctx.root)?;
    let executable = &compilation.executable;
    let artifacts = &compilation.artifacts;
    let bytes = fs::read(executable)?;
    let hash = format!("{:x}", Sha256::digest(&bytes));
    let directory = ctx.root.join("target/hil/observers").join(&hash);
    fs::create_dir_all(&directory)?;
    let runner = directory.join("runner");
    // A private copy prevents concurrent Cargo rebuilds from replacing this run's inode.
    if !runner.exists() {
        fs::write(&runner, &bytes)?;
        fs::set_permissions(&runner, fs::metadata(executable)?.permissions())?;
    }
    if fs::read(&runner)? != bytes {
        return Err("observer executable identity conflict".into());
    }
    let output = oer_process::output(Command::new(&runner).arg("--observer-build"), None)?;
    if !output.status.success() {
        return Err("cannot read executable's embedded build".into());
    }
    let mut build: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    artifacts::apply(&mut build["resolved"], artifacts)?;
    build["resolved"]["selected_profile"] = serde_json::json!(compilation.profile);
    let receipt = serde_json::json!({"executable_sha256":hash,"build":build,"artifacts":artifacts,"profile":compilation.profile});
    // Each invocation owns its receipt, including concurrent launches of the same binary.
    let mut file = tempfile::NamedTempFile::new_in(&directory)?;
    use std::io::Write;
    let bytes = serde_json::to_vec(&receipt)?;
    file.write_all(&bytes)?;
    let receipt_path = directory.join(format!("receipt-{:x}.json", Sha256::digest(&bytes)));
    if !receipt_path.exists() {
        file.persist(&receipt_path)?;
    }
    if fs::read(&receipt_path)? != bytes {
        return Err("observer receipt identity conflict".into());
    }
    let mut current = tempfile::NamedTempFile::new_in(ctx.root.join("target/hil"))?;
    current.write_all(&bytes)?;
    current.persist(ctx.root.join("target/hil/current-observer.json"))?;
    drop(compilation);
    Ok((runner, receipt_path))
}

pub fn run(ctx: &Context, args: &[OsString]) -> Result<std::process::ExitCode> {
    let (options, args) = LeaseOptions::split(args)?;
    let args = args.as_slice();
    match args.first().and_then(|argument| argument.to_str()) {
        Some("queue") => return queue(&args[1..]),
        Some("lease") => return lease(ctx, options, &args[1..]),
        Some("board") => return board(ctx, &options, &args[1..]),
        Some("devices") => return devices(&args[1..]),
        _ => {}
    }
    let (runner, receipt_path) = prepare(ctx)?;
    if hands_off_terminal(args) {
        // Fixture installation ends in a foreground sudo handoff. A supervised
        // child runs in its own process group, which is a background group for
        // the terminal, so sudo could not read the password. Replace this
        // process instead: the runner keeps the foreground group and streams.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            let error = ctx
                .command(&runner)
                .args(args)
                .envs(options.environment(ctx))
                .env("OER_OBSERVER_RECEIPT", &receipt_path)
                .exec();
            return Err(format!("cannot hand the terminal to the HIL runner: {error}").into());
        }
    }
    // Cleanup scopes in the runner have 30-second budgets and may unwind
    // multiple owned fixtures. This is a shutdown allowance, never a run timeout.
    let mut child = oer_process::owned::Child::spawn_with_shutdown_grace(
        ctx.command(&runner)
            .args(args)
            .envs(options.environment(ctx))
            .env("OER_OBSERVER_RECEIPT", &receipt_path),
        std::time::Duration::from_secs(300),
    )?;
    let status = child.wait_forwarding_cancellation()?;
    if produces_runs(args) {
        record_evidence(ctx, &receipt_path)?;
    }
    Ok(exit_code(status))
}

/// Stand lease options accepted before the HIL command, or after `lease`.
#[derive(Debug, Default, PartialEq)]
struct LeaseOptions {
    owner: Option<String>,
    budget: Option<std::time::Duration>,
    short: bool,
}

impl LeaseOptions {
    /// Split leading `--owner NAME`, `--budget DURATION` and `--short` from
    /// the remaining arguments.
    fn split(args: &[OsString]) -> Result<(Self, Vec<OsString>)> {
        let mut options = Self::default();
        let mut rest = args.iter();
        let mut remaining = Vec::new();
        while let Some(argument) = rest.next() {
            let text = argument.to_str().unwrap_or_default();
            let (name, inline) = match text.split_once('=') {
                Some((name, value)) => (name, Some(value.to_owned())),
                None => (text, None),
            };
            let mut value = || -> Result<String> {
                inline
                    .clone()
                    .or_else(|| {
                        rest.next()
                            .and_then(|value| value.to_str().map(str::to_owned))
                    })
                    .ok_or_else(|| format!("{name} requires a value").into())
            };
            match name {
                "--owner" => options.owner = Some(value()?),
                "--budget" => {
                    options.budget = Some(oer_hil_arbiter::parse_duration(&value()?)?);
                }
                "--short" if inline.is_none() => options.short = true,
                _ => {
                    remaining.push(argument.clone());
                    remaining.extend(rest.cloned());
                    break;
                }
            }
        }
        Ok((options, remaining))
    }

    /// Explicit options win; otherwise an enclosing lease's owner, otherwise
    /// this checkout's directory name.
    fn owner(&self, ctx: &Context) -> String {
        self.owner
            .clone()
            .or_else(|| {
                std::env::var(oer_hil_arbiter::OWNER_ENV)
                    .ok()
                    .filter(|owner| !owner.is_empty())
            })
            .or_else(|| {
                ctx.root
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_else(oer_hil_arbiter::default_owner)
    }

    fn environment(&self, ctx: &Context) -> Vec<(&'static str, String)> {
        let mut environment = vec![(oer_hil_arbiter::OWNER_ENV, self.owner(ctx))];
        if let Some(budget) = self.budget {
            environment.push((
                oer_hil_arbiter::BUDGET_ENV,
                format!("{}s", budget.as_secs()),
            ));
        }
        if self.short {
            environment.push((oer_hil_arbiter::SHORT_ENV, String::from("1")));
        }
        environment
    }
}

/// Print the stand's holder, queue, board state and recent leases.
fn queue(args: &[OsString]) -> Result<std::process::ExitCode> {
    let json = match args {
        [] => false,
        [flag] if flag == "--json" => true,
        _ => return Err("usage: cargo hil queue [--json]".into()),
    };
    let status = oer_hil_arbiter::Arbiter::open()?.status()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        println!("{status}");
    }
    Ok(std::process::ExitCode::SUCCESS)
}

/// Exit status of a lease command terminated at twice its budget.
const BUDGET_EXCEEDED_EXIT: u8 = 124;

fn parse_budget(text: &str) -> std::result::Result<std::time::Duration, String> {
    oer_hil_arbiter::parse_duration(text).map_err(|error| error.to_string())
}

/// `cargo hil lease [OPTIONS] -- COMMAND...`
#[derive(Debug, clap::Parser)]
#[command(name = "cargo hil lease", no_binary_name = true)]
struct LeaseCli {
    /// Who holds the lease.
    #[arg(long)]
    owner: Option<String>,
    /// Lease budget, e.g. 90s, 15m or 1h30m.
    #[arg(long, value_parser = parse_budget)]
    budget: Option<std::time::Duration>,
    /// A budget of at most two minutes, granted ahead of the queue head.
    #[arg(long)]
    short: bool,
    /// Journal a flash that COMMAND performs when it succeeds.
    #[command(flatten)]
    flashed: FlashedArgs,
    #[arg(last = true, required = true)]
    command: Vec<OsString>,
}

/// A flash performed outside the HIL runner, recorded in the board journal.
#[derive(Clone, Debug, Default, clap::Args)]
struct FlashedArgs {
    /// Name of the flashed image, e.g. `ieee802154-peer`.
    #[arg(long = "flashed", visible_alias = "image")]
    image: Option<String>,
    /// The flashed application binary, whose SHA-256 is recorded.
    #[arg(long, requires = "image", conflicts_with = "sha256")]
    application: Option<std::path::PathBuf>,
    /// SHA-256 of the flashed application instead of `--application`.
    #[arg(long, requires = "image")]
    sha256: Option<String>,
    /// Serial port of the flashed board; its USB serial number is its MAC.
    #[arg(long, requires = "image", conflicts_with = "device")]
    port: Option<std::path::PathBuf>,
    /// MAC of the flashed board.
    #[arg(long, requires = "image")]
    device: Option<String>,
    /// Chip of the flashed board, registered when still unknown.
    #[arg(long, requires = "image")]
    chip: Option<String>,
    /// Source commit of the image.
    #[arg(long, requires = "image")]
    commit: Option<String>,
}

impl FlashedArgs {
    fn record(
        &self,
        arbiter: &oer_hil_arbiter::Arbiter,
        owner: String,
        origin: String,
    ) -> Result<()> {
        let Some(image) = &self.image else {
            return Ok(());
        };
        let application_sha256 = match (&self.application, &self.sha256) {
            (Some(application), None) => {
                use sha2::Digest as _;
                format!("{:x}", Sha256::digest(fs::read(application)?))
            }
            (None, Some(hash))
                if hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
            {
                hash.to_ascii_lowercase()
            }
            (None, Some(hash)) => return Err(format!("`{hash}` is not a SHA-256").into()),
            _ => return Err("a recorded flash needs --application or --sha256".into()),
        };
        let device = match (&self.device, &self.port) {
            (Some(device), None) => oer_hil_arbiter::normalize_mac(device)?,
            (None, Some(port)) => oer_hil_arbiter::port_mac(port).ok_or_else(|| {
                format!(
                    "{} reports no USB serial number; name the board with --device MAC",
                    port.display()
                )
            })?,
            _ => return Err("a recorded flash needs --port or --device".into()),
        };
        if let Some(chip) = &self.chip {
            arbiter.register_device(oer_hil_arbiter::Device {
                mac: device.clone(),
                chip: Some(chip.clone()),
                ..oer_hil_arbiter::Device::default()
            })?;
        }
        arbiter.record_board_by(
            owner,
            Some(device.clone()),
            oer_hil_arbiter::BoardEventKind::Flashed {
                image: image.clone(),
                application_sha256,
                commit: self.commit.clone(),
                dirty: None,
                origin,
            },
        )?;
        eprintln!("hil-arbiter: recorded {image} on {device}");
        Ok(())
    }
}

/// Run one command, typically a series of HIL commands, under one lease.
/// Nested `cargo hil` commands join the lease instead of queueing.
fn lease(ctx: &Context, outer: LeaseOptions, args: &[OsString]) -> Result<std::process::ExitCode> {
    use clap::Parser as _;
    let cli = LeaseCli::try_parse_from(args)?;
    let options = LeaseOptions {
        owner: cli.owner.or(outer.owner),
        budget: cli.budget.or(outer.budget),
        short: cli.short || outer.short,
    };
    let (program, arguments) = cli
        .command
        .split_first()
        .ok_or("cargo hil lease needs a COMMAND after --")?;
    let work = cli
        .command
        .iter()
        .map(|argument| argument.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    let request = oer_hil_arbiter::Request {
        owner: options.owner(ctx),
        work,
        budget: options.budget,
        short: options.short,
    };
    let arbiter = oer_hil_arbiter::Arbiter::open()?;
    let grant = arbiter.acquire(&request)?;
    let mut child = oer_process::owned::Child::spawn_with_shutdown_grace(
        ctx.command(program)
            .args(arguments)
            .env(oer_hil_arbiter::OWNER_ENV, &request.owner)
            .envs(grant.environment()),
        std::time::Duration::from_secs(300),
    )?;
    let (code, succeeded) = supervise(&grant, &mut child)?;
    if cli.flashed.image.is_some() {
        if succeeded {
            cli.flashed.record(
                &arbiter,
                request.owner.clone(),
                format!("lease `{}`", request.work),
            )?;
        } else {
            eprintln!("hil-arbiter: the command failed; its flash is not recorded");
        }
    }
    Ok(code)
}

/// Warn at the budget and terminate the command group at twice the budget.
/// Returns the exit code and whether the command succeeded.
fn supervise(
    grant: &oer_hil_arbiter::Grant,
    child: &mut oer_process::owned::Child,
) -> Result<(std::process::ExitCode, bool)> {
    let finished = |status: std::process::ExitStatus| (exit_code(status), status.success());
    let Some(budget) = grant.budget() else {
        return Ok(finished(child.wait_forwarding_cancellation()?));
    };
    let started = std::time::Instant::now();
    let mut warned = false;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(finished(status));
        }
        if oer_process::cancellation_requested() {
            child.kill()?;
            return Ok((std::process::ExitCode::from(130), false));
        }
        let elapsed = started.elapsed();
        if !warned && elapsed >= budget {
            grant.warn_over_budget();
            warned = true;
        }
        if elapsed >= budget * 2 {
            grant.mark_budget_exceeded();
            child.kill()?;
            return Ok((std::process::ExitCode::from(BUDGET_EXCEEDED_EXIT), false));
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// `cargo hil board flashed ...`: journal a flash made outside the runner.
fn board(
    ctx: &Context,
    options: &LeaseOptions,
    args: &[OsString],
) -> Result<std::process::ExitCode> {
    use clap::Parser as _;
    #[derive(clap::Parser)]
    #[command(name = "cargo hil board", no_binary_name = true)]
    enum BoardCli {
        /// Record a flash performed outside the HIL runner.
        Flashed(FlashedArgs),
    }
    let BoardCli::Flashed(flashed) = BoardCli::try_parse_from(args)?;
    if flashed.image.is_none() {
        return Err("cargo hil board flashed needs --image NAME".into());
    }
    flashed.record(
        &oer_hil_arbiter::Arbiter::open()?,
        options.owner(ctx),
        String::from("cargo hil board flashed"),
    )?;
    Ok(std::process::ExitCode::SUCCESS)
}

/// `cargo hil devices [--json]` and `cargo hil devices set MAC ...`.
fn devices(args: &[OsString]) -> Result<std::process::ExitCode> {
    use clap::Parser as _;
    #[derive(clap::Parser)]
    #[command(name = "cargo hil devices", no_binary_name = true)]
    struct DevicesCli {
        #[arg(long)]
        json: bool,
        #[command(subcommand)]
        command: Option<DevicesCommand>,
    }
    #[derive(clap::Subcommand)]
    enum DevicesCommand {
        /// Register or change a board's chip, role and name.
        Set {
            mac: String,
            #[arg(long)]
            chip: Option<String>,
            #[arg(long)]
            role: Option<String>,
            #[arg(long)]
            name: Option<String>,
        },
    }
    let cli = DevicesCli::try_parse_from(args)?;
    let arbiter = oer_hil_arbiter::Arbiter::open()?;
    if let Some(DevicesCommand::Set {
        mac,
        chip,
        role,
        name,
    }) = cli.command
    {
        let device = arbiter.set_device(oer_hil_arbiter::Device {
            mac,
            chip,
            role,
            name,
        })?;
        println!("{} {}", device.mac, device.label());
        return Ok(std::process::ExitCode::SUCCESS);
    }
    let status = arbiter.status()?;
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&status.devices)?);
    } else {
        for device in &status.devices {
            println!(
                "{} [{}]: {}",
                device.label,
                device.port.as_deref().unwrap_or("not attached"),
                device
                    .firmware
                    .as_ref()
                    .map_or_else(|| String::from("firmware unknown"), ToString::to_string)
            );
        }
    }
    Ok(std::process::ExitCode::SUCCESS)
}

/// The HIL target the runner executes on.
const HIL_TARGET: &str = "esp32s31";

/// Commands that execute scenarios and write run bundles.
fn produces_runs(args: &[OsString]) -> bool {
    matches!(
        args.first().and_then(|a| a.to_str()),
        Some("run" | "run-all" | "run-plan")
    ) && !args.iter().any(|arg| arg == "--check")
}

/// Record the observations that qualify on this checkout as tracked HIL
/// evidence shards, with the observer receipt the runs were produced under.
/// Failed scenarios record nothing, so this also runs after a failing suite.
fn record_evidence(ctx: &Context, receipt: &std::path::Path) -> Result<()> {
    let status = ctx
        .command("cargo")
        .args(["qualification", "hil-evidence", "--hil-target", HIL_TARGET])
        .env("OER_OBSERVER_RECEIPT", receipt)
        .status()?;
    if !status.success() {
        return Err("recording the HIL evidence shards failed".into());
    }
    Ok(())
}

/// Commands that need the terminal's foreground process group.
fn hands_off_terminal(args: &[OsString]) -> bool {
    matches!(args, [fixture, install, ..] if fixture == "fixture" && install == "install")
        && !args.iter().any(|arg| arg == "--dry-run")
}

pub(crate) fn exit_code(status: std::process::ExitStatus) -> std::process::ExitCode {
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;
    let code = status.code().unwrap_or_else(|| {
        #[cfg(unix)]
        {
            128 + status.signal().unwrap_or(1)
        }
        #[cfg(not(unix))]
        {
            1
        }
    });
    std::process::ExitCode::from(code as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn forwards_nonzero_runner_status() {
        let status = oer_process::owned::Child::spawn(Command::new("sh").args(["-c", "exit 37"]))
            .unwrap()
            .wait_forwarding_cancellation()
            .unwrap();
        assert_eq!(exit_code(status), std::process::ExitCode::from(37));
    }

    #[test]
    fn only_executing_commands_record_evidence() {
        let args = |values: &[&str]| values.iter().map(OsString::from).collect::<Vec<_>>();
        assert!(produces_runs(&args(&["run", "boot-smoke"])));
        assert!(produces_runs(&args(&["run-all", "--tag", "wifi"])));
        assert!(produces_runs(&args(&["run-plan", "plan.json"])));
        assert!(!produces_runs(&args(&["run-plan", "plan.json", "--check"])));
        assert!(!produces_runs(&args(&["plan", "--scenario", "x"])));
        assert!(!produces_runs(&args(&["doctor"])));
    }

    #[test]
    fn lease_options_precede_the_command_and_stop_at_the_first_other_argument() {
        let args = |values: &[&str]| values.iter().map(OsString::from).collect::<Vec<_>>();
        let (options, rest) = LeaseOptions::split(&args(&[
            "--owner",
            "phy",
            "--budget=40m",
            "--short",
            "run",
            "x",
            "--owner",
            "kept",
        ]))
        .unwrap();
        assert_eq!(
            options,
            LeaseOptions {
                owner: Some("phy".into()),
                budget: Some(std::time::Duration::from_secs(2400)),
                short: true,
            }
        );
        assert_eq!(rest, args(&["run", "x", "--owner", "kept"]));
        assert!(LeaseOptions::split(&args(&["--budget"])).is_err());
        assert!(LeaseOptions::split(&args(&["--budget", "soon"])).is_err());
        let (options, rest) = LeaseOptions::split(&args(&["run", "x"])).unwrap();
        assert_eq!(options, LeaseOptions::default());
        assert_eq!(rest.len(), 2);
    }

    #[test]
    fn a_lease_command_is_terminated_at_twice_its_budget() {
        let directory = tempfile::tempdir().unwrap();
        let arbiter = oer_hil_arbiter::Arbiter::at(directory.path()).unwrap();
        let grant = arbiter
            .acquire(&oer_hil_arbiter::Request {
                owner: "test".into(),
                work: "sleep".into(),
                budget: Some(std::time::Duration::from_secs(1)),
                short: false,
            })
            .unwrap();
        let started = std::time::Instant::now();
        let mut child = oer_process::owned::Child::spawn_with_shutdown_grace(
            Command::new("sleep").arg("60"),
            std::time::Duration::from_secs(1),
        )
        .unwrap();
        let (code, succeeded) = supervise(&grant, &mut child).unwrap();
        assert!(!succeeded);
        assert_eq!(code, std::process::ExitCode::from(BUDGET_EXCEEDED_EXIT));
        assert!(started.elapsed() >= std::time::Duration::from_secs(2));
        drop(grant);
        assert_eq!(
            arbiter.history().unwrap()[0].outcome,
            oer_hil_arbiter::LeaseOutcome::BudgetExceeded
        );
    }

    #[test]
    fn lease_records_a_described_flash_on_the_named_board() {
        use clap::Parser as _;
        let cli = LeaseCli::try_parse_from([
            "--owner",
            "802154",
            "--flashed",
            "ieee802154-peer",
            "--sha256",
            &"AB".repeat(32),
            "--device",
            "38:44:be:aa:25:64",
            "--chip",
            "esp32c5",
            "--",
            "idf.py",
            "flash",
        ])
        .unwrap();
        assert_eq!(cli.command, ["idf.py", "flash"]);
        assert!(
            LeaseCli::try_parse_from(["idf.py"]).is_err(),
            "COMMAND follows --"
        );
        assert!(
            LeaseCli::try_parse_from(["--sha256", "ab", "--", "x"]).is_err(),
            "flash details need --flashed"
        );
        let directory = tempfile::tempdir().unwrap();
        let arbiter = oer_hil_arbiter::Arbiter::at(directory.path()).unwrap();
        cli.flashed
            .record(&arbiter, "802154".into(), "test".into())
            .unwrap();
        let events = arbiter.board_events().unwrap();
        assert_eq!(events[0].device.as_deref(), Some("38:44:BE:AA:25:64"));
        assert_eq!(events[0].owner, "802154");
        assert!(matches!(
            &events[0].kind,
            oer_hil_arbiter::BoardEventKind::Flashed { application_sha256, .. }
                if *application_sha256 == "ab".repeat(32)
        ));
        assert_eq!(
            arbiter.devices().unwrap()[0].chip.as_deref(),
            Some("esp32c5")
        );
        let incomplete = FlashedArgs {
            image: Some("x".into()),
            sha256: Some("ab".repeat(32)),
            ..FlashedArgs::default()
        };
        assert!(
            incomplete
                .record(&arbiter, "802154".into(), "test".into())
                .is_err()
        );
    }

    #[test]
    fn only_a_real_fixture_install_takes_the_terminal() {
        let args = |values: &[&str]| values.iter().map(OsString::from).collect::<Vec<_>>();
        assert!(hands_off_terminal(&args(&[
            "fixture",
            "install",
            "--provider",
            "linux-net"
        ])));
        assert!(!hands_off_terminal(&args(&[
            "fixture",
            "install",
            "--provider",
            "linux-net",
            "--dry-run"
        ])));
        assert!(!hands_off_terminal(&args(&["fixture", "check", "x"])));
        assert!(!hands_off_terminal(&args(&["run", "boot-smoke"])));
    }
}
