# Hardware-in-the-loop infrastructure

HIL executes the production driver on real hardware and records typed
qualification evidence. It must not become an alternative implementation of
the radio driver.

```text
hil/
├── protocol/          host/target command and telemetry wire protocol
├── scenarios/         versioned, non-secret host workloads and criteria
├── host/
│   ├── runner/        build, flash and scenario orchestration
│   └── linux-net/     privileged Linux AP/monitor fixture
└── targets/
    └── esp32s31/      current embedded target workspace
```

Target firmware lives under `hil/targets/<chip>`. Machine-readable evidence
lives in immutable bundles under `target/hil/<chip>/runs`. The qualification
evaluator independently checks those bundles; Markdown is not proof input.

Vendor-linked oracles remain isolated under `verification/vendor`; they are
not HIL scenarios or runner commands.

The ownership map and bundle contract are in [the host README](host/README.md).

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

`cargo hil run <scenario>` builds and flashes the required image before the
scenario. Select `--network upstream-xarxa` (default), `patched-xarxa`,
`upstream-smoltcp` or `owned-xarxa` to choose the stack implementation. The same choice
is available for station and access-point examples through `cargo xtask build
firmware <example> --network …`; see the
[implementation guide](../docs/network-implementations.md).
`cargo hil run-all` reuses each image across its scenario group but
does not fail fast. Every invocation retains an immutable evidence bundle in
`target/hil/esp32s31/runs/<run-id>/`, including a canonical JSON suite, JUnit
XML, a standalone HTML report and the exact application image flashed for each
firmware class. The flash operation reads that archived copy, binding firmware
provenance to the bytes sent to the DUT. Completed and interrupted bundles also
carry a deterministic integrity inventory covering every retained file.

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

The Linux helper is installed separately because its narrowly scoped AP,
managed-client, monitor and USB-reset operations require root privileges:

```console
cargo hil fixture install-host
```

`cargo hil doctor` also verifies the installed helper schema and its
non-interactive sudo capability before a scenario takes ownership of WLAN.

The installer needs interactive sudo authorization. Routine scenarios use the
installed narrow helper without prompting. Local Linux AP profiles are generated
from the station credentials and `station_fixture.country`, `channel` and CIDR
`address`; HT/HE mode comes from the scenario. The DHCP range excludes the AP
address and stays inside its subnet. Static station addresses must agree with
that subnet and gateway. The helper receives these values on stdin and keeps the
temporary hostapd configuration under root-owned `/run` with private permissions;
stop/cleanup removes it. No installed credential profiles are consumed.

The runner subscribes to hostapd control events, confirms ENABLED and actual
HT/HE mode, WPA2, channel geometry and IPv4 address before workload execution.
It records these non-secret settings in `fixture-applied.json`. The Linux helper
keeps hostapd startup diagnostics in a group-readable runtime log and includes
its bounded, credential-redacted tail in preparation errors before cleanup.
Debug logging ends before the workload starts; cleanup removes the runtime log.
The Linux helper owns only `wlan0`; cleanup returns it to managed mode.

`cargo hil fixture install-host` first runs `cargo xtask build hostapd` without
root, then installs the resulting binary, build provenance and helper through
interactive sudo. The build uses the pinned hostapd release and reviewed
[coexistence patch](host/linux-net/hostapd/README.md). It requires a C compiler,
make, pkg-config, libnl3 and OpenSSL development files, curl, tar and patch.
Verified cached outputs can be reused without downloading or compiling again.
Updating the helper contract requires rerunning the installer.

For `local-linux`, `station_fixture.coexistence` selects `respect` (default) or
`force-ht40`. The latter applies `noscan=1` only to HT40 scenarios; the OpenWrt
patch also skips client coexistence/intolerance handling in this mode. HT20 and
HE20 retain normal policy. The selected policy is recorded in fixture evidence.
Either policy still fails preparation when actual channel geometry differs
from the scenario: requested 40 MHz never silently becomes an accepted 20 MHz run.
