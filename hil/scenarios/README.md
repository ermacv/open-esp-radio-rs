# Versioned scenario catalog

Folders identify the workload domain: system, IEEE 802.15.4, Bluetooth,
IEEE 802.11 station/access-point/roles/monitor and radio coexistence.
Scenario IDs are stable across folder moves. Tags select overlapping
diagnostic and characterization uses; they do not assign ownership or make a
hardware-readiness claim.

## Roles

Every document declares its `role`. A `qualification` scenario is one that a
[qualification program](../../qualification/README.md) references; every other
scenario is an `investigation` scenario: the catalog of diagnostics,
measurements and experiments no program requires. The role is derived, never
chosen: validating any program fails while a `qualification` scenario is
referenced by no program, and a requirement on an `investigation` scenario can
never be satisfied (its obligation reports `requirement-names-investigation-scenario`).
A scenario on a diagnostic image class (`diagnostic-*`), or one that requests a
profile, must be an `investigation` scenario and may not carry the
`performance` tag: observers and probes the product does not carry never
qualify it or shape a gated figure. `cargo hil run-all --role qualification`
runs every qualification scenario.

## Document shape

Schema 5 documents have a common header and exactly one family table:

```toml
schema = 5
id = "udp-rx-he20-calibration"
description = "..."
role = "investigation"          # or "qualification"; see Roles
repetitions = 3                 # default 1
tags = ["he20"]
unsupported = "..."             # optional; see Unsupported scenarios

[wifi]                          # or [bluetooth], [system], [ieee802154], [coexistence]
image = "correctness"

[wifi.workload]
kind = "station-udp"
link = { phy = "he20" }
duration_seconds = 16
payload_bytes = 1472
offer = { rx_bps = 50000000 }

[wifi.workload.criteria]
minimum_rx_bps = 45000000
```

The family owns every executable value:

- `[system]`, `[ieee802154]` and `[bluetooth]` are tagged workloads whose kind
  implies the firmware image.
- `[coexistence]` runs the joint Wi-Fi/Bluetooth LE image: the Linux adapter
  connects to the GATT application, then the host offers station UDP while an
  ATT echo load runs over the Bluetooth connection for the same interval. It
  requires the station fixture and the Bluetooth adapter, and publishes
  `coexistence.wifi.rx-rate` and `coexistence.bluetooth.echoes`.
- `[wifi]` selects the image and an optional `[wifi.datapath]` initialization
  (placement, checksum, TX-buffer and RX-continuation diagnostics). Each
  workload variant carries its own link expectation, observers, fixture
  mutations and acceptance criteria, so a field that has no meaning for a
  workload cannot be written. Offers are directional: `rx_bps` flows to the
  target, `tx_bps` from it, and the present offers define the direction.

Every table rejects unknown fields. Relations that remain between the image,
the data path and the workload are validated by the family.

Hardware time bounds every iteration. A Wi-Fi scenario runs at most three
traffic phases, counting `repetitions` times workload `cycles`, and no
`duration_seconds` exceeds 16. Only scenarios whose purpose is duration
itself, tagged `soak` or `thermal`, are exempt. A catalog test enforces this
budget.

## Chips

A scenario names no chip. It runs on every chip whose HIL agent builds its
image class: the esp32s31, whose staged boot flow builds every class its
runtime declares, and a chip the ESP-IDF bootloader starts whose
`platform/<chip>/chip.toml` names its flash layout and whose
`hil/targets/<chip>` runtime declares the class's features (the esp32c5
builds `boot-smoke` and `system-watchdog`). A run takes the chip
`cargo hil run --chip CHIP` names, or the only chip that builds every selected
image; when several can, it refuses and names them. It uses that chip's device
under test from the stand file.

## Stand claims

