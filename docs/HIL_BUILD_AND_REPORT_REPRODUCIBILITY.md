# HIL build and report reproducibility

Status: architecture and phase-one implementation, 2026-09-01. This document
defines both the implemented evidence contract and the remaining gates. It does
not change qualification policy by itself.

## Problem statement

The current HIL bundle is strong evidence of what the runner observed, and it
already archives the exact `application.bin` written to the DUT. It does not,
however, provide all of the following guarantees:

- the measured runtime ELF is retained for later symbolization and layout
  comparison;
- a dirty source tree can be reconstructed instead of merely identified by a
  digest;
- local path overrides such as Xarxa, Embassy or esp-hal are identified as
  independent build inputs;
- a rebuild in another checkout produces byte-identical firmware;
- the host, OpenWrt and RF state can be recreated closely enough to reproduce
  a performance observation.

These are separate guarantees. Treating one SHA, one Git patch, or one stored
ELF as "reproducibility" would hide important failure modes.

An observed local example makes the distinction concrete: builds from the
same clean commit and toolchain in different absolute checkout directories
have produced different ELF, runtime binary and application digests. The
archived application remains sufficient for exact replay, but commit identity
alone is not yet sufficient for a byte-identical rebuild.

## Industry model

The proposed format follows the concepts used by SLSA/in-toto provenance:

- **subjects** are the outputs identified by cryptographic digest;
- **resolved dependencies/materials** are source repositories, lock files,
  toolchains and other inputs identified by URI or name and digest;
- a **build definition** records the build type and its externally visible
  parameters;
- **run details** identify the particular builder invocation.

SLSA explicitly recommends that version-controlled configuration be read from
the repository and that resolved dependencies carry their digests. It does
not require large source or output files to be embedded in the attestation.

Large immutable outputs belong in a content-addressed store. Bazel's remote
cache uses the same division between action metadata and a CAS of output
files. A HIL run may hard-link CAS objects into its immutable bundle: the run
remains a normal self-contained directory when copied, while repeated runs of
the same image do not consume another runtime ELF worth of disk space.

Relevant primary references:

- <https://slsa.dev/spec/v1.1/provenance>
- <https://bazel.build/remote/caching>
- <https://reproducible-builds.org/docs/definition/>
- <https://reproducible-builds.org/docs/perimeter/>
- <https://doc.rust-lang.org/cargo/commands/cargo-vendor.html>
- <https://doc.rust-lang.org/rustc/remap-source-paths.html>

The local format does not need to claim SLSA conformance. It should reuse the
model and vocabulary rather than invent an incompatible meaning for
"reproducible".

## Four explicit guarantees

### 1. Exact artifact replay

Exact replay means flashing and executing the bytes used by the original run.
It does not rebuild anything.

Every new run must retain these subjects:

- `application.bin`, the exact bytes written at the OTA application offset;
- `runtime.elf`, needed for symbols, sections, disassembly and layout analysis;
- `runtime.bin`, the packed stage-two payload;
- `bootstrap.elf`, needed to audit the first-stage image composition.

Each subject has a size and SHA-256 digest. `integrity.json` seals the files in
the run. A supported `image replay`/`run --firmware-from` operation must verify
the bundle before touching hardware and must never silently rebuild.

This is the primary mechanism for historical performance A/B. It is cheaper
and stronger than hoping a rebuild happens to reproduce an old layout.

### 2. Source reconstruction

Source reconstruction means recovering all source inputs used by the build.
The normal, preferred input is a clean immutable Git commit plus checked-in
lock files. Cargo locks Git dependencies to exact commits and registry
dependencies to exact versions/checksums; `--locked` must remain mandatory for
qualified builds.

Dirty local builds remain useful for development but are not qualification
evidence:

- tracked modifications may be stored as `git diff --binary HEAD` and tied to
  the base commit;
- untracked content must not be copied automatically because it may contain
  credentials or private `_oracles` inputs;
- a run with untracked build inputs is marked source-incomplete, while paths,
  sizes and digests may be recorded for diagnosis;
- each active local Xarxa, Embassy or esp-hal override is a separate material
  with its own commit, dirty state and optional tracked patch. Recording only
  its absolute path is insufficient.

A patch is therefore a developer convenience, not the primary source
identity. Important experiments should be promoted to clean commits and
pushed before they become baselines. A future explicit source-capsule command
may archive reviewed untracked inputs; ordinary HIL runs must not do so
implicitly.

### 3. Reproducible rebuild

The Reproducible Builds definition is byte identity given the same source,
build environment and build instructions. It is stronger than source
reconstruction.

The build record must include:

- repository and every external source material;
- both workspace and embedded-target lock-file digests;
- image class, target triple, release profile and exact feature set;
- effective codegen flags and build-relevant environment values;
- verbose Rust toolchain identity plus objcopy, linker and espflash versions;
- the versioned HIL build-recipe identity;
- all output subject digests.

The first deterministic-build experiment should run outside the default HIL
critical path and test, rather than assume, these controls:

- `CARGO_INCREMENTAL=0`;
- a source-derived `SOURCE_DATE_EPOCH` where tools consume timestamps;
- `--remap-path-prefix` for workspace, Cargo source and sysroot paths;
- stable locale/timezone and an allow-listed environment;
- two clean builds in different absolute directories followed by byte and
  section/layout comparison.

Rust documents path remapping as best effort and explicitly notes that linker
or external-tool paths may remain. It therefore cannot be declared sufficient
without the two-directory test. Remapping also changes the produced layout, so
it must be introduced as a controlled same-source A/B before becoming the
normal performance image recipe.

Rebuild verification should be opt-in or periodic because it requires a
second build. Normal HIL execution should build once, retain its subjects and
remain approximately as fast as today.

