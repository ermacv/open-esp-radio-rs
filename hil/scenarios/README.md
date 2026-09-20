# Versioned scenario catalog

Folders identify the workload domain: system, IEEE 802.15.4, and IEEE 802.11
station/access-point/roles/monitor. Scenario IDs are stable across folder moves.
Tags select overlapping diagnostic, characterization and qualification uses;
they do not assign ownership or make a hardware-readiness claim.

## Controlled experiments

An optional `[comparison]` with `kind = "wifi-phy-maintenance"` names a
`control` scenario. The experiment is station UDP RX with a PHY maintenance
operation; its control has no such operation. Link, traffic, image, observers,
fixture mutations, repetitions and absolute criteria must match. Only identity,
description, tags and the maintenance operation may differ. Controls cannot
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
schema and repetition bounds; only the runner interprets executable workload
and acceptance fields.

The runner sorts by scenario ID.
`run-all` first traverses `ImageClass::ALL`, then the selected
scenarios of each image in catalog order. Folder traversal order cannot change
physical execution order. Each TOML document owns its image features,
workload criteria and repetition count.

Synthetic serialized compatibility inputs live in `hil/tests/fixtures/catalog`.
They are used by both independent readers and are not part of this catalog.

`criteria.require_post_maintenance_echo = true` is supported only by station UDP
RX with an explicit maintenance operation. It adds
`wifi.maintenance.ip-exchange-resumed`: three fresh ICMP exchanges after a
successful maintenance result and completion of the original UDP session, within
the same captured station epoch. Missing or partial exchange fails the scenario;
`same-link` alone cannot substitute for it. The bounded reply timeout detects
failure and is not an RF execution-time claim. The
[HT20 continuity scenario](ieee80211/station/station-phy-maintenance-continuity.toml)
keeps this functional property separate from existing throughput/latency gates.

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
