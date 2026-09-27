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
    use_shared_store(ctx)?;
    let (options, args) = LeaseOptions::split(args)?;
    let args = args.as_slice();
    match args.first().and_then(|argument| argument.to_str()) {
        None | Some("help" | "--help" | "-h") => println!("{STAND_HELP}"),
        Some("queue") => return queue(&args[1..]),
        Some("dashboard") => {
            return crate::hil_dashboard::serve(
                &crate::hil_store::shared_runs(HIL_TARGET)?,
                &args[1..],
            );
        }
        Some("lease") => return lease(ctx, options, &args[1..]),
        Some("board") => return board(ctx, &options, &args[1..]),
        Some("devices") => return devices(&args[1..]),
        Some("firmware") => return firmware(ctx, &options, &args[1..]),
        Some("flash") => {
            let request = oer_hil_arbiter::Request {
                owner: options.owner(ctx),
                work: String::new(),
                budget: options.budget,
                short: options.short,
                scenarios: Vec::new(),
                claims: Vec::new(),
            };
            return crate::hil_flash::run(ctx, request, &args[1..]);
        }
        Some("runs") => return runs(ctx, &options, &args[1..]),
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
        if let Err(error) = prune_automatically(ctx) {
            eprintln!("hil: automatic pruning of the run store failed: {error}");
        }
    }
    Ok(exit_code(status))
}

/// Stand commands handled here, printed before the runner's own help.
const STAND_HELP: &str = "\
Stand commands (shared by every checkout of this user):
  cargo hil queue [--json]            holders, queue with expected starts, boards, recent leases
  cargo hil dashboard [--port 8765]   live page of the queue, boards, runs and leases on 127.0.0.1
  cargo hil lease [OPTIONS] -- CMD    run CMD under one lease; nested cargo hil joins it
      --board NAME|MAC                boards CMD uses (repeatable); none: the whole stand
      --air shared|exclusive          radio environment; exclusive for RF measurements
      --flashed IMAGE (--application FILE | --sha256 HASH) (--port PORT | --device MAC)
      [--chip CHIP] [--commit REV]    journal CMD's flash when it succeeds
  cargo hil board flashed --image IMAGE ...   journal a flash made inside a lease
  cargo hil devices [--json]          boards: name, chip, port, last firmware
  cargo hil devices set MAC [--chip CHIP] [--name NAME]
  cargo hil runs list [--scenario S] [--outcome O] [--commit C] [--image I] [--since 3d]
  cargo hil runs why RUN              why a run did not pass: failure, missed criteria, log tail
  cargo hil runs compare A B [--measurement TEXT]   measurement means side by side
  cargo hil runs history SCENARIO [--measurement TEXT]
  cargo hil runs pin RUN --reason TEXT | unpin RUN
  cargo hil runs prune [--days 30] [--keep-failed 5] [--apply]
  cargo hil firmware list             tracked ESP-IDF images (peers, vendor references)
  cargo hil firmware build IMAGE      build against the one pinned ESP-IDF
  cargo hil firmware flash IMAGE --board NAME|MAC   flash under a lease of that board, journaled
  cargo hil flash --board NAME|MAC [--image NAME] [--monitor 30s [--until TEXT]] [--air shared|exclusive|none] ELF
                                      flash an ELF for the board's chip under a lease of that board,
                                      journal it, capture the console for a bounded time

Lease options, before any HIL command or after `lease`:
  --owner NAME     default: enclosing lease owner, else the checkout directory name
  --budget DUR     90s, 15m, 1h30m; default: history of the same work or scenarios, else 15m
  --short          budget of at most 2m, granted ahead of earlier conflicting requests

Leases on different boards run in parallel. Choose a budget you expect to
use: once it is spent and a waiting request needs the same resources, a run
of several scenarios yields after its current scenario and queues again, and
a single scenario or lease command is stopped (lease exit status 75). At
twice the budget work is stopped regardless (lease exit status 124).
Scenarios tagged `air-exclusive` claim the air exclusively.