A run claims the boards and fixtures its scenarios require and the frequency
ranges their radio work occupies. The range follows the family: IEEE 802.15.4
scenarios occupy their channels (2 MHz each), Wi-Fi scenarios the channel of
their link in the stand file (its primary channel and, for HT40, the
secondary above or below, each 22 MHz wide), Bluetooth, coexistence and
system scenarios the 2.4 GHz band, and the register diagnostics
(`event-status`, `ed-event`) and the radio-free watchdog none. By default a
scenario tolerates other protocol traffic in its range and transmits
normally. Tags change that:

| Tag | Meaning |
| --- | --- |
| `air-strict` | No other transmitter may use its range: RF levels, noise, sensitivity, A/B throughput |
| `air-noisy` | It transmits without regard for others: a continuous carrier, no CSMA, DTM, a throughput flood |
| `air-exclusive` | Both of the above |

Two runs conflict only where their ranges overlap: a strict run excludes any
transmission there, and a tolerant one excludes a noisy transmitter. Work on
disjoint ranges, or without a radio, runs in parallel. Every run records the
ranges it held, their level and the other leases held at its grant in
`air.json`. Tags are excluded from scenario digests, so adding a tag does not
invalidate recorded evidence.

## Named checks

Named checks are derived from implemented workload behavior and explicit
criteria, not tags. Station UDP RX exposes `udp.rx.target-rate` and
`udp.rx.host-offer-rate` when an RX rate floor is configured;
`udp.rx.maximum-silence` requires an explicit silence limit. Qualification can
require these observations by name without duplicating their thresholds.
`cargo hil plan --proof NAME` filters candidates. It never fills in missing
observations from a suite PASS.

Both the runner and the independent qualification evaluator discover regular
TOML files recursively. The filename stem must equal the document ID. IDs are
unique throughout the catalog. README.md is the only ignored documentation
filename. Symlinks (including a symlink catalog root), special files, other
file extensions and an empty catalog are rejected. The readers never follow
directory links outside the catalog. Each independently checks its required
schema, repetition bounds and the single family table; only the runner
interprets executable workload and acceptance fields.

The runner sorts by scenario ID.
`run-all` first traverses `ImageClass::ALL`, then the selected
scenarios of each image in catalog order. Folder traversal order cannot change
physical execution order. Each TOML document owns its image features,
workload criteria and repetition count.

Synthetic serialized compatibility inputs live in `hil/tests/fixtures/catalog`.
They are used by both independent readers and are not part of this catalog.

### BSS protection

A station UDP workload that transmits on an HT link may declare
`[wifi.workload.induced_protection]` with a `peer` (`non-ht-member` or
`overlapping-legacy-bss`, see the host guide) and a
`minimum_protected_ppdu_percent`. The independent air observer captures
control frames and data sent to the station fixture AP; the target is the one
station other than the laptop peer that sends that data. It publishes:

- `wifi.protection.rts-cts-before-data`: basis points of the target's data
  PPDUs immediately preceded, inside its NAV, by the target's RTS or by a CTS
  to the target, at least the declared percentage. Events are ordered by
  TSFT, because the observer delivers control frames and A-MPDUs on
  different paths and stamps one MPDU per A-MPDU. A CTS to the target answers
  only its RTS, so either observed half shows the exchange; the observer
  loses single control frames, and the remainder bounds that loss.
- `wifi.protection.control-rate`: target RTS frames at a rate outside the
  BSSBasicRateSet and the mandatory rates of their modulation class, or not
  DSSS/HR while the BSS sets ERP Use_Protection, exactly zero. The basic set
  and ERP protection come from the BSS's beacons in the same capture.
- `wifi.protection.nav-covers-exchange`: protected PPDUs with an observed
  RTS whose NAV, from the RTS TSFT plus its airtime and Duration, ends before
  the AP's BlockAck or Ack ends, exactly zero.

Fewer than 50 observed PPDUs, or none with an evaluable NAV, is an
insufficient observation rather than a pass.

An access-point workload may declare `[wifi.workload.protection]` with the
same floor. It requires `clients = { kind = "laptop-and-openwrt", laptop_phy =
"non-ht" }`, an HT link and multi-client UDP TX: the laptop joins without HT
capability, so the target AP must advertise HT protection and protect its HT
PPDUs to the OpenWrt client. The observer captures the whole channel; the
target is the station that sends data to the laptop, and the protected flow
is its individually addressed data to the other station. The same three
checks are published.

