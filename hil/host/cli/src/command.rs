//! The `cargo hil` command: stand commands handled here, every other command
//! forwarded to the runner this crate builds.
//!
//! [`HilCli`] is the one definition of the stand's command line: it parses
//! every stand command, dispatches each to its handler by an exhaustive
//! match, and `__command-tree` walks it, so a command cannot be listed
//! without a handler. The runner's commands are its own clap parser's.
use crate::Result;
use oer_process::Checkout;
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

/// `cargo hil`.
#[derive(clap::Parser)]
#[command(
    name = "cargo hil",
    no_binary_name = true,
    disable_help_flag = true,
    disable_help_subcommand = true
)]
struct HilCli {
    /// Who holds the leases the command takes; default: the enclosing
    /// lease's owner, else the checkout's registered owner.
    #[arg(long)]
    owner: Option<String>,
    #[command(subcommand)]
    command: Option<HilCommand>,
}

/// The stand's commands; any other command is the runner's.
#[derive(clap::Subcommand)]
enum HilCommand {
    /// Holders, balances, queue with expected starts, boards, recent leases.
    #[command(after_help = BALANCE_RULE)]
    Queue {
        #[arg(long)]
        json: bool,
    },
    /// Live page of the queue, boards, runs and leases on 127.0.0.1.
    Dashboard {
        #[arg(long, default_value_t = crate::dashboard::DEFAULT_PORT)]
        port: u16,
    },
    /// Run one command under one lease; nested `cargo hil` joins it.
    Lease(LeaseCli),
    /// Reset, check, read or soak a board under a lease.
    Board(BoardCli),
    /// One peer text-protocol command and its answer.
    #[command(subcommand)]
    Peer(crate::board::PeerCli),
    /// The stand file's boards: name, chip, port, health, last firmware.
    Devices(DevicesCli),
    /// The stand file against the host.
    #[command(subcommand)]
    Stand(crate::stand::StandCli),
    /// Host Wi-Fi radios, Bluetooth adapter and OpenWrt hosts.
    Fixtures,
    /// Tracked ESP-IDF images: list, build, flash.
    #[command(subcommand)]
    Firmware(FirmwareCli),
    /// The flash operation (lease, write, journal, start) on one board.
    Flash(crate::flash::FlashCli),
    /// Runs of the shared run store.
    #[command(subcommand)]
    Runs(RunsCli),
    /// Gated measurements per commit, baselines, regressions.
    #[command(subcommand)]
    Perf(PerfCli),
    /// This checkout's pending evidence.
    #[command(subcommand)]
    Evidence(crate::evidence::EvidenceCli),
    /// This checkout's owner: show, set, merge or forget one.
    Owner {
        #[command(subcommand)]
        command: Option<OwnerCommand>,
    },
    /// Stop another owner's lease.
    Preempt {
        /// The lease number, `#` optional.
        id: String,
        #[arg(long)]
        reason: String,
    },
    /// Symbolized program-counter profiles of a run.
    Profile(ProfileCli),
    /// The first commit at which a scenario stops passing.
    Bisect(crate::experiments::BisectCli),
    /// A/B comparison with noise-aware verdicts.
    Ab(crate::experiments::AbCli),
    /// Block until a job or run ends, or until boards are in service.
    Wait {
        /// Wait until the boards (every board when none is named) and the
        /// stand are in service.
        #[arg(long)]
        service: bool,
        /// A job or run id; with `--service`, boards.
        #[arg(value_name = "JOB|RUN|BOARD")]
        ids: Vec<String>,
    },
    /// A runner command, forwarded with its arguments.
    #[command(external_subcommand)]
    Runner(Vec<OsString>),
}

/// `cargo hil owner` subcommands.
#[derive(clap::Subcommand)]
enum OwnerCommand {
    /// Register this checkout's owner.
    Set { name: String },
    /// Charge OLD's balance and history to NEW.
    Merge { old: String, new: String },
    /// Drop a balance that is no agent's.
    Forget { name: String },
}

pub fn run(ctx: &Checkout, args: &[OsString]) -> Result<std::process::ExitCode> {
    use clap::Parser as _;
    use_shared_store(ctx)?;
    if matches!(args, [only] if only == oer_command_tree::REQUEST) {
        return command_tree(ctx);
    }
    let (owner, command) = match args.first().and_then(|argument| argument.to_str()) {
        // The stand's help, then the runner's.
        None | Some("help" | "--help" | "-h") => {
            println!("{STAND_HELP}");
            (None, HilCommand::Runner(args.to_vec()))
        }
        _ => {
            let cli = HilCli::try_parse_from(args)?;
            (
                cli.owner,
                cli.command.unwrap_or(HilCommand::Runner(Vec::new())),
            )
        }
    };
    let options = LeaseOptions { owner };
    match command {
        HilCommand::Queue { json } => queue(json),
        HilCommand::Dashboard { port } => {
            crate::dashboard::serve(&oer_hil_run_bundle::RunStore::shared()?, port)
        }
        HilCommand::Lease(cli) => lease(ctx, options, cli),
        HilCommand::Board(BoardCli { command }) => {
            crate::board::board(ctx, options.owner(ctx)?, command)
        }
        HilCommand::Peer(cli) => crate::board::peer(ctx, options.owner(ctx)?, cli),
        HilCommand::Preempt { id, reason } => preempt(&options.owner(ctx)?, &id, &reason),
        HilCommand::Owner { command } => checkout_owner(ctx, command),
        HilCommand::Devices(cli) => devices(ctx, &options, cli),
        HilCommand::Stand(cli) => crate::stand::stand(ctx, || options.owner(ctx), cli),
        HilCommand::Fixtures => {
            let lab = oer_hil_lab::config::LabConfig::default_path()?;
            print!(
                "{}",
                oer_hil_stand_host::fixtures::describe(&oer_hil_stand_host::fixtures::probe(&lab))
            );
            Ok(std::process::ExitCode::SUCCESS)
        }
        HilCommand::Firmware(cli) => firmware(ctx, &options, cli),
        HilCommand::Flash(cli) => crate::flash::run(ctx, options.owner(ctx)?, cli),
        HilCommand::Runs(cli) => runs(ctx, &options, cli),
        HilCommand::Evidence(cli) => crate::evidence::command(ctx, cli),
        HilCommand::Perf(cli) => perf(ctx, &options, cli),
        HilCommand::Profile(cli) => profile(ctx, cli),
        HilCommand::Wait { service: true, ids } => wait_for_service(&ids),
        HilCommand::Wait {
            service: false,
            ids,
        } => wait(&ids),
        // A job like a run: its record keeps its own arguments.
        HilCommand::Ab(_) => ab(ctx, options, args),
        HilCommand::Bisect(cli) => crate::experiments::bisect(ctx, &options.owner(ctx)?, cli),
        HilCommand::Runner(args) => runner(ctx, options, args),
    }
}

/// A runner command: its lease and job options are the stand's, taken from
/// among its arguments; the rest go to the runner.
fn runner(
    ctx: &Checkout,
    options: LeaseOptions,
    args: Vec<OsString>,
) -> Result<std::process::ExitCode> {
    let (late, args) = crate::jobs::take::<crate::jobs::RunnerOptions>(args)?;
    let options = options.merged(late.owner)?;
    let enqueue = late.job.enqueue;
    let after = late.job.dependency();
    if (enqueue || after.is_some()) && !produces_runs(&args) {
        return Err("--enqueue and --after apply to run and run-all".into());
    }
    if enqueue {
        let frozen = freeze_enqueued(ctx, &args)?;
        let id = crate::jobs::enqueue(ctx, &options.owner(ctx)?, &args, after, &frozen)?;
        eprintln!("hil: enqueued job {id}; `cargo hil wait {id}` blocks until it ends");
        println!("{id}");
        return Ok(std::process::ExitCode::SUCCESS);
    }
    let owner = options
        .owner(ctx)
        .unwrap_or_else(|_| String::from("unregistered"));
    // Only a command that produces runs is a job; a read-only one is not.
    let mut job = if produces_runs(&args) {
        Some(oer_hil_experiment::job::Running::begin(
            &ctx.root,
            &owner,
            &args,
            after.as_ref(),
        )?)
    } else {
        None
    };
    let args = args.as_slice();
    // An enqueued job runs the runner and sources fixed when it was
    // enqueued; the evidence decision below still reads its own arguments.
    let frozen = crate::jobs::Frozen::inherited()?;
    let runner = match &frozen {
        Some(frozen) => oer_hil_experiment::launch::Runner {
            executable: frozen.runner.clone(),
            receipt: Some(frozen.receipt.clone()),
        },
        None => oer_hil_experiment::launch::Runner::prepare(&ctx.root)?,
    };
    let mut runner_args = match frozen
        .as_ref()
        .and_then(|frozen| frozen.snapshot.as_deref())
    {
        Some(snapshot) => with_source_snapshot(args, snapshot),
        None => args.to_vec(),
    };
    apply_quarantine(&mut runner_args)?;
    if hands_off_terminal(args) {
        // Fixture installation ends in a foreground sudo handoff. A supervised
        // child runs in its own process group, which is a background group for
        // the terminal, so sudo could not read the password. Replace this
        // process instead: the runner keeps the foreground group and streams.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            let mut command = ctx.command(&runner.executable);
            command.args(&runner_args).envs(options.environment(ctx)?);
            if let Some(receipt) = &runner.receipt {
                command.env(oer_hil_observer::receipt::ENV, receipt);
            }
            let error = command.exec();
            return Err(format!("cannot hand the terminal to the HIL runner: {error}").into());
        }
    }
    let mut launch = oer_hil_experiment::launch::Launch::new(&ctx.root, &runner).args(runner_args);
    for (name, value) in options.environment(ctx)? {
        launch = launch.env(name, value);
    }
    // The runner's stand requests are the job's tickets.
    if let Some(job) = &job {
        launch = launch.env(oer_hil_arbiter::jobs::JOB_ENV, job.id());
    }
    let launched = oer_hil_experiment::launch::launch_run(&launch)?;
    if produces_runs(args) {
        // Name the runs in the receipt of whatever invoked this command.
        oer_hil_run_bundle::receipt::record(&launched.runs)?;
        let store = oer_hil_run_bundle::RunStore::shared()?;
        if let Some(job) = job.as_mut() {
            job.finish_with(&store, &launched.runs)?;
        }
        let created = runs_dirty(&store, &launched.runs);
        if std::env::var_os(oer_hil_run_bundle::experiment::EXPERIMENT_ENV).is_some() {
            eprintln!("hil: an A/B experiment run is diagnostic: no evidence recorded");
        } else {
            let flag = |name: &str| {
                args.iter().any(|arg| {
                    arg.to_str()
                        .is_some_and(|arg| arg == name || arg.starts_with(&format!("{name}=")))
                })
            };
            match evidence_skip_reason(
                RunInputs {
                    untracked_sources: flag("--source-include")
                        || flag("--source-snapshot")
                        || flag("--include-untracked"),
                    reduced_repetitions: flag("--repetitions"),
                },
                &created,
            ) {
                None => remember_pending(ctx, &store, options.owner(ctx)?, &launched.runs)?,
                // A runner that created no run has said why itself.
                Some(_) if created.is_empty() => {}
                Some(reason) => eprintln!(
                    "hil: HIL evidence not noted as pending: {reason}; \
                     `cargo qualification hil-evidence --hil-target CHIP --run ID` records it"
                ),
            }
        }
        if let Err(error) = prune_automatically(ctx, &store) {
            eprintln!("hil: automatic pruning of the run store failed: {error}");
        }
    }
    Ok(oer_process::exit_code(launched.status))
}

