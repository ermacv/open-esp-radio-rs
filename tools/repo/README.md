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
| `cargo xtask check architecture` | Compile minimum/default and supported feature profiles; check layer/chip boundaries, isolated facade consumers, public type identities and composition contracts |
| `cargo xtask check bluetooth` | Resolve isolated BLE facade profiles without Wi-Fi dependencies and compile host/target profiles including validation probes |
| `cargo xtask check safety` | Compiler-enforced unsafe policy and reviewed hardware access boundaries |
| `cargo xtask check network` | Resolve isolated network consumers and compile supported profiles |
| `cargo xtask check network-backpressure` | Resolve the pinned minimal Xarxa patch and test UDP device-capacity quiescence/recovery with the production adapter |
| `cargo xtask check network --dependencies-only` | Check the same dependency boundaries without compiling profiles |
| `cargo xtask check examples` | Target type checks of the four examples, station/AP network profiles, both BLE smoke configurations and host application-library tests |
| `cargo xtask check docs --list` | List resolved documentation configurations and genuine inapplicable actions without running doc builds, doctests or qualification evaluation |
| `cargo xtask check docs` | Build separate public/private rustdoc, run applicable host doctests, compile MCU example consumers, check owned Markdown links and render static qualification views |
| `cargo xtask check source-only` | Compose repository suites, including the docs gate, Cargo/Clippy, publication and final-image analysis |
| `cargo xtask check blobray-standalone` | Extract generic Blobray source, check path-dependency containment and compile every target, including its launcher |
| `cargo xtask build firmware <example>` | Build, audit and package a complete staged application; `--flash` writes it and `--monitor` opens the console |
| `cargo xtask build vendor-probes --chip esp32s31` | Build the selected project's three Rust comparison artifacts |
| `cargo xtask build vendor-probes --chip esp32s31 --list-roles` | List declared artifact roles without building or authenticating an artifact |

The root Cargo alias selects this package. `--root PATH` selects an explicit
repository checkout. A nested independent workspace does not acquire the root
workspace's package membership through `--manifest-path`.

Architecture, example and documentation checks resolve their Cargo jobs from
the same typed configuration model. Package metadata owns supported feature
alternatives; the model keeps workspace, package/target, host or MCU target,
feature selection and Cargo build profile separate. A package or target that
cannot take a documentation action is listed with its reason instead of being
silently omitted.

The full docs gate uses the pinned toolchain and target with locked, offline
Cargo operations. Its ignored outputs live below `target/docs/gate/`: the
resolved plan is `job-plan.json`, public and private rustdoc snapshots are
separate, static catalog views are under `catalogs/`, and `report.json` is
written only after every stage succeeds. The gate checks tracked Markdown,
owner documents below `docs/`, package `README.md` files and Cargo `readme`
targets. Arbitrary untracked working notes are not repository documentation.
External URLs are counted as `external-not-checked`; no network requests are
made. Static catalog checking and rendering neither load runtime evidence nor
evaluate readiness, and the command performs no hardware operations.
Rustdoc type-checking of the bootstrap uses an owned empty compile input for
its required `PSRAM_RUNTIME_BIN`; it is never linked, packed or presented as a
firmware image. The source-only image owner continues to provide the real
packed stage-two runtime for final-image analysis.

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
All three station and AP feature profiles are checked in their own locked workspace;
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

Standalone firmware builds keep a Cargo cache per example and network selection,
and copy their ELFs and images into a unique
`target/firmware/esp32s31-<example>/<network-or-none>/build-<id>/` bundle.
Use `--network upstream-xarxa`, `patched-xarxa`, `upstream-smoltcp` or
`owned-xarxa` for station/AP. Omission selects upstream Xarxa; explicit network
Cargo features resolve to their corresponding implementation. See the
[implementation guide](../../docs/network-implementations.md) for smoltcp and
owned-network choices.

Different example workspaces can build concurrently. An overlapping build in
the same workspace fails before changing its dependency lock catalog; the
artifact lease separately protects the selected cache and output snapshot.
Repository metadata checks and isolated graph catalog snapshots take shared
read leases on the same workspace catalog. They wait for a patched build to
restore the original lockfile; a build waits for existing readers. Waiting uses
OS file locks. Readers and independent workspaces can still run concurrently.
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
