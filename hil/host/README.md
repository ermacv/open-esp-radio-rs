# HIL host setup and operations

Run commands in this guide from the repository root. HIL executes the
production driver on real hardware and records typed evidence; it is not an
alternative radio implementation or the qualification evaluator. Read the
[execution and evidence architecture](architecture.md) before interpreting a
bundle.

```text
hil/
├── protocol/          host/target command and telemetry wire protocol
├── scenarios/         versioned, non-secret host workloads and criteria
├── host/
│   ├── runner/        build, flash and scenario orchestration
│   ├── linux-net/     privileged Linux AP/monitor fixture
│   └── linux-bluetooth/ privileged Linux Bluetooth fixture
└── targets/
    └── esp32s31/      current embedded target workspace
```

Target firmware lives under `hil/targets/<chip>`. Machine-readable evidence
lives in immutable bundles under `target/hil/<chip>/runs`. The qualification
evaluator independently checks those bundles; Markdown is not proof input.

Vendor-linked oracles remain isolated under `verification`; they are
not HIL scenarios or runner commands.

The ownership map and bundle contract are in the
[execution and evidence architecture](architecture.md).

`cargo hil` builds the observer through xtask using Cargo's actual artifact
messages, saves its executable receipt, and atomically publishes
`target/hil/current-observer.json`. Cargo uses `--locked`; downloads of pinned
dependencies remain allowed. Set `CARGO_NET_OFFLINE=true` or Cargo's
`net.offline` configuration when offline execution is required. Cancellation of
xtask is forwarded to its owned runner process group, with up to five minutes
for fixture cleanup and evidence sealing; this does not limit campaign runtime.
The wrapper returns the runner's exit code (or `128 + signal` on Unix).

Prepare the same observer descriptor without running HIL with:

```console
cargo xtask hil-observer
```

Qualification reads this descriptor once per evaluation, or the invocation's
receipt selected by `OER_OBSERVER_RECEIPT`. It never builds or executes the
observer. The descriptor selects the required compiler, features and profile;
prepare it again to select a different build configuration. Current normal/build manifests, lock identities and Cargo configuration are
checked once before use. Changes in any domain's normal/build declarations
require explicit preparation because they may alter shared feature unification;
dev-only declarations do not. Rust source and fixture changes are checked only
within the observation's workload scope.
Missing, invalid or stale configuration is reported as
`current-observer-configuration-unavailable`; historical observations remain
visible. Source compatibility is still checked per workload.

## Configure the lab

Run the host interface through the workspace alias:

```console
mkdir -p ~/.config/open-esp-radio
cp hil/lab.example.toml ~/.config/open-esp-radio/lab.toml
chmod 0600 ~/.config/open-esp-radio/lab.toml
cargo hil doctor
```

The lab configuration is `~/.config/open-esp-radio/lab.toml` (or under
`$XDG_CONFIG_HOME`), shared by every checkout of this user. A checkout's
`hil/local.toml` replaces it for that checkout, with a notice; `--lab-config`
names another file. The lab configuration is the only source for the stable
lab-cell and DUT identities, serial devices or board references (a name or MAC
from `cargo hil devices`), STA/AP credentials and addresses, startup artifact and OpenWrt
fixture. It is ignored by Git; scenarios contain no lab secrets or
machine-specific paths. The identities are written into every run manifest so
results from different cells and boards cannot be silently mixed.

Each device under test belongs to a chip: `[targets.<chip>]` names it (its
identity, serial port or board, startup artifact), keyed by a chip id with a
profile in `platform/<chip>/chip.toml`. `[device]` is the same as
`[targets.esp32s31]` and stays the default; another chip's board is resolved
only when a run for that chip asks for it, so an absent board fails only such
a run.

Multi-boot station lifecycle scenarios require a configured startup artifact.
Their first boot may create or replace it; every later boot must report
`Restored` before the station lifecycle can qualify. This makes cold PHY cache
replay an asserted transition rather than an informational UART message.

Peer scenarios run against a reference peer board, an ESP32-C5 with an
ESP-IDF catalog image: the [IEEE 802.15.4 peer](../peers/esp32c5-ieee802154/README.md),
the [Thread peer](../peers/esp32c5-openthread/README.md) or the
[Bluetooth LE Direct Test Mode peer](../peers/esp32c5-ble-dtm/README.md).
Name the board's stable identity and serial port, or its registered board
name, in the `[peer]` table (`[ieee802154_peer]`, its earlier name, is read
the same). A run of a peer scenario claims the peer board beside the device
under test and the air in its one lease. Before the first scenario that
needs an image, the runner brings the board to that image's current catalog
build with `cargo hil firmware flash IMAGE --if-changed`, and writes the
image, application digest and commit the board journal recorded for it into
the scenario's `peer-image.json`, which the scenario's seal covers. Each
workload takes the running peer over with its `SYNC` command instead of a
reset: a USB Serial/JTAG reset of an ESP32-C5 whose radio runs can leave it in
ROM download. The peer drivers share one console transport,
`hil_core::fixture::peer_line`, which also records the transcript of every
exchange.

## Share the stand

Every checkout of this user shares the stand through the
[arbiter](arbiter/README.md). A lease claims the resources its work uses:
boards by MAC, fixtures such as the laptop radio, the OpenWrt host or the
Bluetooth adapter, and the air, shared by all radio work and exclusive for
scenarios tagged `air-exclusive` that measure the radio environment. Leases
whose claims do not conflict run in parallel; conflicting requests are served
by their owners' balances (below). Hardware commands (`run`, `run-all`, `fixture check`, the Bluetooth
fixture check and `cargo xtask build firmware <example> --flash`) wait for
their claims instead of failing when they are busy. A run builds its images
before it queues, so the lease covers flashing and execution only.

```console
cargo hil queue                       # holders, queue with expected starts, board state
cargo hil dashboard                   # the same, live, with recent runs: http://127.0.0.1:8765
cargo hil run <scenario>              # one run, one lease
cargo hil run a b c                   # one run of three scenarios, one lease
cargo hil lease --board esp32c5 -- idf.py -p <port> flash
cargo hil --owner phy lease --board esp32s31 -- sh -c 'cargo hil run a --firmware-from R && cargo hil run a'
```

