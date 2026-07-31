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

Common commands from the repository root:

```text
cargo hil scenarios
cargo hil doctor
cargo hil build radio
cargo hil flash bidirectional --port /dev/ttyACM0
cargo hil traffic bidirectional <device-ip> --phy he20
cargo hil traffic trigger <monitor-interface> --transmitter <bssid> --aid <aid>
cargo hil traffic trigger-hil <monitor-interface> --transmitter <bssid> --aid <aid>
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
| `radio` | baseline open PHY/MAC/STA/WPA2 path | scenario-specific UART inspection |
| `bidirectional` | synthetic raw-MAC uplink while receiving host UDP | `cargo hil traffic bidirectional ...` |
| `udp-tx` | `embassy-net` device-to-host UDP throughput | provide the configured UDP receiver |
| `amsdu` | synthetic A-MSDU inside A-MPDU | inspect raw-MAC HIL markers |
| `network-amsdu` | copy-free `embassy-net` A-MSDU/A-MPDU ownership | provide the configured UDP receiver |
| `he-mcs-gi-matrix` | HE SU MCS0..9 and GI/LTF matrix | inspect matrix terminal marker |
| `he-ldpc-matrix` | HE SU LDPC MCS/GI matrix | use an LDPC-capable AP |
| `he-dcm-matrix` | HE DCM constellation/GI matrix | use an AP advertising the required DCM cells |
| `he-tb` | Trigger-based HE-TB transmit path | `cargo hil traffic trigger-hil ...` |
| `he-delimiter` | HE empty-delimiter/length matrix | inspect delimiter terminal marker |

Build and flash any named scenario with the same identifier:

```text
cargo hil build he-mcs-gi-matrix
cargo hil flash he-mcs-gi-matrix --port /dev/ttyACM0

cargo hil flash he-tb --port /dev/ttyACM0
cargo hil traffic trigger-hil <monitor-interface> \
  --transmitter <bssid> --aid <aid>
```

SSID, passphrase, peer address, channel and bounded rate/MCS overrides remain
external HIL configuration. They are intentionally not written to
`scenario.txt`, because that file must not capture credentials. A scenario
name selects the workload and artifact identity; it does not claim that the
connected AP can satisfy every cell in that workload.

The root-owned AP/monitor helper and its narrow sudo policy live in
`tools/open-radio-net`. Reinstall it from this repository after changing the
helper or moving the checkout:

```text
cargo build -p open-esp-radio-hil-runner
sudo tools/open-radio-net/install.sh
```
