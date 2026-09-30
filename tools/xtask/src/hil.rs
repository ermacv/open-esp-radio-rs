//! Build and launch the HIL observer with a receipt from Cargo's actual artifacts.
use crate::{Context, Result};
use oer_hil_schema::{artifacts, compile::compile};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

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
        Some("peer") => return crate::hil_board::peer(options.owner(ctx)?, &args[1..]),
        Some("preempt") => return preempt(&options.owner(ctx)?, &args[1..]),
        Some("owner") => return owner(ctx, &args[1..]),
        Some("__command-tree") => return command_tree(ctx),
        Some("devices") => return devices(ctx, &options, &args[1..]),
        Some("fixtures") => {
            let lab = oer_hil_stand::config::LabConfig::default_path()?;
            print!(
                "{}",
                crate::hil_fixtures::describe(&crate::hil_fixtures::probe(&lab))
            );
            return Ok(std::process::ExitCode::SUCCESS);
        }
        Some("firmware") => return firmware(ctx, &options, &args[1..]),
        Some("flash") => {
            let request = oer_hil_arbiter::Request {
                owner: options.owner(ctx)?,
                work: String::new(),
                scenarios: Vec::new(),
                claims: Vec::new(),
            };
            return crate::hil_flash::run(ctx, request, &args[1..]);
        }
        Some("runs") => return runs(ctx, &options, &args[1..]),
        Some("evidence") => return crate::hil_evidence::command(ctx, &args[1..]),
        Some("perf") => return perf(ctx, &options, &args[1..]),
        Some("profile") => return profile(ctx, &args[1..]),
        Some("wait") if args.get(1).is_some_and(|arg| arg == "--service") => {
            return wait_for_service(&args[2..]);
        }
        Some("wait") => return wait(ctx, &args[1..]),
        Some("ab") => return ab(ctx, &options.owner(ctx)?, args),
        Some("bisect") => return crate::hil_bisect::run(ctx, &options.owner(ctx)?, &args[1..]),
        _ => {}
    }
    // The runner has no lease options: take them from after the command
    // too, before the runner is built.
    let (options, args) = options.with_late(args)?;
    let (enqueue, after, args) = crate::hil_jobs::take(args)?;
    if (enqueue || after.is_some()) && !produces_runs(&args) {
        return Err("--enqueue and --after apply to run and run-all".into());
    }
    if enqueue {
        let frozen = freeze_enqueued(ctx, &args)?;
        let id = crate::hil_jobs::enqueue(ctx, &options.owner(ctx)?, &args, after, &frozen)?;
        eprintln!("hil: enqueued job {id}; `cargo hil wait {id}` blocks until it ends");
        println!("{id}");
        return Ok(std::process::ExitCode::SUCCESS);
    }
    let owner = options
        .owner(ctx)
        .unwrap_or_else(|_| String::from("unregistered"));
    // Only a command that produces runs is a job; a read-only one is not.
    let mut job = if produces_runs(&args) {
        Some(crate::hil_jobs::Running::begin(
            ctx,
            &owner,
            &args,
            after.as_ref(),
        )?)
    } else {
        None
    };
    let args = args.as_slice();
    // The runner and its image builds are the largest writers to the build
    // disk: sweep stale caches when it runs low, and stop before it is full.
    if let Err(error) = crate::sweep::automatically(&ctx.root) {
        eprintln!("hil: the automatic cache sweep failed: {error}");
    }
    crate::sweep::ensure_space(&ctx.root)?;
    // An enqueued job runs the runner and sources fixed when it was
    // enqueued; the evidence decision below still reads its own arguments.
    let frozen = crate::hil_jobs::Frozen::inherited()?;
    let (runner, receipt_path) = match &frozen {
        Some(frozen) => (frozen.runner.clone(), frozen.receipt.clone()),
        None => prepare(ctx)?,
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
            let error = ctx
                .command(&runner)
                .args(&runner_args)
                .envs(options.environment(ctx)?)
                .env("OER_OBSERVER_RECEIPT", &receipt_path)
                .exec();
            return Err(format!("cannot hand the terminal to the HIL runner: {error}").into());
        }
    }
    // Cleanup scopes in the runner have 30-second budgets and may unwind
    // multiple owned fixtures. This is a shutdown allowance, never a run timeout.
    // The runner names every run it creates here, so exactly this
    // invocation's runs are recorded.
    let run_receipt = tempfile::NamedTempFile::new()?;
    let mut command = ctx.command(&runner);
    command
        .args(&runner_args)
        .envs(options.environment(ctx)?)
        .env("OER_OBSERVER_RECEIPT", &receipt_path)
        .env(RUN_RECEIPT_ENV, run_receipt.path());
    // The runner escalates a bootloader loop to a JTAG reset, through the
    // OpenOCD of the ESP-IDF tools only this wrapper can locate.
    if let Ok((program, scripts)) = crate::hil_jtag::openocd() {
        command
            .env(oer_hil_arbiter::control::OPENOCD_ENV, program)
            .env(oer_hil_arbiter::control::OPENOCD_SCRIPTS_ENV, scripts);
    }
    // An ESP-IDF application is flashed with its chip's catalog bootloader,
    // which only this wrapper builds; the runner asks it back.
    command.env(oer_hil_image::XTASK_ENV, std::env::current_exe()?);
    let mut child = oer_process::owned::Child::spawn_with_shutdown_grace(
        &mut command,
        std::time::Duration::from_secs(300),
    )?;
    let status = child.wait_forwarding_cancellation()?;
    if produces_runs(args) {
        let run_ids = std::fs::read_to_string(run_receipt.path())?
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        forward_run_receipt(&run_ids)?;
        // A run lies in the store of the chip it ran on.
        let stores = oer_chip_profile::supported(&ctx.root)?
            .iter()
            .map(|chip| crate::hil_store::shared_runs(chip))
            .collect::<Result<Vec<_>>>()?;
        if let Some(job) = job.as_mut() {
            job.finish(
                &run_ids,
                &run_ids
                    .iter()
                    .map(|id| {
                        stores
                            .iter()
                            .find_map(|store| crate::hil_runs::load(&store.join(id)))
                            .and_then(|run| run.outcome)
                    })
                    .collect::<Vec<_>>(),
            )?;
        }
        let created = runs_dirty(&stores, &run_ids);
        if std::env::var_os(oer_hil_evidence::experiment::EXPERIMENT_ENV).is_some() {
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
                None => remember_pending(ctx, options.owner(ctx).ok(), &run_ids)?,
                // A runner that created no run has said why itself.
                Some(_) if created.is_empty() => {}
                Some(reason) => eprintln!(
                    "hil: HIL evidence not noted as pending: {reason}; \
                     `cargo hil evidence record --run ID` records it"
                ),
            }
        }
        if let Err(error) = prune_automatically(ctx) {
            eprintln!("hil: automatic pruning of the run store failed: {error}");
        }
    }
    Ok(exit_code(status))
}