`flash --board NAME|MAC ELF` is the manual cycle for images outside the
runner and the ESP-IDF catalog, such as a new chip's first no_std images. It
derives the application image with `espflash save-image` before queueing,
leases only that board and the air (shared; `--air exclusive` for RF
measurements; `--air none` for an image that never enables the radio, which
then runs beside an exclusive air lease), lets
`espflash` write the chip's project bootloader, the partition table and the
application for the board's registered chip, and journals the image under
`--image` or the ELF's file name together with the bootloader digest. It then
resets the board into the application with the RTS line, the boot strap
released: `espflash`'s own reset and monitor connect to the ROM loader first
and leave an esp32c5 in download mode. `--monitor 30s` captures the console,
read from the port itself, into
`target/hil/flash/<mac>/console-*.log` for at most that long; with
`--until TEXT` it ends at the first line containing TEXT and fails when none
does. `--via jtag` writes the bootloader, partition table and
application merged from offset 0 through OpenOCD and the chip's JTAG, and
resets it through the debug module; the console is opened first without
touching the reset lines. It works over any running image, including one that
breaks USB Serial/JTAG resets (see [Hardware errata](../../docs/hardware-errata.md)).
`cargo hil firmware flash IMAGE --board BOARD --jtag` does the same for a
catalog image, writing each of its flash files at its address. The OpenOCD
build comes from the ESP-IDF tools in the shared cache. The lease ends with the capture, so an open monitor never holds a board.

```console
cargo hil flash --board esp32c5 --monitor 30s --until READY target/.../app.elf
```

`dashboard` serves a page on the loopback interface (`--port` changes the
port) that refreshes every two seconds: holders with their time held, the
queue in service order with every owner's balance and expected starts, every board's port and last flash, the newest runs
of the shared store and recent leases with their outcomes. It only reads the
arbiter's state and run manifests; Ctrl+C stops it. One dashboard serves the
host and names itself in `dashboard.json` of the arbiter directory: starting
the same build again prints the running one's address, and another build
stops it and takes over. A dashboard exits once another replaced it or once
the stand's state has a schema newer than it reads.

`run --then 'COMMAND'` runs a shell command after the scenarios while the run
still holds its lease, so nobody flashes the board in between: read the reset
reason or RTC memory after a failure, or attach OpenOCD. The command may use
the boards' ports directly, nested `cargo hil` commands join the lease, and
`OER_HIL_RUN_DIRECTORY` names the run bundle. Its exit status is reported and
recorded as a `then-succeeded` or `then-failed` event, never as the run's
outcome.

```console
cargo hil run wifi-station-wpa3-restart --then 'python tools/read-rtc.py /dev/ttyACM0'
```

`run` accepts several scenarios: it builds every needed image class before
queueing and executes all scenarios in one run bundle under one lease, in the
given order within each image class. With `--firmware-from` every named
scenario must use the replayed image class. Prefer it to a shell loop under
`lease`, where each nested run builds while the stand is held.

`lease` runs one command under one lease; every `cargo hil` command inside it
joins that lease when the lease already holds what it needs, so no other owner
can flash between the runs of a series. `--board NAME|MAC` (repeatable) and
`--air shared|exclusive` name what the command uses. A lease naming neither is
refused: a whole-stand lease blocks every other owner, so it must be asked for
with `--stand`. The lease options precede the HIL command or follow
`lease`; a runner command (`run`, `run-all`, ...) also takes them among its
arguments before `--`, and refuses two different owners:

| Option | Meaning |
| --- | --- |
| `--owner NAME` | Who holds the lease; defaults to an enclosing lease's owner, then the owner registered for the checkout |

The environment variable `OER_HIL_OWNER` carries the same choice. A lease
belongs to one of the agents that use the stand: `stand`, `wifi`, `phy`,
`bluetooth`, `blobray`, `infra`, `802154`, `esp32c5` or `network`. Each checkout
registers its owner once with `cargo hil owner set NAME`, kept in
`owners.json` of the arbiter directory; `cargo hil owner` prints it. Nothing
is derived from the checkout's directory name: a lease requested for no
agent is refused with an error naming `cargo hil owner set`. `cargo hil owner
merge OLD NEW` charges an old name's balance and history to its agent, and
`cargo hil owner forget NAME` drops a balance that belongs to no agent. A request
names no duration: the stand charges the time a lease holds, as follows.

Every lease charges its owner's balance the time it holds, and an owner whose
request waits behind a conflicting lease is credited the time it waits, once
however many requests it queued; an owner doing neither accrues nothing.
Among conflicting requests the owner with the highest balance is served first,
the earlier request on a tie. Balances halve every two hours, stay within an
hour either way, and after each grant the mean over the owners active in the
last day is subtracted. `cargo hil queue` and the dashboard show every balance
and who is served next; the lease history records each lease's charge, its
owner's balance at release and the balances its grant was decided by.

A run of several scenarios that has held its lease for ten minutes finishes
its current scenario when a waiting request that needs its resources belongs
to an owner with a higher balance: it releases the lease, queues again and
continues its remaining scenarios in the same run bundle after flashing its
image again. A single scenario and a `lease` command run to their end. Every
lease ends at one hour: it is stopped with `SIGTERM`, which runs the ordinary
cancellation and fixture cleanup, a `lease` stopped this way exits with status
124, and `SIGKILL` follows five minutes later. `cargo hil preempt ID --reason
TEXT` stops another owner's lease the same way at once: its owner is charged
no longer from that moment, the holder prints who preempted it and why when it
releases, the history records it as `preempted-on-request`, and the user is
notified. Estimates from earlier leases of
the same work only predict when a waiting request starts. `cargo hil evidence
record` records a run's evidence shards. `cargo hil doctor`
reports conflicting holders without queueing.

A command started in the background returns when its lease ends; its exit is
the notification for an agent. The user receives desktop notifications through
`notify-send` when a waiting owner is granted a lease, when the stand becomes
free, and when a lease reaches the hard limit or is preempted;
`OER_HIL_NOTIFY=0` disables them.

Do not wrap `cargo hil` in `timeout`: its clock also runs while the request
waits in the queue, which can last longer than the work itself. A timeout that
fires while the request waits only drops it from the queue; one that fires
during the lease stops the work with `SIGTERM` and the ordinary cleanup, so a
run ends interrupted with no verdict. Every lease already ends at the one-hour
hard limit; to stop a lease that is stuck, use `cargo hil preempt`.

The stand holds several equal boards, currently an ESP32-S31 and an ESP32-C5.
No board has a fixed role: a scenario or other consumer chooses which board it
uses as its device under test or as a peer, and may flash its own firmware.
The lab configuration names the boards the runner uses. Each board is
identified by the MAC address its USB Serial/JTAG port reports as USB serial
number, independent of `/dev/ttyACM*` numbering. Every board is part of the
stand: flash and use any board only under a lease.

