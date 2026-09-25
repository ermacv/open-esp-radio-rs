# Repository checks

`oer-xtask` owns repository source, dependency and build orchestration. Cargo
metadata describes package boundaries; compiler checks and compiled artifacts
supply evidence. These commands do not supply driver behavior, hardware
scenario verdicts or product readiness.

Run from the repository root:

```console
cargo xtask check docs
cargo xtask ```

The PHY archive contains LLVM bitcode. Install `rustup component add
llvm-tools-preview` for the selected toolchain; the audit uses its bundled
`llvm-nm`. Native ELF parsing and symbol policy are implemented in Rust.

| Command | Contract |
| --- | --- |
| `cargo xtask check metadata` | Locked metadata for every actual Cargo workspace island, including unstaged source moves |
| `cargo xtask check architecture` | Run Clippy on minimum/default and supported feature profiles, applying each crate's lint policy; reject Wi-Fi packages in Bluetooth facade profiles; check layer/chip boundaries, isolated facade consumers, public type identities and composition contracts |
| `cargo xtask check safety` | Crate-root unsafe attributes match the reviewed audited list, and reviewed hardware access boundaries |
| `cargo xtask check network` | Resolve isolated network consumers and compile supported profiles |
| `cargo xtask check network-backpressure` | Resolve the pinned minimal Xarxa patch and test UDP device-capacity quiescence/recovery with the production adapter |
| `cargo xtask check network --dependencies-only` | Check the same dependency boundaries without compiling profiles |
| `cargo xtask check docs` | Check owned Markdown local links and check/render the static qualification catalogs and programs; API documentation is `cargo xtask doc` |
| `cargo xtask doc` | Build API documentation as docs.rs would: one `cargo doc --no-deps` per `[package.metadata.docs.rs]` target with `RUSTDOCFLAGS=-D warnings`, then `cargo test --doc --workspace` |
| `cargo xtask check phy` | Build the PHY library for the chip target and audit its artifact and dependency graph |
| `cargo xtask check images` | Build both final performance/correctness HIL application images and run their target audits |
| `cargo xtask check blobray-standalone` | Extract generic Blobray source, check path-dependency containment and compile every target, including its launcher |
| `cargo xtask build firmware <example>` | Build, audit and package a complete staged application; `--flash` writes it and `--monitor` opens the console |
| `cargo xtask build vendor-probes --chip esp32s31` | Build the selected project's three Rust comparison artifacts |
| `cargo xtask build vendor-probes --chip esp32s31 --list-roles` | List declared artifact roles without building or authenticating an artifact |

The root Cargo alias selects this package. `--root PATH` selects an explicit
repository checkout. A nested independent workspace does not acquire the root
workspace's package membership through `--manifest-path`.

Architecture and example checks resolve their Cargo jobs from the same typed
configuration model. Package metadata owns supported feature alternatives; the
model keeps workspace, package/target, host or MCU target, feature selection and
Cargo build profile separate. API documentation instead follows each package's
standard `[package.metadata.docs.rs]` table.

Use focused package tests and target builds for the code being changed. Run
`check docs` for prose/catalog changes and `cargo xtask doc` for API changes.
The CI jobs in `.github/workflows/ci.yml` are the full source checkpoint; they
are not required after every local edit. Shared contracts,
Cargo feature policy, generated PAC and firmware layout changes need the relevant
broader architecture, safety and artifact checks. Partial checks do not establish
full repository coverage.

`check docs` covers tracked Markdown, owner documents below `docs/`, package
`README.md` files and Cargo `readme` targets; arbitrary untracked working notes
are not repository documentation. External URLs are counted as
`external-not-checked`; no network requests are made. Catalog checking and
rendering neither load runtime evidence nor evaluate readiness, and the command
performs no hardware operations.

`check images` first builds the HIL runner and the Blobray audit host in
`tools/blobray/target`. Both image classes then build at once: each owns its output directory and resolves through a private
copy of the committed lockfile. The image owner builds the real performance and
correctness application images through the HIL builder, checks each class's
fresh stack, placement and packed-image artifacts, and runs the final radio
target audit on each reported runtime ELF. Successful builds emit separate
machine-readable image reports with class, target, profile, network, artifact
paths and builder/final-audit verdicts. A failed or incomplete correctness
build cannot be replaced by a previous application image or a successful
performance build. This gate does not run on hardware or measure runtime stack
high-water.

Network dependency checks distinguish released Embassy, original upstream,
maintained owned and research contracts. Released Embassy products
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
All three station and AP feature profiles are checked in their own locked workspace;
library profiles use isolated consumers so unrelated workspace features cannot
hide a dependency leak.

Run the orchestration regressions with:

```console
cargo test -p oer-xtask
cargo test --manifest-path tools/blobray/Cargo.toml -p blobray --test launcher
```

Tests exercise actual temporary Cargo graphs, ownership, argument boundaries,
negative inputs and child-process lifecycle. Cargo and Rust discover tests;
there are no source-spelling or regex checks for required Rust identifiers.
Builds retain normal Cargo parallelism. `OPEN_RADIO_ANALYSIS_BUILD_JOBS` is an
optional explicit local limit for vendor probe builds.

Standalone firmware builds keep a Cargo cache per example and network selection,
and copy their ELFs and images into a unique
`target/firmware/esp32s31-<example>/<network-or-none>/build-<id>/` bundle.
Use `--network upstream-xarxa`, `patched-xarxa`, `upstream-smoltcp` or
`owned-xarxa` for station/AP. Omission selects upstream Xarxa; explicit network
Cargo features resolve to their corresponding implementation. See the
[implementation guide](../../docs/network-implementations.md) for smoltcp and
owned-network choices.

Firmware builds never modify a committed `Cargo.lock`. Each build copies the
workspace catalog into its own cache and resolves through Cargo's
`resolver.lockfile-path` (Cargo 1.97+), so a patched network or local override
writes only that copy, which is archived as the build's effective lockfile.
Metadata checks read committed catalogs without waiting, and builds of
different examples, networks or image classes run concurrently. An overlapping
build of the same output fails on that copy's lease; the artifact lease
separately protects the selected cache and output snapshot.
Successful bundles remain available for inspection, while failed partial bundles
are removed. Flashing uses the completed bundle after releasing the build lease,
so a later build cannot replace the selected image. The serial-device lease
independently protects the complete hardware write transaction.

Standalone extraction copies only nonignored Blobray source, excluding private
inputs and build outputs. It owns its temporary target directory, preserves a
caller-selected Rust toolchain, and otherwise uses the repository's pinned
channel. Extracted Blobray has no dependency on this xtask package.

The built-in analysis supervisor remains owned by Blobray. Linux/OpenWrt
fixture logic and privileged installation remain owned by HIL; xtask does not
install fixtures or change network state. Linux process ownership uses explicit
process groups; Blobray provides cgroup containment or an explicitly selected process-tree watchdog. Unsupported hosts
return an error when the required ownership backend is unavailable.