### 4. Performance experiment reproduction

Byte-identical firmware does not reproduce a throughput result by itself. A
performance run also needs a lab-state record:

- DUT chip/revision, clock and image capabilities;
- OpenWrt version, kernel, Wi-Fi driver/firmware and relevant configuration;
- channel, width, standard, GI/MCS/rate observations, country and TX power;
- associated stations, BA state and concurrent VIFs;
- laptop interface state and the actual route used by the traffic flow;
- traffic generator versions and exact arguments;
- fixture identity, topology and pre-run reset actions;
- RF scan/occupancy evidence where available.

This provenance belongs to the experiment/run, not the firmware build record.
It is the mechanism that prevents a FritzBox/WLAN route or changed OpenWrt
configuration from being confused with a code regression.

## Target storage layout

```text
target/hil/esp32s31/
├── objects/sha256/aa/<digest>       local content-addressed objects
└── runs/<run-id>/
    ├── manifest.json                experiment provenance
    ├── lab-provenance.json          secret-free pre-run cell observation
    ├── source/
    │   ├── repository.patch         optional tracked developer delta
    │   └── overrides/...            optional tracked override deltas
    ├── firmware/<image>/
    │   ├── build-provenance.json    build definition, materials, subjects
    │   ├── application.bin          CAS link/copy
    │   ├── runtime.elf              CAS link/copy
    │   ├── runtime.bin              CAS link/copy
    │   ├── bootstrap.elf            CAS link/copy
    │   └── effective-Cargo.lock     resolution used inside build transaction
    └── integrity.json               exact run-bundle inventory
```

The run contains ordinary regular files. Hard links are only a local storage
optimization; copying or archiving the run materializes a self-contained
bundle. If hard links are unavailable, the runner falls back to a regular
copy.

ELF and firmware binaries must not be committed to the source repository.
Git history should contain the build recipe, scenario, source and compact
accepted-evidence metadata. Run bundles belong in local CAS-backed storage or
an artifact service. A checked-in evidence pointer may name immutable object
digests without putting tens of megabytes into Git.

## Build provenance shape

Use a separate typed record rather than continuing to grow `manifest.json`:

```text
BuildProvenance
  build_id
  build_type / recipe_version
  parameters
    image_class
    runtime_profile
    target
    features
  materials[]
    kind
    uri/name
    commit/tree/digest
    dirty/reconstructable state
    optional patch subject
  environment
    tool versions
    effective flags
    normalized/non-normalized build-root identity
  subjects[]
    role
    path
    size
    sha256
```

`manifest.json` references the build ID and the application subject used by a
scenario. This cleanly separates one firmware construction from the later lab
execution.

## Implementation order and gates

1. Retain all four firmware subjects through a deduplicated CAS and extend
   offline verification to check every subject.
2. Add a separate build-provenance record with clean commit, lock-file,
   recipe, tool and output identities.
3. Capture tracked dirty patches and all local overrides as independent
   materials; fail closed to `source-incomplete` for untracked inputs.
4. Add verified replay of an archived application, then allow a scenario to
   explicitly consume an archived build without rebuilding.
5. Add lab-state snapshots and route assertions to performance runs.
6. Add an opt-in two-directory reproducible-build verifier. Only after it
   proves byte identity should normalized flags become the default recipe.
7. Add CAS inspection/garbage collection based on references from sealed run
   bundles.

Current implementation status:

- steps 1-3 are implemented for new runs; old schema-2 bundles remain valid;
- step 4 is implemented for one scenario through both
  `cargo hil image replay <run-id> <image-class>` and
  `cargo hil run <scenario-id> --firmware-from <run-id>`; the latter produces
  a new self-contained bundle and never invokes Cargo;
- replay provenance is distinct from the runner checkout: the new manifest
  binds the direct source run, its sealed-integrity digest, build ID and
  firmware-source repository. Replayed runs are never accepted as
  current-clean qualification evidence;
- step 5 now records a typed, secret-free `lab-provenance.json` before any
  firmware build or flash. It binds the sanitized cell topology, host kernel,
  boot and interface state, main IPv4 routes, and the managed OpenWrt release,
  kernel, boot, driver/firmware, country, TX power, channel geometry,
  associated-station count and concurrent VIFs to the run manifest;
- each station traffic repetition independently discovers the route after the
  DUT has an address, rejects ARP-flux topology, verifies the socket source and
  asserts Ethernet for the OpenWrt fixture or WLAN for the local-Linux
  fixture. The resulting typed `host-route.json` is sealed with the workload
  evidence. This per-flow observation is deliberately not inferred from the
  earlier run-level route table;
- SSIDs, passphrases and SSH endpoints are structurally absent from lab
  provenance. The offline reader validates the canonical path, schema,
  cell/device binding, timestamps and fixture/observation geometry even if an
  integrity index has been regenerated;
- all Git source/override identities are captured before the build and checked
  again before firmware provenance is published, so an ordinary edit during a
  build fails closed instead of creating a misleading record;
- `reproducibility` remains explicitly `unverified`; the reader rejects a
  `verified` claim until step 6 defines and retains its independent proof;
- multi-image `run-all` replay, deterministic two-directory rebuilds and CAS garbage
  collection remain pending and must not be inferred from exact artifact
  replay.

Acceptance criteria:

- a stored application can be selected and flashed without Cargo compilation;
- the archived ELF digest matches the ELF that produced the application;
- repeated runs of one build do not allocate another physical 74 MiB ELF on a
  hard-link-capable filesystem;
- dirty/untracked/external source state can never be reported as a clean,
  reconstructable build;
- old schema-2 bundles remain verifiable and rebuildable as history views;
- ordinary HIL runs perform one firmware build, not two;
- qualification continues to require a clean committed source state.
