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

`integrity.json` seals the run's complete file inventory. Replay validates the
bundle and selected application before touching hardware. These commands run
from the repository root and require a configured, attached HIL device:

```console
cargo hil image replay <run-id> <image-class>
cargo hil run <scenario-id> --firmware-from <run-id>
```

They consume the archived build without invoking Cargo. A scenario replay
produces a self-contained new bundle whose provenance names the source run,
sealed-integrity digest, build ID and firmware-source repository. Replay is
supported for one scenario; `run-all` does not accept an archived build input.
Replayed runs are not accepted as current-clean qualification evidence.

## Source reconstruction

Build provenance records repository and active local override identities.
Each source material includes its checkout, remote when available, commit,
dirty state, workspace digest and reconstruction status. Tracked changes may
be archived as a binary Git patch against the recorded base commit.

Untracked content is not copied automatically. Its paths, sizes and hashes
may be recorded, but unavailable source content makes reconstruction
incomplete. Active path overrides are independent build materials, not merely
absolute directory names. Source identities are captured before construction
and checked again before publication; an edit during the build invalidates
the record.

Clean commits and locked dependencies remain the qualification inputs.
Archiving a dirty patch is a development convenience and does not convert it
into clean evidence.

## Byte-identical rebuilds

Retaining exact firmware enables replay; it does not prove that rebuilding
the same source produces identical bytes. Use the separate verifier:

```console
cargo hil image verify-rebuild <image-class>
cargo hil image verify-rebuild <image-class> --trim-paths
```

The verifier requires a clean commit and rejects local path overrides. It
creates two detached worktrees with different absolute path lengths and
isolated target roots. It compares all four firmware subjects, the effective
embedded lockfile and full/allocated ELF section layouts. A mismatch produces
a typed report and a failing exit status under
`target/hil/esp32s31/reproducibility/`.

`--trim-paths` selects an explicit experimental Cargo path-treatment variant.
It is not part of ordinary image construction. Normal HIL execution builds
once; it does not run a second build or silently normalize compiler flags.

Run-bundle reproducibility is `unverified`. A passing verifier report is not
bound into build provenance by the current implementation, and the offline
reader rejects an unsupported `verified` claim. Neither a matching source
commit nor a stored ELF changes that rule.

## Build record and storage

`BuildProvenance` contains the build ID/type, image class, runtime profile,
target/features, source materials, lock/file identities, tool/environment
information and output subjects. `manifest.json` binds the application/build
used by a scenario. The build record and lab observations have different
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
    │   └── effective-Cargo.lock
    └── integrity.json
```

Subjects are ordinary files. A local content-addressed store permits hard-link
deduplication; copying is the fallback when linking is unavailable. Copying a
sealed run produces a self-contained bundle. Firmware binaries and generated
reports remain outside tracked source. Automatic CAS garbage collection is
not provided.

## Reproducing a hardware observation

Matching firmware does not recreate RF or host conditions. The runner records
secret-free lab provenance before building or flashing: cell/device identity,
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