Runner commands (`cargo hil run A B C` runs several scenarios under one lease):";

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

fn parse_air(text: &str) -> std::result::Result<oer_hil_arbiter::Mode, String> {
    match text {
        "shared" => Ok(oer_hil_arbiter::Mode::Shared),
        "exclusive" => Ok(oer_hil_arbiter::Mode::Exclusive),
        _ => Err(String::from("use `shared` or `exclusive`")),
    }
}

/// The resources of a lease command: its boards and the air, or the whole
/// stand when it names neither.
fn lease_claims(
    boards: &[String],
    air: Option<oer_hil_arbiter::Mode>,
    devices: &[oer_hil_arbiter::Device],
) -> Result<Vec<oer_hil_arbiter::Claim>> {
    if boards.is_empty() && air.is_none() {
        return Ok(vec![oer_hil_arbiter::Claim::stand()]);
    }
    let mut claims = boards
        .iter()
        .map(|board| {
            Ok(oer_hil_arbiter::Claim::board(&oer_hil_arbiter::board_mac(
                devices, board,
            )?))
        })
        .collect::<Result<Vec<_>>>()?;
    claims.push(oer_hil_arbiter::Claim {
        resource: oer_hil_arbiter::AIR.to_owned(),
        mode: air.unwrap_or(oer_hil_arbiter::Mode::Shared),
    });
    Ok(claims)
}

/// Exit status of a lease command terminated at its budget because waiting
/// requests needed its resources; run it again to queue behind them.
const PREEMPTED_EXIT: u8 = 75;

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
    /// A board the command uses, by registered name or MAC; repeatable.
    /// Without boards or --air the lease claims the whole stand.
    #[arg(long = "board", value_name = "NAME|MAC")]
    boards: Vec<String>,
    /// How the command uses the radio environment: `shared` (default with
    /// boards) or `exclusive` for RF measurements.
    #[arg(long, value_parser = parse_air)]
    air: Option<oer_hil_arbiter::Mode>,
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
    let arbiter = oer_hil_arbiter::Arbiter::open()?;
    let request = oer_hil_arbiter::Request {
        owner: options.owner(ctx),
        work,
        budget: options.budget,
        short: options.short,
        scenarios: Vec::new(),
        claims: lease_claims(&cli.boards, cli.air, &arbiter.devices()?)?,
    };
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

