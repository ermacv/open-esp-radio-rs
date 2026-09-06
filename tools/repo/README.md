# Repository checks

`oer-xtask` owns repository source, dependency and build orchestration. Cargo
metadata describes package boundaries; compiler checks and compiled artifacts
supply evidence. These commands do not supply driver behavior, hardware
scenario verdicts or product readiness.

Run from the repository root:

```console
cargo xtask doctor
cargo xtask check source-only
```

The PHY archive contains LLVM bitcode. Install `rustup component add
llvm-tools-preview` for the selected toolchain; the audit uses its bundled
`llvm-nm`. Native ELF parsing and symbol policy are implemented in Rust.

| Command | Contract |
| --- | --- |
| `cargo xtask check metadata` | Locked metadata for every actual Cargo workspace island, including unstaged source moves |
| `cargo xtask check architecture` | Compile supported feature profiles and check dependency ownership and composition contracts |
| `cargo xtask check safety` | Compiler-enforced unsafe policy and reviewed hardware access boundaries |
| `cargo xtask check network` | Resolve isolated network consumers and compile supported profiles |
| `cargo xtask check network --dependencies-only` | Check the same dependency boundaries without compiling profiles |
| `cargo xtask check examples` | Target type checks of the four examples and the station released-compat/upstream-Xarxa variants |
| `cargo xtask check source-only` | Compose repository suites, Cargo/Clippy, publication and final-image analysis |
| `cargo xtask check blobray-standalone` | Extract generic Blobray source, check path-dependency containment and compile every target, including its launcher |
| `cargo xtask build firmware <example>` | Build, audit and package a complete staged application; `--flash` writes it and `--monitor` opens the console |
| `cargo xtask build vendor-probes --chip esp32s31` | Build the selected project's three Rust comparison artifacts |
| `cargo xtask build vendor-probes --chip esp32s31 --list-roles` | List declared artifact roles without building or authenticating an artifact |

The root Cargo alias selects this package. `--root PATH` selects an explicit
repository checkout. A nested independent workspace does not acquire the root
workspace's package membership through `--manifest-path`.

Network dependency checks distinguish released compatibility, original upstream,
maintained owned and research contracts. Compatibility products
use the official crates.io Embassy network APIs and exclude the owned adapter
and Xarxa. Owned products use fully revision-pinned network forks, with the
Embassy stack and driver resolving to the same source. These source rules do
not prohibit shared platform forks such as ESP-HAL. Research excludes Embassy
and Xarxa from normal and build dependencies, including optional declarations;
its default and complete feature selections are resolved independently.
Original upstream checks require the reviewed full revisions from
`embassy-rs/embassy` and `embassy-rs/xarxa` and reject fork, registry-network or
local-source substitutions. Product and example checks reject mixed network
features. Development-only dependencies do not define a production ownership boundary.
All three station example selections are checked in their own locked workspace;
library profiles use isolated consumers so unrelated workspace features cannot
hide a dependency leak.

Run the orchestration regressions with:

```console
cargo test -p oer-xtask
cargo test -p blobray --test launcher
```

Tests exercise actual temporary Cargo graphs, ownership, argument boundaries,
negative inputs and child-process lifecycle. Cargo and Rust discover tests;
there are no source-spelling or regex checks for required Rust identifiers.
Builds retain normal Cargo parallelism. `OPEN_RADIO_ANALYSIS_BUILD_JOBS` is an
optional explicit local limit for vendor probe builds.

Standalone firmware builds keep a Cargo cache per example and hold that example's
artifact lease until its ELFs and images are copied into a unique
`target/firmware/esp32s31-<example>/build-<id>/` bundle. Different examples can build
concurrently; repeated builds of one example wait cancellably for its cache.
Successful bundles remain available for inspection, while failed partial bundles
are removed. Flashing uses the completed bundle after releasing the build lease,
so a later build cannot replace the selected image. The serial-device lease
independently protects the complete hardware write transaction.

Standalone extraction copies only nonignored Blobray source, excluding private
inputs and build outputs. It owns its temporary target directory, preserves a
caller-selected Rust toolchain, and otherwise uses the repository's pinned
channel. Extracted Blobray has no dependency on this xtask package.

The compiled `blobray-run` limiter remains owned by Blobray. Linux/OpenWrt
fixture logic and privileged installation remain owned by HIL; xtask does not
install fixtures or change network state. Linux process ownership uses explicit
process groups; Blobray provides its own session/systemd resource limiter. Unsupported hosts
return an error when the required ownership backend is unavailable.