/// Stand commands handled here, printed before the runner's own help.
const STAND_HELP: &str = "\
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
  cargo hil board reset BOARD [--via rts|jtag|en] [--download]   reset under a lease; prints the ROM reset line
  cargo hil board check BOARD         attached, firmware, maintenance, reset paths, whether it answers; no reset
  cargo hil board console BOARD [--for 10s] [--until TEXT]       the console without a reset, under a lease
  cargo hil board soak BOARD --cycles N|--for 8h [--via rts,jtag,en]   reset again and again; journal the result
  cargo hil peer send BOARD LINE... [--for 5s]                   one peer text-protocol command and its answer
  cargo hil owner [set NAME]          this checkout's owner: stand, wifi, phy, bluetooth, bluetooth-hil, blobray, infra, 802154, esp32c5, network
  cargo hil preempt ID --reason TEXT   stop another owner's lease: charged no longer, SIGTERM with
                                      cleanup, SIGKILL after 5m; the history and the owner see why
  cargo hil dashboard [--port 8765]   live page of the queue, boards, runs and leases on 127.0.0.1
  cargo hil lease [OPTIONS] -- CMD    run CMD under one lease; nested cargo hil joins it
      --board NAME|MAC                boards CMD uses (repeatable)
      --stand                         claim the whole stand instead; blocks every other owner
      --air shared|exclusive|none     radio environment; exclusive for RF measurements, none without radio
      --flashed IMAGE (--application FILE | --sha256 HASH) (--port PORT | --device MAC)
      [--chip CHIP] [--commit REV]    journal CMD's flash when it succeeds
  cargo hil board flashed --image IMAGE ...   journal a flash made inside a lease
  cargo hil fixtures                  host Wi-Fi radios, Bluetooth adapter and OpenWrt hosts: key, interfaces, channel, CCA busy
  cargo hil devices [--json]          boards: name, chip, port, health, last firmware
  cargo hil devices set MAC [--chip CHIP] [--name NAME] [--reset-uart SERIAL --en LINE --boot LINE]
  cargo hil devices reset BOARD [--download]   reset through the registered reset path
  cargo hil [--owner NAME] devices maintenance BOARD|--stand --reason TEXT   only NAME may claim BOARD (or the stand) until release; other runs wait
  cargo hil devices release BOARD [--confirm reset|power-cycle|rom-answers]
  cargo hil runs list [--scenario S] [--outcome O] [--image I] [--since 3d]
  cargo hil runs why RUN              why a run did not pass: failure, missed criteria, log tail
  cargo hil runs compare A B [--measurement TEXT]   measurement means side by side
  cargo hil runs history SCENARIO [--measurement TEXT]
  cargo hil runs pin RUN --reason TEXT | unpin RUN
  cargo hil runs prune [--days 30] [--keep-failed 5] [--apply]
  cargo hil evidence record [--run ID ...]   write runs' qualifying observations as tracked shards;
                                      default: this checkout's pending clean runs
  cargo hil evidence pending          clean runs whose evidence is not recorded yet
  cargo hil evidence dismiss --run ID ...   drop runs whose evidence will not be recorded
  cargo hil firmware list             tracked ESP-IDF images (peers, vendor references)
  cargo hil firmware build IMAGE      build against the one pinned ESP-IDF
  cargo hil firmware flash IMAGE --board NAME|MAC [--jtag] [--if-changed]   flash under a lease of that board, journaled
  cargo hil flash --board NAME|MAC [--image NAME] [--monitor 30s [--until TEXT]] [--air shared|exclusive|none] [--via usb|jtag] ELF
                                      flash an ELF for the board's chip under a lease of that board,
                                      journal it, capture the console for a bounded time

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
checkout's pending evidence, recorded by `cargo hil evidence record`; record
any other run, such as one from a dirty tree, with `--run ID`.

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
    /// Split a leading `--owner NAME` from the remaining arguments.
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
                _ => {
                    remaining.push(argument.clone());
                    remaining.extend(rest.cloned());
                    break;
                }
            }
        }
        Ok((options, remaining))
    }

    /// Take lease options that follow a runner command, up to a `--`, and
    /// merge them with those before it; different owners are refused.
    fn with_late(self, args: &[OsString]) -> Result<(Self, Vec<OsString>)> {
        let mut options = self;
        let mut remaining = Vec::new();
        let mut rest = args.iter();
        while let Some(argument) = rest.next() {
            let text = argument.to_str().unwrap_or_default();
            if text == "--" {
                remaining.push(argument.clone());
                remaining.extend(rest.by_ref().cloned());
                break;
            }
            let (name, inline) = match text.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (text, None),
            };
            let owner = match name {
                "--owner" => match inline {
                    Some(value) => value.to_owned(),
                    None => rest
                        .next()
                        .and_then(|value| value.to_str())
                        .ok_or("--owner requires a value")?
                        .to_owned(),
                },
                _ => {
                    remaining.push(argument.clone());
                    continue;
                }
            };
            if let Some(earlier) = options.owner.as_ref().filter(|earlier| **earlier != owner) {
                return Err(format!("--owner is given twice, as {earlier} and {owner}").into());
            }
            options.owner = Some(owner);
        }
        Ok((options, remaining))
    }

    /// Explicit options win; otherwise an enclosing lease's owner, otherwise
    /// the owner registered for this checkout. Nothing is derived from the
    /// checkout's directory name.
    fn owner(&self, ctx: &Context) -> Result<String> {
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
    fn environment(&self, ctx: &Context) -> Result<Vec<(&'static str, String)>> {
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
/// the real command line. The stand's own commands and the runner's are
/// merged under `hil`.
fn command_tree(ctx: &Context) -> Result<std::process::ExitCode> {
    use clap::CommandFactory as _;
    use oer_command_tree::{CommandNode, command_tree as walk};
    let path = |words: &[&str]| {
        words
            .iter()
            .map(|word| word.to_string())
            .collect::<Vec<_>>()
    };
    let node = |words: &[&str], subcommands: &[&str], flags: &[&str]| CommandNode {
        path: path(words),
        subcommands: subcommands.iter().map(|word| word.to_string()).collect(),
        flags: flags.iter().map(|word| word.to_string()).collect(),
        forwards: false,
    };
    let (runner, _) = prepare(ctx)?;
    let output = std::process::Command::new(&runner)
        .arg("__command-tree")
        .output()?;
    let mut runner_nodes: Vec<CommandNode> = serde_json::from_slice(&output.stdout)?;
    let runner_top = runner_nodes
        .first()
        .map(|root| root.subcommands.clone())
        .unwrap_or_default();
    // The stand adds lease and evidence options to the runner's run commands.
    for node in &mut runner_nodes {
        if matches!(node.path.as_slice(), [one] if ["run", "run-all"].contains(&one.as_str())) {
            node.flags
                .extend(["--owner", "--enqueue", "--after", "--after-any"].map(String::from));
        }
        node.path.insert(0, String::from("hil"));
    }
    let stand = [
        "queue",
        "status",
        "dashboard",
        "lease",
        "board",
        "peer",
        "devices",
        "fixtures",
        "firmware",
        "flash",
        "runs",
        "perf",
        "evidence",
        "owner",
        "preempt",
        "profile",
        "bisect",
        "ab",
        "wait",
    ];
    let mut root = node(&["hil"], &stand, &["--owner"]);
    root.subcommands.extend(runner_top);
    let mut nodes = vec![
        root,
        node(&["hil", "queue"], &[], &["--json"]),
        node(&["hil", "dashboard"], &[], &["--port"]),
        node(&["hil", "evidence"], &["record", "pending", "dismiss"], &[]),
        node(&["hil", "evidence", "dismiss"], &[], &["--run"]),
        node(&["hil", "evidence", "record"], &[], &["--run"]),
        node(&["hil", "evidence", "pending"], &[], &[]),
        node(&["hil", "owner"], &["set", "merge", "forget"], &[]),
        node(&["hil", "owner", "set"], &[], &[]),
        node(&["hil", "owner", "merge"], &[], &[]),
        node(&["hil", "owner", "forget"], &[], &[]),
        node(&["hil", "preempt"], &[], &["--reason"]),
        node(&["hil", "wait"], &[], &["--service"]),
    ];
    for (name, command) in [
        ("lease", LeaseCli::command()),
        ("board", BoardCli::command()),
        ("perf", PerfCli::command()),
        ("runs", RunsCli::command()),
        ("firmware", FirmwareCli::command()),
        ("devices", DevicesCli::command()),
        ("peer", crate::hil_board::PeerCli::command()),
        ("flash", crate::hil_flash::FlashCli::command()),
        ("profile", ProfileCli::command()),
        ("bisect", crate::hil_bisect::BisectCli::command()),
        ("ab", crate::hil_ab::AbCli::command()),
    ] {
        nodes.extend(walk(&command, &path(&["hil", name])));
    }
    // The stand makes `ab` a job as it does a run.
    if let Some(ab) = nodes
        .iter_mut()
        .find(|node| node.path == path(&["hil", "ab"]))
    {
        ab.flags
            .extend(["--enqueue", "--after", "--after-any"].map(String::from));
    }
    nodes.extend(runner_nodes.into_iter().skip(1));
    println!("{}", serde_json::to_string_pretty(&nodes)?);
    Ok(std::process::ExitCode::SUCCESS)
}

/// `cargo hil owner [set NAME | merge OLD NEW | forget NAME]`.
fn owner(ctx: &Context, args: &[OsString]) -> Result<std::process::ExitCode> {
    const USAGE: &str = "usage: cargo hil owner                 this checkout's owner
       cargo hil owner set NAME        register this checkout's owner
       cargo hil owner merge OLD NEW   charge OLD's balance and history to NEW
       cargo hil owner forget NAME     drop a balance that is no agent's";
    let args = args
        .iter()
        .map(|arg| arg.to_str().ok_or("arguments must be UTF-8"))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let arbiter = oer_hil_arbiter::Arbiter::open()?;
    let parse = |name: &str| oer_hil_arbiter::Owner::parse(name);
    match args.as_slice() {
        [] => match arbiter.checkout_owner(&ctx.root)? {
            Some(owner) => println!("{owner}"),
            None => return Err(oer_hil_arbiter::NoOwner(ctx.root.clone()).into()),
        },
        ["set", name] => {
            let owner = parse(name)?;
            arbiter.set_checkout_owner(&ctx.root, owner)?;
            println!("{} is owned by {owner}", ctx.root.display());
        }
        ["merge", old, new] => {
            let new = parse(new)?;
            arbiter.merge_owner(old, new)?;
            println!("{old}'s balance and history are {new}'s");
        }
        ["forget", name] => {
            let forgotten = arbiter.forget_owner(name)?;
            println!(
                "{name}: {}",
                if forgotten {
                    "balance dropped"
                } else {
                    "no balance"
                }
            );
        }
        _ => return Err(USAGE.into()),
    }
    Ok(std::process::ExitCode::SUCCESS)
}

/// Time a preempted holder gets for cancellation and cleanup, as at the hard
/// limit.
const PREEMPT_GRACE: std::time::Duration = std::time::Duration::from_secs(300);

/// `cargo hil preempt ID --reason TEXT`.
fn preempt(owner: &str, args: &[OsString]) -> Result<std::process::ExitCode> {
    const USAGE: &str = "usage: cargo hil preempt ID --reason TEXT";
    let args = args
        .iter()
        .map(|arg| arg.to_str().ok_or("arguments must be UTF-8"))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let (id, reason) = match args.as_slice() {
        [id, "--reason", reason] | ["--reason", reason, id] => (*id, *reason),
        _ => return Err(USAGE.into()),
    };
    let id = id
        .trim_start_matches('#')
        .parse::<u64>()
        .map_err(|_| format!("{id} is not a lease number\n{USAGE}"))?;
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
fn queue(args: &[OsString]) -> Result<std::process::ExitCode> {
    let json = match args {
        [] => false,
        [flag] if flag == "--json" => true,
        [flag] if flag == "--help" || flag == "-h" => {
            println!("usage: cargo hil queue [--json]\n\n{BALANCE_RULE}");
            return Ok(std::process::ExitCode::SUCCESS);
        }
        _ => return Err("usage: cargo hil queue [--json]".into()),
    };
    let status = oer_hil_arbiter::Arbiter::open()?.status()?;
    let store = crate::hil_jobs::Jobs::open()?;
    let jobs = store.unfinished();
    let ended = store.recently_ended_unjudged(std::time::Duration::from_secs(3600), 5);
    if json {
        let mut value = serde_json::to_value(&status)?;
        value["jobs"] = serde_json::to_value(crate::hil_jobs::views(&jobs, &status))?;
        value["ended_jobs"] = serde_json::to_value(&ended)?;
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("{status}");
        print!("{}", crate::hil_jobs::describe(&jobs, &status));
        print!("{}", crate::hil_jobs::describe_ended(&ended));
    }
    Ok(std::process::ExitCode::SUCCESS)
}

/// `cargo hil ab ...`, a job like a run: `--enqueue` starts it detached and
/// prints its id for `cargo hil wait`, and `--after JOB` orders it.
fn ab(ctx: &Context, owner: &str, args: &[OsString]) -> Result<std::process::ExitCode> {
    let (enqueue, after, args) = crate::hil_jobs::take(args.to_vec())?;
    if enqueue {
        let frozen = crate::hil_jobs::Frozen::capture(ctx)?;
        let id = crate::hil_jobs::enqueue(ctx, owner, &args, after, &frozen)?;
        eprintln!("hil: enqueued job {id}; `cargo hil wait {id}` blocks until it ends");
        println!("{id}");
        return Ok(std::process::ExitCode::SUCCESS);
    }
    let mut job = crate::hil_jobs::Running::begin(ctx, owner, &args, after.as_ref())?;
    // Every round runs the xtask and runner of the experiment's start: a pull
    // into the checkout meanwhile must not change the arms' protocol.
    let frozen = match crate::hil_jobs::Frozen::inherited()? {
        Some(frozen) => frozen,
        None => crate::hil_jobs::Frozen::capture(ctx)?,
    };
    let runs = crate::hil_ab::run(ctx, owner, &frozen, &args[1..])?;
    let (ids, outcomes): (Vec<_>, Vec<_>) = runs.into_iter().unzip();
    job.finish(&ids, &outcomes)?;
    Ok(std::process::ExitCode::SUCCESS)
}

/// `cargo hil wait ID`: block until the job or run ID names ends, and exit
/// with its outcome. A job is waited for through its record, a run by
/// following its bundle.
fn wait(ctx: &Context, args: &[OsString]) -> Result<std::process::ExitCode> {
    let [id] = args else {
        return Err("usage: cargo hil wait JOB|RUN, or cargo hil wait --service [BOARD...]".into());
    };
    let text = id.to_str().ok_or("an id is text")?;
    if crate::hil_jobs::Jobs::open()?.read(text).is_ok() {
        return crate::hil_jobs::wait_command(args);
    }
    // A run lies in the store of the chip it ran on.
    let stores = oer_chip_profile::supported(&ctx.root)?
        .iter()
        .map(|chip| crate::hil_store::shared_runs(chip))
        .collect::<Result<Vec<_>>>()?;
    let run = crate::hil_runs::find_in(&stores, text)
        .ok_or_else(|| format!("{text} is neither a job nor a run of any chip"))?;
    Ok(std::process::ExitCode::from(crate::hil_runs::wait(
        &run.directory,
    )?))
}

/// `cargo hil wait --service [BOARD...]`: block until the boards (every
/// board when none is named) and the stand are back in service.
fn wait_for_service(args: &[OsString]) -> Result<std::process::ExitCode> {
    let arbiter = oer_hil_arbiter::Arbiter::open()?;
    let devices = arbiter.devices()?;
    let macs = args
        .iter()
        .map(|board| {
            let board = board.to_str().ok_or("a board name is text")?;
            oer_hil_arbiter::board_mac(&devices, board)
        })
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
    devices: &[oer_hil_arbiter::Device],
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
        .map(|board| {
            Ok(oer_hil_arbiter::Claim::board(&oer_hil_arbiter::board_mac(
                devices, board,
            )?))
        })
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
    let (_, args) = LeaseOptions::split(args)?;
    let Some((command, rest)) = args.split_first() else {
        return Ok(());
    };
    let parsed = match command.to_str().unwrap_or_default() {
        "lease" => LeaseCli::try_parse_from(rest).map(drop),
        "board" => BoardCli::try_parse_from(rest).map(drop),
        "perf" => PerfCli::try_parse_from(rest).map(drop),
        "runs" => RunsCli::try_parse_from(rest).map(drop),
        "firmware" => FirmwareCli::try_parse_from(rest).map(drop),
        "devices" => DevicesCli::try_parse_from(rest).map(drop),
        "profile" => ProfileCli::try_parse_from(rest).map(drop),
        "flash" => crate::hil_flash::FlashCli::try_parse_from(rest).map(drop),
        "peer" => crate::hil_board::PeerCli::try_parse_from(rest).map(drop),
        _ => return Ok(()),
    };
    parsed.map_err(|error| {
        format!(
            "`cargo hil {}`: {}",
            command.to_string_lossy(),
            error.render().to_string().trim()
        )
        .into()
    })
}

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
    };
    let (program, arguments) = cli
        .command
        .split_first()
        .ok_or("cargo hil lease needs a COMMAND after --")?;
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
        claims: lease_claims(&cli.boards, cli.air, cli.stand, &arbiter.devices()?)?,
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
    let finished = |status: std::process::ExitStatus| (exit_code(status), status.success());
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

/// `cargo hil board flashed ...`: journal a flash made outside the runner.
fn board(
    ctx: &Context,
    options: &LeaseOptions,
    args: &[OsString],
) -> Result<std::process::ExitCode> {
    use clap::Parser as _;
    let flashed = match BoardCli::try_parse_from(args)? {
        BoardCli::Flashed(flashed) => flashed,
        BoardCli::Access(command) => {
            return crate::hil_board::board(ctx, options.owner(ctx)?, command);
        }
    };
    if flashed.image.is_none() {
        return Err("cargo hil board flashed needs --image NAME".into());
    }
    flashed.record(
        &oer_hil_arbiter::Arbiter::open()?,
        options.owner(ctx)?,
        String::from("cargo hil board flashed"),
    )?;
    Ok(std::process::ExitCode::SUCCESS)
}

/// `cargo hil runs list|why|compare|history|pin|unpin|prune` over the shared
/// run store.
/// `cargo hil perf`: gated measurements across commits and their baselines.
fn perf(
    ctx: &Context,
    options: &LeaseOptions,
    args: &[OsString],
) -> Result<std::process::ExitCode> {
    use crate::hil_perf;
    use clap::Parser as _;
    let store = crate::hil_store::shared_runs(HIL_TARGET)?
        .parent()
        .ok_or("the run store has no parent")?
        .to_owned();
    let directory = crate::hil_store::shared_runs(HIL_TARGET)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as u64;
    let find = |id: &str| -> Result<hil_perf::RunSummary> {
        crate::hil_runs::load(&directory.join(id))
            .map(|run| hil_perf::summary(&run))
            .ok_or_else(|| format!("no run {id} in {}", directory.display()).into())
    };
    let baselines = hil_perf::load_baselines(&store)?;
    match PerfCli::try_parse_from(args)? {
        PerfCli::Report {
            scenarios,
            measurement,
            since,
        } => {
            let runs = hil_perf::summaries_since(
                &directory,
                &store.join("perf-cache"),
                now.saturating_sub(since.as_millis() as u64),
            )?;
            print!(
                "{}",
                hil_perf::report(&runs, &scenarios, measurement.as_deref(), &baselines)
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
            let set = hil_perf::set_baseline(
                &store,
                &run,
                &scenarios,
                &reason,
                &options.owner(ctx)?,
                now,
            )?;
            println!("baseline {} for {}", run.id, set.join(", "));
        }
        PerfCli::Check { run } => {
            let regressions = hil_perf::regressions(&find(&run)?, &baselines);
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

fn runs(
    ctx: &Context,
    options: &LeaseOptions,
    args: &[OsString],
) -> Result<std::process::ExitCode> {
    use crate::hil_runs;
    use clap::Parser as _;
    // Each chip's runs lie in its own store; this checkout's runs directory
    // of a chip stands for that chip's store.
    let runs_of = |chip: &str| -> Result<PathBuf> {
        let local = ctx.root.join("target/hil").join(chip).join("runs");
        if local.exists() {
            Ok(local)
        } else {
            crate::hil_store::shared_runs(chip)
        }
    };
    let directory = runs_of(HIL_TARGET)?;
    let stores = oer_chip_profile::supported(&ctx.root)?
        .iter()
        .map(|chip| runs_of(chip))
        .collect::<Result<Vec<_>>>()?;
    // Pins and pruning stay with the esp32s31 store.
    let store = crate::hil_store::shared_runs(HIL_TARGET)?
        .parent()
        .ok_or("the run store has no parent")?
        .to_owned();
    let parsed = RunsCli::try_parse_from(args)?;
    // Reading every run takes long; commands about one run read only it.
    let all = || hil_runs::all_in(&stores);
    let find = |id: &str| {
        hil_runs::find_in(&stores, id).ok_or_else(|| format!("no run {id} in any chip's run store"))
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as u64;
    match parsed {
        RunsCli::List {
            scenario,
            outcome,
            image,
            since,
            limit,
        } => {
            let filter = hil_runs::Filter {
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
                println!("{}", hil_runs::list_line(run));
            }
        }
        RunsCli::Show { run } => print!("{}", hil_runs::show(&find(&run)?)),
        RunsCli::Why { run, tail } => print!("{}", hil_runs::why(&find(&run)?, tail)),
        RunsCli::Compare { a, b, measurement } => print!(
            "{}",
            hil_runs::compare(&find(&a)?, &find(&b)?, measurement.as_deref())
        ),
        RunsCli::History {
            scenario,
            measurement,
            limit,
        } => {
            let all = all()?;
            let containing = all
                .iter()
                .filter(|run| run.scenarios.iter().any(|s| s.id == scenario))
                .cloned()
                .collect::<Vec<_>>();
            let start = containing.len().saturating_sub(limit);
            print!(
                "{}{}",
                hil_runs::stability(&containing, &scenario),
                hil_runs::history(&containing[start..], &scenario, measurement.as_deref())
            );
        }
        RunsCli::Pin { run, reason } => {
            hil_runs::load(&directory.join(&run))
                .ok_or_else(|| format!("no run {run} in {}", directory.display()))?;
            hil_runs::set_pin(
                &store,
                &run,
                Some(
                    serde_json::json!({"by": options.owner(ctx)?, "reason": reason, "unix_millis": now}),
                ),
            )?;
        }
        RunsCli::Unpin { run } => hil_runs::set_pin(&store, &run, None)?,
        RunsCli::Flaky { since, minimum } => {
            let recent = now.saturating_sub(since.as_millis() as u64);
            let runs = all()?
                .into_iter()
                .filter(|run| run.started_millis >= recent)
                .collect::<Vec<_>>();
            print!(
                "{}",
                hil_runs::flaky_report(
                    &hil_runs::stabilities(&runs),
                    minimum,
                    &hil_runs::quarantined(&store)
                )
            );
        }
        RunsCli::Quarantine { scenario, reason } => hil_runs::set_quarantine(
            &store,
            &scenario,
            Some(
                serde_json::json!({"by": options.owner(ctx)?, "reason": reason, "unix_millis": now}),
            ),
        )?,
        RunsCli::Release { scenario } => hil_runs::set_quarantine(&store, &scenario, None)?,
        RunsCli::Prune {
            days,
            keep_failed,
            apply,
        } => {
            let all = hil_runs::all(&directory)?;
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
            if apply {
                let observers = hil_runs::collect_observers(&directory)?;
                if observers > 0 {
                    println!("deleted {observers} observer builds no kept run names");
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
    match FirmwareCli::try_parse_from(args)? {
        FirmwareCli::List => crate::firmware_catalog::list(ctx)?,
        FirmwareCli::Build { image } => {
            crate::firmware_catalog::build(ctx, &image)?;
        }
        FirmwareCli::Bootloader { chip } => {
            println!(
                "{}",
                crate::firmware_catalog::bootloader_build(ctx, &chip)?.display()
            );
        }
        FirmwareCli::Flash {
            image,
            board,
            jtag,
            if_changed,
        } => {
            let request = oer_hil_arbiter::Request {
                owner: options.owner(ctx)?,
                work: format!("firmware flash {image} --board {board}"),
                scenarios: Vec::new(),
                claims: Vec::new(),
            };
            crate::firmware_catalog::flash(ctx, &image, &board, jtag, if_changed, request)?;
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

/// `cargo hil devices [--json]` and `cargo hil devices set MAC ...`.
fn devices(
    ctx: &Context,
    options: &LeaseOptions,
    args: &[OsString],
) -> Result<std::process::ExitCode> {
    use clap::Parser as _;
    let cli = DevicesCli::try_parse_from(args)?;
    let arbiter = oer_hil_arbiter::Arbiter::open()?;
    match cli.command {
        Some(DevicesCommand::Set {
            mac,
            chip,
            name,
            reset_uart,
            en,
            boot,
        }) => {
            let control = match (reset_uart, en, boot) {
                (Some(serial), Some(en), Some(boot)) => Some(oer_hil_arbiter::Control {
                    reset: Some(oer_hil_arbiter::ResetControl {
                        via: oer_hil_arbiter::control::ResetVia::UartRtsDtr,
                        serial,
                        en,
                        boot,
                    }),
                    power: None,
                }),
                _ => None,
            };
            let device = arbiter.set_device(oer_hil_arbiter::Device {
                mac,
                chip,
                name,
                control,
                unknown: Default::default(),
            })?;
            println!("{} {}", device.mac, device.label());
            return Ok(std::process::ExitCode::SUCCESS);
        }
        Some(DevicesCommand::Reset { board, download }) => {
            let devices = arbiter.devices()?;
            let mac = oer_hil_arbiter::board_mac(&devices, &board)?;
            let reset = devices
                .iter()
                .find(|device| device.mac == mac)
                .and_then(|device| device.control.as_ref()?.reset.clone())
                .ok_or_else(|| {
                    format!("board `{board}` has no reset path; see `cargo hil devices set --reset-uart`")
                })?;
            let request = oer_hil_arbiter::Request {
                owner: options.owner(ctx)?,
                work: format!(
                    "devices reset {board}{}",
                    if download { " --download" } else { "" }
                ),
                scenarios: Vec::new(),
                claims: vec![oer_hil_arbiter::Claim::board(&mac)],
            };
            let _grant = arbiter.acquire(&request)?;
            let mode = if download {
                oer_hil_arbiter::BootMode::Download
            } else {
                oer_hil_arbiter::BootMode::Normal
            };
            let banner = reset.reset(mode)?;
            match oer_hil_arbiter::control::reset_line(&banner) {
                Some(line) => println!("{board} ({mac}) reset: {line}"),
                None => {
                    return Err(format!(
                        "{board} ({mac}) printed no ROM reset line after the reset: {banner:?}"
                    )
                    .into());
                }
            }
            return Ok(std::process::ExitCode::SUCCESS);
        }
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
                    let mac = oer_hil_arbiter::board_mac(&arbiter.devices()?, &board)?;
                    (board, mac)
                }
                (None, false) => unreachable!("clap requires a board or --stand"),
            };
            let owner = options.owner(ctx)?;
            arbiter.set_maintenance(oer_hil_arbiter::Maintenance {
                mac: mac.clone(),
                owner: owner.clone(),
                reason,
                since_unix: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_secs(),
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
            let mac = oer_hil_arbiter::board_mac(&arbiter.devices()?, &board)?;
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
                        crate::hil_board::boots(&board)
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
fn collect_objects(ctx: &Context) -> oer_hil_evidence::build::CollectedObjects {
    let mut checkouts = vec![ctx.root.clone()];
    if let Ok(arbiter) = oer_hil_arbiter::Arbiter::open()
        && let Ok(registered) = arbiter.registered_checkouts()
    {
        checkouts.extend(registered);
    }
    checkouts.sort();
    checkouts.dedup();
    let mut total = oer_hil_evidence::build::CollectedObjects::default();
    for checkout in checkouts {
        let target = checkout.join("target/hil").join(HIL_TARGET);
        match oer_hil_evidence::build::collect_objects(&target) {
            Ok(collected) => {
                total.objects += collected.objects;
                total.bytes += collected.bytes;
            }
            Err(error) => eprintln!(
                "hil: cannot collect the firmware objects of {}: {error}",
                checkout.display()
            ),
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
    // Over its size budget the store also loses its oldest runs that only
    // their age kept; every other rule still holds.
    let remaining = all
        .iter()
        .filter(|run| keep.contains_key(&run.id))
        .cloned()
        .collect::<Vec<_>>();
    let sizes = remaining
        .iter()
        .map(|run| (run.id.clone(), hil_runs::exclusive_bytes(&run.directory)))
        .collect();
    let kept = hil_runs::retained(
        &remaining,
        &hil_runs::Retention {
            keep_days: 0,
            keep_failed: PRUNE_KEEP_FAILED,
        },
        now,
        &hil_runs::pins(store).into_keys().collect(),
        &hil_runs::cited_by_shards(&ctx.root),
    );
    let budget = run_store_budget()?;
    for run in hil_runs::over_budget(&remaining, &kept, &sizes, budget) {
        freed += sizes[&run.id];
        std::fs::remove_dir_all(&run.directory)?;
        removed += 1;
    }
    let objects = collect_objects(ctx);
    if objects.objects > 0 {
        eprintln!(
            "hil: deleted {} firmware objects no run links to ({} MiB)",
            objects.objects,
            objects.bytes >> 20
        );
    }
    if removed > 0 {
        hil_runs::collect_observers(&runs)?;
        eprintln!(
            "hil: pruned {removed} runs ({} MiB) no rule keeps; see `cargo hil runs prune`",
            freed >> 20
        );
    }
    Ok(())
}

/// Make this checkout's run directory of every supported chip a link to the
/// shared store.
fn use_shared_store(ctx: &Context) -> Result<()> {
    for chip in oer_chip_profile::supported(&ctx.root)? {
        let local = ctx.root.join("target/hil").join(&chip).join("runs");
        crate::hil_store::link_runs(&local, &crate::hil_store::shared_runs(&chip)?)?;
    }
    Ok(())
}

/// The HIL target the runner executes on.
pub(crate) const HIL_TARGET: &str = "esp32s31";

/// Check an enqueued `run`'s scenarios and options with the runner now, so
/// a mistake shows in the terminal instead of in a job that ends no-run
/// minutes later.
/// Check an enqueued command and fix what it runs with: this xtask, the
/// runner and, for a `run` that builds, a source snapshot of the checkout
/// taken now with the run's own source options.
fn freeze_enqueued(ctx: &Context, args: &[OsString]) -> Result<crate::hil_jobs::Frozen> {
    let mut frozen = crate::hil_jobs::Frozen::capture(ctx)?;
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
    ctx: &Context,
    frozen: &crate::hil_jobs::Frozen,
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
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()
        .ok()?;
    output.status.success().then_some(())?;
    checkout_of_common_dir(Path::new(String::from_utf8(output.stdout).ok()?.trim()))
}

/// The checkout whose `.git` directory is `common`.
fn checkout_of_common_dir(common: &Path) -> Option<PathBuf> {
    (common.file_name()? == ".git").then(|| common.parent().map(Path::to_owned))?
}

/// Leave quarantined scenarios out of a `run-all`, and warn of a `run` that
/// names one.
fn apply_quarantine(args: &mut Vec<OsString>) -> Result<()> {
    let store = crate::hil_store::shared_runs(HIL_TARGET)?
        .parent()
        .ok_or("the run store has no parent")?
        .to_owned();
    let quarantined = crate::hil_runs::quarantined(&store);
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
                        entry["reason"].as_str().unwrap_or("no reason")
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

/// The runner's run receipt variable; see oer-hil-evidence.
const RUN_RECEIPT_ENV: &str = "OER_HIL_RUN_RECEIPT";

/// Name `run_ids` in the receipt of whatever invoked this command (a
/// bisection's step), when it asked for one.
fn forward_run_receipt(run_ids: &[String]) -> Result<()> {
    let Some(path) = std::env::var_os(RUN_RECEIPT_ENV) else {
        return Ok(());
    };
    use std::io::Write as _;
    let mut receipt = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    for id in run_ids {
        writeln!(receipt, "{id}")?;
    }
    Ok(())
}

/// Whether each of `run_ids` was built from a dirty tree. A run lies in the
/// store of the chip it ran on; one whose manifest no store holds counts as
/// dirty.
fn runs_dirty(stores: &[PathBuf], run_ids: &[String]) -> Vec<bool> {
    run_ids
        .iter()
        .map(|id| {
            stores
                .iter()
                .find_map(|store| std::fs::read(store.join(id).join("manifest.json")).ok())
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .and_then(|manifest| manifest["repository"]["dirty"].as_bool())
                .unwrap_or(true)
        })
        .collect()
}

/// Why a finished invocation records no evidence: it created no run, a run
/// came from a dirty tree, or untracked sources were added with
/// `--source-include` or `--include-untracked`. Such runs are experiments;
/// `cargo hil evidence record --run ID` records them anyway.
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
fn remember_pending(ctx: &Context, owner: Option<String>, run_ids: &[String]) -> Result<()> {
    let store = crate::hil_store::shared_runs(HIL_TARGET)?;
    let pending = run_ids
        .iter()
        .map(|run| crate::hil_evidence::Pending {
            run: run.clone(),
            scenarios: crate::hil_evidence::passed_scenarios(&store.join(run)),
            owner: owner.clone(),
        })
        .collect::<Vec<_>>();
    crate::hil_evidence::remember(&ctx.root, &pending)?;
    let passed = pending
        .iter()
        .filter(|run| !run.scenarios.is_empty())
        .map(|run| format!(" --run {}", run.run))
        .collect::<String>();
    if !passed.is_empty() {
        eprintln!(
            "hil: evidence not recorded in hil/evidence; record it with the change it qualifies: \
             cargo hil evidence record{passed}"
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

/// Record the observations that qualify on this checkout as tracked HIL
/// evidence shards, with the observer receipt the runs were produced under.
/// Failed scenarios record nothing, so this also runs after a failing suite.
pub(crate) fn record_evidence(
    ctx: &Context,
    receipt: &std::path::Path,
    run_ids: &[String],
) -> Result<()> {
    // Every checkout's runs share one store, so recording evidence reads all
    // of them. One recording at a time on the host, each in a memory-capped
    // scope, keeps simultaneous runs of several agents from exhausting it.
    let lock_path = evidence_lock_path()?;
    std::fs::create_dir_all(lock_path.parent().ok_or("lock path has no parent")?)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)?;
    if fs2::FileExt::try_lock_exclusive(&lock).is_err() {
        eprintln!("hil: waiting for another checkout's evidence recording");
        fs2::FileExt::lock_exclusive(&lock)?;
    }
    let mut command = if std::process::Command::new("systemd-run")
        .args(["--user", "--scope", "-q", "--", "true"])
        .status()
        .is_ok_and(|status| status.success())
    {
        let mut capped = ctx.command("systemd-run");
        capped.args([
            "--user",
            "--scope",
            "-q",
            "-p",
            EVIDENCE_MEMORY_MAX,
            "-p",
            "MemorySwapMax=0",
            "--",
            "cargo",
        ]);
        capped
    } else {
        ctx.command("cargo")
    };
    let status = command
        .args(["qualification", "hil-evidence", "--hil-target", HIL_TARGET])
        .args(run_ids.iter().flat_map(|id| ["--run", id.as_str()]))
        .env("OER_OBSERVER_RECEIPT", receipt)
        .status()?;
    if !status.success() {
        return Err(format!(
            "recording the HIL evidence shards failed (it runs under {EVIDENCE_MEMORY_MAX})"
        )
        .into());
    }
    Ok(())
}

/// Memory limit of one evidence recording.
const EVIDENCE_MEMORY_MAX: &str = "MemoryMax=6G";

fn evidence_lock_path() -> Result<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".cache"))
        })
        .ok_or("HOME is required to locate the evidence lock")?;
    Ok(base.join("open-esp-radio/qualification/hil-evidence.lock"))
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

// The clap parsers of the stand commands, walked by `__command-tree`.
#[derive(clap::Parser)]
#[command(name = "cargo hil board", no_binary_name = true)]
enum BoardCli {
    /// Record a flash performed outside the HIL runner.
    Flashed(FlashedArgs),
    #[command(flatten)]
    Access(crate::hil_board::BoardCommand),
}

#[derive(clap::Parser)]
#[command(name = "cargo hil perf", no_binary_name = true)]
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
#[derive(clap::Parser)]
#[command(name = "cargo hil profile", no_binary_name = true)]
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
fn profile(ctx: &Context, args: &[OsString]) -> Result<std::process::ExitCode> {
    use clap::Parser as _;
    let cli = ProfileCli::try_parse_from(args)?;
    let run = crate::hil_store::shared_runs(HIL_TARGET)?.join(&cli.run);
    if !run.is_dir() {
        return Err(format!("no run {} in the store", cli.run).into());
    }
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
            let report = oer_hil_execution::profile::report(&path, elf.as_deref(), cli.top)?;
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
            fs::write(&destination, &report)?;
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

#[derive(clap::Parser)]
#[command(name = "cargo hil firmware", no_binary_name = true)]
enum FirmwareCli {
    /// Catalog images: name, chip, project and last build.
    List,
    /// Build an image against the pinned ESP-IDF.
    Build { image: String },
    /// Build a chip's project bootloader and print its ESP-IDF build
    /// directory; the runner asks for it to flash an ESP-IDF application.
    Bootloader { chip: String },
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
    /// Register or change a board's chip, name and reset path.
    Set {
        mac: String,
        #[arg(long)]
        chip: Option<String>,
        #[arg(long)]
        name: Option<String>,
        /// USB serial number of a USB-to-UART bridge whose modem lines
        /// drive the chip's EN and BOOT; needs `--en` and `--boot`.
        #[arg(long, value_name = "USB_SERIAL", requires_all = ["en", "boot"])]
        reset_uart: Option<String>,
        /// The bridge line that pulls EN low.
        #[arg(long, value_name = "rts|dtr", requires = "reset_uart")]
        en: Option<oer_hil_arbiter::control::Line>,
        /// The bridge line that pulls the boot strap low.
        #[arg(long, value_name = "rts|dtr", requires = "reset_uart")]
        boot: Option<oer_hil_arbiter::control::Line>,
    },
    /// Reset a board through its registered reset path under a lease of
    /// that board, and print the reset reason its ROM reports.
    Reset {
        #[arg(value_name = "NAME|MAC")]
        board: String,
        /// Hold the boot strap low: the ROM waits for a download.
        #[arg(long)]
        download: bool,
    },
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
    use super::{
        OsString, Path, PathBuf, checkout_of_common_dir, has_flag, runs_dirty, source_options,
        with_source_snapshot,
    };

    #[test]
    fn a_run_is_found_clean_in_the_store_of_the_chip_it_ran_on() {
        let esp32s31 = tempfile::tempdir().unwrap();
        let esp32c5 = tempfile::tempdir().unwrap();
        for (store, id, dirty) in [(&esp32s31, "a", true), (&esp32c5, "b", false)] {
            let run = store.path().join(id);
            std::fs::create_dir(&run).unwrap();
            std::fs::write(
                run.join("manifest.json"),
                format!(r#"{{"repository":{{"dirty":{dirty}}}}}"#),
            )
            .unwrap();
        }
        let stores = [esp32s31.path().to_owned(), esp32c5.path().to_owned()];
        let ids = ["a", "b", "missing"].map(String::from);
        assert_eq!(runs_dirty(&stores, &ids), [true, false, true]);
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
        assert!(!report.starts_with(root.join("target/hil/esp32s31/runs")));
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
        assert!(!produces_runs(&args(&["plan", "--scenario", "x"])));
        assert!(!produces_runs(&args(&["doctor"])));
    }

    #[test]
    fn lease_options_precede_the_command_and_stop_at_the_first_other_argument() {
        let args = |values: &[&str]| values.iter().map(OsString::from).collect::<Vec<_>>();
        let (options, rest) =
            LeaseOptions::split(&args(&["--owner", "phy", "run", "x", "--owner", "kept"])).unwrap();
        assert_eq!(
            options,
            LeaseOptions {
                owner: Some("phy".into()),
            }
        );
        assert_eq!(rest, args(&["run", "x", "--owner", "kept"]));
        // The runner takes no lease options: they may follow its command.
        let (options, rest) = LeaseOptions::default()
            .with_late(&args(&["run", "x", "--owner", "phy", "y", "--", "--owner"]))
            .unwrap();
        assert_eq!(options.owner.as_deref(), Some("phy"));
        assert_eq!(rest, args(&["run", "x", "y", "--", "--owner"]));
        let (options, _) = options.with_late(&args(&["run", "--owner=phy"])).unwrap();
        assert_eq!(options.owner.as_deref(), Some("phy"));
        assert!(
            options
                .clone()
                .with_late(&args(&["run", "--owner", "bt"]))
                .is_err()
        );
        assert!(
            LeaseOptions::default()
                .with_late(&args(&["run", "--owner"]))
                .is_err()
        );
        let (options, rest) = LeaseOptions::split(&args(&["run", "x"])).unwrap();
        assert_eq!(options, LeaseOptions::default());
        assert_eq!(rest.len(), 2);
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
            Command::new("sleep").arg("60"),
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

    #[test]
    fn a_lease_claims_its_boards_and_the_air_or_explicitly_the_whole_stand() {
        use oer_hil_arbiter::{AIR, Claim, Mode};
        let devices = [oer_hil_arbiter::Device {
            mac: "38:44:BE:AA:25:64".into(),
            name: Some("esp32c5".into()),
            ..oer_hil_arbiter::Device::default()
        }];
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