/// Stand commands handled here, printed before the runner's own help.
pub(crate) const STAND_HELP: &str = "\
Stand commands (shared by every checkout of this user):
  cargo hil perf report|baseline|check   gated measurements per commit, baselines, regressions
  cargo hil profile RUN [--scenario S] [--repetition N] [--top N]   symbolized program-counter profiles
  cargo hil bisect --good A --bad B --scenario S [--layout-seed N]   first commit at which S stops passing
  cargo hil ab --a VARIANT --b VARIANT --scenario S [--repetitions N] [--layout-seeds K] [--enqueue] [--after JOB]
                                      A/B comparison with noise-aware verdicts; --enqueue makes it a job
  cargo hil run ... --enqueue [--after JOB]   start the run detached as a job and print its id
  cargo hil run S... --repetitions N  each scenario N times instead of its own count; never evidence
  cargo hil wait --service [BOARD...] block until the boards (all when none) and the stand are in service
  cargo hil wait JOB|RUN              block until the job or run ends; exit 0 passed, 1 failed, 2 interrupted, 3 blocked, 4 broken, 5 no run, 6 abandoned
  cargo hil queue [--json]            holders, balances, queue with expected starts, boards, recent leases
  cargo hil board reset BOARD [--via rts|jtag|download|power]   reset under a lease; prints the ROM reset line
  cargo hil board check BOARD         attached, firmware, maintenance, reset paths, whether it answers; no reset
  cargo hil board console BOARD [--for 10s] [--until TEXT]       the console without a reset, under a lease
  cargo hil board soak BOARD --cycles N|--for 8h [--via rts,jtag,power]   reset again and again; journal the result
  cargo hil peer send BOARD LINE... [--for 5s]                   one peer text-protocol command and its answer
  cargo hil owner [set NAME]          this checkout's owner: stand, wifi, phy, bluetooth, bluetooth-hil, blobray, infra, 802154, esp32c5, network
  cargo hil preempt ID --reason TEXT   stop another owner's lease: charged no longer, SIGTERM with
                                      cleanup, SIGKILL after 5m; the history and the owner see why
  cargo hil dashboard [--port 8765]   live page of the queue, boards, runs and leases on 127.0.0.1
  cargo hil lease [OPTIONS] -- CMD    run CMD under one lease; nested cargo hil joins it
      --board NAME|MAC                boards CMD uses (repeatable)
      --stand                         claim the whole stand instead; blocks every other owner
      --air shared|exclusive|none     radio environment; exclusive for RF measurements, none without radio
      --flashed IMAGE (--application FILE | --sha256 HASH) --device NAME|MAC
      [--commit REV]                  journal CMD's flash when it succeeds
  cargo hil fixtures                  host Wi-Fi radios, Bluetooth adapter and OpenWrt hosts: key, interfaces, channel, CCA busy
  cargo hil devices [--json]          the stand file's boards: name, chip, port, health, last firmware
  cargo hil stand discover [--blink HUB:PORT | --verify-power BOARD]   attached boards against the stand file
  cargo hil stand doctor              the stand file, uhubctl without sudo, NetworkManager leaving wlan0
  cargo hil [--owner NAME] devices maintenance BOARD|--stand --reason TEXT   only NAME may claim BOARD (or the stand) until release; other runs wait
  cargo hil devices release BOARD [--confirm reset|power-cycle|rom-answers]
  cargo hil runs list [--scenario S] [--outcome O] [--image I] [--since 3d]
  cargo hil runs why RUN              why a run did not pass: failure, missed criteria, log tail
  cargo hil runs compare A B [--measurement TEXT]   measurements side by side, judged against their noise
  cargo hil runs history SCENARIO [--measurement TEXT]
  cargo hil runs pin RUN --reason TEXT | unpin RUN
  cargo hil runs prune [--days 30] [--keep-failed 5] [--apply]
  cargo hil evidence pending          clean runs whose evidence is not recorded yet
  cargo hil evidence dismiss --run ID ...   drop runs whose evidence will not be recorded
  cargo hil firmware list             tracked ESP-IDF images (peers, vendor references)
  cargo hil firmware build IMAGE      build against the one pinned ESP-IDF
  cargo hil firmware flash IMAGE --board NAME|MAC [--jtag] [--if-changed]   build, then the flash operation
  cargo hil flash --board NAME|MAC [--image NAME] [--monitor 30s [--until TEXT]] [--air shared|exclusive|none] [--via usb|jtag] BUNDLE|ELF
                                      the flash operation (lease, write, journal, start) on one board,
                                      then capture the console for a bounded time

Lease options, before any HIL command, after `lease`, or among a runner command's arguments:
  --owner NAME     default: enclosing lease owner, else the checkout directory name

Leases on different boards run in parallel. There is no budget to request:
every lease charges its owner's balance the time it holds, and waiting for a
lease that blocks you credits your balance the time waited. Among
conflicting requests the owner with the highest balance goes first, the
earlier request on a tie; balances halve every 2h and stay within 1h either
way. A run of several scenarios that has held 10m yields after its current
scenario to a waiter with a higher balance and queues again; a single
scenario or lease command runs to its end. Every lease ends at 1h (lease
exit status 124). `cargo hil queue` shows every balance and who goes next.
Scenarios tagged `air-exclusive` claim the air exclusively.

