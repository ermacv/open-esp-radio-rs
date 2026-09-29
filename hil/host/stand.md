# Sharing the HIL stand

Every checkout of this user shares one stand: its boards, host fixtures and
air. This guide covers leases, boards and their firmware; the
[host guide](README.md) covers configuration, building and running, and
[runs and investigation](runs.md) what a run leaves behind.


The [arbiter](arbiter/README.md) orders every use of the stand. A lease claims the resources its work uses:
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

A leased command that is itself a stand command (`cargo hil …` or
`oer-stand …`) has its arguments parsed as that command will before the lease
queues, so a mistyped option fails at once instead of after the wait; commands
the runner parses (`run`, `run-all`, `image`) are checked when they run.

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
of the shared store and recent leases with their outcomes. Commands appear
without their build-input paths (the full line is the tooltip), claims name
boards and fixtures, a running run whose runner died shows as abandoned and
an `ab` arm is marked. A run's ID links its `report.html`, and an enqueued
job links the end of its log. It only reads the arbiter's state, run
manifests, reports and job logs; Ctrl+C stops it. One dashboard serves the
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
request waits because of another owner, behind that owner's lease or a
request of that owner served first, is credited the time it waits, once
however many requests it queued. A request waiting behind its own owner's
lease earns nothing, so queuing more work behind a running lease does not
offset its charge. Among conflicting requests the owner with the highest
balance is served first, the earlier request on a tie. Balances halve every
two hours and stay within an hour either way, and every settlement subtracts
the mean over the owners active in the last day, so balances move smoothly and
their sum stays near zero: an idle owner drifts up while others hold and down
while others wait. `cargo hil queue` and the dashboard show every balance
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

## Boards

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

## ESP-IDF firmware catalog

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

Every flash that ends in the ROM download mode (`firmware flash`, `flash` and
the runner's own) starts the new image through the board's registered EN
reset path when it has one, a power-on reset, and waits for its port to
return. After the download mode an RTS reset through the USB Serial/JTAG
starts an esp32c5's application with a silent console that later RTS resets
do not revive (the host sees EOF or `EPROTO` on the port); after a power-on
reset RTS resets work. A board without an EN path is started through RTS.

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

## Host fixtures

`cargo hil fixtures` probes the host fixtures the lab configuration names:
this host's Wi-Fi radios and Bluetooth adapter and the OpenWrt station fixture
and air observer over SSH. It prints each one's lease key (the claim a run
takes it by), whether it answers, its model and firmware, and each
interface's type, channel and CCA busy share (busy over active time of the
frequency in use, from the radio's survey). The dashboard's Fixtures section
shows the same, refreshed every minute, with the lease holding each fixture.
