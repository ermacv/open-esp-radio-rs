# Blobray

`blobray` turns compiled vendor ELF files and static archives
into reviewable evidence for writing and verifying Rust replacements. It
discovers MMIO, builds linked semantic IR, records function and data-flow
relationships, maintains reviewed register/interface/function knowledge, and
compares vendor and Rust observable effects.

The normal interface is project-oriented. You do not need to learn the
low-level analysis commands before using a project.

## Start here

The standalone Blobray build is target-neutral. A product repository may
link one reviewed compiled-analysis descriptor in a thin host. In this repository,
`cargo blobray` selects the ESP32-S31 host under
`verification/vendor/targets/esp32s31/blobray-host`; invoking the generic
package directly installs no chip knowledge. Production comparison remains in
the generic engine and is configured by data, not target-owned verdict code.

For an existing project:

```console
cargo blobray project doctor \
  --project /path/to/vendor-project.toml

cargo blobray project files \
  --project /path/to/vendor-project.toml

cargo blobray project status \
  --project /path/to/vendor-project.toml
```

These three commands answer different questions:

- `doctor`: is the configuration valid, are local inputs usable, and are the
  reviewed workspaces internally consistent?
- `files`: which files are local, external, reviewed, generated, or missing?
- `status`: which analysis, review, verification, and publication phases have
  usable outputs and current policy results?

`status` is the fast everyday overview. It checks typed gates and compact
summaries, but deliberately does not deserialize every artifact-wide report or
regenerate publication output. Use `project doctor` for deep input/evidence
and reviewed-workspace validation and `project check` to reproduce every
generated result byte for byte before publishing or merging a replacement.

Blobray assurance is not the product readiness authority. The repository's
[verification and qualification contract](../../docs/VERIFICATION_AND_QUALIFICATION.md)
defines which results are supporting research and which exact production
traces may enter the qualification ledger.

Use `--details` only when you need the complete component or file inventory.
Use `--format json` for automation. Human results go to stdout; diagnostics,
tracing, and progress go to stderr.

Inspect the project-owned incremental cache without changing it:

```console
cargo blobray project cache stats \
  --project /path/to/vendor-project.toml
```

`cache stats` is a read-only inventory command. JSON reports an absent cache as
`present: false`; the command does not initialize, migrate, compact, prune, or
delete cache data. In particular, it is not a garbage-collection command.
Use `--details` for paths and the per-kind query breakdown; cache writes and
automatic compaction remain part of analysis execution.

The `cargo blobray` alias intentionally keeps Cargo output
visible. A cold invocation may first compile the optimized host binary or wait
for another Cargo process to release its build lock; those messages describe
preparation before the Blobray process and its own progress UI can start.
For repeated inspection with an already built binary, the resource-limited
launcher below skips Cargo entirely.

## New project workflow

1. Create a project and declare the physical MMIO ranges:

   ```console
   cargo blobray project init \
     --directory radio-project \
     --id radio \
     --source vendor \
     --mmio radio=0x60000000..0x60010000
   ```

2. Bind caller-owned binaries in the ignored local run spec:

   ```console
   cargo blobray project inputs init \
     --project radio-project/vendor-project.toml \
     --bind source-artifact:vendor=/opt/vendor/libvendor.a
   ```

3. Inspect the file map and follow its prerequisite-ordered `Next` actions.
   A fresh generic project uses these actions to create the reviewed code,
   interface, and function packs before whole-project analysis. Run `doctor`
   whenever you want a deep validity check of the inputs created so far:

   ```console
   cargo blobray project doctor --project radio-project/vendor-project.toml
   cargo blobray project files --project radio-project/vendor-project.toml
   ```

4. When `project files` reports `READY TO ANALYZE`, generate symbol, MMIO,
   interface, linked-IR, navigation, and review evidence:

   ```console
   tools/blobray/scripts/run-limited \
     project analyze --project radio-project/vendor-project.toml
   ```