Runs never write tracked files: a clean run's passed scenarios become this
checkout's pending evidence, recorded by `cargo qualification hil-evidence
--hil-target CHIP --pending`; record any other run, such as one from a dirty
tree, with `--run ID` instead of `--pending`.

Runner commands (`cargo hil run A B C` runs several scenarios under one lease):";

/// How the stand orders conflicting requests, for `cargo hil queue --help`.
const BALANCE_RULE: &str = "\
Who goes next: every lease charges its owner's balance the time it holds, and
waiting behind a conflicting lease credits the time waited. The owner with the
highest balance is served first, the earlier request on a tie; balances halve
every 2h and stay within 1h either way. A run of several scenarios that has
held 10m yields after its current scenario to a waiter with a higher balance.
Every lease ends at 1h. There is no budget to request.";

/// Stand lease options accepted before the HIL command, or after `lease`,
/// and after a runner command.
#[derive(Clone, Debug, Default, PartialEq)]
struct LeaseOptions {
    owner: Option<String>,
}

impl LeaseOptions {
    /// These options with the owner `late` names among a runner command's
    /// arguments; two different owners are refused.
    fn merged(self, late: Option<String>) -> Result<Self> {
        match (self.owner, late) {
            (Some(earlier), Some(late)) if earlier != late => {
                Err(format!("--owner is given twice, as {earlier} and {late}").into())
            }
            (earlier, late) => Ok(Self {
                owner: late.or(earlier),
            }),
        }
    }

    /// Explicit options win; otherwise an enclosing lease's owner, otherwise
    /// the owner registered for this checkout. Nothing is derived from the
    /// checkout's directory name.
    fn owner(&self, ctx: &Checkout) -> Result<String> {
        if let Some(owner) = self.owner.clone().or_else(|| {
            std::env::var(oer_hil_arbiter::OWNER_ENV)
                .ok()
                .filter(|owner| !owner.is_empty())
        }) {
            return Ok(owner);
        }
        let arbiter = oer_hil_arbiter::Arbiter::open()?;
        if let Some(owner) = arbiter.checkout_owner(&ctx.root)? {
            return Ok(owner.id().to_owned());
        }
        // A worktree added from a registered checkout acts for that
        // checkout's owner until it registers one of its own.
        if let Some(main) = main_checkout(&ctx.root)
            && main != ctx.root
            && let Some(owner) = arbiter.checkout_owner(&main)?
        {
            return Ok(owner.id().to_owned());
        }
        Err(oer_hil_arbiter::NoOwner(ctx.root.clone()).into())
    }

    /// The owner for the runner, when one is known; a runner that leases
    /// without one is refused by the arbiter, one that does not lease needs
    /// none.
    fn environment(&self, ctx: &Checkout) -> Result<Vec<(&'static str, String)>> {
        Ok(self
            .owner(ctx)
            .ok()
            .map(|owner| (oer_hil_arbiter::OWNER_ENV, owner))
            .into_iter()
            .collect())
    }
}

/// `cargo hil __command-tree`: every `cargo hil` command path with its
/// subcommands and long flags, as JSON, for checking documentation against
/// the real command line: the stand's parser ([`HilCli`]) walked, and the
/// runner's own tree merged under `hil` with the options the stand takes
/// from a runner command ([`crate::jobs::RunnerOptions`]).
fn command_tree(ctx: &Checkout) -> Result<std::process::ExitCode> {
    use clap::{Args as _, CommandFactory as _};
    use oer_command_tree::{CommandNode, command_tree as walk};
    let runner = oer_hil_observer::prepare::prepare(&ctx.root)?.runner;
    let output = std::process::Command::new(&runner)
        .arg(oer_command_tree::REQUEST)
        .output()?;
    let mut runner_nodes: Vec<CommandNode> = serde_json::from_slice(&output.stdout)?;
    let runner_top = runner_nodes
        .first()
        .map(|root| root.subcommands.clone())
        .unwrap_or_default();
    let stand_options = walk(
        &crate::jobs::RunnerOptions::augment_args(clap::Command::new("options")),
        &[],
    )
    .remove(0)
    .flags;
    for node in &mut runner_nodes {
        if matches!(node.path.as_slice(), [one] if produces_runs(&[OsString::from(one)])) {
            node.flags.extend(stand_options.iter().cloned());
        }
        node.path.insert(0, String::from("hil"));
    }
    let mut nodes = walk(&HilCli::command(), &[String::from("hil")]);
    nodes[0].subcommands.extend(runner_top);
    nodes.extend(runner_nodes.into_iter().skip(1));
    println!("{}", serde_json::to_string_pretty(&nodes)?);
    Ok(std::process::ExitCode::SUCCESS)
}

/// `cargo hil owner [set NAME | merge OLD NEW | forget NAME]`.
fn checkout_owner(ctx: &Checkout, command: Option<OwnerCommand>) -> Result<std::process::ExitCode> {
    let arbiter = oer_hil_arbiter::Arbiter::open()?;
    match command {
        None => match arbiter.checkout_owner(&ctx.root)? {
            Some(owner) => println!("{owner}"),
            None => return Err(oer_hil_arbiter::NoOwner(ctx.root.clone()).into()),
        },
        Some(OwnerCommand::Set { name }) => {
            let owner = oer_hil_arbiter::Owner::parse(&name)?;
            arbiter.set_checkout_owner(&ctx.root, owner)?;
            println!("{} is owned by {owner}", ctx.root.display());
        }
        Some(OwnerCommand::Merge { old, new }) => {
            let new = oer_hil_arbiter::Owner::parse(&new)?;
            arbiter.merge_owner(&old, new)?;
            println!("{old}'s balance and history are {new}'s");
        }
        Some(OwnerCommand::Forget { name }) => {
            let forgotten = arbiter.forget_owner(&name)?;
            println!(
                "{name}: {}",
                if forgotten {
                    "balance dropped"
                } else {
                    "no balance"
                }
            );
        }
    }
    Ok(std::process::ExitCode::SUCCESS)
}

/// Time a preempted holder gets for cancellation and cleanup, as at the hard
/// limit.
const PREEMPT_GRACE: std::time::Duration = std::time::Duration::from_secs(300);

/// `cargo hil preempt ID --reason TEXT`.
fn preempt(owner: &str, id: &str, reason: &str) -> Result<std::process::ExitCode> {
    let id = id
        .trim_start_matches('#')
        .parse::<u64>()
        .map_err(|_| format!("{id} is not a lease number"))?;
    let end = oer_hil_arbiter::Arbiter::open()?.preempt(id, owner, reason, PREEMPT_GRACE)?;
    println!(
        "lease #{id} {}",
        match end {
            oer_hil_arbiter::preempt::PreemptEnd::Released => "released after SIGTERM",
            oer_hil_arbiter::preempt::PreemptEnd::Killed => {
                "did not release within the grace and was killed"
            }
        }
    );
    Ok(std::process::ExitCode::SUCCESS)
}

/// Print the stand's holder, queue, board state and recent leases.
fn queue(json: bool) -> Result<std::process::ExitCode> {
    let arbiter = oer_hil_arbiter::Arbiter::open()?;
    let status = arbiter.status()?;
    let store = arbiter.jobs();
    let jobs = store.unfinished();
    let ended = store.recently_ended_unjudged(std::time::Duration::from_secs(3600), 5);
    if json {
        let mut value = serde_json::to_value(&status)?;
        value["jobs"] = serde_json::to_value(oer_hil_arbiter::jobs::views(&jobs, &status))?;
        value["ended_jobs"] = serde_json::to_value(&ended)?;
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("{status}");
        print!("{}", oer_hil_arbiter::jobs::describe(&jobs, &status));
        print!("{}", oer_hil_arbiter::jobs::describe_ended(&ended));
    }
    Ok(std::process::ExitCode::SUCCESS)
}

/// `cargo hil ab ...`, a job like a run: `--enqueue` starts it detached and
/// prints its id for `cargo hil wait`, and `--after JOB` orders it. The job
/// records the command's own arguments, `args` without the stand's options.
fn ab(ctx: &Checkout, options: LeaseOptions, args: &[OsString]) -> Result<std::process::ExitCode> {
    use clap::Parser as _;
    let (stand, args) = crate::jobs::take::<crate::jobs::RunnerOptions>(args.to_vec())?;
    let owner = options.merged(stand.owner)?.owner(ctx)?;
    let after = stand.job.dependency();
    if stand.job.enqueue {
        let frozen = crate::jobs::Frozen::capture(ctx)?;
        let id = crate::jobs::enqueue(ctx, &owner, &args, after, &frozen)?;
        eprintln!("hil: enqueued job {id}; `cargo hil wait {id}` blocks until it ends");
        println!("{id}");
        return Ok(std::process::ExitCode::SUCCESS);
    }
    let Some(HilCommand::Ab(cli)) = HilCli::try_parse_from(&args)?.command else {
        return Err("cargo hil ab: not an A/B comparison".into());
    };
    let mut job =
        oer_hil_experiment::job::Running::begin(&ctx.root, &owner, &args, after.as_ref())?;
    // Every round runs the runner of the experiment's start: a pull into the
    // checkout meanwhile must not change the arms' protocol.
    let frozen = match crate::jobs::Frozen::inherited()? {
        Some(frozen) => frozen,
        None => crate::jobs::Frozen::capture(ctx)?,
    };
    let runner = oer_hil_experiment::launch::Runner {
        executable: frozen.runner,
        receipt: Some(frozen.receipt),
    };
    let runs = crate::experiments::ab(ctx, &owner, &runner, cli)?;
    let (ids, outcomes): (Vec<_>, Vec<_>) = runs.into_iter().unzip();
    job.finish(&ids, &outcomes)?;
    Ok(std::process::ExitCode::SUCCESS)
}

/// `cargo hil wait ID`: block until the job or run ID names ends, and exit
/// with its outcome. A job is waited for through its record, a run by
/// following its bundle.
fn wait(ids: &[String]) -> Result<std::process::ExitCode> {
    let [id] = ids else {
        return Err("usage: cargo hil wait JOB|RUN, or cargo hil wait --service [BOARD...]".into());
    };
    if oer_hil_arbiter::Arbiter::open()?.jobs().read(id).is_ok() {
        return crate::jobs::wait_command(id);
    }
    let run = oer_hil_analysis::Run::open(&oer_hil_run_bundle::RunStore::shared()?, id)
        .map_err(|_| format!("{id} is neither a job nor a run"))?;
    Ok(std::process::ExitCode::from(oer_hil_analysis::runs::wait(
        run.directory(),
        |line| println!("{line}"),
    )?))
}

/// `cargo hil wait --service [BOARD...]`: block until the boards (every
/// board when none is named) and the stand are back in service.
fn wait_for_service(boards: &[String]) -> Result<std::process::ExitCode> {
    let arbiter = oer_hil_arbiter::Arbiter::open()?;
    let stand = arbiter.stand()?;
    let macs = boards
        .iter()
        .map(|board| Ok(stand.resolve(board)?.mac()?))
        .collect::<Result<Vec<_>>>()?;
    arbiter.wait_for_service(&macs, |out| {
        for entry in out {
            eprintln!(
                "hil: waiting: {} is {} by {}: {}",
                if entry.mac == oer_hil_arbiter::STAND_SERVICE {
                    "the stand"
                } else {
                    entry.mac.as_str()
                },
                match entry.kind {
                    oer_hil_arbiter::ServiceKind::Maintenance => "under maintenance",
                    oer_hil_arbiter::ServiceKind::Quarantine => "quarantined",
                },
                entry.owner,
                entry.reason
            );
        }
    })?;
    println!("in service");
    Ok(std::process::ExitCode::SUCCESS)
}

/// Exit status of a lease command terminated at the hard limit.
const HARD_LIMIT_EXIT: u8 = 124;

/// How a lease command uses the radio environment; `None` claims no air.
#[derive(Clone, Copy, Debug, PartialEq)]
struct AirArg(Option<oer_hil_arbiter::Mode>);

fn parse_air(text: &str) -> std::result::Result<AirArg, String> {
    match text {
        "shared" => Ok(AirArg(Some(oer_hil_arbiter::Mode::Shared))),
        "exclusive" => Ok(AirArg(Some(oer_hil_arbiter::Mode::Exclusive))),
        "none" => Ok(AirArg(None)),
        _ => Err(String::from("use `shared`, `exclusive` or `none`")),
    }
}

/// The resources of a lease command: its boards and the air, or the whole
/// stand when it names neither.
fn lease_claims(
    boards: &[String],
    air: Option<AirArg>,
    stand: bool,
    file: &oer_hil_stand_model::StandFile,
) -> Result<Vec<oer_hil_arbiter::Claim>> {
    match (stand, boards.is_empty() && air.is_none()) {
        (true, true) => return Ok(vec![oer_hil_arbiter::Claim::stand()]),
        (true, false) => return Err("--stand claims everything; drop --board and --air".into()),
        // A whole-stand lease blocks every other owner, so it is never the
        // accidental result of naming nothing.
        (false, true) => {
            return Err(
                "name what the command uses with --board NAME|MAC (repeatable) and \
                        --air, or claim the whole stand with --stand"
                    .into(),
            );
        }
        (false, false) => {}
    }
    let mut claims = boards
        .iter()
        .map(|board| Ok(oer_hil_arbiter::Claim::board(&file.resolve(board)?.mac()?)))
        .collect::<Result<Vec<_>>>()?;
    // Boards use the air shared unless told otherwise.
    if let Some(mode) = air.map_or(Some(oer_hil_arbiter::Mode::Shared), |AirArg(mode)| mode) {
        claims.push(oer_hil_arbiter::Claim {
            resource: oer_hil_arbiter::AIR.to_owned(),
            mode,
        });
    }
    if claims.is_empty() {
        return Err("--air none needs --board: the lease would claim nothing".into());
    }
    Ok(claims)
}

/// The `cargo hil` arguments of a leased command that is itself a stand
/// command, through `cargo hil` or the installed `oer-stand`.
fn nested_hil(program: &OsString, arguments: &[OsString]) -> Option<Vec<OsString>> {
    let name = Path::new(program).file_name()?.to_str()?;
    match (
        name,
        arguments.first().and_then(|argument| argument.to_str()),
    ) {
        ("cargo", Some("hil")) => Some(arguments[1..].to_vec()),
        ("oer-stand", _) => Some(arguments.to_vec()),
        _ => None,
    }
}

/// Parse `cargo hil` arguments as the stand command they name will, without
/// running it. Commands the runner parses itself pass unchecked here.
fn check_hil_arguments(args: &[OsString]) -> Result<()> {
    use clap::Parser as _;
    HilCli::try_parse_from(args).map(drop).map_err(|error| {
        format!(
            "`cargo hil {}`: {}",
            args.iter()
                .map(|argument| argument.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" "),
            error.render().to_string().trim()
        )
        .into()
    })
}

fn parse_budget(text: &str) -> std::result::Result<std::time::Duration, String> {
    oer_hil_arbiter::parse_duration(text).map_err(|error| error.to_string())
}

/// `cargo hil lease [OPTIONS] -- COMMAND...`
#[derive(Debug, clap::Args)]
struct LeaseCli {
    /// Who holds the lease.
    #[arg(long)]
    owner: Option<String>,
    /// A board the command uses, by registered name, chip or MAC;
    /// repeatable.
    #[arg(long = "board", value_name = "NAME|MAC")]
    boards: Vec<String>,
    /// Claim the whole stand: every board, fixture and the air.
    #[arg(long)]
    stand: bool,
    /// How the command uses the radio environment: `shared` (default with
    /// boards), `exclusive` for RF measurements, or `none` for work that
    /// never enables a radio, which then runs beside an exclusive air lease.
    #[arg(long, value_parser = parse_air)]
    air: Option<AirArg>,
    /// Journal a flash that COMMAND performs when it succeeds.
    #[command(flatten)]
    flashed: FlashedArgs,
    #[arg(last = true, required = true)]
    command: Vec<OsString>,
}

/// A flash a leased command performed outside the flash operation,
/// recorded in the board journal when the command succeeds.
#[derive(Clone, Debug, Default, clap::Args)]
struct FlashedArgs {
    /// Name of the flashed image, e.g. `ieee802154-peer`.
    #[arg(long = "flashed")]
    image: Option<String>,
    /// The flashed application binary, whose SHA-256 is recorded.
    #[arg(long, requires = "image", conflicts_with = "sha256")]
    application: Option<std::path::PathBuf>,
    /// SHA-256 of the flashed application instead of `--application`.
    #[arg(long, requires = "image")]
    sha256: Option<String>,
    /// The flashed board: its stand-file id, its chip or its MAC.
    #[arg(long, value_name = "NAME|MAC", requires = "image")]
    device: Option<String>,
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
        let sha256 = match (&self.application, &self.sha256) {
            (Some(application), None) => oer_durable::sha256_file(application)?,
            (None, Some(hash)) => hash.clone(),
            _ => return Err("a recorded flash needs --application or --sha256".into()),
        };
        let device = self
            .device
            .as_deref()
            .ok_or("a recorded flash needs --device NAME|MAC")?;
        let mac = arbiter.stand()?.resolve(device)?.mac()?;
        arbiter.record_flash(
            owner,
            &mac,
            &oer_hil_arbiter::ImageIdentity {
                name: image.clone(),
                sha256,
                commit: self.commit.clone(),
                dirty: None,
                origin,
            },
        )?;
        eprintln!("hil-arbiter: recorded {image} on {mac}");
        Ok(())
    }
}

/// Why a lease refuses `command`: a hub port is switched only by a power
/// cycle of a board of the stand file (`cargo hil board reset BOARD --via
/// power`), never by `uhubctl` in a leased command, which could leave a port
/// off. The program is matched, directly or as a command of a shell's `-c`
/// script; an argument that only names it (`grep uhubctl`) passes.
fn switches_hub_power(command: &[OsString]) -> Option<String> {
    let is_uhubctl = |word: &str| {
        Path::new(word)
            .file_name()
            .is_some_and(|name| name == "uhubctl")
    };
    let (program, arguments) = command.split_first()?;
    let program = program.to_string_lossy();
    let shell = matches!(
        Path::new(program.as_ref())
            .file_name()
            .and_then(|name| name.to_str()),
        Some("sh" | "bash" | "zsh" | "fish" | "dash")
    );
    let script_runs_it = || {
        arguments
            .iter()
            .skip_while(|argument| *argument != "-c")
            .nth(1)
            .is_some_and(|script| {
                script
                    .to_string_lossy()
                    .split([';', '&', '|', '\n', '(', ')', '`'])
                    .filter_map(|segment| {
                        segment.split_whitespace().find(|word| {
                            !word.contains('=')
                                && !matches!(*word, "sudo" | "env" | "exec" | "command")
                        })
                    })
                    .any(is_uhubctl)
            })
    };
    (is_uhubctl(&program) || (shell && script_runs_it())).then(|| {
        "a lease runs no `uhubctl`: cycle a board's hub port with \
         `cargo hil board reset BOARD --via power`"
            .to_owned()
    })
}

