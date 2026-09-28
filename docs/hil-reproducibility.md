# HIL artifact and build provenance

The HIL runner owns firmware construction and run-bundle publication. This
reference describes what its archived artifacts prove and how replay differs
from rebuilding. Qualification applies its own evidence policy; an archived
application is not automatically current qualification evidence.

## Artifact replay

A run archives four firmware subjects with sizes and SHA-256 digests:

| Subject | Purpose |
| --- | --- |
| `application.bin` | Exact application image written to the DUT |
| `runtime.elf` | Runtime symbols, sections and layout for analysis |
| `runtime.bin` | Packed stage-two payload |
| `bootstrap.elf` | First-stage image composition |

The bundle also archives `effective-Cargo.lock` for the runtime workspace and
`bootstrap-Cargo.lock` for the shared platform workspace. Both resolved graphs
are build materials and survive replay.

`integrity.json` seals the run's complete file inventory. Replay validates the
bundle and selected application before touching hardware. It runs from the
repository root and requires a configured, attached HIL device:

```console
cargo hil run <scenario-id> --firmware-from <run-id>
```

It consumes the archived build without invoking Cargo. A scenario replay
produces a self-contained new bundle whose provenance names the source run,
sealed-integrity digest, build ID and firmware-source repository. `run` replays
one image class for every scenario it names; `run-all` does not accept an
archived build input.
Replayed runs are not accepted as direct current-source qualification evidence.

## Source reconstruction

Normal HIL builds capture source inputs before construction, verify the snapshot
manifest and archive, then build in an isolated checkout. Firmware publication
checks that frozen inputs remain unchanged. The archived snapshot records each
source role, commit, dirty state, file content, size and executable mode; build
provenance binds these content identities with `source-snapshot` status.

Tracked files are captured automatically. Nonignored untracked files require
explicit inclusion before capture. Active path overrides are independent source
materials. Build execution never falls back to the live checkout. Older bundles
can contain clean-commit or tracked-patch provenance; incomplete source records
remain diagnostic inputs.

A verified snapshot matching the current sources in every file the observation
depends on ([which files](../qualification/evidence-reviews.md)) is direct
qualification evidence, subject to the scenario's remaining requirements.
Dirty state and commit identity do not override that content match.
Differences between observed and current inputs need an explicit
[property/build applicability review](../qualification/evidence-reviews.md).
Archiving source bytes establishes provenance, not a passing experiment.

Run-bundle reproducibility is `unverified`: retaining exact firmware enables
replay but does not prove that rebuilding the same source produces identical
bytes, and the offline reader rejects a `verified` claim. Neither a matching
source commit nor a stored ELF changes that rule.

## Build record and storage

`BuildProvenance` contains the build ID/type, image class, runtime profile,
target, selected network integration, effective runtime features, source materials, lock/file identities, tool/environment
information and output subjects. The image builder records the tool versions
when it builds (`image::Artifacts::environment`); archival never re-queries the
host, so a recorded version names the tool that produced the image. A tool
without a version leaves the build composition unestablished for reviews.
`manifest.json` binds the application/build used by a scenario. The build record and lab observations have different
owners and meanings.

```text
target/hil/esp32s31/
├── objects/sha256/<prefix>/<digest>
├── reproducibility/
└── runs/<run-id>/
    ├── manifest.json
    ├── lab-provenance.json
    ├── source/
    ├── firmware/<image>/
    │   ├── build-provenance.json
    │   ├── application.bin
    │   ├── runtime.elf
    │   ├── runtime.bin
    │   ├── bootstrap.elf
    │   ├── bootstrap-Cargo.lock
    │   └── effective-Cargo.lock
    └── integrity.json
```

`runs/` is a link to the run store all checkouts of this user share
([find and compare runs](../hil/host/runs.md#find-and-compare-runs)); the
object store and reproducibility reports stay with the checkout.

Subjects are ordinary files. A local content-addressed store permits hard-link
deduplication; copying is the fallback when linking is unavailable. Copying a
sealed run produces a self-contained bundle. Firmware binaries and generated
reports remain outside tracked source. Pruning deletes the objects no file
links to any more, in this checkout and every registered one (see
[find and compare runs](../hil/host/runs.md#find-and-compare-runs)); an object
copied rather than linked into a run is not needed by it.

## Reproducing a hardware observation

Matching firmware does not recreate RF or host conditions. The runner records
secret-free lab provenance after building its images and before flashing: cell/device identity,
host kernel/boot/interface state, routes and applicable managed OpenWrt radio
observations. SSIDs, passphrases and SSH endpoints are omitted.

Each station traffic repetition discovers its route after the DUT has an
address, rejects ambiguous topology, checks the socket source and verifies
the appropriate Ethernet or WLAN path. The resulting `host-route.json` is
sealed with workload evidence. This per-flow check is independent of the
earlier run-level route snapshot.

The offline reader validates schema, canonical paths, identity, timestamps and
fixture/observation geometry as well as integrity hashes. See the
[HIL runner](../hil/host/README.md) for operations and
[qualification](verification-and-qualification.md) for admissible evidence.
