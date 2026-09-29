# HIL runs and investigation

Every run leaves an immutable bundle in the shared run store. This guide covers
finding and comparing runs, what the runner records when a repetition fails and
how it brings a board back, and the tools for experiments: enqueued runs,
bisection, A/B comparison, performance tracking, profiles and layout seeds.
The bundle contract is in the [architecture](architecture.md).

## Find and compare runs

Every checkout's `target/hil/esp32s31/runs` is a link to one store shared by
all checkouts of this user, `~/.local/share/open-esp-radio/hil/esp32s31/runs`
(`$XDG_DATA_HOME`, or `$OER_HIL_STORE/<target>/runs`). `cargo hil` creates the
link, and refuses a checkout whose own run directory holds runs. Qualification reads the store from any checkout and still decides
per bundle whether it applies to that checkout's sources.

```console
cargo hil runs list --scenario station-reconnect --outcome failed --since 7d
cargo hil runs show <run-id>          # where its artifacts are
cargo hil runs why <run-id>
cargo hil runs compare <run-a> <run-b> --measurement mbps
cargo hil runs history <scenario> --measurement mbps
cargo hil runs flaky --since 7d      # scenarios that did not always pass
cargo hil runs quarantine <scenario> --reason "loses its hello" # out of run-all
cargo hil runs release <scenario>
cargo hil runs pin <run-id> --reason "A/B baseline"
cargo hil runs prune                  # list what the rule would delete
cargo hil runs prune --apply
```

`list` shows each run's time, outcome, checkout, commit (`+` when dirty),
images and scenario outcomes, filtered by scenario, outcome, image class or
digest prefix and age. `show` prints the run's
directory in the store, its reports, and per scenario and repetition the
outcome, the artifact directory and the files in it; use it rather than
`find`, which does not follow the store link. `why` names, per failed
repetition, the recorded failure, the measurements that missed their
criteria (a criterion miss, unlike a fault), cleanup failures, host USB
events of its boards, the artifact directory and the end of `uart.log`. `compare` and `history` use per-scenario
means of numeric measurements over repetitions. These views never decide
qualification. `cargo hil wait RUN` follows a run's `events.jsonl`, printing
each step, until the run ends or its runner is gone, and exits with its
outcome: 0 passed, 1 failed, broken, blocked or skipped, 2 interrupted,
abandoned or on a quarantined board; an agent that started a run in the
background waits on it instead of polling its log. The same `cargo hil wait`
takes a job id (below) or `--service`. `why`, `compare` and `pin` read only the
runs they name.

## Failed repetitions and board recovery

