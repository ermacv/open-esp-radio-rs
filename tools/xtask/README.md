# Repository checks

`oer-xtask` is the repository's entry command line (host layer `entry`): the
binary parses arguments and calls the owner of each job. Its library holds
the repository operations — the [check registry](#the-check-registry) and the
gate that selects from it, CI input planning and coverage, push, CI state, locks, worktrees and the
repository checks. Domain work lives with its owners and their command
lines: images in [`oer-image`](../image/pipeline/README.md) and
[`oer-hil-image`](../../hil/host/README.md), vendor probes, scenario runs and
shard regeneration in [`oer-vendor-evidence`](../../verification/evidence/README.md)
(`cargo verification`), the register inventory in the
[register tool](../registers/README.md), the Blobray workspace helper in
[`oer-toolchain`](../toolchain/README.md). These
commands do not supply driver behavior, hardware scenario verdicts or
product readiness. The stand's and HIL's commands are not xtask's: `cargo
stand` is `oer-stand`, `cargo hil` is [`oer-hil-cli`](../../hil/host/cli/README.md).

Run from the repository root:

```console
cargo tidy check
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
| `cargo xtask check changed [--base REV] [--full]` | The push gate over what this checkout changed against the merge base with `REV` (default `origin/main`), committed, uncommitted or untracked: every fast check of [the registry](#the-check-registry) the change selects ([selection](#the-push-gate)). `--full` adds the full-tier checks it selects, as CI runs them on every push: among them the tests of every dependent package, `check docs`, root-workspace Clippy, API documentation of the affected root packages, the HIL image classes whose last build read a changed file, the examples, the PHY, network, architecture, register, qualification and provenance checks for their inputs |
| `cargo xtask check tier TIER --job JOB` | Run every check of the registry that CI job `JOB` has up to `TIER` (`fast`, `full` or `nightly`) over the whole tree, as that job does; a failing check does not stop the others. Each CI job is one such call |
| `cargo xtask check tier --list` | Print the registry: each check's id, tier, CI job and what it checks; a check marked whole-tier-only never runs for a change |
| `cargo xtask ci-status` | Print each workflow whose newest finished run on `main` failed, naming its failed jobs, or that `main` is green; the session start hook runs it |
| `cargo xtask hooks [--check]` | Write `.claude/hooks/heavy-commands.json`, the commands the Claude Code pre-tool hook keeps out of the foreground, from `oer_xtask::hooks`; `--check` only fails when it is stale (a test checks it too) |
| `cargo xtask push [--draft]` | Push the current branch after the registry's fast checks passed on exactly `HEAD`: refuses `main`, a detached `HEAD`, uncommitted changes to tracked files and untracked `.rs`, `Cargo.toml` or `Cargo.lock` files, gates the files `HEAD` changed against its merge base with `origin/main`, pushes the branch, opens its pull request when it has none and enables auto-merge (rebase), so GitHub merges it once the required `ci-ok` check passes. `--draft` opens a draft and enables nothing. A branch rebased since its last push replaces its remote copy with `--force-with-lease` bound to the copy the push saw, so commits pushed there meanwhile refuse it. It never waits for CI and never rebases |

## Locks, caches and worktrees

Cargo runs offline here (`.cargo/config.toml`); these commands keep lock files, the Cargo cache and build caches in step.

| Command | Contract |
| --- | --- |
| `cargo xtask lock [--check]` | Update every workspace's `Cargo.lock` to its manifests after a dependency or pin change (online); `--check` only verifies, offline, that every lock matches its manifests and names every stale one. `cargo tidy fetch` downloads what the locks name and the cache lacks: its independent host-only workspace needs no firmware Git sources to start. If tidy's registry dependencies are missing, use `cargo --config net.offline=false tidy fetch` to bootstrap it |
| `cargo xtask worktree add PATH --branch B [--from REV]` / `remove PATH` / `prepare` | Create a Git worktree whose `target/` starts from this checkout's build outputs (without incremental data, HIL outputs and vendor firmware builds, whose CMake caches name the source checkout), so only the workspace's own crates rebuild; `remove` deletes it with its `target/`. When `target/` is a btrfs subvolume the seed is an instant snapshot, otherwise a reflink copy taking minutes; `prepare` turns this checkout's `target/` into a subvolume once (run it while no build uses `target/`) |

## Source and architecture checks

Host-side policy over the source tree and the Cargo graph.

| Command | Contract |
| --- | --- |
| `cargo xtask check docs` | Check owned Markdown local links, repository paths and `cargo <alias>` commands in inline code, and check/render the static qualification catalogs and programs; API documentation is `cargo xtask doc` |
| `cargo xtask check capabilities [--changed FILE ...]` | Check the `// CAPABILITY: <id>` code anchors against every qualification catalog ([rules](../../qualification/README.md#code-anchors)) and list the entries anchored in the changed files |
| `cargo xtask doc` | Build API documentation as docs.rs would: one `cargo doc --no-deps` per `[package.metadata.docs.rs]` target with `RUSTDOCFLAGS=-D warnings`, then `cargo test --doc --workspace` |
| `cargo xtask check metadata` | Locked metadata for every actual Cargo workspace island, including unstaged source moves; every island applies the root `[patch]` replacements and resolves each Git package to one commit; every island repeats the root `[workspace.lints]` and every package inherits it, except the standalone Blobray workspace and the generated raw PAC |
| `cargo xtask check architecture` | Run Clippy on minimum/default and supported feature profiles of every production package for its chip's Rust target (`platform/<chip>/chip.toml`), applying each crate's lint policy; reject Wi-Fi packages in Bluetooth facade profiles; check that contract, protocol, hardware, role and service packages reach no HAL or Embassy crate but `embassy-sync`, isolated facade consumers, public type identities and composition feature contracts; crate-root unsafe attributes match the reviewed audited list and direct PAC dependencies the reviewed consumer list; no Rust file writes a `link_section` literal naming an input section of a region the boot zeroes (the platform layout's list, `platform/espressif/staged-layout/src/zeroed.rs`), and every package that builds esp-hal for `esp32s31` also enables its `static-interrupts`; and the register publication's checks (`oer_register_tool::checks`): handwritten PAC operations are single transactions, and every MMIO word both the radio PAC and pinned esp-hal write has a reviewed entry in `registers/<chip>/shared-words.toml`. Classification, package names and the layer, platform and role rules of every dependency are [`cargo tidy check`](../tidy/README.md) rules |
| `cargo xtask check network` | Resolve isolated network consumers and audit their dependency boundaries; CI compiles the profiles |
| `cargo xtask check feature-sets` | Test every root-workspace package with each feature set its `open-radio.test-feature-sets` declares; `check changed` does the same for the packages it tests |

## Firmware builds and image checks

The PHY library's target build. Images are not xtask's: `cargo fw build`
([`oer-fw`](../fw/src/main.rs)) builds the examples, `cargo hil images check`
([`oer-hil-cli`](../../hil/host/cli/README.md)) builds or type-checks the HIL
image classes, and both go through [`oer-image`](../image/pipeline/README.md); the
gate runs them as processes.

[`tools/phy-archive`](../phy-archive/src/lib.rs) holds verification's
compiled-symbol audit (`oer-check-phy-archive`). The gate runs it as a
separate process so the xtask entry package does not link the ELF reader;
the PHY check passes the compiled archive and its allowed source packages
to it.

| Command | Contract |
| --- | --- |
| `cargo xtask check phy --chip CHIP` | Build the PHY library for the chip target and audit its artifact and dependency graph |

## Other applications

The gate links only the repository model and the foundation; every other
application runs as a process: `cargo verification` (pins, provenance,
scenarios, evidence, probes: [vendor verification](../../verification/README.md)),
`cargo registers` (the [register tool](../registers/README.md), the inventory
included), `cargo hil` (runs, images, sweeps of HIL build outputs),
`cargo stand` (the shared stand, its installation) and `cargo fw` (the dev kit).

| Command | Contract |
| --- | --- |
| `cargo xtask check blobray-standalone` | Extract generic Blobray source with the `tools/riscv` crates, `tools/elf` and the host tool lookup (`tools/toolchain`, `tools/process`) it takes by path, check path-dependency containment, then build and test every extracted crate |
| `cargo xtask check isa-conformance [--cc CLANG]` | Fetch the pinned RISC-V architectural tests and Sail model, then compare every signature of Blobray's RISC-V executor with Sail's on the same ELF |

## The check registry

One registry lists every repository check (`oer_xtask::registry::CHECKS`;
`cargo xtask check tier --list` prints it): its id, its tier, the CI job that
runs it, the changes that select it and its runner. A tier says when a check
runs:

- `fast`: the push gate — `cargo xtask push` and `check changed` run every
  fast check a change selects, within a minute warm for a typical change;
- `full`: CI on every push, and `check changed --full`;
- `nightly`: once a night on `main`.

Every runner works over a change's reach or over the whole tree. Each CI job
(`.github/workflows/ci.yml`, `docs.yml`, `nightly.yml`) prepares its runner
(caches, a toolchain component, clang, GNU ld) and runs `cargo xtask check
tier TIER --job JOB`; a test holds the workflows to the registry, so no
workflow lists checks of its own. A check without a trigger runs only with
its whole tier: the final image builds, every image class, the examples'
builds, the conformance check (it needs clang and the Sail model), Blobray's
whole workspace and its standalone extraction (both need GNU RISC-V ld 2.47),
the probes and the host stands. The session start hook
reads CI's state through `ci-status`, and the pre-tool hook reads its
heavy-command list from `cargo xtask hooks`.

## CI input reuse

`registry::workflows` declares each workflow's jobs and extra tool versions,
deriving check IDs from the check registry. Its validation requires every
full-tier check to belong to one workflow job. Within `oer_xtask::ci`,
`planning` computes job identities, `coverage` selects reuse and validates
results using explicit evidence and time, and `runner` coordinates the
GitHub and environment adapters. The coverage policy has no GitHub,
filesystem, environment or clock calls.

Every job's key covers the **complete Git tree**, its check IDs, runner OS
and architecture, compiler flags, actual rustc/Cargo versions and the
job's declared extra tools. ISA conformance also includes clang and lld.
The tree already includes workflow and checker policy, Cargo manifests,
locks and tracked dependency sources. No Cargo dependency analysis or
narrow source boundary participates in planning. Git commit IDs and
`ImageVersion` do not enter input keys: different commits with the same
tree and tool inputs can share successful checks across branches and
merges. Any tracked source or policy change invalidates every job's key.
Other image-provided programs, such as gcc, cmake and Python, are not
fingerprinted; reusable coverage remains limited to seven days.

Planning requires a clean checkout so the tree hash describes its source
files. Repository source inputs must be tracked; external sources are
materialized from pins in tracked configuration by their existing owners.
Ignored or outside local source mutations are outside this reuse contract.
Plan/proof schema changes invalidate prior evidence.

The repository variable `CI_REUSE_MODE` controls push runs:

| Value | Behavior |
| --- | --- |
| `observe` (default) | Execute every job; the workflow summary shows which jobs could reuse earlier successful coverage |
| `reuse` | Execute jobs without matching successful coverage |
| `full` | Execute every job without looking for previous results |

Manual runs always use `full`. Nightly retains its independent complete
checks. Review `observe` summaries and validate equal-tree pushes, GitHub
[reruns](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/re-run-workflows-and-jobs)
of all jobs, one job and failed jobs before enabling `reuse`. The gate
must reject failures, cancellations and unexpected skips, and reuse chains
must preserve the original verification date and source run ID. Setting
`CI_REUSE_MODE` back to `full` disables reuse immediately.

`prepare` reads `ci-proof` artifacts from the latest 30 successful completed
push runs of the same workflow in this repository. A proof must belong to
that run, attempt and commit. Every reused job must match its input key
and check list and have an original verification no older than seven
days. Reuse preserves that original date; chains cannot renew it.
Unavailable APIs, expired/deleted artifacts and invalid evidence cause
jobs to execute. Plans and proofs are retained for 14 days.

`ci check-environment` compares an executing job's actual tools with the
plan and reports qualification through its `reusable` job output. A
version mismatch, unavailable plan or failed qualification allows the job
to execute but excludes its result from future coverage. Runner image
updates alone do not change qualification. `ci-ok` and `docs-ok` always run `ci verify`:
every executed job must succeed, and every skipped job must have explicit
valid coverage. The final gate requires a valid plan even when a job
could not download it. Other successful, qualified jobs still enter the
proof.
No branch protection change is needed for the existing required `ci-ok`.

Reuse trusts successful push workflows from every branch of this
repository. Branch writers and their automation can change the workflow
that produces proofs and required checks; run/attempt/commit binding
assumes these producers are trusted. Keep `observe` or `full` if this
producer scope is unsuitable.

The `ci-tools` Cargo cache shared by preparation and final gates is keyed
by runner OS, the Rust toolchain file and root lock file. It keeps one key
across source-only commits; its OS prefix can restore an older cache on
a miss, and Cargo checks which outputs need rebuilding. Cache contents
never establish successful coverage. Blobray builds through its existing
owner when image checks execute.

Inspect input keys locally from a clean checkout, without executing
checks or accessing GitHub. Keep generated files outside the checkout:

```console
cargo xtask ci environment --workflow ci --output /tmp/ci-environment.json
cargo xtask ci plan --workflow ci --environment /tmp/ci-environment.json --output /tmp/ci-plan.json
cargo xtask ci environment --workflow docs --output /tmp/docs-environment.json
cargo xtask ci plan --workflow docs --environment /tmp/docs-environment.json --output /tmp/docs-plan.json
```

`ci environment` reads the workflow's declared programs: CI needs installed
clang/lld, while documentation does not. A missing declared version
rejects planning. GitHub-only commands (`prepare`, `verify`,
`check-environment`) use the runner's authenticated `gh` with
`contents: read` and `actions: read`; they never write through the API.
Runner output files belong in its temporary directory.

The design uses [Bazel's action-cache practice](https://bazel.build/remote/caching):
identify actions by inputs, commands and environment, and execute on a
miss. GitHub's [cache scope](https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching#restrictions-for-accessing-a-cache)
explains why [workflow artifacts](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/download-workflow-artifacts)
carry coverage between branches.
[skip-duplicate-actions](https://github.com/fkirc/skip-duplicate-actions)
demonstrates deduplicating runs after rebase/squash merges when resulting
files match. Conditional jobs inside an always-started workflow preserve
the required gate; [workflow path filters](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/trigger-a-workflow)
can leave required checks pending.

## The push gate

`cargo xtask push` and `cargo xtask check changed` select through the
repository model ([`oer-repo`](../repo/README.md)), without Cargo:

- a file inside a package selects that package; a virtual workspace's
  manifest selects every package of the workspace, and `rust-toolchain.toml`,
  `.cargo/config.toml`, `clippy.toml` and `rustfmt.toml` every package;
- a package's `open-radio.inputs` patterns name repository files outside it
  that its tests read (the qualification evaluator reads the catalogs and
  the HIL targets' manifests, the HIL evidence writer and runner are read by
  the evaluator, xtask reads the workflows and the hooks), so a change there
  selects it;
- a `Cargo.lock` change selects the members whose resolved dependencies
  changed: those reaching, through the lock's dependency lists, an entry
  that is new or differs from the base;
- the model's path-dependency graph adds every package of the same
  workspace that depends on a selected one, through any dependency kind.

The fast checks a selection triggers: `tidy` over the whole tree, `fmt` of
the selected packages of every workspace a Rust file changed in, `lock` of
every workspace when a package manifest changed (a firmware workspace locks
the production crates it builds through path dependencies) or of the
workspace whose own manifest or lock changed, `capabilities` when Rust or
catalogs did, `clippy -D warnings` of the selected host packages and every
package depending on them, so a changed interface fails where it is used,
and `test` with the declared `feature-sets` (limited to 20 minutes) of the
selected host packages. Host packages are every package of the root
workspace and host or portable packages of the others. When the change
reaches a package that builds only for a chip, or a chip or family package
whose chip-target `cfg` code the host skips, `images-type-check`
type-checks the two final HIL images (`performance`, `correctness`) and
every other image class whose runtime compiles such a package, by the
class's Cargo graph (`cargo tree` with its features for the chip target),
since the host checks never compile that code. The gate names the full-tier
checks the change selects and leaves them to CI. Both commands print the
state of CI on `main` (`ci-status`).

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
CI's full tier (`cargo xtask check tier --list`) is the full source
checkpoint; it is not required after every local edit. Shared contracts,
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

Every image is built by the one image pipeline,
[`oer-image`](../image/pipeline/README.md): `build firmware` with the examples' spec,
`check firmware` and `compare images` through `oer-hil-image`'s class specs.
A full `check firmware` of `performance` or `correctness` then builds the
Blobray audit host in `tools/blobray/target` and runs the final radio target
audit on the class's runtime ELF: a call into the vendor radio ROM fails the
class. Encoding uses the `espflash` library, so no build needs the `espflash`
executable. This gate does not run on hardware or measure runtime stack
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

The full `images` job also builds `boot-smoke` for every staged HIL target,
including C5, through the same coverage, placement and stack gates as a run.
Radio final images additionally receive Blobray's forbidden-ROM audit.
