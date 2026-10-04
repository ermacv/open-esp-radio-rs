# HIL stand file

`oer-hil-stand-schema` owns the stand file,
`~/.config/open-esp-radio/stand.toml` (mode 0600: it holds Wi-Fi
credentials), and its rules. The example is
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

## Host files

[`udev/52-oer-uhubctl.rules`](../udev/52-oer-uhubctl.rules) lets the stand's
user switch the hub ports without sudo;
[`networkmanager/oer-unmanaged.conf`](../networkmanager/oer-unmanaged.conf)
leaves the laptop radio to the HIL fixtures.