5. Review discovered registers and fields instead of promoting generated facts
   directly to hardware truth:

   ```console
   cargo blobray registers review --project radio-project/vendor-project.toml
   cargo blobray registers validate --project radio-project/vendor-project.toml
   ```

6. Publish the reviewed SVD and restricted Rust register API, then verify the
   complete project:

   ```console
   cargo blobray project publish --project radio-project/vendor-project.toml
   cargo blobray project verify --project radio-project/vendor-project.toml
   cargo blobray project check --project radio-project/vendor-project.toml
   ```

MMIO discovery is therefore an early analysis step, but it is not itself the
register specification. Generated facts identify addresses, widths, masks,
functions, and evidence. A human-reviewed register model supplies names,
access policy, fields, and safe public operations. SVD/PAC publication happens
only after that review boundary.

## Investigating code

Use focused inspection when project status identifies a function, register, or
blocker:

```console
cargo blobray project research next \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo blobray inspect function libpp:wDev_AppendRxBlocks \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo blobray inspect function libpp:hal_mac_set_bssid \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --replacement --case station-bank-preserves-policy

cargo blobray project browse \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

`project research next` ranks the current blockers and unreviewed register or
interface facts by guaranteed, optimistic and co-blocked downstream impact;
use `--scope ID` to focus Wi-Fi, BLE or 802.15.4. `inspect` is the detailed
console view. Use `project research next --finding ID` after reanalysis to
distinguish a currently open finding from an ID that is merely not present;
neither state is a completion claim. Owned unreviewed registers expose a
`review-required` raw TOML draft through `inspect register`, while external or
already reviewed addresses never receive a publication draft. With
`--replacement`, a matching
concrete case shows one canonical ordered effect trace with separate vendor
and Rust instruction provenance; use `--case ID` to select a case and
`--details` to show all matching cases. `project browse` is the supplementary
read-only TUI. Both consume the same typed application reports.

Low-level engines are grouped under `advanced` and are intended for focused
backend development or debugging, for example:

```console
cargo blobray advanced mmio discover --help
cargo blobray advanced ir export --help
cargo blobray advanced execute compare --help
```

The generic repository boundary and add-on composition workflow are documented
in [architecture](docs/architecture.md).

## Resource safety

Artifact-wide analysis defaults to four workers. The ESP32-S31 combined image
uses about 450 MiB at that setting and analyzes roughly three times faster than
the single-worker path. Build the optimized binary once and use the hard-limit
wrapper for real vendor inputs:

```console
CARGO_BUILD_JOBS=2 cargo build --profile blobray \
  -p blobray-esp32s31 --bin blobray

tools/blobray/scripts/run-limited \
  project analyze --check --project /path/to/vendor-project.toml
```

The wrapper enforces a 1-GiB resident-memory limit and a 15-minute timeout.
Use `--jobs 1` explicitly when working under a tighter memory budget; raise the
worker count above four only after measuring the target project.
User-systemd mode also disables swap. When a usable user-systemd scope is not
available, a Linux watchdog measures the complete spawned process tree and
enforces the same aggregate RSS and time limits without imposing a misleading
virtual-address-space cap. Linked-IR bundles
remain internally sharded as JSONL for bounded streaming; the public console
format is human or JSON.

## Documentation

- [Project workflow and file ownership](docs/project-workflow.md)
- [Binary analysis, semantic IR and pseudo-Rust](docs/analysis-and-semantic-ir.md)
- [Verification policy and evidence](docs/verification.md)
- [Register discovery, SVD and PAC generation](docs/registers-and-pac.md)
- [Architecture and responsibility boundaries](docs/architecture.md)
- [Persistent formats and schemas](docs/formats.md)
- [Read-only TUI](docs/tui.md)

Shell completions and the complete command manual are generated from the same
`clap` grammar:

```console
cargo blobray tooling completions bash --output blobray.bash
cargo blobray tooling manpage --output blobray.1
```
