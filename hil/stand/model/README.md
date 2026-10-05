# HIL stand model

`oer-hil-stand-model` owns the stand file,
`~/.config/open-esp-radio/stand.toml` (mode 0600: it holds Wi-Fi
credentials), and its rules; the one resolver of a board name; and the paths
of the stand's state. The example is
[`stand.example.toml`](../stand.example.toml); the design is issue #2
(sections 2.1 and 2.6).

## Contents

- `[stand]`: the stand's id and its air policy (`exclusive`: a run that uses
  the air holds all of it while it executes).
- `[[hub]]`: each USB hub with per-port power by its USB 2 and USB 3
  locations, the `protected` ports a cascaded hub hangs on (never switched),
  the `switchable` ports whose power `uhubctl` really cuts, and the touch
  button of each port.
- `[[board]]`: the pool. A board is identified by its USB serial number (the
  MAC of its USB Serial/JTAG port) and names its chip, radios, the roles a
  run may give it, its hub port and its reset ladder. A board's role in a run
  comes from what the run flashes on it (our image for a device under test,
  ESP-IDF firmware for a peer), so no board is fixed as one or the other.
- The fixture sections (`[bluetooth]`, `[station]`, `[access_point]`,
  `[station_fixture]`, `[air_observer]`, `[legacy_bss]`), which the HIL host
  reads with its own types.

## Rules

`StandFile::validate` checks the schema version, identifiers, unique hub
locations, buttons, board ids and USB serials, each board's chip profile
(`platform/<chip>/chip.toml`) and radios against its properties, and that a
board hangs on a described, unprotected port of its own, switchable when its
reset ladder has `power`. `StandFile::load` also requires the file to be
private to its owner.

`select_board` picks a run's board of a chip and role: the one the run names,
or the only candidate. Until the stand's scheduler assigns boards, a pool
with several candidates needs a named one.

## Boards and their names

A board's identity is its MAC: its USB serial number, which an Espressif USB
Serial/JTAG port sets to the chip's MAC, in `mac::normalize`'s form
(`38:44:BE:AA:25:64`). Claims, lock files, the board journal and OpenOCD's
`adapter serial` all name a board by it; a vendor/product/serial triple adds
nothing on these ports, and a `/dev/tty*` path changes with every
re-enumeration. `StandFile::resolve(query)` is the one way a command or
component turns a board name into a board: its stand-file id, its chip when
it is the only board of that chip, or its MAC (any case, with or without
separators). A MAC outside the stand file names no board. `Board::reference`
gives a `BoardRef { id, mac, chip }`, `StandFile::hub_port` the switchable
hub port of a board that resets by power, and `StandFile::label(mac)` its
`id (chip)`. A board's port is board I/O's: `oer_hil_board::ports::port_of`
finds the attached board by its MAC at its `/dev/serial/by-id` link.

## State paths

`paths` places the stand's state after the XDG base directories, through
`oer-durable`'s `xdg`: `stand_file()` (the configuration directory),
`arbiter()` (`open-esp-radio/arbiter` in the cache directory, or
`$OER_HIL_ARBITER_DIR`) and `locks()` (`open-esp-radio/leases` in the cache
directory). The stand file and the lock files never move with
`OER_HIL_ARBITER_DIR`: a private arbiter (a test's, a bisection's revision
runner) reads the same stand file as the runner's laboratory configuration
and excludes through the same locks.

## Host files

[`udev/52-oer-uhubctl.rules`](../udev/52-oer-uhubctl.rules) lets the stand's
user switch the hub ports without sudo;
[`networkmanager/oer-unmanaged.conf`](../networkmanager/oer-unmanaged.conf)
leaves the laptop radio to the HIL fixtures.
