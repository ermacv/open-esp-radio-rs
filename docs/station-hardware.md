# Build and investigate an ESP32-S31 station

Use this route after [the host tutorial](first-contribution.md). Reading source
and provenance requires no board. Binary research requires your own identified
artifacts; on-device observations require an ESP32-S31 board and the configured
lab. These are separate prerequisites.

## Trace channel configuration

Read [the scan handoff](binary-to-station.md#production-layers-during-a-scan), then
follow these owners:

1. [Runtime target binding](../crates/runtime/embassy/esp32s31/ieee80211/src/roles/scan/target.rs)
   implements the scan port and retains the caller's hardware owner.
2. [Chip `ScanPhy`](../crates/hardware/esp32s31/driver/ieee80211/sta/src/hardware/channel.rs)
   distinguishes initial channel selection from stop/retune/restore.
3. [PHY channel algorithms](../crates/hardware/esp32s31/phy/src/channel.rs)
   use restricted HAL authority. Follow referenced helpers and data to their
   source identities; register publication alone does not supply RF tables.
4. [Reviewed register model and provenance](../registers/esp32s31/README.md)
   and [publication policy](../registers/esp32s31/publication/README.md)
   define the permitted PAC interface.

From the repository root, check that the reviewed sources reproduce the
published artifacts:

```console
cargo registers generate --manifest registers/esp32s31/publication/registers.toml --check
```

Success establishes publication consistency, not correct RF behavior. To compare
captured code/data, use [captured PHY research](../verification/vendor/projects/esp32s31/README.md#captured-phy-research-with-next)
and [Blobray's task map](../tools/blobray/README.md#choose-a-task). Supply your own
ELF/archive/ROM and required source identities; retain unknowns, assumptions and
model boundaries. The legacy project command grammar on reference pages is not
available through current `cargo blobray`.

## Build an application

Install the checkout's toolchain, configured embedded target, `rust-src` and
`llvm-tools-preview`. Fetch public dependencies for the root, platform and
station workspaces before using offline checks. From the repository root:

```console
cargo fetch --locked
cargo fetch --locked --manifest-path platform/esp32s31/Cargo.toml
cargo fetch --locked --manifest-path examples/esp32s31-station/Cargo.toml
cargo xtask build firmware station --network upstream-xarxa
```

The [station example](../examples/esp32s31-station/README.md) owns credentials,
network selection and exact flashing commands. The build checks placement and
packages boot plus application images. A successful build does not show that a
device scanned, associated or acquired an IP address.

For an attached board, follow that example's credential and serial-port setup,
then its explicit flash/monitor command. With the expected AP and network,
observe scan, association/WPA2, DHCP and the documented UDP/ICMP behavior. An
application log is useful debugging information; it is not a sealed independent
qualification observation.

## Move from an observation to evidence

Read [HIL host setup](../hil/host/README.md) and the
[ESP32-S31 target guide](../hil/targets/esp32s31/README.md) before running a
scenario. They own board ports, fixture access, lab configuration, observer
requirements and image selection. Select the scenario corresponding to the
claim in [the scenario catalog](../hil/scenarios/README.md); do not substitute
an unrelated passing experiment. Hardware commands can flash the board and
configure lab fixtures.

Expected outputs are the scenario's typed observations and sealed run bundle,
including failed or incomplete attempts. Follow
[verification and qualification](verification-and-qualification.md) to assess
their applicability to the selected Wi-Fi STA program. The
[qualification commands](../qualification/README.md) read existing evidence;
they do not run missing experiments.

The portal's generated status pages are declarations-only and explicitly
`not-evaluated`. For a real readiness decision, use the evaluator with your
applicable evidence and retain its full provenance in ignored output storage.

Next: use [CONTRIBUTING](../CONTRIBUTING.md) to select checks for the owner you
change and describe the exact evidence boundary in your PR.