A board, or the whole stand, can be taken out of service: `cargo hil --owner
NAME devices maintenance BOARD|--stand --reason TEXT` records it in
`maintenance.json` of the arbiter directory. Until `cargo hil devices release
BOARD|--stand`, a request by another owner that claims the board (any board,
for `--stand`) or the whole stand does not get it: a run waits, printing the
reason, and starts once it is back in service; a tool acting on the board
itself (`board`, `lease`, `flash`) is refused with the reason at once. A
quarantined board is out of service the same way. A lease already held runs
on. `cargo hil queue` and the dashboard list what is out of service.

An agent that needs the stand waits for it with a shell command, never for a
chat message: `cargo hil wait --service [BOARD...]` blocks until the named
boards (every board when none is named) and the stand are back in service,
printing what it waits for, and exits 0.

`cargo hil devices`, `cargo hil queue` and the dashboard show each board's
health from the stand's own records, without touching the board: `ok`,
`recovered recently` (the stand recovered it within the last hour, with how
many of those recoveries were hardware-level), `maintenance`, `QUARANTINED`
or `not attached`.

A board is named by its full chip name (`esp32s31`, `esp32c5`). The registry
names boards itself: the only board of a chip is the chip, and once a chip has
several boards each becomes the chip with the last four hexadecimal digits of
its MAC (`esp32c5-2564`); write the same on the board. A name set with
`cargo hil devices set --name` that follows neither form is kept. Wherever a
board is expected (`--board`, `board =` in the lab configuration), its
registered name, its MAC, or its chip when it is the only registered board of
that chip selects it; a chip with several registered boards is refused with
their names.

A board may also have a reset path that does not depend on its USB
Serial/JTAG port: a DevKit's USB-to-UART bridge whose modem lines drive the
chip's EN and BOOT pins. `cargo hil devices set MAC --reset-uart USB_SERIAL
--en rts --boot dtr` records it, with the lines named explicitly, in the
registry's `control.reset`; `control.power` is reserved for a switchable hub
port. Both are absent unless registered. `cargo hil devices reset BOARD`
resets the chip through EN under a lease of the board, as its RST button does,
and prints the reset reason and boot mode the ROM reports on the bridge;
`--download` holds the boot strap low so the ROM waits for a download, and is
never implied. The bridge port is the stand's: a terminal that opens it with
its default modem lines resets the chip.

Tools reach a board's port only through the stand's commands, which release
RTS before DTR so opening a port never resets the chip, and hold a lease of
the board while they use it; a script or terminal that opens a board or
bridge port itself can reset the chip or pull its boot strap.
`cargo hil board reset BOARD` resets through the USB Serial/JTAG RTS line
(default), `--via jtag` through OpenOCD and the chip's debug module, or
`--via en` through the registered bridge, and prints the reset line the ROM
reports. `board check` reports, without resetting, whether the board is
attached, its last flash, maintenance, its reset paths and whether it answers
the peer text protocol. `board console --for DUR [--until TEXT]` prints and
saves the console under `target/hil/console/<mac>/`. `board soak --cycles N
| --for DUR [--via rts,jtag,en] [--batch 10]` resets the board through each
path once per cycle, `--batch` cycles per lease so other owners may use the
board between batches, and stops at the first reset after which the ROM does
not boot from flash, saving the console that follows; the board journal
records the paths, cycles, resets and any failure. `cargo hil peer send
BOARD LINE` sends one peer command and prints the output up to that command's
`@OK` or `@ERR`; it also claims the air shared, since a peer command may
transmit. Every OpenOCD session the stand starts is stopped when it outlives
its timeout (ten minutes to program, one otherwise). A tool that holds a long
interactive session under a board lease, such as the calibration cross-check,
opens the port through the same library functions,
`oer_hil_runner_core::session::reset::{open_without_reset,
reset_into_application}`, never through `serialport` itself.

```console
cargo hil devices                     # boards: label, port, last firmware
cargo hil devices set 38:44:BE:AA:25:64 --chip esp32c5 --name esp32c5
cargo hil devices set 38:44:BE:AA:25:64 --reset-uart 5B90165754 --en rts --boot dtr
cargo hil devices reset esp32c5       # rst:0x1 (POWERON),boot:0x18 (SPI_FAST_FLASH_BOOT)
cargo hil board check esp32c5
cargo hil board reset esp32c5 --via jtag
cargo hil board console esp32c5 --for 30s --until @READY
cargo hil board soak esp32c5 --for 8h --via rts,jtag,en
cargo hil peer send esp32c5 SYNC
cargo hil lease --board esp32c5 --flashed ieee802154-peer --application build/peer.bin \
    --port /dev/serial/by-id/usb-Espressif_USB_JTAG_serial_debug_unit_38:44:BE:AA:25:64-if00 \
    --chip esp32c5 -- idf.py -p /dev/ttyACM1 flash
