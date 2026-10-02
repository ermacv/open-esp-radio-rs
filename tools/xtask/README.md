# Repository checks

`oer-xtask` owns repository source, dependency and build orchestration. Cargo
metadata describes package boundaries; compiler checks and compiled artifacts
supply evidence. These commands do not supply driver behavior, hardware
scenario verdicts or product readiness.

Run from the repository root:

```console
cargo xtask check tidy
cargo xtask check docs
cargo xtask doc
```

The PHY archive contains LLVM bitcode. Install `rustup component add
llvm-tools-preview` for the selected toolchain; the audit uses its bundled
`llvm-nm`. Native ELF parsing and symbol policy are implemented in Rust.

## Gate and push

The gate every change passes before it reaches `main`; see [the push gate](#the-push-gate) for what it selects and runs.

| Command | Contract |
| --- | --- |
| `cargo xtask check changed [--base REV] [--full]` | The push gate over what this checkout changed against the merge base with `REV` (default `origin/main`), committed, uncommitted or untracked: see [the gate](#the-push-gate). `--full` adds what CI checks on `main` after a push: root-workspace Clippy, docs.rs-style API documentation of the affected root packages, a `cargo check` of the HIL image classes the change reaches (those whose last build read a changed file, as its `source-inputs.json` records, and every class without such a record), a link of the station example for `platform/` changes, `check phy` for production manifest changes, `check network` for manifest, lockfile or network-owner changes, `cargo registers validate` and `generate --check` for register changes, and `check provenance` of every chip with pinned vendor artifacts for code, model, vendor-doc or fact changes |
| `cargo xtask push` | Push `HEAD` to `main` after the gate passed on exactly that tree: refuses uncommitted changes to tracked files and untracked `.rs`, `Cargo.toml` or `Cargo.lock` files, gates the files `HEAD` changed against its merge base with `origin/main`, then fetches and rebases. When the incoming commits affect none of the packages this push affects (changed packages and their dependents) it pushes at once, otherwise it reruns the gate for the packages both affect; a non-fast-forward rejection goes round again. No lock, queue or nested `cargo xtask`; `stand-install` is a separate command |

## Locks, caches and worktrees

Cargo runs offline here (`.cargo/config.toml`); these commands keep lock files, the Cargo cache and build caches in step.

| Command | Contract |
| --- | --- |
| `cargo xtask lock [--check]` | Update every workspace's `Cargo.lock` to its manifests after a dependency or pin change (online); `--check` only verifies, offline, that every lock matches its manifests and names every stale one. `cargo tidy fetch` downloads what the locks name and the cache lacks: it builds only `oer-tidy`, so it works after a pull that changed what xtask itself needs |
| `cargo xtask sweep [--apply]` | List, or remove, this checkout's rebuildable build caches (`incremental` crate data unused for a day, HIL image caches unused for 3 days). Directories whose Cargo build lock is held and what a queued HIL job was fixed with are skipped; run bundles, evidence and archives are never touched, and no other checkout's `target/` is. Nothing sweeps implicitly, and no command refuses to start for lack of disk space |
| `cargo xtask worktree add PATH --branch B [--from REV]` / `remove PATH` / `prepare` | Create a Git worktree whose `target/` starts from this checkout's build outputs (without incremental data, HIL outputs and vendor firmware builds, whose CMake caches name the source checkout), so only the workspace's own crates rebuild; `remove` deletes it with its `target/`. When `target/` is a btrfs subvolume the seed is an instant snapshot, otherwise a reflink copy taking minutes; `prepare` turns this checkout's `target/` into a subvolume once (run it while no build uses `target/`) |
| `cargo xtask stand-install` | Build the xtask of `origin/main` in its own clone under `~/.local/share/open-esp-radio/stand-tool` and install `~/.local/bin/oer-stand`, which runs operational `cargo hil` commands against the caller's checkout without building its tree |

## Source and architecture checks

Host-side policy over the source tree and the Cargo graph.

| Command | Contract |
| --- | --- |
| `cargo xtask check tidy` | Run the fast integrity tier of [`oer-tidy`](../tidy/README.md) over the checkout in seconds: every Rust file reachable from a crate root, capability anchors and vendor citations only in reachable files, every repository path that qualification and evidence records name present, every package in a discovered workspace with its lock file, every declared dependency named by its package, and every package classified with a known layer and platform, named by the `oer-<tokens>` rule and keeping the evidence and HIL role edges. The push gate always runs it, and CI runs it as its own early job |
| `cargo xtask check docs` | Check owned Markdown local links, repository paths and `cargo <alias>` commands in inline code, and check/render the static qualification catalogs and programs; API documentation is `cargo xtask doc` |
| `cargo xtask check capabilities [--changed FILE ...]` | Check the `// CAPABILITY: <id>` code anchors against every qualification catalog ([rules](../../qualification/README.md#code-anchors)) and list the entries anchored in the changed files |
| `cargo xtask doc` | Build API documentation as docs.rs would: one `cargo doc --no-deps` per `[package.metadata.docs.rs]` target with `RUSTDOCFLAGS=-D warnings`, then `cargo test --doc --workspace` |
| `cargo xtask check metadata` | Locked metadata for every actual Cargo workspace island, including unstaged source moves; every island applies the root `[patch]` replacements and resolves each Git package to one commit; every island repeats the root `[workspace.lints]` and every package inherits it, except the standalone Blobray workspace and the generated raw PAC |
| `cargo xtask check architecture` | Run Clippy on minimum/default and supported feature profiles of every production package for its chip's Rust target (`platform/<chip>/chip.toml`), applying each crate's lint policy; reject Wi-Fi packages in Bluetooth facade profiles; check layer/platform edges, that contract, protocol, hardware, role and service packages reach no HAL or Embassy crate but `embassy-sync`, isolated facade consumers, public type identities and composition feature contracts; crate-root unsafe attributes match the reviewed audited list and direct PAC dependencies the reviewed consumer list. Classification and package names are `check tidy` rules |
| `cargo xtask check network` | Resolve isolated network consumers and audit their dependency boundaries; CI compiles the profiles |
| `cargo xtask check feature-sets` | Test every root-workspace package with each feature set its `open-radio.test-feature-sets` declares; `check changed` does the same for the packages it tests |

## Firmware builds and image checks

Target builds of the HIL image classes, standalone examples and the PHY library.

| Command | Contract |
| --- | --- |
| `cargo xtask check firmware --all \| --class CLASS… \| --list [--type-check] [--jobs N]` | Build every HIL image class (or each `--class`) from a snapshot of the checkout in one of the host's build slots, as `cargo hil image build --source-snapshot` does, with its stack, placement and application audits, and print one PASS/FAIL line per class; a failure does not stop the remaining classes, and any failure fails the command. One seed class per dependency family builds first and the classes of its family start from its compiled units, up to `--jobs` at once (default: half the cores, at most 8). `--list` prints every class with the runtime features it builds with. `--type-check` only runs `cargo check` of each runtime with the class's features: `cargo xtask check firmware --class performance --type-check` is the cheap way to type-check one image class |
| `cargo xtask check images` | Build both final performance/correctness HIL application images and run their target audits |
| `cargo xtask check phy --chip CHIP` | Build the PHY library for the chip target and audit its artifact and dependency graph |
| `cargo xtask build firmware <example> [--type-check]` | Build, audit and package a complete staged application; `--flash` writes it under a HIL stand lease and `--monitor` opens the console. Without `--port` it flashes the only attached board registered as `esp32s31`. `--type-check` only runs `cargo check` of the runtime with the image's target, features and compiler flags, as CI does for every example |
| `cargo xtask build vendor-probes --chip esp32s31` | Build the three Rust probe images of the ESP32-S31 vendor comparison |
| `cargo xtask build vendor-probes --chip esp32c5` | Build the Rust probe image of the ESP32-C5 vendor comparison |
| `cargo xtask build vendor-probes --chip esp32s31 --list-roles` | List declared artifact roles without building or authenticating an artifact |
| `cargo xtask compare elf OLD NEW` / `compare images --base REV [--class C]...` | Compare linked RISC-V images function by function modulo placement: formed addresses (branches, `auipc`/`lui` pairs, `.word`, data pointers) become symbol+offset, legacy mangling hashes and LLVM clone numbers are dropped, identical-code-folded names pair by body; `--alias FROM=TO` applies a reviewed rename, `--allow NAME` a reviewed scheduling tie and `--show NAME` prints the instruction diff of matching functions. `images` builds each class at REV in a detached worktree and in this checkout. Fails unless every function is equivalent: the gate for pure code moves between crates |
| `cargo xtask hil-observer` | Prepare the current HIL observer configuration without running HIL |

## Vendor verification and provenance

Pinned vendor artifacts, the evidence shards compared against them and the provenance of recovered facts; see [vendor verification](../../verification/README.md).

| Command | Contract |
| --- | --- |
| `cargo xtask vendor-fetch CHIP` | Download the chip's pinned vendor artifacts into `target/vendor` and verify each against `verification/<chip>/artifacts.toml` |
| `cargo xtask check provenance --chip CHIP` | Fail when a vendor function a production `SOURCE:` block, register-model evidence source or register or field description cites changed in, or vanished from, the pinned artifacts since its reviewed fingerprint in `verification/<chip>/facts/provenance.toml`, or is not registered; requires `cargo xtask vendor-fetch` |
| `cargo xtask vendor-diff --chip CHIP --old A --new B` | Classify every function of two archive revisions by relocation-normalized code: unchanged, references renamed, renamed, changed (with similarity), removed (with the closest candidate) or added; `--baseline DIR` compares every pinned artifact with its namesake in `DIR` |
| `cargo xtask vendor-provenance --chip CHIP --accept NAME[,NAME] [--show]` | Record the pinned fingerprint of cited functions after reviewing their facts, printing how each registered fingerprint moves; `--show` prints each function's annotated pinned code first; `--rebuild --baseline DIR` recomputes the registry from the current citations with fingerprints of the revision in `DIR` |
| `cargo xtask vendor-scenario SCENARIO ...` | Build Blobray and the typed vendor scenarios, then run one scenario with the forwarded arguments |
| `cargo xtask evidence --chip CHIP [SCENARIO...]` | Rewrite the vendor evidence shards whose recorded sources changed, or the named scenarios' shards; builds the probes and runs the scenarios with the pinned artifacts, then prints each shard's changed claims (verdicts, cases, coverage, untriaged locations) apart from its changed source digests and lines. Each probe build, the Blobray and scenario builds, each stand and each scenario run print a `phase <name>: <seconds> s` line as they end, so a slow run shows where its time went. The verification owner runs it, including after a merge that conflicted in `verification/<chip>/evidence/scenarios` |
| `cargo xtask evidence --chip CHIP --check [--changed-since REV] [SCENARIO...]` | Rerun the named scenarios, or every one, and fail unless each committed shard equals its rerun, printing the changed claims of each that differs. `--changed-since` reruns only shards that record a file changed since `REV`, in the worktree or untracked, and names each skipped one; a shard that does not parse is always rerun |
| `cargo xtask vendor-firmware --chip CHIP [PROJECT]` | Build the tracked vendor firmware of `verification/<chip>/hil-vendor` against the ESP-IDF revision pinned in `artifacts.toml`, checked out once per host in `~/.cache/open-esp-radio/esp-idf/<revision>-<pins>` (`OER_IDF_CACHE` overrides the directory; every checkout shares it) with every IDF submodule whose repository is a pinned source at its pinned revision and verified artifacts; fails when the image links an archive named like a pinned artifact that differs from it. Installs the IDF tools once into the cache's `idf-tools`; preparation holds the cache lock exclusively and builds hold it shared. A checkout's former `target/vendor-firmware/esp-idf` and `idf-tools` are removed on its first build; writes `build.json` next to each image |
| `cargo xtask register-inventory --chip CHIP [--output DIR]` | Compare the vendor's statically resolved accesses inside the publication's owned MMIO ranges with the register model: words no register declares, masked bits outside every declared field, and opaque declared bits the vendor touches; words touched by a function the provenance registry names rank first. Blobray's `register-accesses` analyzes every pinned vendor binary in one process, cached in `target/register-inventory/<chip>/analysis` by the digests of the Blobray host and the inputs and by the ranges; writes `report.txt` and `report.json` beside it. Addresses computed at run time stay outside the inventory |
| `cargo xtask check blobray-standalone` | Extract generic Blobray source, check path-dependency containment, then build and test every Blobray crate |
| `cargo xtask check isa-conformance [--cc CLANG]` | Fetch the pinned RISC-V architectural tests and Sail model, then compare every signature of Blobray's RISC-V executor with Sail's on the same ELF |

## The push gate

`cargo xtask push` and `cargo xtask check changed` run the same gate, about
two minutes warm for a typical change. It selects through `oer-tidy`'s model
of the tree, without Cargo:

- a file inside a package selects that package; a virtual workspace's
  manifest selects every package of the workspace, and `rust-toolchain.toml`,
  `.cargo/config.toml`, `clippy.toml` and `rustfmt.toml` every package;
- a package's `open-radio.inputs` patterns name repository files outside it
  that its tests read (the qualification evaluator reads the catalogs and
  the HIL targets' manifests, the HIL evidence writer and runner are read by
  the evaluator, xtask reads the workflows), so a change there selects it;
- a `Cargo.lock` change selects the members whose resolved dependencies
  changed: those reaching, through the lock's dependency lists, an entry
  that is new or differs from the base;
- one `cargo metadata --no-deps` per workspace adds every package that
  depends on a selected one, through any dependency kind.

It then runs `check tidy` over the whole tree, `cargo fmt --check` of the
selected packages of every workspace a Rust file changed in, `lock --check` of every workspace whose
manifests or lock changed, `check docs` when Markdown or qualification files
changed and `check capabilities` when Rust or catalogs did, and Clippy with
`-D warnings` and the tests (with their declared feature sets, limited to 20
minutes) of the selected host packages: every package of the root workspace,
and host or portable packages of the others. Packages that build only for a
chip, the HIL images, API documentation and the PHY, network, register and
provenance audits are CI's after the push, or `check changed --full`
locally. Both commands print any CI workflow whose newest run on `main`
failed, or a one-line warning when `gh` cannot tell.

## Output

The checks, `doc`, `lock` and `push` print one line per step. The output of
the commands they run goes to a log under `target/xtask/logs/` (kept three
days); a failure prints that log's diagnostics, at most 60 lines, and its
path. `--verbose` streams everything instead.

## Configuration model

The root Cargo alias selects this package. `--root PATH` selects an explicit
repository checkout. A nested independent workspace does not acquire the root
workspace's package membership through `--manifest-path`.

Architecture and example checks resolve their Cargo jobs from the same typed
configuration model. Package metadata owns supported feature alternatives; the
model keeps workspace, package/target, host or MCU target, feature selection and
Cargo build profile separate. API documentation instead follows each package's
standard `[package.metadata.docs.rs]` table.

## Choosing checks

Use focused package tests and target builds for the code being changed. Run
`check docs` for prose/catalog changes and `cargo xtask doc` for API changes.
The CI jobs in `.github/workflows/ci.yml` are the full source checkpoint; they
are not required after every local edit. Shared contracts,
Cargo feature policy, generated PAC and firmware layout changes need the relevant
broader architecture, safety and artifact checks. Partial checks do not establish
full repository coverage.

## Documentation check scope

`check docs` covers tracked Markdown, owner documents below `docs/`, package
`README.md` files and Cargo `readme` targets; arbitrary untracked working notes
are not repository documentation. External URLs are counted as
`external-not-checked`; no network requests are made. Catalog checking and
rendering neither load runtime evidence nor evaluate readiness, and the command
performs no hardware operations.

## Image checks

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

## Network dependency checks

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

## Orchestration tests

Run the orchestration regressions with:

```console
cargo test -p oer-xtask
```

Tests exercise actual temporary Cargo graphs, ownership, argument boundaries,
negative inputs and child-process lifecycle. Cargo and Rust discover tests;
there are no source-spelling or regex checks for required Rust identifiers.
Builds retain normal Cargo parallelism. `OPEN_RADIO_ANALYSIS_BUILD_JOBS` is an
optional explicit local limit for vendor probe builds.

## Standalone firmware builds

Standalone firmware builds keep a Cargo cache per example and copy their ELFs
and images into a unique `target/firmware/esp32s31-<example>/build-<id>/`
bundle. Station and AP link the owned Xarxa/Embassy stack, their default
feature; see the [implementation guide](../../docs/network-implementations.md).

Firmware builds never modify a committed `Cargo.lock`. Each build copies the
workspace catalog into its own cache and resolves through Cargo's
`resolver.lockfile-path` (Cargo 1.97+), so a `[patch]` or local override
writes only that copy, which is archived as the build's effective lockfile.
Metadata checks read committed catalogs without waiting, and builds of
different examples or image classes run concurrently. An overlapping
build of the same output fails on that copy's lease; the artifact lease
separately protects the selected cache and output snapshot.
Successful bundles remain available for inspection, while failed partial bundles
are removed. Flashing uses the completed bundle after releasing the build lease,
so a later build cannot replace the selected image. The serial-device lease
independently protects the complete hardware write transaction.

## Blobray extraction and host boundaries

Standalone extraction copies only nonignored Blobray source, excluding private
inputs and build outputs. It owns its temporary target directory, preserves a
caller-selected Rust toolchain, and otherwise uses the repository's pinned
channel. Extracted Blobray has no dependency on this xtask package.

Blobray owns its analysis limits. Linux/OpenWrt fixture logic and privileged
installation remain owned by HIL; xtask does not install fixtures or change
network state. Linux process ownership uses explicit process groups.
Unsupported hosts return an error when the required ownership backend is
unavailable.