When a repetition fails, the runner attaches to the target without resetting
it and asks for its boot evidence, waiting up to 20 s for the target's hang
watchdog to reset a stalled image. A post-mortem hang makes the failure
`hang`: which executor stalled, where each hart was, named from the run's
archived `runtime.elf` (function, the function it was inlined into, and the
source line), and the code the stalled hart kept running; a core 1 stall seen
together with a core 0 stall is reported as depending on core 0's timers.
The watchdog also sees a task that awaits forever while both executors run:
a task whose work can wait for it owns a named slot (`console`,
`session-evidence`) in `oer_hil_target_core::liveness`, armed while its work
waits, and a slot armed past its deadline (5 s for the console, 10 s for
session evidence) is a hang of that task, reported as `hang: task <slot> made
no progress for N ms while executors ran`. A
reset the runner did not cause (neither USB Serial/JTAG nor JTAG) makes it
`unexpected-reset` with the reset reason and code. Both name the last
checkpoints the ended boot passed. The exchange and the workload's own
failure are kept in the repetition's `post-mortem/`. When the image keeps an
[event trace](../protocol/diagnostics.md#event-trace), the runner pages it
out into `post-mortem/trace.json`, raw, and `post-mortem/trace.txt`, decoded
oldest first, then lets the target record again; `runs why` shows the
trace's last events.

A traffic session that ends without passing names its first failed check in
its `Finished` event (a typed `SessionFailure`: no datagrams, no terminal
marker, receive, socket or transmit errors, an incomplete TCP connection or
transfer, a pattern mismatch, receive loss, a failed ownership audit, or
control-link corruption), and the runner puts it in the repetition's failure
message. At the start of every repetition the runner resolves the kernel USB
device (`3-8`) behind the device's and the reference peer's serial ports; at
its end it reads their disconnects and enumerations since the start from the
kernel log (`journalctl --dmesg`) into `usb-events.json`, written only when
there are any. A board whose USB bridge dropped off the bus mid-session thus
shows as such rather than as a silent target. When the kernel log cannot be
read, the runner says so and the repetition's result stands.

When the image preflight gets no hello and the console ends in a boot loop
(at least two consecutive ROM resets of the same kind, such as `rst:0x7
(HP_SYS_HP_WDT0_RESET)` right after the second-stage bootloader), the RTS
reset and the reflash have not cleared it: firmware can leave low-power and
PMU state (a powered-down MPLL) that survives both. The runner escalates: a
system reset through the chip's builtin USB-JTAG (OpenOCD `reset run`, whose
executable the `cargo hil` wrapper passes to the runner), then EN and the hub
port's power when the board has them, and after each step asks the image for
its capabilities again; the first step after which it answers clears the loop
and the repetition continues. Every step and its ROM line are recorded in the
repetition's `reset-escalation.json` and a clearing step in the board journal.
When no step clears it, the board is quarantined with trigger `boot-loop` for
a person, and runs and `cargo hil wait --service` wait for its release.

A target that does not answer within those 20 s is first read through its
JTAG, where the stand's OpenOCD reaches it: the runner halts the current hart,
reads `pc`, `ra`, `sp`, `mcause`, `mepc`, `mtval` and `mstatus`, lets it run on
without a reset, names the code addresses from the image's ELF and writes them
to `post-mortem/jtag.json`, so the place it stopped survives the resets below.
The read never changes the repetition's outcome.

