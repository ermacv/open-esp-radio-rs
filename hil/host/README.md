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

Vendor-linked oracles remain isolated under `verification/vendor`; they are
not HIL scenarios or runner commands.

The ownership map and bundle contract are in the
[execution and evidence architecture](architecture.md).

## Configure the lab

Run the host interface through the workspace alias:

```console
cp hil/local.example.toml hil/local.toml
chmod 0600 hil/local.toml
cargo hil doctor
```

`hil/local.toml` is the only source for the stable lab-cell and DUT identities,
serial device, STA/AP credentials and addresses, startup artifact and OpenWrt
fixture. It is ignored by Git; scenarios contain no lab secrets or
machine-specific paths. The identities are written into every run manifest so
results from different cells and boards cannot be silently mixed.

Multi-boot station lifecycle scenarios require a configured startup artifact.
Their first boot may create or replace it; every later boot must report
`Restored` before the station lifecycle can qualify. This makes cold PHY cache
replay an asserted transition rather than an informational UART message.

## Build and run

`cargo hil run <scenario>` builds and flashes the required image before the
scenario. Select `--network upstream-xarxa` (default), `patched-xarxa`,
`upstream-smoltcp` or `owned-xarxa` to choose the stack implementation. The same choice
is available for station and access-point examples through `cargo xtask build
firmware <example> --network …`; see the
[implementation guide](../../docs/network-implementations.md).
`cargo hil image build performance` and `cargo hil image build correctness`
perform the same final stack/move, placement, source-graph and packed-image
checks without flashing or loading private lab configuration. Each successful
build emits one JSON report on stdout with class, target, profile, network,
class-owned artifact paths and build-audit verdicts; diagnostics stay on stderr.
An ELF or `application.bin` left beside a failed build is not a successful
image report.
`cargo hil run-all` reuses each image across its scenario group but
does not fail fast. Every invocation retains an immutable evidence bundle in
`target/hil/esp32s31/runs/<run-id>/`, including a canonical JSON suite, JUnit
XML, a standalone HTML report and the exact application image flashed for each
firmware class. The flash operation reads that archived copy, binding firmware
provenance to the bytes sent to the DUT. Completed and interrupted bundles also
carry a deterministic integrity inventory covering every retained file.

## Inspect evidence

The target-level `history.json` and `history.html` are deterministic derived
views over those bundles. Rebuild them at any time with
`cargo hil report rebuild`; no DUT or private lab configuration is required.
Verify the structure and content digests of one bundle with
`cargo hil report verify <run-id>`, or omit the ID to verify all bundles. This
also runs without a DUT or private lab configuration.

Qualification v4 independently reads the sealed bundles instead of trusting a
handwritten HIL status. A capability is HIL-qualified only when its declared
scenario and repetition requirement is satisfied by a completed bundle for
the exact current commit, and both the producer and evaluator worktrees are
clean. Scenario IDs and achievable repetition counts are checked against the
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
capability, sudo or hardware step and do not load `hil/local.toml`:

```console
cargo hil fixture install --provider linux-net --dry-run
cargo hil fixture install --provider linux-bluetooth --dry-run
```

The helper `capabilities` and sudoers validation subprocesses each have a
five-second execution deadline; timeout is an installation/preparation failure.
Bluetooth preparation and execution share the versioned contract in
[`bluetooth/contract.rs`](runner/src/fixture/bluetooth/contract.rs).
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

Bluetooth policy admits only named adapters. It defaults to the dedicated
`hci0`; repeat `--adapter hciN` to install a sorted finite allow-list. This
replaces the old broad `hci*` sudo rule. Reinstall with the adapters selected by
the private lab configuration before using another controller. `install-host`
remains an exact compatibility alias for `install --provider linux-net` only:

```console
cargo hil fixture install-host
```

The tracked `linux-net/install.sh` and `linux-bluetooth/install.sh` entry points
delegate to these Cargo commands when run unprivileged and reject a legacy
`sudo install.sh` invocation. Linux is required; the installer does not claim
support for another Unix host.

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
bundle. The prior generation, including a copied legacy installation, is
retained. Reinstalling the same bundle verifies it and refreshes its fixed-path
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
active. An upgrade from the previous volatile-lock layout also takes the old
`/run/open-radio-fixture/<provider>.session.lock` when that file still exists;
an active old consumer therefore cannot be bypassed with the new inode. A
missing old file after reboot is not recreated or required. Installation never
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
The Linux helper owns only `wlan0`; cleanup returns it to managed mode.

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

`cargo hil fixture install --provider linux-net` first runs `cargo xtask build hostapd` without
root, then installs the resulting binary, build provenance and helper through
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

`cargo hil fixture probe-plan` prints the finite request schedule without
loading lab configuration, opening interfaces or accessing the ESP. Installing
the helper or executing the scenario is separate from this offline preview.

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
rather than falling back to average throughput. The no-maintenance PHY
bidirectional control uses 250 ms to reject long delivery stalls independently
of its throughput floor. This is a delivery-continuity limit, not an RF airtime
measurement. Multi-client receive windows do not publish one ambiguous maximum.

The `diagnostic-station-absence-{unannounced,pm}-rx` pair holds the same physical
maintenance access for 10 ms without running a PHY algorithm. `station_pause =
{ synthetic = { duration_micros = 10000, notify_ap = true } }` selects confirmed
PM=1 before local stop and confirmed PM=0 after RX/MAC restoration. Durations
are bounded to 1..=200000 us; this is an experimental hold, not a listen/DTIM
schedule or a promise about AP buffer capacity. The ordinary idle power-save
planner cannot concurrently own this exchange; conflicting control ownership
returns Busy. A failed PM=1 requires acknowledged PM=0 recovery before normal
traffic resumes. An ambiguous return retains the runner in quarantine.
The reported round trip includes both PM exchanges when selected, while the
requested hold starts only after physical admission. These scenarios do not
enable PM notification for automatic calibration or qualify long absences.

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
