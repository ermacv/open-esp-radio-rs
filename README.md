# open-esp-radio-rs

Source-only `no_std` radio drivers for Espressif chips, with typed hardware
ownership, Embassy integration and independent verification tooling.

The production implementation targets **ESP32-S31**. IEEE 802.11 has STA, AP,
same-channel STA+AP and monitor compositions. Bluetooth LE and IEEE 802.15.4
provide narrower implemented operations with explicit limits. A source API,
host test or target build does not establish on-air readiness: the
[qualification evaluator](qualification/README.md) derives that from accepted
evidence. ESP32-C5 has investigation inputs, not a production radio backend.

See [driver architecture](driver/README.md) for ownership and supported
composition, and the [IEEE 802.11](driver/chips/esp32s31/ieee80211/FEATURES.md)
and [Bluetooth](driver/chips/esp32s31/bluetooth/FEATURES.md) feature references
for implementation coverage and limitations.

## Start here

| Task | Documentation |
| --- | --- |
| Build a station application | [Station example](examples/esp32s31-station/README.md) |
| Build another radio role | [AP](examples/esp32s31-access-point/README.md), [monitor](examples/esp32s31-monitor/README.md), [Bluetooth controller](examples/esp32s31-bluetooth-controller/README.md) |
| Understand component boundaries | [Repository architecture](docs/architecture.md) |
| Check source and dependency policy | [Repository tooling](tools/repo/README.md) |
| Build or execute hardware scenarios | [ESP32-S31 HIL](hil/targets/esp32s31/README.md) |
| Compare vendor and compiled Rust behavior | [Verification](verification/README.md) |
| Review register/PAC publication | [Registers](registers/README.md) |
| Find a reference or maintain documentation | [Documentation index](docs/README.md) |

## Repository layout

| Path | Owner |
| --- | --- |
| `driver/` | Shipping protocol, chip, adapter, runtime and integration code |
| `platform/` | Shared board boot, staged entry and memory placement |
| `examples/` | Application/board composition and API examples |
| `hil/` | Hardware protocol, runner, fixtures, scenarios and test images |
| `qualification/` | Capability programs and independent readiness evaluation |
| `registers/` | Reviewed hardware descriptions and PAC publication inputs |
| `verification/` | Chip knowledge and concrete vendor comparison projects |
| `tools/` | Blobray, memory analysis and `cargo xtask` repository operations |
| `docs/` | Cross-component contracts and documentation conventions |

Applications own board startup, credentials, network stacks, DHCP and sockets.
The driver owns radio resources and their execution lifetime. Ordinary driver
and HIL builds do not link vendor radio archives or the radio ROM ABI; private
vendor inputs are confined to opt-in comparison workflows. See
[public source policy](docs/source-policy.md).

## Development checks

Run from the repository root with the toolchain selected by
`rust-toolchain.toml`. Cargo manifests define the MSRV, target dependencies and
supported feature profiles; lockfiles pin their resolution.

```console
cargo check --workspace --locked --offline
cargo test --workspace --locked --offline
cargo fmt --all -- --check
cargo xtask check source-only
```

The complete source-only gate also needs the embedded target and the selected
toolchain's `llvm-tools-preview` component. It checks dependency and ownership
boundaries, generated PAC outputs and compiled artifacts. Independent example,
integration and HIL workspaces have their own build configuration; the root
workspace check alone does not cover them.

Linux/OpenWrt fixture operations belong to HIL. Hardware commands require the
configured lab and an attached device; source checks do not install fixtures
or change network state.
