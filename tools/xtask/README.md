# Repository checks

`oer-xtask` owns repository source, dependency and build orchestration. Cargo
metadata describes package boundaries; compiler checks and compiled artifacts
supply evidence. These commands do not supply driver behavior, hardware
scenario verdicts or product readiness.

Run from the repository root:

```console
cargo xtask check docs
cargo xtask doc
```

The PHY archive contains LLVM bitcode. Install `rustup component add
llvm-tools-preview` for the selected toolchain; the audit uses its bundled
`llvm-nm`. Native ELF parsing and symbol policy are implemented in Rust.

| Command | Contract |
| --- | --- |
| `cargo xtask check changed [--base REV]` | Before a push: `cargo fmt --check` for every workspace a file changed against the merge base with `REV` (default `origin/main`, including uncommitted and untracked files) belongs to; root-workspace Clippy with `-D warnings`; tests and docs.rs-style API documentation of the changed root packages; `check docs` for Markdown/qualification changes, `check capabilities` for Rust source or catalog changes, `check metadata` for manifest changes, `check phy` for production crate manifest changes, `check network` for manifest, lockfile or network-owner changes, a link of the station example for `platform/` changes, `cargo registers validate` and `generate --check` of every register publication for `registers/` or `tools/registers` changes, `check provenance` for every chip with pinned vendor artifacts (fetching missing ones into the shared store first) when code, register models, vendor docs or provenance facts changed, and a `cargo check` of the HIL firmware feature sets a HIL source change can break. Other workspaces are formatted only; it is not full coverage |
| `cargo xtask compare elf OLD NEW` / `compare images --base REV [--class C]...` | Compare linked RISC-V images function by function modulo placement: formed addresses (branches, `auipc`/`lui` pairs, `.word`, data pointers) become symbol+offset, legacy mangling hashes and LLVM clone numbers are dropped, identical-code-folded names pair by body; `--alias FROM=TO` applies a reviewed rename, `--allow NAME` a reviewed scheduling tie and `--show NAME` prints the instruction diff of matching functions. `images` builds each class at REV in a detached worktree and in this checkout. Fails unless every function is equivalent: the gate for pure code moves between crates |
| `cargo xtask stand-install` | Build the xtask of `origin/main` in its own clone under `~/.local/share/open-esp-radio/stand-tool` and install `~/.local/bin/oer-stand`, which runs operational `cargo hil` commands against the caller's checkout without building its tree |
| `cargo xtask push` | Push this checkout's commits to `main`: refuses uncommitted tracked changes, rebases onto `origin/main` and runs `check changed` in a fresh `cargo xtask`, then enters the machine-wide push queue (`$XDG_DATA_HOME/open-esp-radio/push.lock`, overridable with `OER_PUSH_QUEUE`; a waiter sees its holder). Inside the queue no other session moves `main`, so a push checks at most twice: once outside, and once more inside only when `main` moved meanwhile. It pushes only a checked revision, runs `stand-install` when the pushed commits change `tools/xtask`, `tools/process`, `hil/host` or `hil/schema` |
| `cargo xtask worktree add PATH --branch B [--from REV]` / `remove PATH` / `prepare` | Create a Git worktree whose `target/` starts from this checkout's build outputs (without incremental data, HIL outputs and vendor firmware builds, whose CMake caches name the source checkout), so only the workspace's own crates rebuild; `remove` deletes it with its `target/`. When `target/` is a btrfs subvolume the seed is an instant snapshot, otherwise a reflink copy taking minutes; `prepare` turns this checkout's `target/` into a subvolume once (run it while no build uses `target/`) |
| `cargo xtask fetch` | Download the dependencies every workspace's lock file names and the local cache lacks; Cargo runs offline in this repository, so a pull that changed a lock file needs it before a plain `cargo build` |
| `cargo xtask lock` | Update every workspace's `Cargo.lock` to its manifests after a dependency or pin change; `check metadata` reports every stale lock in one run |
| `cargo xtask sweep [--all-checkouts] [--apply]` | List, or remove, rebuildable build caches: `incremental` crate data unused for a day and HIL image caches unused for 3 days or built for a removed network implementation. Directories whose Cargo build lock is held are skipped; run bundles, evidence and archives are never touched. `check changed` and the HIL runner commands sweep every sibling checkout at most once a day for the whole host, and at once, with 2-hour and 12-hour limits, while less than 200 GiB are free; below 20 GiB free they refuse to build |
| `cargo xtask check metadata` | Locked metadata for every actual Cargo workspace island, including unstaged source moves; every island applies the root `[patch]` replacements and resolves each Git package to one commit; every island repeats the root `[workspace.lints]` and every package inherits it, except the standalone Blobray workspace and the generated raw PAC |
| `cargo xtask check architecture` | Run Clippy on minimum/default and supported feature profiles, applying each crate's lint policy; reject Wi-Fi packages in Bluetooth facade profiles; check layer/chip boundaries, isolated facade consumers, public type identities and composition contracts; crate-root unsafe attributes match the reviewed audited list and direct PAC dependencies the reviewed consumer list |
| `cargo xtask check network` | Resolve isolated network consumers and audit their dependency boundaries; CI compiles the profiles |
| `cargo xtask check docs` | Check owned Markdown local links, repository paths and `cargo <alias>` commands in inline code, and check/render the static qualification catalogs and programs; API documentation is `cargo xtask doc` |
| `cargo xtask check capabilities [--changed FILE ...]` | Check the `// CAPABILITY: <id>` code anchors against every qualification catalog ([rules](../../qualification/README.md#code-anchors)) and list the entries anchored in the changed files |
| `cargo xtask doc` | Build API documentation as docs.rs would: one `cargo doc --no-deps` per `[package.metadata.docs.rs]` target with `RUSTDOCFLAGS=-D warnings`, then `cargo test --doc --workspace` |
| `cargo xtask check phy --chip CHIP` | Build the PHY library for the chip target and audit its artifact and dependency graph |
| `cargo xtask check images` | Build both final performance/correctness HIL application images and run their target audits |
| `cargo xtask check firmware --all \| --class CLASS… [--type-check] [--jobs N]` | Build every HIL image class (or each `--class`) from a snapshot of the checkout in one of the host's build slots, as `cargo hil image build --source-snapshot` does, with its stack, placement and application audits, and print one PASS/FAIL line per class; a failure does not stop the remaining classes, and any failure fails the command. One seed class per dependency family builds first and the classes of its family start from its compiled units, up to `--jobs` at once (default: half the cores, at most 8). `check changed` runs it for the classes a change reaches: those whose last build read a changed file, as the build's `source-inputs.json` records it (sources, workspace and Cargo configuration, toolchain, stack policy, partition table and the image builder's own sources), and every class without such a record. A root `Cargo.lock` change also runs the vendor-package audit. `--type-check` only runs `cargo check` of each runtime |
| `cargo xtask check blobray-standalone` | Extract generic Blobray source, check path-dependency containment, then build and test every Blobray crate |
| `cargo xtask check provenance --chip CHIP` | Fail when a vendor function a production `SOURCE:` block, register-model evidence source or register or field description cites changed in, or vanished from, the pinned artifacts since its reviewed fingerprint in `verification/<chip>/facts/provenance.toml`, or is not registered; requires `cargo xtask vendor-fetch` |
| `cargo xtask vendor-fetch CHIP` | Download the chip's pinned vendor artifacts into `target/vendor` and verify each against `verification/<chip>/artifacts.toml` |
| `cargo xtask vendor-scenario SCENARIO ...` | Build Blobray and the typed vendor scenarios, then run one scenario with the forwarded arguments |
| `cargo xtask hil-observer` | Prepare the current HIL observer configuration without running HIL |
| `cargo xtask vendor-firmware --chip CHIP [PROJECT]` | Build the tracked vendor firmware of `verification/<chip>/hil-vendor` against the ESP-IDF revision pinned in `artifacts.toml`, checked out once per host in `~/.cache/open-esp-radio/esp-idf/<revision>-<pins>` (`OER_IDF_CACHE` overrides the directory; every checkout shares it) with every IDF submodule whose repository is a pinned source at its pinned revision and verified artifacts; fails when the image links an archive named like a pinned artifact that differs from it. Installs the IDF tools once into the cache's `idf-tools`; preparation holds the cache lock exclusively and builds hold it shared. A checkout's former `target/vendor-firmware/esp-idf` and `idf-tools` are removed on its first build; writes `build.json` next to each image |
| `cargo xtask vendor-diff --chip CHIP --old A --new B` | Classify every function of two archive revisions by relocation-normalized code: unchanged, references renamed, renamed, changed (with similarity), removed (with the closest candidate) or added; `--baseline DIR` compares every pinned artifact with its namesake in `DIR` |
| `cargo xtask evidence --chip CHIP [SCENARIO...]` | Rewrite the vendor evidence shards whose recorded sources changed, or the named scenarios' shards; builds the probes and runs the scenarios with the pinned artifacts, then prints each shard's changed claims (verdicts, cases, coverage, untriaged locations) apart from its changed source digests and lines. Each probe build, the Blobray and scenario builds, each stand and each scenario run print a `phase <name>: <seconds> s` line as they end, so a slow run shows where its time went. The verification owner runs it, including after a merge that conflicted in `verification/<chip>/evidence/scenarios` |
| `cargo xtask evidence --chip CHIP --check [--changed-since REV] [SCENARIO...]` | Rerun the named scenarios, or every one, and fail unless each committed shard equals its rerun, printing the changed claims of each that differs. `--changed-since` reruns only shards that record a file changed since `REV`, in the worktree or untracked, and names each skipped one; a shard that does not parse is always rerun |
| `cargo xtask vendor-provenance --chip CHIP --accept NAME[,NAME] [--show]` | Record the pinned fingerprint of cited functions after reviewing their facts, printing how each registered fingerprint moves; `--show` prints each function's annotated pinned code first; `--rebuild --baseline DIR` recomputes the registry from the current citations with fingerprints of the revision in `DIR` |
| `cargo xtask register-inventory --chip CHIP [--output DIR]` | Compare the vendor's statically resolved accesses inside the publication's owned MMIO ranges with the register model: words no register declares, masked bits outside every declared field, and opaque declared bits the vendor touches; words touched by a function the provenance registry names rank first. Blobray analyzes every pinned vendor binary once, cached in `target/register-inventory/<chip>/analysis` by the digests of the Blobray host and the inputs; writes `report.txt` and `report.json` beside it. Addresses computed at run time stay outside the inventory |
| `cargo xtask build firmware <example>` | Build, audit and package a complete staged application; `--flash` writes it under a HIL stand lease and `--monitor` opens the console. Without `--port` it flashes the only attached board registered as `esp32s31` |
| `cargo xtask build vendor-probes --chip esp32s31` | Build the three Rust probe images of the ESP32-S31 vendor comparison |
| `cargo xtask build vendor-probes --chip esp32c5` | Build the Rust probe image of the ESP32-C5 vendor comparison |
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

Network dependency checks resolve each network consumer in isolation and
audit its boundary: neutral interface crates depend on no network stack, owned
products and adapters use the revision-pinned owned Xarxa/Embassy forks with
the stack and driver resolving to the same source, the datapath and radio core
stay free of network stacks, and research excludes Embassy and Xarxa from
normal and build dependencies, including optional declarations. These source
rules do not prohibit shared platform forks such as ESP-HAL. Development-only
dependencies do not define a production ownership boundary. Library profiles
use isolated consumers so unrelated workspace features cannot hide a
dependency leak.

Run the orchestration regressions with:

```console
cargo test -p oer-xtask
```

Tests exercise actual temporary Cargo graphs, ownership, argument boundaries,
negative inputs and child-process lifecycle. Cargo and Rust discover tests;
there are no source-spelling or regex checks for required Rust identifiers.
Builds retain normal Cargo parallelism. `OPEN_RADIO_ANALYSIS_BUILD_JOBS` is an
optional explicit local limit for vendor probe builds.

Standalone firmware builds keep a Cargo cache per example and network selection,
and copy their ELFs and images into a unique
`target/firmware/esp32s31-<example>/<network-or-none>/build-<id>/` bundle.
Station and AP use the owned Xarxa/Embassy stack (`--network owned-xarxa`, the
default and only value); see the
[implementation guide](../../docs/network-implementations.md).

Firmware builds never modify a committed `Cargo.lock`. Each build copies the
workspace catalog into its own cache and resolves through Cargo's
`resolver.lockfile-path` (Cargo 1.97+), so a `[patch]` or local override
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
