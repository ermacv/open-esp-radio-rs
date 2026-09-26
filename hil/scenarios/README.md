# Versioned scenario catalog

Folders identify the workload domain: system, IEEE 802.15.4, Bluetooth and
IEEE 802.11 station/access-point/roles/monitor. Scenario IDs are stable across
folder moves. Tags select overlapping diagnostic, characterization and
qualification uses; they do not assign ownership or make a hardware-readiness
claim.

## Document shape

Schema 5 documents have a common header and exactly one family table:

```toml
schema = 5
id = "udp-rx-he20-calibration"
description = "..."
repetitions = 3                 # default 1
transfer = "identical-image"    # default unchanged-functional-contract
tags = ["he20"]
control = "udp-rx-he20-ceiling" # controlled experiments only

[wifi]                          # or [bluetooth], [system], [ieee802154]
image = "correctness"

[wifi.workload]
kind = "station-udp"
link = { phy = "he20" }
duration_seconds = 30
payload_bytes = 1472
offer = { rx_bps = 50000000 }

[wifi.workload.maintenance]
operation = "calibration"

[wifi.workload.criteria]
minimum_rx_bps = 45000000
```

The family owns every executable value:

- `[system]`, `[ieee802154]` and `[bluetooth]` are tagged workloads whose kind
  implies the firmware image. A Bluetooth workload that runs with or without
  automatic PHY maintenance selects the image through a typed field
  (`active_maintenance`, `exercise` or `phy_maintenance`).
- `[wifi]` selects the image and an optional `[wifi.datapath]` initialization
  (placement, checksum, TX-buffer and RX-continuation diagnostics). Each
  workload variant carries its own link expectation, observers, fixture
  mutations and acceptance criteria, so a field that has no meaning for a
  workload cannot be written. Offers are directional: `rx_bps` flows to the
  target, `tx_bps` from it, and the present offers define the direction.

Every table rejects unknown fields. Relations that remain between the image,
the data path and the workload are validated by the family.

## Controlled experiments

An optional top-level `control` names the control scenario. The experiment is
station UDP RX with a `[wifi.workload.maintenance]` operation; its control has
none. Link, traffic, image, data path, observers, fixture mutations,
repetitions and absolute criteria must match. Only identity, description, tags
and the maintenance table may differ. Controls cannot
themselves name controls. Both catalog consumers validate this relation.

`cargo hil plan` includes the control, without adding qualification prerequisite
suites. A control is an execution relation, not inherited evidence that all
Wi-Fi behavior is correct. Qualification requires the experiment and control
to pass in the same eligible sealed run with equal repetition counts. This is
not a relative non-regression criterion, and HT40 evidence does not establish
the corresponding HE20 integration.

## Named checks

Named checks are derived from implemented workload behavior and explicit
criteria, not tags. Station UDP RX exposes `udp.rx.target-rate` and
`udp.rx.host-offer-rate` when an RX rate floor is configured;
`udp.rx.maximum-silence` requires an explicit silence limit. With maintenance,
the workload also records `wifi.maintenance.transaction-valid` and
`wifi.maintenance.same-link`. The latter checks the station lifecycle through
the observed session; it is not a standalone proof of RF quality or throughput.
Qualification can require these observations by name without duplicating their
thresholds. `cargo hil plan --proof NAME` filters candidates before expanding
control relations. It never fills in missing observations from a suite PASS.

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

`maintenance.require_post_maintenance_echo = true` is supported only by station
UDP RX. It adds
`wifi.maintenance.ip-exchange-resumed`: three fresh ICMP exchanges after a
successful maintenance result and completion of the original UDP session, within
the same captured station epoch. Missing or partial exchange fails the scenario;
`same-link` alone cannot substitute for it. The bounded reply timeout detects
failure and is not an RF execution-time claim. The
[HT20 continuity scenario](ieee80211/station/station-phy-maintenance-continuity.toml)
keeps this functional property separate from existing throughput/latency gates.

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

## Transfer policy

The optional top-level `transfer` field is `unchanged-functional-contract`
(default) or `identical-image`. A whole-scenario timing, memory or RF guarantee
requires `identical-image`; maintenance deadlines and watchdog reset windows
are included even when no named numeric check exists. This field governs
review applicability and does not alter execution. Named checks have their own
explicit transfer contract in the evaluator. Functional checks may be reviewed
separately while a timing guarantee remains bound to the application image.
