# open-esp-radio-rs

Source-only `no_std` radio drivers for Espressif chips, with typed hardware
ownership, Embassy integration and independent verification tooling.

The production implementation targets **ESP32-S31**. IEEE 802.11 has STA, AP,
same-channel STA+AP and monitor compositions. Bluetooth LE and IEEE 802.15.4
provide narrower implemented operations with explicit limits. A source API,
host test or target build does not establish on-air readiness: the
[qualification evaluator](qualification/README.md) derives that from accepted
evidence. ESP32-C5 has investigation inputs, not a production radio backend.

See [driver architecture](crates/README.md) for ownership and supported
composition, and the [ESP32-S31 radio capability map](crates/hardware/esp32s31/driver/FEATURES.md)
for shared lifecycle, PHY, coexistence and all three protocol inventories.

The project connects open radio implementation with typed resource ownership and
checks against compiled production code. It lets contributors investigate a
hardware operation, implement it in Rust, and trace the evidence behind its
documented behavior.

| Your interest | A useful starting point | Requirements |
| --- | --- | --- |
| Radio drivers and embedded Rust | [Follow a real channel change](docs/channel-walkthrough.md) from reviewed hardware facts to STA | Source reading and host tests need no board; on-air work needs ESP32-S31 hardware |
| Binary analysis and hardware research | [Blobray task map](tools/blobray/README.md#choose-a-task) and the synthetic exercise | Linux host; real research also needs your own identified vendor artifacts |
| Portable Rust and resource ownership | [First host contribution](docs/first-contribution.md) | Rust basics and public build dependencies; no private inputs |
| A station application | [Buildable station example](examples/esp32s31-station/README.md) and [capability limits](crates/hardware/esp32s31/driver/FEATURES.md) | Supported board, toolchain and network configuration |

Start with the [documentation routes](docs/README.md) and
[contribution guide](CONTRIBUTING.md). The [documentation portal build](tools/docs/README.md)
provides searchable guides, API configurations and a declarations-only capability
map. The [Pages address](https://ermacv.github.io/open-esp-radio-rs/) becomes
available after successful manual publication; local preview works independently.

The workspace is licensed under [MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE).

## Start here

| Task | Documentation |
| --- | --- |
| Understand the architecture | [From binary evidence to a Wi-Fi station](docs/binary-to-station.md) |
| Trace one operation through the layers | [Channel change: evidence to STA](docs/channel-walkthrough.md) |
| Make a first contribution | [Host tutorial](docs/first-contribution.md), [contribution guide](CONTRIBUTING.md) |
| Investigate hardware | [Blobray task map](tools/blobray/README.md#choose-a-task), [review and publication](registers/README.md) |
| Build and validate a station | [ESP32-S31 route](docs/station-hardware.md) |
| Use the public `oer` API | [Radio facade](crates/oer/README.md) |
| Choose a network stack and understand its patches | [Network implementations](docs/network-implementations.md) |
| Build a station application | [Station example](examples/esp32s31-station/README.md) |
| Build another radio role | [AP](examples/esp32s31-access-point/README.md), [monitor](examples/esp32s31-monitor/README.md), [Bluetooth controller](examples/esp32s31-bluetooth-controller/README.md) |
| Understand component boundaries | [Repository architecture](docs/architecture.md) |
| See implementation, knowledge, observations and next work | [Project capability map](qualification/README.md#everyday-status-and-next-work) |
| Assess a selected STA or BLE scope | [Qualification programs](qualification/README.md) |
| Check source and dependency policy | [Repository tooling](tools/repo/README.md) |
| Build or execute hardware scenarios | [ESP32-S31 HIL](hil/targets/esp32s31/README.md) |
| Compare vendor and compiled Rust behavior | [Verification](verification/README.md) |
| Review register/PAC publication | [Registers](registers/README.md) |
| Find a reference or maintain documentation | [Documentation index](docs/README.md) |

## Repository layout

| Path | Owner |
| --- | --- |
| `crates/` | Shipping protocol, chip, adapter, runtime and integration code |
| `platform/` | Shared board boot, staged entry and memory placement |
| `examples/` | Application/board composition and API examples |
| `hil/` | Hardware protocol, runner, fixtures, scenarios and test images |
| `qualification/` | Engineering map, capability programs and independent evaluation |
| `registers/` | Reviewed hardware descriptions and PAC publication inputs |
| `verification/` | Chip knowledge and concrete vendor comparison projects |
| `tools/` | Blobray, memory analysis and `cargo xtask` repository operations |
| `experiments/` | Experimental engines and their host compositions |
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
cargo xtask check docs
```

For API changes, run `cargo xtask doc`: one `cargo doc --no-deps` per
documentation target with `RUSTDOCFLAGS=-D warnings`, as each package's
`[package.metadata.docs.rs]` selects, and `cargo test --doc --workspace`. Run
focused package tests and the relevant target profile while iterating.

Use `cargo xtask check source-only` for the complete source checkpoint. It also needs the embedded target and the selected
toolchain's `llvm-tools-preview` component. It checks dependency and ownership
boundaries, generated PAC outputs and compiled artifacts. Independent example,
integration, HIL and Blobray workspaces have their own build configuration; the
root workspace check alone does not cover them. Blobray host tests run with
`cargo test --manifest-path tools/blobray/Cargo.toml --workspace --locked --offline`
and need LLD plus a GNU RV32 linker from binutils 2.47 or later.

Linux/OpenWrt fixture operations belong to HIL. Hardware commands require the
configured lab and an attached device; source checks do not install fixtures
or change network state.