A target that does not answer within those 20 s climbs the recovery ladder:
an EN pulse through the board's registered reset path, or an RTS pulse on its
own USB Serial/JTAG port when it has none, then the same query again. No
board is on a switchable hub port, so the stand cannot power-cycle one; a
board neither reset brings back needs a person. The failure then names where core 0 was when the reset hit, from the
ROM banner's saved program counter symbolized like a hang: stuck in code, or
idle in its executor. A step that brings it back is journaled as a recovery, `hardware` when
the port had vanished or the ROM waited for a download. When the ROM answers
a reset, booting from flash or waiting for a download, but the firmware does
not, the failure names a firmware or host fault: the stand can reflash the
board, so it goes on serving. The run then records its remaining repetitions of that
image class as `blocked` without touching the board, so a broken image frees
the lease within about a minute instead of repeating the wait. Only a board whose ROM stays silent after every
reset path the stand has (EN, then RTS), which no script can bring back to a
state where firmware can be loaded, is quarantined; frequent recoveries are
shown as its health, never a quarantine. A cancelled run judges no board. On a
quarantine the repetition and the run's remaining repetitions end
`board-quarantined`,
which is no verdict on the code under test, every request for the board is
refused with the reason, and the user is notified. What the stand saw stays in
the repetition's `post-mortem/`, which the quarantine names. After pressing
the board's reset button or power-cycling it, `cargo hil devices release
BOARD --confirm reset|power-cycle` returns it once an RTS reset shows it
booting from flash; the release is journaled. `--confirm rom-answers` returns
a board nobody touched, such as one an older runner quarantined although its
ROM answered: the same RTS check must show the ROM booting.

## Operational commands without a rebuild

`cargo hil` builds this xtask from the caller's working tree before every
command. For the operational commands (`queue`, `lease`, `devices`, `board`,
`runs`, `perf`, `dashboard`) use the installed tool instead:

```console
cargo xtask stand-install      # build origin/main's xtask once, install oer-stand
oer-stand queue
oer-stand runs why <run-id>
```

`oer-stand` runs the `main` build against the caller's checkout (its Git top
level), so owners and local paths are the caller's and every checkout writes the
shared arbiter state with one binary. Reinstall after arbiter or stand changes
land on `main`. Runs, image builds and flashing keep using `cargo hil`, which
builds the runner from the caller's sources.

## Program-counter profiles

A scenario with a [`[profile]` table](../scenarios/README.md#program-counter-profile)
arms the image's sampler right after each boot's hello; the workload opens
and closes the sampled window, and when the capture finishes the runner pages
out the raw `(pc, ra)` samples of both harts into the capture's
`profile.json`, with the target's status, and disarms it. A drain that fails
is recorded there as an error, never as a workload failure.

```console
cargo hil profile <run-id>                       # every profiled repetition
cargo hil profile <run-id> --scenario S --repetition 1 --top 20
```

`profile` symbolizes each `profile.json` against the run's own
`firmware/<image>/runtime.elf`, prints each hart's most sampled functions
(the outermost frame of the inline chain), their most frequent inlined frame
and the callers the return addresses name, and writes the report to this
checkout's `target/hil/profiles/<run>/<scenario>/<repetition>/profile.txt`;
it never writes into the sealed run bundle. A return address names the caller only while the sampled
function has not called another, so callers are exact for leaf functions.

## Code layout seeds

Throughput of code that runs from cached external memory depends on where
the linker happens to place each function, so an unrelated change can move a
figure by tens of percent. `run`, `run-all` and `image build` accept `--layout-seed N` (a nonzero u32): the runtime's
ordinary code and read-only data are linked in the order that seed shuffles
them to, through the platform's `OER_LAYOUT_SEED`. Without the flag the
runner removes any inherited `OER_LAYOUT_SEED`, so the natural order is the
only unrecorded layout. The seed is part of the image: seeded artifacts get
directories of their own, the build record's parameters name it, and the
run manifest's `firmware[]` entry carries `layout_seed`, which `report
verify` requires to equal the build record's. Comparing several seeds of the
same commit separates placement from the source change under test.

## Enqueued runs

```console
id=$(cargo hil run <scenario> --enqueue)          # returns at once with a job id
cargo hil run <other> --enqueue --after "$id"     # starts once that job has ended
cargo hil wait "$id"                              # blocks until the job ends
```

`run` and `run-all` with `--enqueue` record a job in the arbiter
directory's `jobs/`, start the same command detached in its own process group
(output in `target/hil/jobs/<id>.log`), print the job's id and return. The job
moves its record from pending to started to finished, with the typed outcome
of its runs (the worst of them) and their ids. `--after JOB` makes a run,
enqueued or in the foreground, start once that job has ended with a judged
run, passed or failed; after one that ended without (no run, blocked, broken,
interrupted, abandoned) it does not start and ends no-run itself, saying why,
so a broken chain stops at its first link. `--after-any JOB` starts it
whatever the outcome. `--enqueue` first has the runner check a `run`'s
scenarios, target and options (`run --validate-only`), so a mistake such as
an unknown scenario fails in the terminal instead of in the job. It also
fixes what the job runs with before it waits: a copy of the xtask below
`target/hil/jobs/xtask/`, the runner built now and, for a `run` that builds,
a source snapshot of the checkout taken now with the run's own
`--source-include` and `--include-untracked`. It then builds the run's images
from that snapshot and runs their audits (`run --build-only`), so a failing
build or audit ends the command in the terminal instead of the job after its
wait. Edits, pulls and rebuilds of the
checkout while the job waits or runs do not reach it; a job whose fixed parts
were deleted meanwhile fails and asks to be enqueued again. `wait JOB` blocks until the job ends and exits with its outcome:
0 passed, 1 failed, 2 interrupted (or on a quarantined board), 3 blocked or
skipped, 4 broken, 5 no run created, 6 abandoned (its process is gone without
finishing, told by its PID and start time). Every `run` and `run-all`, enqueued or in the foreground, is such a job from its start, so
`queue` and the dashboard's Preparing section show runs before they ask for
the stand: each with its phase, from the arbiter's holders and queue matched
by the job's process or its runner child (waits for a job, building images,
waiting for the stand, holding the stand). Building costs no balance and
takes no place in the queue. A job whose process is gone is recorded as
abandoned when the list is read. `queue` also lists the jobs that ended
without a judged run within the last hour (at most 5) with the last line of
their log, under `ended_jobs` in `queue --json`; job records are kept for a
week. `queue --json` has the jobs, with `phase`, under `jobs`; the dashboard also shows a whole-stand maintenance. An agent waits
with `wait`, never by looking for process names.

## Bisecting a scenario

```console
cargo hil bisect --good <commit> --bad <commit> --scenario <id> [--layout-seed N]
```

`bisect` searches the commits after `--good` up to `--bad` (which must
descend from it) for the first one at which the scenario does not pass. Each
tested commit is checked out, detached, in the bisection's worktree below
`target/hil/bisect/<id>/`. When its `PROTOCOL_VERSION` is this checkout's,
this checkout's runner judges it: `run --source-snapshot DIR` builds the
revision's firmware from a snapshot of the worktree and runs this checkout's
host code and scenario. A commit with another protocol version runs its own
runner, built in the worktree, while the bisection holds a whole-stand lease;
that runner uses a private arbiter directory with a copy of the stand's
`devices.json`.

A passed run makes the commit good and a failed one bad. A commit whose image
does not compile or does not link (told apart from the run's archived build
log), or whose own runner starts no run, is broken: neither good nor bad, and
the search probes the untested commit nearest the middle instead. Any other
run outcome (blocked, broken, interrupted, a quarantined board) stops the
bisection, since the next steps would meet the same stand. `report.json`
records every step (commit, subject, runner, verdict and run) and the
conclusion: the first bad commit (exit 0), the commits broken revisions leave
ambiguous (exit 1), or why it stopped (exit 2). Its runs record no evidence.

## A/B comparison

```console
cargo hil ab --a 'rev=main' --b 'rev=main;override:xarxa=/home/me/src/xarxa' \
  --scenario udp-rx-ht40-task-residence-saturated --repetitions 3 --layout-seeds 2
