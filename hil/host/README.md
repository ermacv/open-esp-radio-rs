# HIL host setup and operations

Run commands in this guide from the repository root. HIL executes the
production driver on real hardware and records typed evidence; it is not an
alternative radio implementation or the qualification evaluator. Read the
[execution and evidence architecture](architecture.md) before interpreting a
bundle.

```text
hil/
├── protocol/          host/target command and telemetry wire protocol
├── schema/            evidence contract shared with qualification
├── scenarios/         versioned, non-secret host workloads and criteria
├── evidence/          recorded evidence shards per chip
├── host/
│   ├── runner/        CLI, run orchestration and workload dispatch
│   ├── execution/     repetition context, failure classification, cleanup evidence
│   ├── stand/         laboratory config and locks, recovery, post-mortem
│   ├── link/          UART session, protocol, transports, measurements, peer console
│   ├── evidence/      run writer, seal, verification and reports
│   ├── image/         image builder, audits and firmware records
│   ├── scenario/      scenario envelope, catalog, campaign plan and requirements
│   ├── image-class/   image classes and the keys each serves
│   ├── source-snapshot/ source snapshots builds and runs are made from
│   ├── durable/       atomic host files and digests
│   ├── board/         board support: flash, reset and boot flows per chip
│   ├── runner-*/      one radio family's workloads and fixtures each
│   ├── arbiter/       leases, claims, balances and the board journal
│   ├── fixture/       finite Linux helpers and their contract
│   ├── fixture-install/ root-executed fixture installation
│   ├── linux-net/     privileged Linux AP/monitor fixture
│   └── linux-bluetooth/ privileged Linux Bluetooth fixture
├── peers/             ESP-IDF reference peer images (esp32c5)
├── bootloaders/       second-stage bootloader per chip
├── agent/            chip-independent HIL agent logic
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
`target/hil/current-observer.json`. Cargo uses `--locked` and, like every Cargo
command in this repository, runs offline (`.cargo/config.toml`); `cargo tidy
fetch` downloads what a changed lock file needs. Cancellation of
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

## Guides

| Guide | Content |
| --- | --- |
| [Sharing the stand](stand.md) | Leases, owners and balances, boards and their reset paths, the ESP-IDF firmware catalog, host fixture probing, the dashboard |
| [Runs and investigation](runs.md) | The shared run store, failure post-mortems and board recovery, enqueued runs, bisection, A/B comparison, performance, profiles and layout seeds |
| [Host fixtures](fixtures.md) | Network fixture preparation and restoration, evidence boundaries, Linux fixture software installation, probe load and air observers |
| [Execution and evidence architecture](architecture.md) | Package ownership, the run lifecycle and the bundle contract |
| [Stand arbiter](arbiter/README.md) | The queue, claims, balances and state files |

## Configure the stand

Run the host interface through the workspace alias:

```console
mkdir -p ~/.config/open-esp-radio
cp hil/stand/stand.example.toml ~/.config/open-esp-radio/stand.toml
chmod 0600 ~/.config/open-esp-radio/stand.toml
cargo hil doctor
```

The stand file is `~/.config/open-esp-radio/stand.toml` (or under
`$XDG_CONFIG_HOME`), shared by every checkout of this user; `--stand-file`
names another file. Its schema and rules belong to
[`oer-hil-stand-schema`](../stand/schema/README.md). It is the only source for
the stand's identity, its hubs and pool of boards, STA/AP credentials and
addresses and the fixtures; it lives outside the repository, and scenarios
contain no lab secrets or machine-specific paths. The stand id and the
boards' ids are written into every run manifest so results from different
stands and boards cannot be silently mixed.

A run's device under test is a board of the pool with the `dut` role and the
run's chip, which has a profile in `platform/<chip>/chip.toml`. There is no
default chip: a command that uses a device under test takes it from its
`--chip` or from the only chip that builds its scenarios' images, and checks
and resolves only that board, so an absent board, or one whose chip has no
profile yet, fails only the runs that take it. With several candidates of the
chip, `--board ID` names one, until the stand's scheduler assigns boards.

Multi-boot station lifecycle scenarios require the board's
`startup-artifact`.
Their first boot may create or replace it; every later boot must report
`Restored` before the station lifecycle can qualify. This makes cold PHY cache
replay an asserted transition rather than an informational UART message.

Peer scenarios run against a reference peer board, an ESP32-C5 with an
ESP-IDF catalog image: the [IEEE 802.15.4 peer](../peers/esp32c5-ieee802154/README.md),
the [Thread peer](../peers/esp32c5-openthread/README.md) or the
[Bluetooth LE Direct Test Mode peer](../peers/esp32c5-ble-dtm/README.md).
The peer is a board of the pool with the `peer` role, other than the device
under test; `--peer-board ID` names it when there are several, and its chip
must be the one the scenario's peer image targets (`hil/peers/*/firmware.toml`).
A run of a peer scenario claims the peer board beside the device under test
and the air in its one lease. Before the first scenario that
needs an image, the runner brings the board to that image's current catalog
build with `cargo hil firmware flash IMAGE --if-changed`, and writes the
image, application digest and commit the board journal recorded for it into
the scenario's `peer-image.json`, which the scenario's seal covers. Each
workload takes the running peer over with its `SYNC` command instead of a
reset: a USB Serial/JTAG reset of an ESP32-C5 whose radio runs can leave it in
ROM download. The peer drivers share one console transport,
`oer_hil_link::peer_line`, which also records the transcript of every
exchange.

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
`hil/schema/scenario-v5-defaults.json` (generated from the typed scenario
families by the runner's catalog test) and exclude top-level `description`,
`role`, `tags` and `unsupported`; all execution fields remain bound, including
repetitions and the complete family table. A plan neither executes hardware
nor invalidates sealed observations.

`cargo hil run <scenario>` builds and flashes the required image before the
scenario with the owned Xarxa/Embassy network stack, the only network
implementation; see the [implementation guide](../../docs/network-implementations.md).
`cargo hil image build performance` and `cargo hil image build correctness`
perform the same final stack/move, placement, source-graph and packed-image
checks without flashing or loading private stand file. Each successful
build emits one JSON report on stdout with class, target, profile, network,
class-owned artifact paths and build-audit verdicts; diagnostics stay on stderr.
An ELF or `application.bin` left beside a failed build is not a successful
image report.

For an explicit source snapshot, use:

```console
cargo hil image snapshot --source-include crates/path/to/new.rs
cargo hil image build performance --source-snapshot ~/.cache/open-esp-radio/build/source-snapshots/<snapshot-id>
```

`image build` takes several classes; from one source snapshot they share one
materialization of it and are built one after the other.

Snapshot capture is offline and does not load fixture secrets, build firmware or
access a device. Tracked regular files are captured automatically. Every
nonignored untracked file must be explicitly named with a repeated
`--source-include`, or, with `--include-untracked`, lie inside a path package
of the firmware workspaces (`hil/targets/esp32s31`,
`platform/esp32s31/bootstrap` and `hil/targets/esp32c5`, as Cargo's locked
metadata lists them), the
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
`--source-snapshot` validate and materialize these inputs in one of the host's
build slots, `~/.cache/open-esp-radio/build/<chip>/source-build-<n>/`
(`OER_BUILD_ROOT` replaces `~/.cache/open-esp-radio/build`). Every checkout
of the host shares the slots: a build takes a free one under an exclusive lock
and waits while all are busy. A slot keeps each unchanged file's bytes and
modification time, so Cargo rebuilds only the packages whose sources differ
from the slot's previous build, whichever checkout ran it. Cargo uses that
directory, including its copied configuration and snapshot-local override
paths. Artifacts (including copies of both ELFs) and the snapshot reference
remain under `~/.cache/open-esp-radio/build/<chip>/snapshot-builds/`. The live
checkout is not a build source for this explicit mode.

Each slot keeps its compile caches beside it, in
`source-build-<n>.cache/<profile>-<class>-<network>/`; `cargo xtask check
firmware` builds its classes there too. At most half the host's cores, and no
more than its memory holds at 2 GB each, compile image runtimes at once across
all checkouts; further builds wait for a free build slot in
`~/.cache/open-esp-radio/build/tokens/`.

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


`cargo hil run S... --repetitions N` runs each selected scenario N times
(1 to 20) instead of its own count, for a quicker look while debugging. The run
records the reduced count, and `cargo hil` does not note it as pending evidence.

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
also runs without a DUT or private stand file.

Qualification v4 independently reads the sealed bundles instead of trusting a
handwritten HIL status. A capability is HIL-qualified only when its declared
scenario and repetition requirement is satisfied by a completed bundle or a
separately sealed attempt bound to the current source composition. A verified snapshot matching the current
source inputs the observation depends on is directly applicable even when dirty, provided the executed
procedure and relevant host observer inputs also match. Its existence alone does
not establish this match or a passing observation. Scenario IDs and achievable repetition counts are checked against the
versioned catalog in `hil/scenarios`.