/// Run one command, typically a series of HIL commands, under one lease.
/// Nested `cargo hil` commands join the lease instead of queueing.
fn lease(ctx: &Checkout, outer: LeaseOptions, cli: LeaseCli) -> Result<std::process::ExitCode> {
    let options = LeaseOptions {
        owner: cli.owner.or(outer.owner),
    };
    let (program, arguments) = cli
        .command
        .split_first()
        .ok_or("cargo hil lease needs a COMMAND after --")?;
    if let Some(refusal) = switches_hub_power(&cli.command) {
        return Err(refusal.into());
    }
    // A mistake in a nested stand command fails now, not after the wait.
    if let Some(nested) = nested_hil(program, arguments) {
        check_hil_arguments(&nested).map_err(|error| format!("not leased: {error}"))?;
    }
    let work = cli
        .command
        .iter()
        .map(|argument| argument.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    let arbiter = oer_hil_arbiter::Arbiter::open()?;
    let request = oer_hil_arbiter::Request {
        owner: options.owner(ctx)?,
        work,
        scenarios: Vec::new(),
        claims: lease_claims(&cli.boards, cli.air, cli.stand, &arbiter.stand()?)?,
    };
    let grant = arbiter.acquire(&request)?;
    let mut child = oer_process::owned::Child::spawn_with_shutdown_grace(
        ctx.command(program)
            .args(arguments)
            .env(oer_hil_arbiter::OWNER_ENV, &request.owner)
            .envs(grant.environment()),
        std::time::Duration::from_secs(300),
    )?;
    let (code, succeeded) = supervise(&grant, &mut child, oer_hil_arbiter::HARD_LIMIT)?;
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

/// Supervise an indivisible command: it runs to its end, charged the time it
/// holds, and is terminated only at the hard limit every lease has. Returns
/// the exit code and whether the command succeeded.
fn supervise(
    grant: &oer_hil_arbiter::Grant,
    child: &mut oer_process::owned::Child,
    limit: std::time::Duration,
) -> Result<(std::process::ExitCode, bool)> {
    let finished =
        |status: std::process::ExitStatus| (oer_process::exit_code(status), status.success());
    if grant.is_nested() {
        return Ok(finished(child.wait_forwarding_cancellation()?));
    }
    let started = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(finished(status));
        }
        if oer_process::cancellation_requested() {
            child.kill()?;
            return Ok((std::process::ExitCode::from(130), false));
        }
        if started.elapsed() >= limit {
            grant.mark_hard_limit();
            child.kill()?;
            return Ok((std::process::ExitCode::from(HARD_LIMIT_EXIT), false));
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// `cargo hil runs list|why|compare|history|pin|unpin|prune` over the shared
/// run store.
/// `cargo hil perf`: gated measurements across commits and their baselines.
fn perf(ctx: &Checkout, options: &LeaseOptions, cli: PerfCli) -> Result<std::process::ExitCode> {
    use oer_hil_analysis::perf;
    let store = oer_hil_run_bundle::RunStore::shared()?;
    let now = oer_durable::unix_millis();
    let find = |id: &str| -> Result<perf::RunSummary> {
        Ok(perf::summary(&oer_hil_analysis::Run::open(&store, id)?))
    };
    let baselines = perf::baselines(&store)?;
    match cli {
        PerfCli::Report {
            scenarios,
            measurement,
            since,
        } => {
            let runs = perf::summaries_since(&store, now.saturating_sub(since.as_millis() as u64))?;
            print!(
                "{}",
                perf::report(&runs, &scenarios, measurement.as_deref(), &baselines)
            );
        }
        PerfCli::Baseline {
            run,
            scenarios,
            reason,
        } => {
            let run = find(&run)?;
            let reason = if run.networks.is_empty() {
                reason
            } else {
                format!("{reason} [network {}]", run.networks.join(","))
            };
            let set =
                perf::set_baseline(&store, &run, &scenarios, &reason, &options.owner(ctx)?, now)?;
            println!("baseline {} for {}", run.id, set.join(", "));
        }
        PerfCli::Check { run } => {
            let regressions = perf::regressions(&find(&run)?, &baselines);
            if regressions.is_empty() {
                println!("no gated measurement of {run} regressed against its baseline");
            } else {
                for line in &regressions {
                    println!("REGRESSED {line}");
                }
                return Ok(std::process::ExitCode::FAILURE);
            }
        }
    }
    Ok(std::process::ExitCode::SUCCESS)
}

/// `cargo hil runs list|show|why|compare|history|pin|unpin|flaky|quarantine|release|prune`
/// over the shared run store.
fn runs(ctx: &Checkout, options: &LeaseOptions, parsed: RunsCli) -> Result<std::process::ExitCode> {
    use oer_hil_analysis::{Run, retention, runs};
    use oer_hil_run_bundle::store::{Note, Notes, Sidecar};
    let store = oer_hil_run_bundle::RunStore::shared()?;
    // Reading every run takes long; commands about one run read only it.
    let all = || Run::all(&store);
    let find = |id: &str| Run::open(&store, id);
    let now = oer_durable::unix_millis();
    let note = |reason: String| -> Result<Note> {
        Ok(Note {
            by: options.owner(ctx)?,
            reason,
            unix_millis: now,
        })
    };
    match parsed {
        RunsCli::List {
            scenario,
            outcome,
            image,
            since,
            limit,
        } => {
            let filter = runs::Filter {
                scenario,
                outcome,
                image,
                since_millis: since.map(|since| now.saturating_sub(since.as_millis() as u64)),
            };
            let all = all()?;
            let matching = all
                .iter()
                .filter(|run| filter.matches(run))
                .collect::<Vec<_>>();
            for run in &matching[matching.len().saturating_sub(limit)..] {
                println!("{}", runs::list_line(run));
            }
        }
        RunsCli::Show { run } => print!("{}", runs::show(&find(&run)?)),
        RunsCli::Why { run, tail } => print!("{}", runs::why(&find(&run)?, tail)),
        RunsCli::Compare { a, b, measurement } => print!(
            "{}",
            runs::compare(&find(&a)?, &find(&b)?, measurement.as_deref())
        ),
        RunsCli::History {
            scenario,
            measurement,
            limit,
        } => {
            let containing = all()?
                .into_iter()
                .filter(|run| run.scenario(&scenario).is_some())
                .collect::<Vec<_>>();
            let start = containing.len().saturating_sub(limit);
            print!(
                "{}{}",
                runs::stability(&containing, &scenario),
                runs::history(&containing[start..], &scenario, measurement.as_deref())
            );
        }
        RunsCli::Pin { run, reason } => {
            find(&run)?;
            store.note(Sidecar::Pins, &run, Some(note(reason)?))?;
        }
        RunsCli::Unpin { run } => store.note(Sidecar::Pins, &run, None)?,
        RunsCli::Flaky { since, minimum } => {
            let recent = now.saturating_sub(since.as_millis() as u64);
            let runs = all()?
                .into_iter()
                .filter(|run| run.started_millis() >= recent)
                .collect::<Vec<_>>();
            print!(
                "{}",
                runs::flaky_report(
                    &runs::stabilities(&runs),
                    minimum,
                    &store.read::<Notes>(Sidecar::Quarantine)?
                )
            );
        }
        RunsCli::Quarantine { scenario, reason } => {
            store.note(Sidecar::Quarantine, &scenario, Some(note(reason)?))?;
        }
        RunsCli::Release { scenario } => store.note(Sidecar::Quarantine, &scenario, None)?,
        RunsCli::Prune {
            days,
            keep_failed,
            apply,
        } => {
            let pruned = retention::prune(
                &store,
                &ctx.root,
                &retention::Retention {
                    keep_days: days,
                    keep_failed,
                },
                None,
                apply,
            )?;
            for (run, bytes) in &pruned.removed {
                if apply {
                    println!("deleted {} ({} MiB)", run.id(), bytes >> 20);
                } else {
                    println!("would delete {}", runs::list_line(run));
                }
            }
            if apply {
                if pruned.observers > 0 {
                    println!(
                        "deleted {} observer builds no kept run names",
                        pruned.observers
                    );
                }
                let objects = collect_objects(ctx);
                if objects.objects > 0 {
                    println!(
                        "deleted {} firmware objects no run links to ({} MiB)",
                        objects.objects,
                        objects.bytes >> 20
                    );
                }
            }
            println!(
                "{} {} of {} runs, {} MiB held only by them; {} kept{}",
                if apply { "deleted" } else { "would delete" },
                pruned.removed.len(),
                pruned.total,
                pruned.freed() >> 20,
                pruned.kept,
                if apply { "" } else { "; add --apply to delete" }
            );
        }
    }
    Ok(std::process::ExitCode::SUCCESS)
}

/// `cargo hil firmware list | build IMAGE | flash IMAGE --board BOARD`.
fn firmware(
    ctx: &Checkout,
    options: &LeaseOptions,
    cli: FirmwareCli,
) -> Result<std::process::ExitCode> {
    match cli {
        FirmwareCli::List => crate::firmware_catalog::list(ctx)?,
        FirmwareCli::Build { image } => {
            crate::firmware_catalog::build(ctx, &image)?;
        }
        FirmwareCli::Flash {
            image,
            board,
            jtag,
            if_changed,
        } => {
            let via = if jtag {
                oer_hil_board::Via::Jtag
            } else {
                oer_hil_board::Via::Usb
            };
            crate::firmware_catalog::flash(
                ctx,
                &image,
                &board,
                via,
                if_changed,
                options.owner(ctx)?,
            )?;
        }
    }
    Ok(std::process::ExitCode::SUCCESS)
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum ConfirmArg {
    Reset,
    PowerCycle,
    /// No person acted: the release check's RTS reset shows the ROM
    /// answering, so the board never needed one.
    RomAnswers,
}

/// `cargo hil devices [--json]` and its board commands.
fn devices(
    ctx: &Checkout,
    options: &LeaseOptions,
    cli: DevicesCli,
) -> Result<std::process::ExitCode> {
    let arbiter = oer_hil_arbiter::Arbiter::open()?;
    match cli.command {
        Some(DevicesCommand::Maintenance {
            board,
            stand,
            reason,
        }) => {
            let (board, mac) = match (board, stand) {
                (_, true) => (
                    String::from("the stand"),
                    String::from(oer_hil_arbiter::STAND_SERVICE),
                ),
                (Some(board), false) => {
                    let mac = arbiter.stand()?.resolve(&board)?.mac()?;
                    (board, mac)
                }
                (None, false) => unreachable!("clap requires a board or --stand"),
            };
            let owner = options.owner(ctx)?;
            arbiter.set_maintenance(oer_hil_arbiter::Maintenance {
                mac: mac.clone(),
                owner: owner.clone(),
                reason,
                since_unix: oer_durable::unix_seconds(),
                kind: oer_hil_arbiter::ServiceKind::Maintenance,
                trigger: None,
                evidence: None,
                unknown: Default::default(),
            })?;
            println!("{board} ({mac}) is under maintenance by {owner}");
            let claim = if mac == oer_hil_arbiter::STAND_SERVICE {
                oer_hil_arbiter::Claim::stand()
            } else {
                oer_hil_arbiter::Claim::board(&mac)
            };
            for holder in arbiter.conflicting_holders(&[claim])? {
                println!(
                    "still held by #{} {} `{}` until it ends",
                    holder.id, holder.owner, holder.work
                );
            }
            return Ok(std::process::ExitCode::SUCCESS);
        }
        Some(DevicesCommand::Release {
            board: None,
            stand: true,
            ..
        }) => {
            if arbiter.clear_maintenance(oer_hil_arbiter::STAND_SERVICE)? {
                println!("the stand is back in service");
            } else {
                println!("the stand was not under maintenance");
            }
            return Ok(std::process::ExitCode::SUCCESS);
        }
        Some(DevicesCommand::Release { board, confirm, .. }) => {
            let board = board.ok_or("name a board or pass --stand")?;
            let mac = arbiter.stand()?.resolve(&board)?.mac()?;
            if arbiter.is_quarantined(&mac)? {
                let confirmation = match confirm.ok_or(
                    "the board is quarantined: reset or power-cycle it, then pass \
                     --confirm reset|power-cycle; --confirm rom-answers returns a board whose \
                     ROM answers the stand's reset without a person",
                )? {
                    ConfirmArg::Reset => oer_hil_arbiter::Confirmation::Reset,
                    ConfirmArg::PowerCycle => oer_hil_arbiter::Confirmation::PowerCycle,
                    ConfirmArg::RomAnswers => oer_hil_arbiter::Confirmation::RomAnswers,
                };
                let answer =
                    arbiter.release_quarantine(&mac, &options.owner(ctx)?, confirmation, || {
                        crate::board::boots(ctx, &board)
                    })?;
                println!("{board} ({mac}) is back in service; it booted: {answer}");
                return Ok(std::process::ExitCode::SUCCESS);
            }
            if arbiter.clear_maintenance(&mac)? {
                println!("{board} ({mac}) is back in service");
            } else {
                println!("{board} ({mac}) was not under maintenance");
            }
            return Ok(std::process::ExitCode::SUCCESS);
        }
        None => {}
    }
    let status = arbiter.status()?;
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&status.devices)?);
    } else {
        for device in &status.devices {
            println!(
                "{} [{}]{}: {}",
                device.label,
                device.port.as_deref().unwrap_or("not attached"),
                device
                    .health
                    .as_ref()
                    .map(|health| format!(" (health: {health})"))
                    .unwrap_or_default(),
                device
                    .firmware
                    .as_ref()
                    .map_or_else(|| String::from("firmware unknown"), ToString::to_string)
            );
        }
    }
    Ok(std::process::ExitCode::SUCCESS)
}

/// Delete the firmware objects no run links to from this checkout's object
/// store and those of the other registered checkouts; a checkout that
/// cannot be collected is reported and skipped.
fn collect_objects(ctx: &Checkout) -> oer_hil_run_bundle::build::CollectedObjects {
    let mut checkouts = vec![ctx.root.clone()];
    if let Ok(arbiter) = oer_hil_arbiter::Arbiter::open()
        && let Ok(registered) = arbiter.registered_checkouts()
    {
        checkouts.extend(registered);
    }
    checkouts.sort();
    checkouts.dedup();
    let mut total = oer_hil_run_bundle::build::CollectedObjects::default();
    for checkout in checkouts {
        // Build objects stay per chip.
        let chips = oer_chip_profile::supported(&checkout).unwrap_or_default();
        for target in chips
            .iter()
            .map(|chip| checkout.join("target/hil").join(chip))
        {
            match oer_hil_run_bundle::build::collect_objects(&target) {
                Ok(collected) => {
                    total.objects += collected.objects;
                    total.bytes += collected.bytes;
                }
                Err(error) => eprintln!(
                    "hil: cannot collect the firmware objects of {}: {error}",
                    target.display()
                ),
            }
        }
    }
    total
}

/// Rule of the automatic pruning, which `cargo hil runs prune` defaults to.
const PRUNE_DAYS: u64 = 30;
const PRUNE_KEEP_FAILED: usize = 5;
/// Automatic pruning runs at most once per this interval.
const PRUNE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// Delete, at most once a day, the runs of the shared store that no rule
/// keeps; see `cargo hil runs prune`.
/// The run store's size budget: `OER_HIL_RUN_STORE_BUDGET_GIB` gibibytes,
/// 40 by default.
fn run_store_budget() -> Result<u64> {
    let gibibytes = match std::env::var(RUN_STORE_BUDGET_ENV) {
        Ok(text) => text
            .parse::<u64>()
            .map_err(|_| format!("{RUN_STORE_BUDGET_ENV} is not a number of GiB: {text}"))?,
        Err(_) => RUN_STORE_BUDGET_GIB,
    };
    Ok(gibibytes << 30)
}

const RUN_STORE_BUDGET_ENV: &str = "OER_HIL_RUN_STORE_BUDGET_GIB";
const RUN_STORE_BUDGET_GIB: u64 = 40;

fn prune_automatically(ctx: &Checkout, store: &oer_hil_run_bundle::RunStore) -> Result<()> {
    let marker = store.root().join("last-prune");
    if std::fs::metadata(&marker)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age < PRUNE_INTERVAL)
    {
        return Ok(());
    }
    std::fs::write(&marker, b"")?;
    let pruned = oer_hil_analysis::retention::prune(
        store,
        &ctx.root,
        &oer_hil_analysis::retention::Retention {
            keep_days: PRUNE_DAYS,
            keep_failed: PRUNE_KEEP_FAILED,
        },
        Some(run_store_budget()?),
        true,
    )?;
    let objects = collect_objects(ctx);
    if objects.objects > 0 {
        eprintln!(
            "hil: deleted {} firmware objects no run links to ({} MiB)",
            objects.objects,
            objects.bytes >> 20
        );
    }
    if !pruned.removed.is_empty() {
        eprintln!(
            "hil: pruned {} runs ({} MiB) no rule keeps; see `cargo hil runs prune`",
            pruned.removed.len(),
            pruned.freed() >> 20
        );
    }
    Ok(())
}

