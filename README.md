# open-esp-radio-rs

Source-only `no_std` radio drivers for Espressif chips, with typed hardware
ownership, Embassy integration and independent verification tooling.

The production implementation targets **ESP32-S31**. IEEE 802.11 has portable
protocols and an ESP32-S31 driver core, but no ESP32-S31 composition yet.
Bluetooth LE and IEEE 802.15.4 provide narrower implemented operations with
explicit limits. A source API, host test or target build does not establish
that an operation works on air. ESP32-C5 has investigation inputs, not a
production radio backend.

See [driver architecture](crates/README.md) for ownership and supported
composition, and the [ESP32-S31 radio feature map](crates/hardware/esp32s31/driver/FEATURES.md)
for shared lifecycle, PHY, coexistence and all three protocol inventories.

The project connects open radio implementation with typed resource ownership and
checks against compiled production code. It lets contributors investigate a
hardware operation, implement it in Rust, and trace the evidence behind its
documented behavior.

| Your interest | A useful starting point | Requirements |
| --- | --- | --- |
| Binary analysis and hardware research | [Blobray task map](tools/blobray/README.md#choose-a-task) | Linux host; real research also needs your own identified vendor artifacts |

The [documentation index](docs/README.md) lists the contracts that cross code
owners; `cargo xtask doc` builds the API documentation.

The workspace is licensed under [MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE),
except the crates holding the ESP-IDF port, which are Apache-2.0 only.
[`NOTICE`](NOTICE) lists the third-party material and its licenses; the
[facade](crates/oer/README.md#license) states its effective license.

## Start here

| Task | Documentation |
| --- | --- |
| Investigate hardware | [Blobray task map](tools/blobray/README.md#choose-a-task), [review and publication](registers/README.md) |
| Use the public `oer` API | [Radio facade](crates/oer/README.md) |
| Understand component boundaries | [Repository architecture](docs/architecture.md) |
| Build API docs, lockfiles and worktrees | [Repository tooling](tools/xtask/README.md) |
| Build or execute hardware scenarios | [ESP32-S31 HIL](hil/targets/esp32s31/README.md) |
| Compare vendor and compiled Rust behavior | [Verification](verification/README.md) |
| Review register/PAC publication | [Registers](registers/README.md) |
| Find a reference or maintain documentation | [Documentation index](docs/README.md) |

## Repository layout

| Path | Owner |
| --- | --- |
| `crates/` | Shipping protocol, chip, adapter, runtime and integration code |
| `platform/` | Shared board boot, staged entry and memory placement |
| `hil/` | Hardware protocol, runner, fixtures, scenarios and test images |
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
```

`cargo xtask push` pushes the branch and opens its pull request as a draft,
which a person marks ready and merges.

For API changes, run `cargo xtask doc`: one `cargo doc --no-deps` per
documentation target with `RUSTDOCFLAGS=-D warnings`, as each package's
`[package.metadata.docs.rs]` selects, and `cargo test --doc --workspace`. Run
focused package tests and the relevant target profile while iterating.

Independent example, integration, HIL and Blobray workspaces have their own
build configuration; the root workspace check alone does not cover them. Blobray host tests run with
`cargo test --manifest-path tools/blobray/Cargo.toml --workspace --locked --offline`
and need LLD plus a GNU RV32 linker from binutils 2.47 or later.

Linux/OpenWrt fixture operations belong to HIL. Hardware commands require the
configured lab and an attached device; source checks do not install fixtures
or change network state.