/// Supervise an indivisible command: at its budget it is reported, and it is
/// terminated as soon as a waiting request needs its resources; at twice the
/// budget it is terminated regardless. Returns the exit code and whether the
/// command succeeded.
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
    let mut checked = started;
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
        if warned && checked.elapsed() >= std::time::Duration::from_millis(500) {
            checked = std::time::Instant::now();
            if grant.blocks_waiters() {
                grant.mark_preempted();
                child.kill()?;
                return Ok((std::process::ExitCode::from(PREEMPTED_EXIT), false));
            }
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

/// `cargo hil runs list|why|compare|history|pin|unpin|prune` over the shared
/// run store.
fn runs(
    ctx: &Context,
    options: &LeaseOptions,
    args: &[OsString],
) -> Result<std::process::ExitCode> {
    use crate::hil_runs;
    use clap::Parser as _;
    #[derive(clap::Parser)]
    #[command(name = "cargo hil runs", no_binary_name = true)]
    enum RunsCli {
        /// Runs of every checkout, newest last.
        List {
            #[arg(long)]
            scenario: Option<String>,
            /// passed, failed, broken, blocked, interrupted; with --scenario,
            /// that scenario's outcome.
            #[arg(long)]
            outcome: Option<String>,
            /// Commit prefix.
            #[arg(long)]
            commit: Option<String>,
            /// Image class or application SHA-256 prefix.
            #[arg(long)]
            image: Option<String>,
            /// Checkout directory name.
            #[arg(long)]
            checkout: Option<String>,
            /// Only runs this recent, e.g. 3d or 12h.
            #[arg(long, value_parser = parse_budget)]
            since: Option<std::time::Duration>,
            #[arg(long, default_value_t = 30)]
            limit: usize,
        },
        /// Why a run did not pass.
        Why {
            run: String,
            /// Lines of each failed repetition's uart.log.
            #[arg(long, default_value_t = 15)]
            tail: usize,
        },
        /// Measurement means of two runs side by side.
        Compare {
            a: String,
            b: String,
            /// Only measurements whose name contains this text.
            #[arg(long)]
            measurement: Option<String>,
        },
        /// A scenario's outcomes and measurements across runs.
        History {
            scenario: String,
            #[arg(long)]
            measurement: Option<String>,
            #[arg(long, default_value_t = 30)]
            limit: usize,
        },
        /// Keep a run from pruning, e.g. an A/B baseline.
        Pin {
            run: String,
            #[arg(long)]
            reason: String,
        },
        Unpin {
            run: String,
        },
        /// List, or with --apply delete, runs no rule keeps.
        Prune {
            #[arg(long, default_value_t = PRUNE_DAYS)]
            days: u64,
            #[arg(long, default_value_t = PRUNE_KEEP_FAILED)]
            keep_failed: usize,
            #[arg(long)]
            apply: bool,
        },
    }
    let local = ctx.root.join("target/hil").join(HIL_TARGET).join("runs");
    let directory = if local.exists() {
        local
    } else {
        crate::hil_store::shared_runs(HIL_TARGET)?
    };
    let store = crate::hil_store::shared_runs(HIL_TARGET)?
        .parent()
        .ok_or("the run store has no parent")?
        .to_owned();
    let all = hil_runs::all(&directory)?;
    let find = |id: &str| {
        all.iter()
            .find(|run| run.id == id)
            .ok_or_else(|| format!("no run {id} in {}", directory.display()))
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as u64;
    match RunsCli::try_parse_from(args)? {
        RunsCli::List {
            scenario,
            outcome,
            commit,
            image,
            checkout,
            since,
            limit,
        } => {
            let filter = hil_runs::Filter {
                scenario,
                outcome,
                commit,
                image,
                checkout,
                since_millis: since.map(|since| now.saturating_sub(since.as_millis() as u64)),
            };
            let matching = all
                .iter()
                .filter(|run| filter.matches(run))
                .collect::<Vec<_>>();
            for run in &matching[matching.len().saturating_sub(limit)..] {
                println!("{}", hil_runs::list_line(run));
            }
        }
        RunsCli::Why { run, tail } => print!("{}", hil_runs::why(find(&run)?, tail)),
        RunsCli::Compare { a, b, measurement } => print!(
            "{}",
            hil_runs::compare(find(&a)?, find(&b)?, measurement.as_deref())
        ),
        RunsCli::History {
            scenario,
            measurement,
            limit,
        } => {
            let containing = all
                .iter()
                .filter(|run| run.scenarios.iter().any(|s| s.id == scenario))
                .cloned()
                .collect::<Vec<_>>();
            let start = containing.len().saturating_sub(limit);
            print!(
                "{}",
                hil_runs::history(&containing[start..], &scenario, measurement.as_deref())
            );
        }
        RunsCli::Pin { run, reason } => {
            find(&run)?;
            hil_runs::set_pin(
                &store,
                &run,
                Some(
                    serde_json::json!({"by": options.owner(ctx), "reason": reason, "unix_millis": now}),
                ),
            )?;
        }
        RunsCli::Unpin { run } => hil_runs::set_pin(&store, &run, None)?,
        RunsCli::Prune {
            days,
            keep_failed,
            apply,
        } => {
            let pinned = hil_runs::pins(&store).into_keys().collect();
            let keep = hil_runs::retained(
                &all,
                &hil_runs::Retention {
                    keep_days: days,
                    keep_failed,
                },
                now,
                &pinned,
                &hil_runs::cited_by_shards(&ctx.root),
            );
            let mut freed = 0;
            let mut removed = 0;
            for run in all.iter().filter(|run| !keep.contains_key(&run.id)) {
                let bytes = hil_runs::exclusive_bytes(&run.directory);
                freed += bytes;
                removed += 1;
                if apply {
                    std::fs::remove_dir_all(&run.directory)?;
                    println!("deleted {} ({} MiB)", run.id, bytes >> 20);
                } else {
                    println!("would delete {}", hil_runs::list_line(run));
                }
            }
            println!(
                "{} {removed} of {} runs, {} MiB held only by them; {} kept{}",
                if apply { "deleted" } else { "would delete" },
                all.len(),
                freed >> 20,
                keep.len(),
                if apply { "" } else { "; add --apply to delete" }
            );
        }
    }
    Ok(std::process::ExitCode::SUCCESS)
}

/// `cargo hil firmware list | build IMAGE | flash IMAGE --board BOARD`.
fn firmware(
    ctx: &Context,
    options: &LeaseOptions,
    args: &[OsString],
) -> Result<std::process::ExitCode> {
    use clap::Parser as _;
    #[derive(clap::Parser)]
    #[command(name = "cargo hil firmware", no_binary_name = true)]
    enum FirmwareCli {
        /// Catalog images: name, chip, project and last build.
        List,
        /// Build an image against the pinned ESP-IDF.
        Build { image: String },
        /// Build an image, then flash it to a board under a lease of that
        /// board and journal it.
        Flash {
            image: String,
            #[arg(long, value_name = "NAME|MAC")]
            board: String,
        },
    }
    match FirmwareCli::try_parse_from(args)? {
        FirmwareCli::List => crate::firmware_catalog::list(ctx)?,
        FirmwareCli::Build { image } => {
            crate::firmware_catalog::build(ctx, &image)?;
        }
        FirmwareCli::Flash { image, board } => {
            let request = oer_hil_arbiter::Request {
                owner: options.owner(ctx),
                work: format!("firmware flash {image} --board {board}"),
                budget: options.budget,
                short: options.short,
                scenarios: Vec::new(),
                claims: Vec::new(),
            };
            crate::firmware_catalog::flash(ctx, &image, &board, request)?;
        }
    }
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
        /// Register or change a board's chip and name.
        Set {
            mac: String,
            #[arg(long)]
            chip: Option<String>,
            #[arg(long)]
            name: Option<String>,
        },
    }
    let cli = DevicesCli::try_parse_from(args)?;
    let arbiter = oer_hil_arbiter::Arbiter::open()?;
    if let Some(DevicesCommand::Set { mac, chip, name }) = cli.command {
        let device = arbiter.set_device(oer_hil_arbiter::Device { mac, chip, name })?;
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

/// Rule of the automatic pruning, which `cargo hil runs prune` defaults to.
const PRUNE_DAYS: u64 = 30;
const PRUNE_KEEP_FAILED: usize = 5;
/// Automatic pruning runs at most once per this interval.
const PRUNE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// Delete, at most once a day, the runs of the shared store that no rule
/// keeps; see `cargo hil runs prune`.
fn prune_automatically(ctx: &Context) -> Result<()> {
    use crate::hil_runs;
    let runs = crate::hil_store::shared_runs(HIL_TARGET)?;
    let store = runs.parent().ok_or("the run store has no parent")?;
    let marker = store.join("last-prune");
    if std::fs::metadata(&marker)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age < PRUNE_INTERVAL)
    {
        return Ok(());
    }
    std::fs::write(&marker, b"")?;
    let all = hil_runs::all(&runs)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as u64;
    let keep = hil_runs::retained(
        &all,
        &hil_runs::Retention {
            keep_days: PRUNE_DAYS,
            keep_failed: PRUNE_KEEP_FAILED,
        },
        now,
        &hil_runs::pins(store).into_keys().collect(),
        &hil_runs::cited_by_shards(&ctx.root),
    );
    let (mut removed, mut freed) = (0, 0);
    for run in all.iter().filter(|run| !keep.contains_key(&run.id)) {
        freed += hil_runs::exclusive_bytes(&run.directory);
        std::fs::remove_dir_all(&run.directory)?;
        removed += 1;
    }
    if removed > 0 {
        eprintln!(
            "hil: pruned {removed} runs ({} MiB) no rule keeps; see `cargo hil runs prune`",
            freed >> 20
        );
    }
    Ok(())
}

/// Make this checkout's run directory the shared store, migrating its runs.
fn use_shared_store(ctx: &Context) -> Result<()> {
    let local = ctx.root.join("target/hil").join(HIL_TARGET).join("runs");
    match crate::hil_store::link_runs(&local, &crate::hil_store::shared_runs(HIL_TARGET)?)? {
        crate::hil_store::Linked::Migrated { runs, kept } => eprintln!(
            "hil: moved {runs} runs into the shared store; the old directory is {}",
            kept.display()
        ),
        crate::hil_store::Linked::Deferred { active } => eprintln!(
            "hil: run {active} is in progress; this checkout joins the shared store later"
        ),
        crate::hil_store::Linked::Existing | crate::hil_store::Linked::Created => {}
    }
    Ok(())
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
                scenarios: Vec::new(),
                claims: Vec::new(),
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
    fn a_lease_claims_its_boards_and_the_air_or_else_the_whole_stand() {
        use oer_hil_arbiter::{AIR, Claim, Mode};
        let devices = [oer_hil_arbiter::Device {
            mac: "38:44:BE:AA:25:64".into(),
            chip: None,
            name: Some("esp32c5".into()),
        }];
        assert_eq!(lease_claims(&[], None, &devices).unwrap(), [Claim::stand()]);
        assert_eq!(
            lease_claims(&["esp32c5".into()], None, &devices).unwrap(),
            [Claim::board("38:44:BE:AA:25:64"), Claim::shared(AIR)]
        );
        assert_eq!(
            lease_claims(&[], Some(Mode::Exclusive), &devices).unwrap(),
            [Claim::exclusive(AIR)]
        );
        assert!(lease_claims(&["s3".into()], None, &devices).is_err());
    }

    #[test]
    fn a_lease_command_is_preempted_at_its_budget_when_others_wait() {
        let directory = tempfile::tempdir().unwrap();
        let arbiter = oer_hil_arbiter::Arbiter::at(directory.path()).unwrap();
        let grant = arbiter
            .acquire(&oer_hil_arbiter::Request {
                owner: "test".into(),
                work: "sleep".into(),
                budget: Some(std::time::Duration::from_secs(1)),
                short: false,
                scenarios: Vec::new(),
                claims: vec![oer_hil_arbiter::Claim::board("AA")],
            })
            .unwrap();
        let mut waiter = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "hil::tests::queue_behind_board_aa", "--ignored"])
            .env(oer_hil_arbiter::DIRECTORY_ENV, directory.path())
            .spawn()
            .unwrap();
        let started = std::time::Instant::now();
        let mut child = oer_process::owned::Child::spawn_with_shutdown_grace(
            Command::new("sleep").arg("60"),
            std::time::Duration::from_secs(1),
        )
        .unwrap();
        let (code, succeeded) = supervise(&grant, &mut child).unwrap();
        assert!(!succeeded);
        assert_eq!(code, std::process::ExitCode::from(PREEMPTED_EXIT));
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        drop(grant);
        assert!(waiter.wait().unwrap().success());
        assert_eq!(
            arbiter.history().unwrap()[0].outcome,
            oer_hil_arbiter::LeaseOutcome::Preempted
        );
    }

    /// Helper process for the preemption test: waits for board AA.
    #[test]
    #[ignore = "run by a_lease_command_is_preempted_at_its_budget_when_others_wait"]
    fn queue_behind_board_aa() {
        let arbiter = oer_hil_arbiter::Arbiter::open().unwrap();
        let grant = arbiter
            .acquire(&oer_hil_arbiter::Request {
                owner: "waiter".into(),
                work: "wait".into(),
                budget: Some(std::time::Duration::from_secs(60)),
                short: false,
                scenarios: Vec::new(),
                claims: vec![oer_hil_arbiter::Claim::board("AA")],
            })
            .unwrap();
        drop(grant);
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