`laptop_phy` also selects the capabilities of a laptop-only client,
`clients = { kind = "laptop", laptop_phy = "non-ht" }`. Such a client has no
Block Ack agreement, so the AP sends it single ERP-OFDM MPDUs;
`access-point-non-ht-client-ceiling-tx` gates that transmit path.

### Station sleep

`station-sleep` starts the station with workload `power_save` (`min-modem`, or
`{ max-modem = { listen_interval = N } }`) and has the OpenWrt fixture run
`access_point_beacon = { interval_tu, dtim_period }`. The fixture sets them as
temporary UCI options (the radio's `beacon_int`, the BSS's `dtim_period`),
verifies them in hostapd's generated configuration, records the requested,
original and applied values in `fixture-applied.json` and reverts them with the
rest of the profile, also after a failed run; read-only, local and external
fixtures refuse a beacon schedule. ICMP probing at the workload's cadence keeps
the link in use; latency and loss are recorded, not judged. The workload
enables the RF sleep trace channel and writes `station-sleep.json`: for each RF
sleep, the MAC local-time counter's and the monotonic distance between the
sleep and wake edges, and whether the counter ran, held still or jumped. It
publishes `wifi.station.rf-sleep-observed` once a sleep was paired.
`station-sleep-dtim1` and `station-sleep-dtim3` are the DTIM 1/3 matrix at a
100 TU beacon interval under `min-modem`; `station-sleep-max-modem` sleeps for
listening intervals of 10 beacons. All are `investigation` scenarios.

### AP availability

`station-ap-loss` waits for connection, stops the controlled AP, requires beacon
loss, restarts it and requires the next connection generation. With workload
`require_recovery_echo = true`, three fresh ICMP replies are mandatory after
reconnection. It publishes `wifi.station.ap-loss-reconnected`, optionally
`wifi.station.recovered-ip-exchange`, and `wifi.station.control-responsive`.
An IP reply does not establish recovered throughput or a new DHCP lease.

`station-ap-absence` preserves its default: remove an already connected AP and
require generation-one no-candidate exhaustion. Workload `initially_absent = true`
selects the separate initial-start experiment used by
`station-ap-initial-absence`: stop the AP before starting the station, require
service admission followed by three candidate attempts in generation zero and
retry exhaustion without a connection. Role admission is distinct from
association; an accepted start alone cannot pass this experiment.
The modes publish `wifi.station.recovery-retry-exhausted` or
`wifi.station.initial-retry-exhausted` respectively. Both query the control
interface afterwards and publish `wifi.station.control-responsive`. The host
observes production lifecycle events; it does not implement retry policy.

## Program-counter profile

An optional top-level `[profile]` table (`harts = "both" | "core0" |
"core1"`, `period-us` in 100..=100000) samples the program counter of the
image's harts during the workload's measured window. The image class must
compile the sampler (`pc-profile`, today `diagnostic-task-residence`), and only
an `investigation` scenario without the `performance` tag may request it:
sampling perturbs timing. The table is part of the procedure, so a profiled scenario
is a different scenario from its unprofiled copy, and its evidence never
stands in for a throughput or timing measurement.

## Unsupported scenarios

The optional top-level `unsupported` field states why the current firmware
cannot run a scenario: no image it builds serves the workload's image class,
or the class's image does not declare a role the workload drives. The runner
refuses an explicitly named unsupported scenario before building any image;
a tag or run-all selection skips it and prints its id and reason once. A
qualification requirement on it stays open, and `cargo qualification next`
shows the reason. The field is not part of the executed procedure, so
recorded evidence stays comparable. The runner's catalog test fails when a
marked scenario is served, or an unmarked one is not, by the image classes
declared in `hil/targets/esp32s31/agent/Cargo.toml` and their declared
capabilities; after flashing, the runner checks the same roles against the
capabilities the image reports.
