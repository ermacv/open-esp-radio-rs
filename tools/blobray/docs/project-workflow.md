# Project workflow

The Blobray is project-oriented. A `vendor-project.toml` names reviewed
configuration and generated outputs; private binary paths belong in the
ignored local run specification, never in the reviewed manifest.

## The normal loop

For an existing project:

```console
cargo blobray project doctor --project path/to/vendor-project.toml
cargo blobray project files --project path/to/vendor-project.toml
tools/blobray/scripts/run-limited \
  project analyze --project path/to/vendor-project.toml --jobs 1
cargo blobray project status --project path/to/vendor-project.toml
```

- `doctor` validates configuration, local inputs and reviewed workspaces.
- `files` explains ownership and whether each input or generated output exists.
- `analyze` refreshes symbol, MMIO, interface, linked-IR and navigation facts.
- `status` reads the current results without regenerating the whole project.

Before publishing or merging a replacement, run:

```console
cargo blobray project publish --project path/to/vendor-project.toml
cargo blobray project verify --project path/to/vendor-project.toml
cargo blobray project check --project path/to/vendor-project.toml
```

`project check` is the fail-closed reproducibility gate. It verifies generated
outputs and the configured verification policy. It does not decide whether the
product is ready to ship; the repository qualification ledger is the sole
readiness authority. Unqualified implementations remain reported as coverage
debt in review-scope details but do not make `project status` incomplete and
are not silently promoted into mandatory verification claims. Only an explicit
verification-policy requirement makes a production trace a Blobray gate.

## Focused and full analysis

Focused inspection, a selected IR profile and `project analyze` use the same
recovery engine. Full project analysis enumerates all configured roots; it
does not switch to a second bulk algorithm. The current persistent cutover
reuses a complete immutable profile/stage projection, including explicit
incomplete/blocker results, and restores its generated files from CAS. Reuse
between only partially overlapping root sets requires the remaining
function-granular query cutover and is not reported as a cache hit today.
All stale IR profiles selected by one `project analyze` invocation enter the
builder together, so shared catalogs and reviewed interface knowledge are
loaded once. Cache-current profiles are not rebuilt.

The local persistent store lives below `generated/.blobray-cache/`. Removing
that directory only causes a cold recomputation. It never removes reviewed
knowledge or generated publication artifacts. Profile IDs and output paths
are bindings, so renaming an otherwise identical investigation does not change
its analysis query key. Changing artifact bytes, provenance, ABI/backend
revision or a semantic input read by the current stage creates a new key. Once
function-granular persistence is enabled, that invalidation narrows to the
affected query dependency closure.

For performance measurements, set `BLOBRAY_REPORT_USAGE=1` when
calling `scripts/run-limited`. This selects its process-session watchdog and
prints elapsed time plus peak RSS for the complete Blobray process tree.
External `/usr/bin/time` otherwise measures only the `systemd-run` wrapper on
hosts where the systemd limiter is available.

## Creating a project

```console
cargo blobray project init \
  --directory radio-project \
  --id radio \
  --source vendor \
  --mmio radio=0x60000000..0x60010000

cargo blobray project inputs init \
  --project radio-project/vendor-project.toml \
  --bind source-artifact:vendor=/opt/vendor/libvendor.a
```

Generated files are disposable analysis results. Reviewed packs, policies,
register models and dispositions are source inputs. `_oracles/`, vendor
binaries, disassembly dumps and credentials remain private.

## Investigating one function

```console
cargo blobray inspect function archive:phy_chip_set_chan \
  --project path/to/vendor-project.toml

cargo blobray inspect function archive:phy_chip_set_chan \
  --project path/to/vendor-project.toml --replacement
```

The ordinary view is for understanding vendor behavior. The replacement view
adds reviewed ownership, production binding, proof strength and verification
status. These are deliberately separate questions.

Use `project browse` for navigation and `advanced ...` only for backend
debugging or a focused low-level experiment.

## Resource limits

Real binaries must be analyzed through `scripts/run-limited`. The wrapper
limits aggregate memory and runtime; a large analysis should fail visibly
rather than make the development machine unusable. Build the optimized host
once when iterating repeatedly:

```console
CARGO_BUILD_JOBS=2 cargo build --profile blobray \
  -p blobray-esp32s31 --bin blobray
```
