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
lives in immutable bundles under `target/hil/<chip>/runs`; dated narratives in
`qualification/targets/esp32s31/records` are historical context, not proof
consumed by the qualification evaluator.

Vendor-linked oracles remain isolated under `verification/vendor`; they are
not HIL scenarios or runner commands.

The current ownership map is in [the host README](host/README.md).
[`docs/HIL_ARCHITECTURE_AUDIT.md`](../docs/HIL_ARCHITECTURE_AUDIT.md) preserves
the dated 2026-09-01 audit and implementation history.

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
scenario. `cargo hil run-all` reuses each image across its scenario group but
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

The controlled OpenWrt AP and the HIL host share its laboratory LAN; reverse
flows use the local IPv4 route selected for the discovered target. The
upstream FRITZ!Box supplies Internet access and optional HE20 compatibility
smoke tests, but is not an exact-delivery fixture.

An AP scenario that uses the controlled OpenWrt client starts from a fresh
OpenWrt wireless epoch. The runner removes its scoped forwarding/VIF state,
restarts the OpenWrt wireless subsystem, and verifies the requested channel
and width before resetting the DUT. This is part of measurement isolation:
mt76/mac80211 can otherwise retain a pathological state across many virtual
client lifecycles in which every BA32 succeeds and all retry/error counters
remain zero, but several milliseconds of idle time appear between aggregates.
The AP report records whether this fixture preparation ran and how long it
took.

The Linux helper is installed separately because its narrowly scoped AP,
managed-client, monitor and USB-reset operations require root privileges:

```console
cargo build -p open-esp-radio-hil-runner
sudo hil/host/linux-net/install.sh
```

`cargo hil doctor` also verifies the installed helper schema and its
non-interactive sudo capability before a scenario takes ownership of WLAN.