```

`ab` compares two variants of the firmware on the same scenarios. A variant
is a repository revision (`rev=`, HEAD when omitted) and, after `;`, local
checkouts that replace pinned dependencies (`override:esp-hal=`,
`override:embassy=`, `override:xarxa=`, the checkouts `ESP_HAL_ROOT`,
`EMBASSY_ROOT` and `OPEN_RADIO_XARXA_ROOT` name). A variant may also add or remove
runtime features of every image class it builds, `features=+trace,-psram-stack`:
its runs build with `cargo hil run --features`, which only an experiment
accepts, the build record keeps the delta, `report verify` checks the image's
features against it, and qualification never counts such an image as its
class's. Each arm's revision is
checked out in a worktree below `target/hil/ab/<id>/` and captured with its
overrides into a source snapshot; this checkout's runner builds and runs
both, so both revisions must have this checkout's `PROTOCOL_VERSION`. For
every layout seed `1..=K` the first round runs A, then B, building their
images; the remaining rounds replay those exact images. Each round, A then B
of one seed, holds a whole-stand lease of its own, so drift of the air and
the calibrations falls on both arms, and releases it: shorter work of owners
with a higher balance goes between rounds, and the queue estimates the next
round from the rounds before it. Every run records `experiment` (its id, arm and variant: the
commit and each override's path, commit and dirtiness) in its manifest and
no evidence. A comparison takes hours, so like a run it is a job: `--enqueue`
starts it detached and prints the job id for `cargo hil wait`, and `--after
JOB` orders it after another job. Every round runs the xtask and runner fixed
when the comparison was enqueued or started, so a pull into the checkout
meanwhile cannot change the protocol the arms are run with. A job enqueued
from a worktree without an owner of its own runs for the owner of the
checkout the worktree was added from.

The report takes one value per run and measurement (the mean over the run's
repetitions) and compares the arms with `hil_perf::compare`: each arm's mean,
deviation and count, the difference B − A with the half-width of its Welch
95 % confidence interval, and a verdict: `significant` (the interval excludes
zero and the difference is at least 2 % of A's mean) with the better arm,
`within-noise`, or `insufficient-repetitions` (fewer than 3 runs on a side).
A gated measurement's direction is its gate's; an ungated one is compared as
if higher were better and says so. `ab-report.json` beside the worktrees
holds the variants, every run and every comparison; the command prints a
summary.

## Performance across commits

A scenario's gated measurements, those with an `at-least` or `at-most`
threshold such as UDP rates, are its performance figures; the threshold also
fixes which direction is better.

```console
cargo hil perf report --since 7d                     # every gated scenario, per commit
cargo hil perf report udp-tx-ht40-ceiling --measurement host-rate
cargo hil perf baseline <run-id> --reason "owned-xarxa baseline"
cargo hil perf check <run-id>                        # exit 1 on a regression
```

`report` summarizes clean-commit runs per commit and code layout (`natural`
or `seed N`) as mean ± sample deviation over repetitions, marks figures past
their gate, and compares each row with the scenario's baseline. For a commit
measured with more than one layout it adds the spread of the layout means next
to the repetition noise: a spread well above that noise means placement, not
the source, moves the figure. A move in the worse direction by more than twice the
baseline's deviation and 2 % of its mean is `REGRESSED`; the same margin the
other way is `improved`. `baseline` accepts only a completed run of a clean
commit and records its network implementation; baselines live in
`perf-baselines.json` beside the store and are shared by every checkout.
Summaries of sealed runs are cached in `perf-cache/`, so a report reads each
bundle once. These views never decide qualification.

After every command that executes scenarios, `cargo hil` applies this rule
to the shared store, at most once a day, and reports what it deleted.
`flaky` counts every scenario's repetitions over the period (7 days by
default) and lists those with at least `--minimum` (3) that did not always
pass, least passing first: how many passed, how many failed as the code under
test (`failed`) and how many as the stand (`broken` or `blocked`), whether the
scenario both passed and did not (flaky), its newest failure and whether it is
quarantined. `quarantine` records a scenario with its reason in the
esp32s31 store's `quarantine.json`: `run-all` then leaves it out, and a `run`
that names it runs it with a warning; `release` takes it back. Quarantine
changes no run's outcome or evidence.

`prune` keeps pinned runs, runs cited by committed evidence shards, runs whose
firmware another run replayed, runs younger than `--days` (30), incomplete
runs, the latest pass and the `--keep-failed` (5) newest failures of every
scenario and image class. The automatic rule also holds the esp32s31 store to
a size budget, `OER_HIL_RUN_STORE_BUDGET_GIB` (40 GiB by default): over it, it
deletes the oldest runs that only their age kept until the store fits, and
every other reason above still keeps its runs. Run by hand without `--apply` it only lists the other runs and
the bytes only they hold; hard-linked firmware shared with kept runs is not
counted. Pins live in the store's `pins.json`. Pruning with `--apply`, and the
automatic rule, also delete the observer builds in the store's `observers/`
that no remaining run names and that were stored more than an hour ago; a
starting run stores its build before its manifest names it. Run verification
requires every build there to hash to its name. They also delete the firmware
objects of this checkout's and every registered checkout's
`target/hil/<target>/objects/` that no run links to any more: a run's
firmware is a hard link to its object, so an object with a single link serves
no run and only saves a later identical archive a copy. Collection holds the
store's lock exclusively, and archiving holds it shared, so no run reuses an
object while it is deleted.