/// Make this checkout's run directory a link to the shared store.
fn use_shared_store(ctx: &Checkout) -> Result<()> {
    oer_hil_run_bundle::RunStore::shared()?.link(&ctx.root)?;
    Ok(())
}

/// Check an enqueued `run`'s scenarios and options with the runner now, so
/// a mistake shows in the terminal instead of in a job that ends no-run
/// minutes later.
/// Check an enqueued command and fix what it runs with: this binary, the
/// runner and, for a `run` that builds, a source snapshot of the checkout
/// taken now with the run's own source options.
fn freeze_enqueued(ctx: &Checkout, args: &[OsString]) -> Result<crate::jobs::Frozen> {
    let mut frozen = crate::jobs::Frozen::capture(ctx)?;
    if args.first().and_then(|arg| arg.to_str()) != Some("run") {
        return Ok(frozen);
    }
    let output = oer_process::output(
        ctx.command(&frozen.runner)
            .args(args)
            .arg("--validate-only"),
        Some(std::time::Duration::from_secs(120)),
    )?;
    if !output.status.success() {
        return Err(format!(
            "not enqueued: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    if has_flag(args, "--firmware-from") {
        return Ok(frozen);
    }
    if has_flag(args, "--source-snapshot") {
        build_before_queue(ctx, &frozen, args)?;
        return Ok(frozen);
    }
    let output = oer_process::output(
        ctx.command(&frozen.runner)
            .args(["image", "snapshot"])
            .args(source_options(args)),
        Some(std::time::Duration::from_secs(600)),
    )?;
    if !output.status.success() {
        return Err(format!(
            "not enqueued: the source snapshot failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let snapshot: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let directory = snapshot["directory"]
        .as_str()
        .ok_or("the source snapshot names no directory")?;
    frozen.snapshot = Some(PathBuf::from(directory));
    build_before_queue(
        ctx,
        &frozen,
        &with_source_snapshot(args, Path::new(directory)),
    )?;
    Ok(frozen)
}

/// Build and audit a run's images with its fixed runner before the job
/// waits: a build or audit that fails does so now, in the terminal, not
/// after the queue.
fn build_before_queue(
    ctx: &Checkout,
    frozen: &crate::jobs::Frozen,
    run_args: &[OsString],
) -> Result<()> {
    eprintln!("hil: building the run's images before it queues");
    let status = ctx
        .command(&frozen.runner)
        .args(run_args)
        .arg("--build-only")
        .status()?;
    if !status.success() {
        return Err("not enqueued: the run's images did not build (see above)".into());
    }
    Ok(())
}

/// The main checkout of the worktree at `root`: the directory holding the
/// repository's common `.git`.
fn main_checkout(root: &Path) -> Option<PathBuf> {
    let common = oer_process::git::text(
        root,
        ["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()?;
    checkout_of_common_dir(Path::new(&common))
}

/// The checkout whose `.git` directory is `common`.
fn checkout_of_common_dir(common: &Path) -> Option<PathBuf> {
    (common.file_name()? == ".git").then(|| common.parent().map(Path::to_owned))?
}

/// Leave quarantined scenarios out of a `run-all`, and warn of a `run` that
/// names one.
fn apply_quarantine(args: &mut Vec<OsString>) -> Result<()> {
    let quarantined: oer_hil_run_bundle::store::Notes = oer_hil_run_bundle::RunStore::shared()?
        .read(oer_hil_run_bundle::store::Sidecar::Quarantine)?;
    match args.first().and_then(|arg| arg.to_str()) {
        Some("run-all") => {
            for scenario in quarantined.keys() {
                args.push("--exclude".into());
                args.push(scenario.into());
            }
        }
        Some("run") => {
            for (scenario, entry) in &quarantined {
                if args.iter().any(|arg| arg.to_str() == Some(scenario)) {
                    eprintln!(
                        "hil: `{scenario}` is quarantined ({}); running it as asked",
                        entry.reason
                    );
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Whether `args` hold the option `name`, alone or as `name=value`.
fn has_flag(args: &[OsString], name: &str) -> bool {
    args.iter().any(|arg| {
        arg.to_str().is_some_and(|arg| {
            arg == name
                || arg
                    .strip_prefix(name)
                    .is_some_and(|rest| rest.starts_with('='))
        })
    })
}

/// The options of `args` that choose a run's sources: they make its snapshot.
const SOURCE_OPTIONS: [(&str, bool); 2] =
    [("--source-include", true), ("--include-untracked", false)];

/// `args`' source options, with their values.
fn source_options(args: &[OsString]) -> Vec<OsString> {
    split_source_options(args).0
}

/// `args` split into the source options and the rest.
fn split_source_options(args: &[OsString]) -> (Vec<OsString>, Vec<OsString>) {
    let (mut sources, mut rest) = (Vec::new(), Vec::new());
    let mut arguments = args.iter();
    while let Some(argument) = arguments.next() {
        let text = argument.to_str().unwrap_or_default();
        match SOURCE_OPTIONS.iter().find(|(name, _)| {
            text == *name
                || text
                    .strip_prefix(name)
                    .is_some_and(|rest| rest.starts_with('='))
        }) {
            Some((name, takes_value)) => {
                sources.push(argument.clone());
                if *takes_value && text == *name {
                    sources.extend(arguments.next().cloned());
                }
            }
            None => rest.push(argument.clone()),
        }
    }
    (sources, rest)
}

/// The runner arguments of a run whose sources were captured into
/// `snapshot` when it was enqueued: its source options already made it.
fn with_source_snapshot(args: &[OsString], snapshot: &Path) -> Vec<OsString> {
    let mut arguments = split_source_options(args).1;
    arguments.push("--source-snapshot".into());
    arguments.push(snapshot.into());
    arguments
}

/// Whether each of `runs` in `store` was built from a dirty tree; one whose
/// manifest the store does not hold counts as dirty.
fn runs_dirty(
    store: &oer_hil_run_bundle::RunStore,
    runs: &[oer_hil_run_bundle::RunId],
) -> Vec<bool> {
    runs.iter()
        .map(|run| {
            store
                .open(run.as_str())
                .map_or(true, |bundle| bundle.manifest().repository.dirty)
        })
        .collect()
}

/// Why a finished invocation records no evidence: it created no run, a run
/// came from a dirty tree, or untracked sources were added with
/// `--source-include` or `--include-untracked`. Such runs are experiments;
/// `cargo qualification hil-evidence --run ID` records them anyway.
/// What a run was given beyond the tracked checkout and the catalog.
#[derive(Clone, Copy, Default)]
struct RunInputs {
    untracked_sources: bool,
    /// `--repetitions` replaced the scenarios' own counts.
    reduced_repetitions: bool,
}

fn evidence_skip_reason(inputs: RunInputs, created_dirty: &[bool]) -> Option<&'static str> {
    if created_dirty.is_empty() {
        Some("the runner created no run")
    } else if created_dirty.iter().any(|dirty| *dirty) {
        Some("the run was built from a dirty tree")
    } else if inputs.untracked_sources {
        Some("the run included untracked sources")
    } else if inputs.reduced_repetitions {
        Some("--repetitions replaced the scenarios' repetitions")
    } else {
        None
    }
}

/// Note clean runs as pending evidence of this checkout: a run never writes
/// tracked files.
fn remember_pending(
    ctx: &Checkout,
    store: &oer_hil_run_bundle::RunStore,
    owner: String,
    runs: &[oer_hil_run_bundle::RunId],
) -> Result<()> {
    use oer_hil_run_bundle::store::pending;
    let pending = runs
        .iter()
        .map(|run| pending::Pending {
            run: run.to_string(),
            scenarios: store
                .open(run.as_str())
                .ok()
                .and_then(|bundle| bundle.suite().ok().flatten())
                .map(|suite| {
                    suite
                        .scenarios
                        .into_iter()
                        .filter(|scenario| scenario.outcome.is_passed())
                        .map(|scenario| scenario.scenario)
                        .collect()
                })
                .unwrap_or_default(),
            owner: owner.clone(),
        })
        .collect::<Vec<_>>();
    pending::remember(&ctx.root, &pending)?;
    if pending.iter().any(|run| !run.scenarios.is_empty()) {
        eprintln!(
            "hil: evidence not recorded in hil/evidence; record it with the change it qualifies: \
             cargo qualification hil-evidence --hil-target CHIP --pending"
        );
    }
    Ok(())
}

/// Commands that execute scenarios and write run bundles.
fn produces_runs(args: &[OsString]) -> bool {
    matches!(
        args.first().and_then(|a| a.to_str()),
        Some("run" | "run-all")
    )
}

/// Commands that need the terminal's foreground process group.
fn hands_off_terminal(args: &[OsString]) -> bool {
    matches!(args, [fixture, install, ..] if fixture == "fixture" && install == "install")
        && !args.iter().any(|arg| arg == "--dry-run")
}

// The clap parsers of the stand commands, parts of [`HilCli`].
#[derive(clap::Args)]
struct BoardCli {
    #[command(subcommand)]
    command: crate::board::BoardCommand,
}

#[derive(clap::Subcommand)]
enum PerfCli {
    /// Gated measurements of clean runs per commit, against each
    /// scenario's baseline.
    Report {
        /// Scenario IDs; every scenario with a gated measurement when omitted.
        scenarios: Vec<String>,
        /// Only measurements whose name contains this text.
        #[arg(long)]
        measurement: Option<String>,
        /// Only runs this recent, e.g. 7d.
        #[arg(long, value_parser = parse_budget, default_value = "14d")]
        since: std::time::Duration,
    },
    /// Make a clean sealed run the baseline of its scenarios.
    Baseline {
        run: String,
        /// Scenarios to set; all gated scenarios of the run when omitted.
        #[arg(long = "scenario")]
        scenarios: Vec<String>,
        #[arg(long)]
        reason: String,
    },
    /// Fail when a run's gated measurements regressed against the
    /// baselines.
    Check { run: String },
}

/// `cargo hil profile RUN [--scenario S] [--repetition N] [--top N]`.
#[derive(clap::Args)]
struct ProfileCli {
    /// The run whose profiled repetitions to report.
    run: String,
    /// Only this scenario's repetitions.
    #[arg(long)]
    scenario: Option<String>,
    /// Only this repetition number.
    #[arg(long)]
    repetition: Option<u8>,
    /// The most sampled functions shown per hart.
    #[arg(long, default_value_t = 15)]
    top: usize,
}

/// Where `cargo hil profile` keeps the report of the profile at `relative`
/// (its directory below the scenario) of `run`'s `scenario`: under this
/// checkout's target directory, never inside the sealed run bundle.
fn profile_report_path(root: &Path, run: &str, scenario: &str, relative: &Path) -> PathBuf {
    root.join("target/hil/profiles")
        .join(run)
        .join(scenario)
        .join(relative)
        .join("profile.txt")
}

/// Report every `profile.json` a run's repetitions left, symbolized against
/// the run's own image, and keep each report under this checkout's
/// `target/hil/profiles/`.
fn profile(ctx: &Checkout, cli: ProfileCli) -> Result<std::process::ExitCode> {
    let run = oer_hil_run_bundle::RunStore::shared()?
        .open(&cli.run)?
        .directory()
        .to_owned();
    let mut found = 0;
    let mut scenarios = fs::read_dir(run.join("scenarios"))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect::<Vec<_>>();
    scenarios.sort();
    for scenario in scenarios {
        let id = scenario
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if cli.scenario.as_ref().is_some_and(|wanted| *wanted != id) {
            continue;
        }
        // The scenario's result names the image class every family ran.
        let image = fs::read(scenario.join("result.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|result| result["image"].as_str().map(str::to_owned));
        let elf = image
            .map(|image| run.join("firmware").join(image).join("runtime.elf"))
            .filter(|elf| elf.is_file());
        let mut profiles = Vec::new();
        collect_profiles(&scenario, &mut profiles)?;
        profiles.sort();
        for path in profiles {
            let repetition = path
                .strip_prefix(&scenario)
                .ok()
                .and_then(|relative| relative.components().next())
                .and_then(|first| {
                    first
                        .as_os_str()
                        .to_str()?
                        .strip_prefix("repetition-")?
                        .parse::<u8>()
                        .ok()
                });
            if cli
                .repetition
                .is_some_and(|wanted| Some(wanted) != repetition)
            {
                continue;
            }
            let report = oer_hil_analysis::profile::report(&path, elf.as_deref(), cli.top)?;
            let relative = path
                .parent()
                .unwrap_or(&path)
                .strip_prefix(&scenario)
                .unwrap_or(Path::new(""));
            println!("== {id} {}", relative.display());
            print!("{report}");
            // The run bundle is sealed: a derived report written into it
            // would fail its integrity inventory.
            let destination = profile_report_path(&ctx.root, &cli.run, &id, relative);
            fs::create_dir_all(destination.parent().ok_or("profile report has no parent")?)?;
            oer_durable::atomic_write(&destination, report.as_bytes())?;
            eprintln!("hil: report {}", destination.display());
            found += 1;
        }
    }
    if found == 0 {
        return Err(format!(
            "run {} has no profile.json in the selected repetitions",
            cli.run
        )
        .into());
    }
    Ok(std::process::ExitCode::SUCCESS)
}

fn collect_profiles(
    directory: &std::path::Path,
    found: &mut Vec<std::path::PathBuf>,
) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_profiles(&path, found)?;
        } else if path.file_name().is_some_and(|name| name == "profile.json") {
            found.push(path);
        }
    }
    Ok(())
}

#[derive(clap::Subcommand)]
enum RunsCli {
    /// Runs of every checkout, newest last.
    List {
        #[arg(long)]
        scenario: Option<String>,
        /// passed, failed, broken, blocked, interrupted; with --scenario,
        /// that scenario's outcome.
        #[arg(long)]
        outcome: Option<String>,
        /// Image class or application SHA-256 prefix.
        #[arg(long)]
        image: Option<String>,
        /// Only runs this recent, e.g. 3d or 12h.
        #[arg(long, value_parser = parse_budget)]
        since: Option<std::time::Duration>,
        #[arg(long, default_value_t = 30)]
        limit: usize,
    },
    /// Where a run's artifacts are: its directory in the shared store and
    /// each repetition's artifact directory.
    Show {
        run: String,
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
    /// Scenarios whose repetitions did not always pass, least passing
    /// first, with the stand's failures counted apart from the code's.
    Flaky {
        /// Only runs this recent, e.g. 3d or 12h.
        #[arg(long, value_parser = parse_budget, default_value = "7d")]
        since: std::time::Duration,
        /// Fewest repetitions a scenario needs to be judged.
        #[arg(long, default_value_t = 3)]
        minimum: usize,
    },
    /// Leave a scenario out of every `run-all` until it is released; an
    /// explicit `run` of it still runs, with a warning.
    Quarantine {
        scenario: String,
        #[arg(long)]
        reason: String,
    },
    Release {
        scenario: String,
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

#[derive(clap::Subcommand)]
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
        /// Write and reset through OpenOCD and the chip's JTAG instead of
        /// espflash and the USB Serial/JTAG reset lines.
        #[arg(long)]
        jtag: bool,
        /// Leave the board alone when its journaled flash is this image
        /// with the digest of the current build.
        #[arg(long)]
        if_changed: bool,
    },
}

#[derive(clap::Args)]
struct DevicesCli {
    #[arg(long)]
    json: bool,
    #[command(subcommand)]
    command: Option<DevicesCommand>,
}
#[derive(clap::Subcommand)]
enum DevicesCommand {
    /// Take a board out of service: until `release`, only this owner
    /// (`--owner`, else the checkout) may claim it. Leases already held
    /// run on.
    #[command(group(clap::ArgGroup::new("what").required(true).args(["board", "stand"])))]
    Maintenance {
        #[arg(value_name = "NAME|MAC")]
        board: Option<String>,
        /// The whole stand: only this owner's requests are served, other
        /// runs wait until `release --stand`.
        #[arg(long)]
        stand: bool,
        #[arg(long)]
        reason: String,
    },
    /// Return a board, or the whole stand, to service.
    #[command(group(clap::ArgGroup::new("what").required(true).args(["board", "stand"])))]
    Release {
        #[arg(value_name = "NAME|MAC")]
        board: Option<String>,
        #[arg(long)]
        stand: bool,
        /// What returns a quarantined board: a person pressed its reset
        /// button or power-cycled it, or its ROM answers the stand's own
        /// reset. It returns only if it then boots.
        #[arg(long, value_enum)]
        confirm: Option<ConfirmArg>,
    },
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_stand_command_line_is_one_parser_and_the_rest_is_the_runner_s() {
        use clap::{CommandFactory as _, Parser as _};
        super::HilCli::command().debug_assert();
        let tree = oer_command_tree::command_tree(&super::HilCli::command(), &[]);
        assert!(tree[0].subcommands.iter().any(|name| name == "queue"));
        // `status` was once advertised without a handler: an unknown word is
        // a runner command, never a stand node.
        assert!(!tree[0].subcommands.iter().any(|name| name == "status"));
        let parsed = super::HilCli::try_parse_from(["status"]).unwrap();
        assert!(matches!(parsed.command, Some(super::HilCommand::Runner(_))));
        let ab = tree.iter().find(|node| node.path == ["ab"]).unwrap();
        for flag in ["--enqueue", "--after", "--after-any", "--scenario"] {
            assert!(ab.flags.iter().any(|known| known == flag), "{flag}");
        }
        assert!(super::HilCli::try_parse_from(["wait", "--service", "a", "b"]).is_ok());
        assert!(super::HilCli::try_parse_from(["evidence", "dismiss"]).is_err());
    }

    #[test]
    fn a_lease_runs_no_hub_power_switch() {
        let command = |words: &[&str]| words.iter().map(OsString::from).collect::<Vec<_>>();
        for refused in [
            command(&["uhubctl", "-l", "3-8.3", "-p", "2", "-a", "off"]),
            command(&["sh", "-c", "uhubctl -l 3-8.3 -p 2 -a off; sleep 5"]),
            command(&["/usr/sbin/uhubctl", "-a", "cycle"]),
            command(&["bash", "-c", "sleep 1 && sudo uhubctl -a on"]),
        ] {
            assert!(super::switches_hub_power(&refused).is_some(), "{refused:?}");
        }
        for allowed in [
            command(&["cargo", "hil", "peer", "send"]),
            command(&[
                "cargo",
                "hil",
                "devices",
                "set",
                "AA",
                "--power-uhubctl",
                "3-8.3",
                "--power-port",
                "2",
            ]),
            command(&["sh", "-c", "echo uhubctl is refused"]),
        ] {
            assert!(super::switches_hub_power(&allowed).is_none(), "{allowed:?}");
        }
    }

    use super::{
        OsString, Path, PathBuf, checkout_of_common_dir, has_flag, runs_dirty, source_options,
        with_source_snapshot,
    };

    #[test]
    fn a_run_is_clean_only_when_its_manifest_says_so() {
        let directory = tempfile::tempdir().unwrap();
        let store = oer_hil_run_bundle::RunStore::at(directory.path());
        for (id, dirty) in [("1-a", true), ("2-b", false)] {
            oer_hil_run_bundle::run::test_support::write_run(
                &store.run(id),
                1,
                oer_hil_run_bundle::run::RunState::Completed,
                Vec::new(),
                |manifest| manifest.repository.dirty = dirty,
            );
        }
        let ids = ["1-a", "2-b", "3-missing"].map(oer_hil_run_bundle::RunId::new);
        assert_eq!(runs_dirty(&store, &ids), [true, false, true]);
    }

    #[test]
    fn an_enqueued_run_builds_from_its_snapshot_instead_of_its_source_options() {
        let args = [
            "run",
            "a",
            "--source-include",
            "x.rs",
            "--source-include=y.rs",
            "--include-untracked",
            "--layout-seed",
            "3",
        ]
        .map(OsString::from);
        assert_eq!(
            source_options(&args),
            [
                "--source-include",
                "x.rs",
                "--source-include=y.rs",
                "--include-untracked"
            ]
            .map(OsString::from)
        );
        assert_eq!(
            with_source_snapshot(&args, Path::new("/snapshots/s1")),
            [
                "run",
                "a",
                "--layout-seed",
                "3",
                "--source-snapshot",
                "/snapshots/s1"
            ]
            .map(OsString::from)
        );
        assert!(has_flag(&args, "--layout-seed"));
        assert!(!has_flag(&args, "--firmware-from"));
        // A flag that only begins with the name is another flag.
        assert!(!has_flag(
            &[OsString::from("--source-includes")],
            "--source-include"
        ));
    }

    #[test]
    fn a_nested_stand_command_is_checked_before_its_lease_waits() {
        let words = |text: &str| text.split(' ').map(OsString::from).collect::<Vec<_>>();
        let nested = super::nested_hil(
            &OsString::from("cargo"),
            &words("hil firmware flash esp32c5-hello --include-untracked"),
        )
        .unwrap();
        assert_eq!(
            nested,
            words("firmware flash esp32c5-hello --include-untracked")
        );
        assert!(super::check_hil_arguments(&nested).is_err());
        assert!(super::check_hil_arguments(&words("firmware list")).is_ok());
        // The runner parses its own commands when they run.
        assert!(super::check_hil_arguments(&words("run a --anything")).is_ok());
        assert!(
            super::nested_hil(&OsString::from("/usr/bin/openocd"), &words("-f board.cfg"))
                .is_none()
        );
        assert!(
            super::nested_hil(
                &OsString::from("/home/u/.local/bin/oer-stand"),
                &words("runs list")
            )
            .is_some()
        );
    }

    #[test]
    fn a_worktree_belongs_to_the_checkout_holding_the_common_git_directory() {
        assert_eq!(
            checkout_of_common_dir(Path::new("/home/dev/checkout/.git")),
            Some(PathBuf::from("/home/dev/checkout"))
        );
        // A bare repository has no checkout of its own.
        assert_eq!(
            checkout_of_common_dir(Path::new("/srv/repository.git")),
            None
        );
    }

    #[test]
    fn a_profile_report_stays_outside_the_sealed_run() {
        let root = Path::new("/checkout");
        let report = profile_report_path(root, "run-1", "udp", Path::new("repetition-001"));
        assert_eq!(
            report,
            Path::new("/checkout/target/hil/profiles/run-1/udp/repetition-001/profile.txt")
        );
        assert!(!report.starts_with(root.join("target/hil/runs")));
    }

    #[test]
    fn evidence_is_noted_as_pending_only_for_clean_runs() {
        let clean = RunInputs::default();
        assert_eq!(evidence_skip_reason(clean, &[false]), None);
        assert!(evidence_skip_reason(clean, &[]).is_some());
        assert!(evidence_skip_reason(clean, &[false, true]).is_some());
        let untracked = RunInputs {
            untracked_sources: true,
            ..clean
        };
        assert!(evidence_skip_reason(untracked, &[false]).is_some());
        // A run with fewer repetitions than its scenarios require is a look,
        // not qualification evidence.
        let reduced = RunInputs {
            reduced_repetitions: true,
            ..clean
        };
        assert!(evidence_skip_reason(reduced, &[false]).is_some());
    }

    use super::*;
    #[test]
    fn only_executing_commands_record_evidence() {
        let args = |values: &[&str]| values.iter().map(OsString::from).collect::<Vec<_>>();
        assert!(produces_runs(&args(&["run", "boot-smoke"])));
        assert!(produces_runs(&args(&["run-all", "--tag", "wifi"])));
        assert!(!produces_runs(&args(&["plan", "--scenario", "x"])));
        assert!(!produces_runs(&args(&["doctor"])));
    }

    #[test]
    fn the_owner_precedes_a_stand_command_and_may_follow_a_runner_command() {
        use clap::Parser as _;
        let args = |values: &[&str]| values.iter().map(OsString::from).collect::<Vec<_>>();
        let cli = HilCli::try_parse_from(args(&["--owner", "phy", "run", "x", "--owner", "kept"]))
            .unwrap();
        assert_eq!(cli.owner.as_deref(), Some("phy"));
        let Some(HilCommand::Runner(runner)) = cli.command else {
            panic!("run is the runner's");
        };
        assert_eq!(runner, args(&["run", "x", "--owner", "kept"]));
        // The runner takes no lease options: they may follow its command.
        let (late, rest) = crate::jobs::take::<crate::jobs::RunnerOptions>(args(&[
            "run", "x", "--owner", "phy", "y", "--", "--owner",
        ]))
        .unwrap();
        assert_eq!(rest, args(&["run", "x", "y", "--", "--owner"]));
        let options = LeaseOptions::default().merged(late.owner).unwrap();
        assert_eq!(options.owner.as_deref(), Some("phy"));
        let options = options.merged(Some("phy".into())).unwrap();
        assert_eq!(options.owner.as_deref(), Some("phy"));
        assert!(options.clone().merged(Some("bt".into())).is_err());
        assert_eq!(options.merged(None).unwrap().owner.as_deref(), Some("phy"));
        // A stand command takes the owner only before it.
        assert!(HilCli::try_parse_from(args(&["queue", "--owner", "phy"])).is_err());
    }

    #[test]
    fn a_lease_command_is_terminated_at_the_hard_limit() {
        let directory = tempfile::tempdir().unwrap();
        let arbiter = oer_hil_arbiter::Arbiter::at(directory.path()).unwrap();
        let grant = arbiter
            .acquire(&oer_hil_arbiter::Request {
                owner: "stand".into(),
                work: "sleep".into(),
                scenarios: Vec::new(),
                claims: Vec::new(),
            })
            .unwrap();
        let started = std::time::Instant::now();
        let mut child = oer_process::owned::Child::spawn_with_shutdown_grace(
            std::process::Command::new("sleep").arg("60"),
            std::time::Duration::from_secs(1),
        )
        .unwrap();
        let (code, succeeded) =
            supervise(&grant, &mut child, std::time::Duration::from_secs(1)).unwrap();
        assert!(!succeeded);
        assert_eq!(code, std::process::ExitCode::from(HARD_LIMIT_EXIT));
        assert!(started.elapsed() >= std::time::Duration::from_secs(1));
        drop(grant);
        assert_eq!(
            arbiter.history().unwrap()[0].outcome,
            oer_hil_arbiter::LeaseOutcome::HardLimit
        );
    }

    const TEST_STAND: &str = "schema = 1\n[stand]\nid = \"t\"\nair = \"exclusive\"\n\
             [[hub]]\nid = \"h\"\nusb2 = \"1-1\"\n\
             [[board]]\nid = \"c5-a\"\nusb-serial = \"38:44:BE:AA:25:64\"\nchip = \"esp32c5\"\n\
             radios = [\"ble\"]\nroles = [\"peer\"]\nport = { hub = \"h\", port = 1 }\nreset = [\"jtag\"]\n";

    #[test]
    fn a_lease_claims_its_boards_and_the_air_or_explicitly_the_whole_stand() {
        use oer_hil_arbiter::{AIR, Claim, Mode};
        let devices = oer_hil_stand_model::StandFile::parse(TEST_STAND).unwrap();
        assert_eq!(
            lease_claims(&[], None, true, &devices).unwrap(),
            [Claim::stand()]
        );
        // Naming nothing never claims the whole stand by accident.
        assert!(lease_claims(&[], None, false, &devices).is_err());
        assert!(lease_claims(&["esp32c5".into()], None, true, &devices).is_err());
        assert_eq!(
            lease_claims(&["esp32c5".into()], None, false, &devices).unwrap(),
            [Claim::board("38:44:BE:AA:25:64"), Claim::shared(AIR)]
        );
        assert_eq!(
            lease_claims(&[], Some(AirArg(Some(Mode::Exclusive))), false, &devices).unwrap(),
            [Claim::exclusive(AIR)]
        );
        assert!(lease_claims(&["s3".into()], None, false, &devices).is_err());
        assert_eq!(
            lease_claims(&["esp32c5".into()], Some(AirArg(None)), false, &devices).unwrap(),
            [Claim::board("38:44:BE:AA:25:64")]
        );
        assert!(lease_claims(&[], Some(AirArg(None)), false, &devices).is_err());
    }

    #[test]
    fn lease_records_a_described_flash_on_the_named_board() {
        use clap::Parser as _;
        let parse = |words: &[&str]| -> std::result::Result<LeaseCli, clap::Error> {
            match HilCli::try_parse_from(std::iter::once("lease").chain(words.iter().copied()))?
                .command
            {
                Some(HilCommand::Lease(cli)) => Ok(cli),
                _ => panic!("lease parses as lease"),
            }
        };
        let cli = parse(&[
            "--owner",
            "802154",
            "--flashed",
            "ieee802154-peer",
            "--sha256",
            &"AB".repeat(32),
            "--device",
            "38:44:be:aa:25:64",
            "--",
            "idf.py",
            "flash",
        ])
        .unwrap();
        assert_eq!(cli.command, ["idf.py", "flash"]);
        assert!(parse(&["idf.py"]).is_err(), "COMMAND follows --");
        assert!(
            parse(&["--sha256", "ab", "--", "x"]).is_err(),
            "flash details need --flashed"
        );
        let directory = tempfile::tempdir().unwrap();
        let arbiter = oer_hil_arbiter::Arbiter::at(directory.path()).unwrap();
        std::fs::write(arbiter.stand_file(), TEST_STAND).unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(
            arbiter.stand_file(),
            std::os::unix::fs::PermissionsExt::from_mode(0o600),
        )
        .unwrap();
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