cargo hil board flashed --image NAME --sha256 HASH --device MAC   # inside a lease
```

Tracked ESP-IDF firmware forms one catalog: every project with a
`firmware.toml` beside its `CMakeLists.txt`, peers in `hil/peers/<project>/`,
vendor references in `verification/<chip>/hil-vendor/<project>/` and each
chip's second-stage bootloader in `hil/bootloaders/<chip>/` (`kind =
"bootloader"`), which `cargo hil flash` writes to every board of that chip.
A bootloader entry is never flashed on its own. `hold = "REASON"` in a
manifest refuses every flash of that image, including a run's automatic peer
restore, which then blocks its scenarios with the reason; `firmware list`
shows it. The
manifest names the image as the board journal records it, the target chip and
the chip whose `artifacts.toml` pins the one ESP-IDF revision and vendor
archives every image builds against. Builds are reproducible
(`CONFIG_APP_REPRODUCIBLE_BUILD`), so equal sources give an equal digest.

```console
cargo hil firmware list
cargo hil firmware build ieee802154-peer
cargo hil firmware flash ieee802154-peer --board esp32c5
```

`firmware flash` builds the image, leases only the named board with the air
shared, writes every file of the build's `flasher_args.json` and journals the
image with the digest from its `build.json`, the repository commit and whether
the project differs from it. A board whose registered chip differs from the
image's target is refused.

The runner records its own flashes and registers the board it flashes as
`esp32s31` when its chip is unknown.
Any other flash (a peer, a vendor image, a manual `espflash`) is recorded
with `lease --flashed`, which journals it only when the command succeeds, or
with `board flashed`. Name the board with `--port` or `--device`, and the
application with `--application FILE` or `--sha256`.

On every grant the arbiter prints the last flashed firmware of every board
(image, commit, application hash, owner) and lists the flashes other owners
made since this owner's previous lease. It only reports this state; it never
erases or restores it. The startup artifact, which carries the PHY calibration
cache, is a host file uploaded at every boot; a relative path belongs to each
checkout, so another owner's cache never reaches a run. `cargo hil queue` lists
the newest upload or write of every such file. The queue, history, journal and device registry live in the
user's host cache, so every checkout of the repository shares them. A checkout without the arbiter
still fails fast on the fixture locks and bypasses the queue; a granted holder
waits for such a process to finish.

`cargo hil fixtures` probes the host fixtures the lab configuration names:
this host's Wi-Fi radios and Bluetooth adapter and the OpenWrt station fixture
and air observer over SSH. It prints each one's lease key (the claim a run
takes it by), whether it answers, its model and firmware, and each
interface's type, channel and CCA busy share (busy over active time of the
frequency in use, from the radio's survey). The dashboard's Fixtures section
shows the same, refreshed every minute, with the lease holding each fixture.

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

### Operational commands without a rebuild

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

### Program-counter profiles

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
and the callers the return addresses name, and writes the report beside it as
`profile.txt`. A return address names the caller only while the sampled
function has not called another, so callers are exact for leaf functions.

### Code layout seeds

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

### Enqueued runs

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
an unknown scenario fails in the terminal instead of in the job. `wait JOB` blocks until the job ends and exits with its outcome:
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

### Bisecting a scenario

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

### A/B comparison

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
images; the remaining rounds replay those exact images, alternating A and B
under one whole-stand lease, so drift of the air and the calibrations falls
on both arms. Every run records `experiment` (its id, arm and variant: the
commit and each override's path, commit and dirtiness) in its manifest and
no evidence. A comparison takes hours, so like a run it is a job: `--enqueue`
starts it detached and prints the job id for `cargo hil wait`, and `--after
JOB` orders it after another job.

The report takes one value per run and measurement (the mean over the run's
repetitions) and compares the arms with `hil_perf::compare`: each arm's mean,
deviation and count, the difference B − A with the half-width of its Welch
95 % confidence interval, and a verdict: `significant` (the interval excludes
zero and the difference is at least 2 % of A's mean) with the better arm,
`within-noise`, or `insufficient-repetitions` (fewer than 3 runs on a side).
A gated measurement's direction is its gate's; an ungated one is compared as
if higher were better and says so. `ab-report.json` beside the worktrees
holds the variants, every run and every comparison; the command prints a
summary. Image features as a variant are not supported yet.

### Performance across commits

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
`prune` keeps pinned runs, runs cited by committed evidence shards, runs whose
firmware another run replayed, runs younger than `--days` (30), incomplete
runs, the latest pass and the `--keep-failed` (5) newest failures of every
scenario and image class. Run by hand without `--apply` it only lists the other runs and
the bytes only they hold; hard-linked firmware shared with kept runs is not
counted. Pins live in the store's `pins.json`. Pruning with `--apply`, and the
automatic rule, also delete the observer builds in the store's `observers/`
that no remaining run names and that were stored more than an hour ago; a
starting run stores its build before its manifest names it. Run verification
requires every build there to hash to its name.

## Build and run

`cargo hil plan <scenario>` prints the selection's plan without opening a lab
configuration or device: each scenario's procedure digest, image, repetitions,
requirements and the named checks it supplies. `--proof <check>` filters the selected scenarios by an actually implemented
named check. Repeated `--proof` arguments require all named checks, and `--tag`
can restrict the profile. For example:

```console
cargo hil plan --tag he20 --proof udp.rx.maximum-silence
```

The plan lists provided checks and selection reasons; it does not infer success
from tags, select by changed files, or claim minimum coverage of an arbitrary
product program. Selection never expands beyond the requested scenarios.
Scenario digests drop null values, fill schema-5 defaults from
`hil/schema/scenario-v5-defaults.json` and exclude top-level `description`,
`tags` and `transfer`; all execution fields remain bound, including
repetitions and the complete family table. A plan neither executes hardware
nor invalidates sealed observations.

`cargo hil run <scenario>` builds and flashes the required image before the
scenario with the owned Xarxa/Embassy network stack, the only network
implementation; see the [implementation guide](../../docs/network-implementations.md).
`cargo hil image build performance` and `cargo hil image build correctness`
perform the same final stack/move, placement, source-graph and packed-image
checks without flashing or loading private lab configuration. Each successful
build emits one JSON report on stdout with class, target, profile, network,
class-owned artifact paths and build-audit verdicts; diagnostics stay on stderr.
An ELF or `application.bin` left beside a failed build is not a successful
image report.

For an explicit source snapshot, use:

```console
cargo hil image snapshot --source-include crates/path/to/new.rs
cargo hil image build performance --source-snapshot target/hil/esp32s31/source-snapshots/<snapshot-id>
```

`image build` takes several classes; from one source snapshot they share one
materialization of it and are built one after the other.

Snapshot capture is offline and does not load fixture secrets, build firmware or
access a device. Tracked regular files are captured automatically. Every
nonignored untracked file must be explicitly named with a repeated
`--source-include`, or, with `--include-untracked`, lie inside a path package
of the firmware workspaces (`hil/targets/esp32s31` and
`platform/esp32s31/bootstrap`, as Cargo's locked metadata lists them), the
packages an image build reads, or inside the HIL host packages and scenarios
(`hil/host`, `hil/schema` and `hil/scenarios`), which the run reads;
unresolved files block capture with their names and the `--source-include`
arguments that add them all, ready to paste, before content is archived. The manifest lists every archived untracked
file with `by: source-include`, `by: image-package` or `by: hil-host`, and a run with any
option is not noted as pending evidence; `cargo hil evidence record --run ID`
records it. Directory selections and ignored files are not accepted.
For the configured local overrides, qualify each new file with `esp-hal:`,
`embassy:` or `xarxa:`. No symlink or submodule content is silently followed;
such inputs require review and are rejected by this capture interface.

The content-addressed directory contains `manifest.json`, `snapshot.json` and
`sources.tar`. It records exact file bytes and executable modes for the main
source and configured overrides. Subsequent capture does not overwrite an
existing identity, and corrupt stored material is rejected. Builds with
`--source-snapshot` validate and materialize these inputs in the fixed workspace
`target/hil/esp32s31/source-build/`, held under an exclusive lock and replaced
on each build; Cargo uses that directory, including its copied configuration
and snapshot-local override paths. Artifacts (including copies of both ELFs)
and the snapshot reference remain under
`target/hil/esp32s31/snapshot-builds/`. The live checkout is not a build source
for this explicit mode.

Live and snapshot builds of one image class and network share the Cargo cache
`target/hil/esp32s31/build-cache/<profile>-<class>-<network>/`. Cargo
fingerprints decide reuse: registry packages are compiled once, while the
freshly materialized path packages always rebuild in place under the stable
workspace path.

This fixes the source input set, not the entire build environment: tools, Cargo
package caches and user-level configuration are still external. It does not
prove byte-identical rebuilds or authorize transfer of HIL evidence. Fresh `run` and `run-all` executions capture and bind a source
snapshot before building firmware; pass explicit `--source-include` arguments
or `--include-untracked` for nonignored untracked inputs. Replay uses the archived artifact rather than
claiming a current build. Standalone image builds use the snapshot only when
`--source-snapshot` is supplied.
Such a build also publishes `target/hil/esp32s31/builds/<identity>/build.json`,
firmware, source snapshot and `integrity.json`. Its identity is the SHA-256 of
the integrity seal. A review can name this destination without fabricating a
scenario or hardware observation; the record supplies no PASS or repetitions.


`cargo hil run-all` runs the scenarios carrying each `--tag`, or the whole
catalog only with an explicit `--all`. It reuses each image across its scenario group but
does not fail fast. Every invocation retains an immutable evidence bundle in
`target/hil/esp32s31/runs/<run-id>/`, including a canonical JSON suite, JUnit
XML, a standalone HTML report and the exact application image flashed for each
firmware class. The flash operation reads that archived copy, binding firmware
provenance to the bytes sent to the DUT. Completed and interrupted bundles also
carry a deterministic integrity inventory covering every retained file.

The manifest records `runner.observer`, including the SHA-256 of the running
executable and its embedded build/source identity. Firmware capture does not
replace this identity. The qualification evaluator uses
[`observer-inputs.json`](../schema/observer-inputs.json) to select relevant
observer inputs; it does not require equality of the entire runner binary.
Its schema-4 registry identifies a workload as `<family>/<kind>` (for example
`wifi/station-udp`), classifies its timing sensitivity, and scopes its inputs by
the `common` group plus the scenario family's group (`wifi`, `bluetooth`,
`system` or `ieee802154`), whose direct dependencies are the runner family
packages. A workload's source inputs are the runner package, every path package
in that projected dependency closure and the listed non-Cargo `data` files;
source paths are never enumerated.
Legacy bundles need an explicit provenance review before becoming applicable.

`RunSession` also publishes `attempts/<scenario>.json` immediately after a
scenario's complete repetition set (including cleanup) has been recorded. This
schema-1 seal contains a completion snapshot, the result and a size/SHA-256
inventory of the scenario directory, its bound firmware class, source patches,
run plan and lab provenance. Firmware bytes are referenced in place, not copied
per scenario. A bound firmware class cannot be replaced within that invocation.
The seal is published atomically and cannot be overwritten by the runner.

Qualification can consume a closed attempt even if a later scenario or the
campaign process is interrupted before the final suite seal. Unpublished
temporary seals and unfinished repetition sets supply no completion proof.
Sealing an attempt does not release or recover fixture resources, resume a
partially executed protocol, or certify an early phase of an unfinished
lifecycle. Fixture cleanup and recovery remain with their existing owners.

## Inspect evidence

`cargo hil runs history <scenario>` reads a scenario's outcomes and
measurements straight from the bundles in the store and starts with its pass
rate and how many of its newest runs in a row did not pass; there is no
derived history file to rebuild. Trends are scenario aggregates, not a proof
of comparable firmware or fixture conditions. Verify the structure and content digests of one bundle with
`cargo hil report verify <run-id>`, or omit the ID to verify all bundles. This
also runs without a DUT or private lab configuration.

Qualification v4 independently reads the sealed bundles instead of trusting a
handwritten HIL status. A capability is HIL-qualified only when its declared
scenario and repetition requirement is satisfied by a completed bundle or a
separately sealed attempt under the default current-source-composition policy or an explicit
[property-scoped applicability review](../../qualification/evidence-reviews.md)
with validated build and owner bindings. A verified snapshot matching the current
source inputs the observation depends on is directly applicable even when dirty, provided the executed
procedure and relevant host observer inputs also match. Its existence alone does
not establish this match or a passing observation. Scenario IDs and achievable repetition counts are checked against the
versioned catalog in `hil/scenarios`.

## Prepare and restore network fixtures

The controlled OpenWrt AP and HIL host share the fixture LAN. Reverse flows
use the local IPv4 route selected for the discovered target. External AP
fixtures provide compatibility workloads; exact-delivery scenarios require
the controlled fixture declared by the scenario.

Every network scenario owns a prepared station AP, including target-AP tests
that first qualify a station connection. OpenWrt `radio` and `ap_section` identify
the UCI resources; the transient netdev is not used to infer ownership. The
runner discovers PHY/AP capabilities, applies the scenario's HT20/HT40/HE20
profile, channel, WPA2 credentials and WMM, then checks enabled hostapd, generated
HT/HE settings, width and center frequency. `phys` is an access policy, not
hardware discovery. Original options, pending UCI edits and radio up/down state
are restored; HIL does not commit temporary settings to flash.

AP scenarios derive target bandwidth from their link profile. A router hosting
their managed client uses the same primary and secondary channel. These scenarios
start a fresh epoch of the selected radio; other radios and the wired uplink are
not brought down. Scoped client forwarding/VIF cleanup remains a separate owner.
`fixture-applied.json` records actual settings without network credentials.
Cleanup failures are retained and quarantine subsequent network workloads in
the same runner invocation.

Fixture preparation can be exercised without opening the serial port, building
firmware, resetting or transmitting traffic from the DUT:

```console
cargo hil fixture check udp-tx-he20
```

This command uses the same prerequisites and profile owner as `run`, opens and
stops the required OpenWrt packet captures, restores the AP and writes its report
to `target/hil/fixture-checks`. Control scenarios also exercise AP stop/restart.
Scenarios requesting the independent laptop observer exercise Linux monitor
setup, capture readiness, tshark decoding and managed-interface restoration;
`fixture-monitor.json` retains the capture result. This passive check uses a
synthetic parser filter and sends no target traffic.
It does not establish target associations or qualify target throughput.

A station UDP workload may declare `induced_protection`. The laptop radio
then acts as a real, standard-conformant peer of the OpenWrt station fixture:
`non-ht-member` joins its BSS with HT, VHT and HE disabled, so the AP must
advertise non-HT mixed HT protection; `overlapping-legacy-bss` hosts an
802.11b BSS (1, 2, 5.5 and 11 Mb/s, no ERP or HT element) on the AP's channel,
so the AP must set ERP Use_Protection. No AP setting is forced. Before the
workload, the independent air observer must see every beacon of a two-second
window carry the expected protection within ten seconds; otherwise the run is
blocked with `protection-peer/fixture-protection.json`. The legacy BSS needs
the laptop's regulatory domain in the optional `[legacy_bss] country` lab
section and the schema-13 network helper.

Every 802.11 air observer (the laptop monitor, the OpenWrt TX-monitor tap and
the probe-load management capture) decodes its capture through
`hil_wifi::evidence::air`: one tshark run with a fixed field set yields typed
`AirFrame` records, and each analyzer works on those frames in memory. A
malformed record, unparsable field or invalid timestamp fails the whole
capture; absent fields stay absent.
`doctor` checks available tools and capabilities without applying a profile;
a successful doctor result does not assert that current radio settings already
match the selected scenario.

## Collection and failure boundaries

Capture handles acknowledge readiness before the session starts. Dumpcap's
opened-file notification and tcpdump's opened-interface notification replace
startup sleeps. The runner explicitly stops capture after session collection;
traffic duration does not set an early capture stop. Independent process
watchdogs and file limits remain failure bounds. Passive observers use the AP's
actual primary frequency, width and center frequency, including HE20 geometry.
Tshark parsing and monitor setup/teardown are invoked by the runner.

AP workload evidence and qualification are separate. `cycle-progress.json`
retains each available traffic, link and teardown result even if another stage
fails; `access-point-report.json` retains completed boots/cycles and the boot
error. Multi-client UDP additionally writes `delivery-progress.json` before
applying gates, including partial host sends, target evidence and worker errors.

Host UDP collectors finish from the correlated `Finished` transport count.
Complete delivery returns immediately. If packets remain undelivered, a two-second
delivery deadline bounds collection after that event; reaching it records
`delivery-deadline`, never proof of a drained radio. Each `*-reception.json`
retains the expected/unique/undelivered packet counts, partial bursts and the
termination reason even on target failure, I/O error, cancellation or unwinding.
There is no additional fixed reception window after the configured workload.

Serial I/O waits for descriptor readiness or explicit command/shutdown events.
SIGINT/SIGTERM notifications wake both serial protocol waiters and UDP collectors;
periodic polling is unnecessary for cancellation. Physical USB reset timing and
exclusive-port acquisition remain owned by the serial setup boundary.

For multi-client RX offers, `minimum_host_offer_percent` independently checks
bytes accepted by host UDP send calls over both the requested window and the
sender's elapsed time. The AP comparison scenarios require 95%. An under-offer
invalidates the requested load condition; it does not identify a DUT delivery
fault. No criterion means `not-assessed`, never an implicit load validation.
Host socket admission is not an on-air transmission measurement.

Diagnostic image features can change scheduling and linked code placement.
Compare performance only with the recorded image/configuration identity; a
more instrumented image is a separate experiment, not interchangeable evidence.

## Linux fixture software installation

Linux network and Bluetooth fixtures share one provisioning workflow. Preview
the exact provider plan first; these commands do not execute any listed build,
capability, sudo or hardware step and do not load the lab configuration:

```console
cargo hil fixture install --provider linux-net --dry-run
cargo hil fixture install --provider linux-bluetooth --dry-run
```

The helper `capabilities` and sudoers validation subprocesses each have a
five-second execution deadline; timeout is an installation/preparation failure.
Bluetooth preparation and execution share the versioned contract in
[`bluetooth_contract.rs`](fixture-install/src/bluetooth_contract.rs).
Synchronous HCI command receive failures report the command identity, elapsed
time, packet count and last event identity in the helper's existing error
report. They do not record event payloads or keys, retry commands, or extend
the two-second deadline when unrelated events arrive. A DTM Test End timeout
is a peer-check failure, not evidence that the DUT has stopped transmitting.

Like every Cargo alias, `cargo hil` may compile the runner before it can print
the plan. That Cargo startup is not an installer plan step. Missing helper
binaries appear as `missing`, while present files remain `present-unverified`.

Install exactly one provider with interactive authorization:

```console
cargo hil fixture install --provider linux-net
cargo hil fixture install --provider linux-bluetooth
```

Installation is stand maintenance: it claims only
`fixture-software:<provider>` exclusively and goes ahead of every waiting
request, whatever the balances. Every lease holding a conflicting claim,
such as a run of that provider, is preempted at once with the reason
`fixture maintenance: <provider> install by <owner>`: `SIGTERM` with the
ordinary cleanup, `SIGKILL` after the grace, no further charge, a history
record and a notice to the user. Waiting requests it overtakes say so in
their wait line, and a preempted run ends interrupted, not failed; rerun
it. Later runs of the provider wait for the installation. Runs take the
software lease only once granted, so a queued run never blocks an
installation. Preparation happens before queueing; `sudo` prompts only once
the claim is granted.

Bluetooth policy admits only named adapters. It defaults to the dedicated
`hci0`; repeat `--adapter hciN` to install a sorted finite allow-list. Reinstall
with the adapters selected by the private lab configuration before using
another controller.

Cargo is the single installation entry point and runs as the unprivileged
operator. Linux is required; installation is not supported on other hosts.

Preparation runs as the operator. Network preparation retains the pinned
hostapd build, patch and provenance plus the locked probe and fixed-launcher
builds. Bluetooth preparation builds only its finite helper, fixed launcher and
installer with the same locked `target/hil/fixture-build` output; it does not
build or download hostapd. The runner rejects an invocation already running as
root before starting either build. It then transfers the foreground terminal
to ordinary `sudo`; no password pipe or askpass path exists, and the general
HIL runner never runs as root.

The privileged apply owner imports the content-addressed bundle into
`/var/lib/open-radio/fixture/<provider>/generations/`. It validates artifact
bytes, root ownership/modes, the exact helper contract and a candidate policy
against the effective sudoers policy before publication. Stable commands under
`/usr/local` enter finite provider launchers. For an operational command the
launcher first acquires the provider software lease, rejects unfinished or
unprovable installation state, resolves one committed `current` generation and
then executes that generation's root-owned helper. The lease remains live for
the helper call. Switching `current` is the commit point. The network helper
uses hostapd from the selected generation, so one invocation cannot combine
helper and daemon bytes from different generations. The read-only
`capabilities` operation remains available to installation verification without
operational admission or hardware effects.

One persisted transaction journal covers stable links, policy and generation
selection. Pre-commit failure restores the prior files and sudoers policy.
Post-commit software verification failure rolls back; a receipt-write failure
keeps the activated generation and leaves an explicit recovery-required journal.
The next authorized install recovers that journal before considering a new
bundle. The prior managed generation is retained. Unmanaged or incomplete
installations require explicit operator recovery; installation does not import
arbitrary existing files. Reinstalling the same bundle verifies it and refreshes its fixed-path
receipt without switching generations. Generations are not automatically
deleted while their lifetime is unknown.

Every runner operation that may use a provider holds its shared software lease
for the complete fixture/run boundary. The empty root-owned lease file is
`/var/lib/open-radio/fixture/<provider>/session.lock`; it is persistent,
read-only to the operator and is never replaced by repeat installation or
upgrade. Kernel `flock` ownership, not file contents, identifies a live owner,
so clearing `/run` or rebooting does not require reinstallation. All applies
serialize through one root-owned installation lock, then installation requires
the selected provider's exclusive lease and fails while such a session is
active. Installation never
stops hostapd, an adapter or another service to force an upgrade. A directly
started network AP is also detected through its root-owned pid file. The lease
is software-update ownership only and never acquires the DUT, opens SSH or
discovers fixture hardware.

Operational admission checks the existing transaction journal, `current`
selector, successful receipt and selected artifact identity after taking the
same shared lease that conflicts with apply. A pending/corrupt journal, missing
or unsafe selector, invalid receipt, writable state path or artifact mismatch
returns `recovery-required` before a hardware-touching helper runs. Only the
installer performs recovery. Its direct generation `capabilities` check does
not recursively acquire the operational lease. Launcher descriptors are closed
by long-lived network daemons; direct AP lifetime remains protected by the
existing pid-file check, while foreground helper and runner leases retain their
original lifetimes.

After activation the installer checks installed bytes, policy and only the
helper's non-hardware `capabilities` operation. It does not run `doctor`,
`fixture check`, Bluetooth DTM/connect-reset, identity discovery, serial, HCI,
SSH, network mutation, flashing or RF checks. The fixed root-owned receipt at
`/var/lib/open-radio/fixture/<provider>/receipt.json` reports the generation,
source dirty/content identity, artifact and policy hashes, previous generation,
checks and terminal/recovery state. It contains no lab credentials and is not
HIL evidence or qualification input. A successful receipt means software is
installed consistently; privileged platform behavior and hardware acceptance
remain separate operator checks.

| Operation | Software/filesystem writes or privilege | Host network mutation | Adapter reset/rfkill | RF transmission | DUT required |
| --- | --- | --- | --- | --- | --- |
| `fixture install … --dry-run` | No installer step; Cargo may build the runner | No | No | No | No |
| `fixture install --provider …` | Builds as user, then interactive privileged system writes | No | No | No | No |
| installed helper `capabilities` | Read-only software inspection; network helper uses its existing finite sudo grant | No | No | No | No |
| `fixture bluetooth-check` | Writes a local report and uses the finite helper grant | No | Yes, with restoration | Yes, bounded DTM TX | No |
| `fixture check <scenario>` | Writes a local report and may use finite helper grants | Depends on scenario | Depends on scenario | Depends on scenario | No |
| `run` / `run-all` | Builds and writes evidence; uses selected fixture grants | Depends on scenario | Depends on scenario | Yes for radio scenarios | Yes |

`cargo hil doctor` verifies installed helper schemas and non-interactive finite
sudo availability before a selected scenario takes ownership. It remains a
separate readiness inspection, not post-install verification. Local Linux AP
profiles are generated from station credentials and
`station_fixture.country`, `channel` and CIDR `address`; HT/HE mode comes from
the scenario. The DHCP range excludes the AP address and stays inside its
subnet. Static station addresses must agree with that subnet and gateway. The
helper receives these values on stdin and keeps the temporary hostapd
configuration under root-owned `/run` with private permissions; stop/cleanup
removes it. No installed credential profiles are consumed. The dedicated
network provider continues to own only `wlan0`.

The runner subscribes to hostapd control events, confirms ENABLED and actual
HT/HE mode, WPA2, channel geometry and IPv4 address before workload execution.
It records these non-secret settings in `fixture-applied.json`. The Linux helper
keeps hostapd startup diagnostics in a group-readable runtime log and includes
its bounded, credential-redacted tail in preparation errors before cleanup.
Debug logging ends before the workload starts; cleanup removes the runtime log.
The Linux helper owns only `wlan0`. While it does, a per-boot
`/run/NetworkManager/conf.d/90-open-radio-hil.conf` keeps NetworkManager from
managing any `wlan0` netdev, including each one the helper recreates;
cleanup removes it and returns `wlan0` to managed mode.

AP scenarios wait for the matching `WifiAccessPointStarted` event and validate
the successful `Idle` to `AccessPoint` transition before starting either external
client. Sending the start command is not readiness: the target completes the
request after activating AP RX interrupts, publishing the first beacon and
applying its network configuration. Initialization failures produce a start
failure instead of an early success followed by a stop error.
The controlled Linux client starts with its network disabled. The runner attaches
through the group-accessible private supplicant control socket before enabling
that network, then waits for events and `wpa_state=COMPLETED`. It does not poll
status on a timer. The connection watchdog is 20 seconds. In each cycle's
`linux-client/` directory, `helper.log` records setup failures, `control.jsonl`
records timestamped events, status replies and scan results, and `connection.json`
records the final state, last rejection/disconnect and outcome. Unknown states
remain unknown rather than being classified as discovery failures. The transcript
is bounded to 4096 records and records no credential-setting commands. Connection
artifacts survive restoration of the managed interface. When the OpenWrt fixture
has `monitor_interface` configured, AP scenarios capture management and control
frames before enabling the Linux client and retain capture through traffic and
AP stop. A failed connection also finishes the capture before restoring clients.
The cycle owns `management.pcap` and capture counts in `management.json`. Capture
readiness and stop are explicit events; immediate packet delivery preserves short
connection captures. Empty captures and capture-socket drops report incomplete
fixture evidence. The monitor is removed before client restoration and on errors.
This on-router monitor is not an independent receiver: missing ACKs in its tap
alone do not prove that no ACK was transmitted over the air.
Diagnostic firmware retains the first failed probe response receiver, Sequence
Control, publication/completion times, final rate and retry report in UART output.
The record identifies a terminal failure; ordinary retry attempts do not create it.
Probe responses use one hardware attempt and a global 10-ms admission interval.
Excess requests are discarded without deferred response timers; changing sender
MAC does not bypass the budget. Authentication, association, EAPOL and data retain
their own retry policy. `tx_probe_ack_timeouts` is a subset of
`tx_hardware_failures`, not a successful delivery count. The AP gate reconciles
this named subset and unacknowledged disconnects against the total; unrelated
failures, timeouts, collision limits and saturated totals still fail.
This bounds software retry amplification, not RF contention or interference.
Unlike [hostapd's no-ACK submission](https://chromium.googlesource.com/chromiumos/third_party/hostap/+/fb2d4c1a3971302455730191117b0e91ce9b8793/src/ap/beacon.c)
for wildcard broadcast probes, this backend
still waits for the ordinary hardware completion and records a missing ACK.
Control transcripts include host Unix timestamps for comparison with pcap; clock
offset between hosts must be checked before interpreting sub-millisecond timing.

For multi-flow UDP TX with driver observation, terminal `OTXFLOW` records describe
flow 1 at socket admission and at the radio's claim of an Ethernet owner. Once
that flow shows activity, both boundaries are printed, including a boundary
with zero packets. `first_us` measures time from the diagnostic interval start
to first admission; `idle_us` includes silence before the first packet and after
the last. `gap_us` measures only intervals between admitted packets. `errors`
counts terminal socket failures, which close `pending_us` without counting a
packet. These are supplemental UART diagnostics, not MAC completion or host
reception evidence. The current per-flow records do not correlate individual
packets with hardware publication and completion.

`cargo hil fixture install --provider linux-net` first builds hostapd as
`cargo hil fixture build-hostapd` does, without root, then installs the resulting binary, build provenance and helper through
interactive sudo. The build uses the pinned hostapd release and reviewed
[coexistence patch](linux-net/hostapd/README.md). It requires a C compiler,
make, pkg-config, libnl3 and OpenSSL development files, curl, tar and patch.
Verified cached outputs can be reused without downloading or compiling again.
Updating the helper contract requires rerunning the installer.

For `local-linux`, `station_fixture.coexistence` selects `respect` (default) or
`force-ht40`. The latter applies `noscan=1` only to HT40 scenarios; the OpenWrt
patch also skips client coexistence/intolerance handling in this mode. HT20 and
HE20 retain normal policy. The selected policy is recorded in fixture evidence.
Either policy still fails preparation when actual channel geometry differs
from the scenario: requested 40 MHz never silently becomes an accepted 20 MHz run.

### Controlled probe-request load

`diagnostic-ap-probe-load` combines a 12-second AP two-client UDP TX window
with a finite Linux probe source. The primary offered rate is 130 Mbit/s;
the secondary sends one 1472-byte datagram every 50 ms. The scenario requires
at least 200 secondary datagrams and a maximum 250 ms interarrival gap.
These are progress gates, not a throughput qualification.

The runner prepares the source after client association and starts it only
after the device acknowledges the UDP session. Offsets from that Start event
are: one directed-SSID request at 1 s, 200 requests from one MAC at 3–3.995 s,
and 200 requests from distinct locally administered MACs at 6–6.995 s.
The source uses event/deadline waits, rejects pacing lateness above 4 ms,
and stops on controller EOF or cancellation.

`cargo hil fixture install --provider linux-net` builds and installs the bounded Rust helper
`open-radio-probe`. The helper accepts no command-line arguments or arbitrary
frame input. It checks the controlled `wlan0` association SSID/channel, owns
a temporary monitor interface on the same PHY, and removes it before reporting
completion. It does not retune the associated interface. Monitor coexistence
and injection support are requirements of this Linux adapter; creation or
injection failure fails the scenario. Those capabilities still require an
actual fixture run; host tests alone do not establish them.

The runner automatically captures management traffic on OpenWrt and invokes
tshark after capture shutdown. `probe-source.json` records submission,
associated BSSID, pacing and errors; `probe-air.json` records observed request
and response counts. All 401 source/sequence pairs must appear in the capture,
with no reported kernel drops. Responses must come from the associated AP and
cover both source modes. Retries, duplicate response sequences and more than
105 responses in any one-second window fail the gate. The five-frame margin
allows capture timing variation around the driver's 10 ms admission interval.
Successful socket submission alone cannot satisfy these gates. OpenWrt is a
separate observer device, but its capture still shares its client PHY.

A private `[air_observer]` section can attach a second OpenWrt host to station
UDP RX/bidirectional runs: `ssh_target`, `phy` and `interface` name its SSH
endpoint, dedicated PHY and temporary monitor interface. The PHY must initially
have no interfaces; the runner refuses to retune an active AP/client. It locks
both OpenWrt hosts, verifies distinct boot identities, derives the channel from
the active AP and checks the observer's actual geometry after tcpdump readiness.
The monitor and capture are owned until explicit Stop and cleaned up on errors.
`independent-openwrt-air.pcap` and adjacent JSON record passive air evidence;
the AP's own TX monitor remains a separate observation boundary. Captures with
socket drops are retained but fail completeness. Fixture checks exercise setup,
readiness and teardown without requiring a packet to arrive before immediate
Stop; real traffic captures require at least one frame. Missing passive frames
alone cannot establish over-the-air loss, and encrypted payloads require either
a captured handshake/decryption or correlation with the AP's MAC identities.

Independent OpenWrt air runs also collect `host-wire.pcapng` on the selected
host route, including ARP and both UDP directions. This observes the host packet
socket boundary, not a hardware transmit acknowledgement. Capture drop counts
are retained and checked independently from radio and application drops.

Station UDP RX and bidirectional scenarios can require
`criteria.maximum_rx_silence_ms`. The gate consumes complete-window typed
transport evidence, including the trailing silence; missing observation fails
rather than falling back to average throughput. This is a delivery-continuity
limit, not an RF airtime measurement. Multi-client receive windows do not publish one ambiguous maximum.

Managed OpenWrt RX runs retain `openwrt-wifi-egress.pcap` in the repetition
artifacts. This is plaintext packet-socket evidence on the AP wireless
interface, before driver/hardware transmission; it does not prove over-air
delivery. The existing readiness/Stop capture owner bounds its lifetime,
retains a 128-byte packet prefix, checks capture drops, and removes its private
remote files. Same-boot probes keep separate capture directories. Independent
observer and host clocks are not assumed synchronized; correlate packet
identities before comparing timestamps across hosts.

An OpenWrt fixture may set `read_only = true` in its private lab configuration
when the AP also carries essential connectivity. HIL verifies the existing
SSID, WPA2 credentials/settings and active PHY/channel geometry; it neither
applies nor restores AP configuration. A mismatch fails before DUT traffic.
Scenarios requiring AP stop/restart, an OpenWrt client, rate overrides or an
AP-side monitor are rejected. Read-only station counters and packet capture on
an existing interface remain available; independent observers may be used.
This mode is explicit and never a fallback from failed automatic preparation.
