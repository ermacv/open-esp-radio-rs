# ESP32-S31 hardware-in-the-loop platform

This is a private embedded workspace for qualifying the public driver crates.
It is deliberately excluded from the root host workspace: normal
`cargo test --workspace --all-targets` must not build target-only binaries.

This directory is the test harness, not another radio-driver layer. It owns
the concrete board clock tree, boot flow, flash and PSRAM placement, Embassy
executor, and the full `embassy-net`/smoltcp application used for traffic
tests. Reusable network-driver and ESP32-S31 platform bindings live under
`../../crates/integration`; PAC/HAL/PHY/Wi-Fi behavior lives under
`../../crates/esp32s31` and `../../crates/wifi`.

The authoritative performance profile is `psram-code-psram-data`. Its image
has two stages:

1. a Flash/SRAM bootstrap initializes and verifies external memory;
2. a separately linked runtime executes code and ordinary data from PSRAM,
   while its stack, DMA objects and interrupt closure remain in internal SRAM.

Board electrical settings, credentials, traffic generation and PASS/FAIL
reporting belong here. PHY/MAC/STA behavior belongs in `../../crates` and must
be moved there before a new behavior is entered in the canonical feature
ledger.

The closed vendor oracle is never a dependency of this workspace. It is an
explicitly excluded sibling workspace at `../vendor-oracle/esp32s31` and is
reachable only through `cargo hil oracle ...`.

The `trace-probes` member is another test-only artifact. It contains retained
LTO wrappers used by `cargo vendor-code-validator` to compare compiled PAC/HAL
leaves with vendor ELF/archive symbols; it is not linked into board firmware.

Common commands from the repository root:

```text
cargo hil scenarios
cargo hil doctor
cargo hil build radio
cargo hil flash radio --port /dev/ttyACM0
cargo hil build udp-tx
cargo hil traffic trigger <monitor-interface> --transmitter <bssid> --aid <aid>
cargo hil oracle verify
cargo hil oracle build
cargo hil oracle flash --port /dev/ttyACM0
```

## Firmware scenarios

`cargo hil scenarios` prints the authoritative list. Each scenario has its
own directory below
`target/hil/esp32s31/psram-code-psram-data-<artifact-name>` and a
`scenario.txt` manifest containing the non-secret compile-time mode selection.
The runner removes inherited scenario-selection variables before applying the
named mode, so an old shell export cannot silently combine two HIL workloads.

| Scenario | Firmware workload | Host-side follow-up |
| --- | --- | --- |
| `boot-smoke` | bootstrap, Flash/PSRAM and stage-two runtime | inspect UART PASS marker |
| `radio` | production `WifiRunner` PHY/MAC/STA/WPA2 path | scenario-specific UART inspection |
| `udp-tx` | `embassy-net` device-to-host UDP throughput | provide the configured UDP receiver |
| `tcp-rx` | runtime-configured host-to-device TCP stream | `cargo hil traffic tcp-rx <device-ipv4>` |

The `radio` path uses the production `StaJoinRunner` for Open Authentication
and Association, `Wpa2HandshakeRunner` for the WPA2 Message 1/3 exchange, and
`Wpa2KeyInstallRunner` for typed PTK/GTK publication, Message 4 and rollback.
`Esp32s31ControlTx` owns Probe, Authentication, Association, EAPOL and the
protected bootstrap publication until it transfers the same pinned descriptor,
EDCA state, calibrated power and executor adapters to `Esp32s31SingleMpduTx`.
Their HIL adapters now own diagnostics and finite PAC/RX capabilities only;
absolute Embassy deadlines, retry state, RX-before-timeout ordering and
live-ring/key/TX ownership are shared driver behavior rather than parallel
test-only loops. The connected handoff wraps that ordinary owner in
`Esp32s31ConnectedTx`: referenced A-MPDU leases, BlockAck retry and the
beacon-loss deadline now run on this one production runner. The opt-in
connected power planner also ACK-gates PM=1 and restores PM=0 before queued
network data, but the HIL does not enable it or consume its doze permit. Actual
modem sleep remains disabled until its complete PAC sleep/wakeup transaction
is qualified.

Build and flash a named scenario with the same identifier:

```text
cargo hil build radio
cargo hil flash radio --port /dev/ttyACM0
```

The former raw-MAC, A-MPDU/A-MSDU and HE matrix scenarios depended on the
second connected event loop that predated the production backend. They are
intentionally absent from `cargo hil scenarios`; selecting their old build
environment fails at compile time. Reintroduce each workload only after its
aggregate/HE transaction is owned by the production backend and scheduled by
`WifiRunner`, so a HIL result cannot accidentally qualify a parallel driver.

SSID, passphrase, peer address, channel and bounded rate/MCS overrides remain
external HIL configuration. They are intentionally not written to
`scenario.txt`, because that file must not capture credentials. A scenario
name selects the workload and artifact identity; it does not claim that the
connected AP can satisfy every cell in that workload.

STA connection scans all ESP32-S31 2.4-GHz channels automatically.
`OPEN_RADIO_STA_CHANNEL` is only a preferred first-channel hint for controlled
setups; a stale hint cannot pin the connection to the wrong channel.

The root-owned AP/monitor helper and its narrow sudo policy live in
`tools/open-radio-net`. Reinstall it from this repository after changing the
helper or moving the checkout:

```text
cargo build -p open-esp-radio-hil-runner
sudo tools/open-radio-net/install.sh
```
