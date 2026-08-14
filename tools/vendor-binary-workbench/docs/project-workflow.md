# Project workflow

The Workbench is project-oriented. A `vendor-project.toml` names reviewed
configuration and generated outputs; private binary paths belong in the
ignored local run specification, never in the reviewed manifest.

## The normal loop

For an existing project:

```console
cargo vendor-binary-workbench project doctor --project path/to/vendor-project.toml
cargo vendor-binary-workbench project files --project path/to/vendor-project.toml
tools/vendor-binary-workbench/scripts/run-limited \
  project analyze --project path/to/vendor-project.toml --jobs 1
cargo vendor-binary-workbench project status --project path/to/vendor-project.toml
```

- `doctor` validates configuration, local inputs and reviewed workspaces.
- `files` explains ownership and whether each input or generated output exists.
- `analyze` refreshes symbol, MMIO, interface, linked-IR and navigation facts.
- `status` reads the current results without regenerating the whole project.

Before publishing or merging a replacement, run:

```console
cargo vendor-binary-workbench project publish --project path/to/vendor-project.toml
cargo vendor-binary-workbench project verify --project path/to/vendor-project.toml
cargo vendor-binary-workbench project check --project path/to/vendor-project.toml
```

`project check` is the fail-closed reproducibility gate. It verifies generated
outputs and the configured verification policy. It does not decide whether the
product is ready to ship; the repository qualification ledger is the sole
readiness authority. Unqualified implementations remain reported as coverage
debt in review-scope details but do not make `project status` incomplete and
are not silently promoted into mandatory verification claims. Only an explicit
verification-policy requirement makes a production trace a Workbench gate.

## Creating a project

```console
cargo vendor-binary-workbench project init \
  --directory radio-project \
  --id radio \
  --source vendor \
  --mmio radio=0x60000000..0x60010000

cargo vendor-binary-workbench project inputs init \
  --project radio-project/vendor-project.toml \
  --bind source-artifact:vendor=/opt/vendor/libvendor.a
```

Generated files are disposable analysis results. Reviewed packs, policies,
register models and dispositions are source inputs. `_oracles/`, vendor
binaries, disassembly dumps and credentials remain private.

## Investigating one function

```console
cargo vendor-binary-workbench inspect function archive:phy_chip_set_chan \
  --project path/to/vendor-project.toml

cargo vendor-binary-workbench inspect function archive:phy_chip_set_chan \
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
CARGO_BUILD_JOBS=2 cargo build --profile workbench \
  -p open-radio-vendor-workbench-esp32s31-host --bin vendor-binary-workbench
```
