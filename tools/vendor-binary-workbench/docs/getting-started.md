# Getting started with the ESP32-S31 project

This is the canonical first-use path for the checked-in real project. It keeps
private vendor binaries outside Git and uses the project pipeline instead of a
sequence of low-level engine commands.

## 1. Bind local artifacts

Create `verification/vendor/targets/esp32s31/local.toml` from authenticated
caller-owned inputs. The exact required roles are visible with:

```console
cargo vendor-binary-workbench project files \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --details
```

Initialize or replace bindings explicitly:

```console
cargo vendor-binary-workbench project inputs init \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --bind source-inventory:libpp=/opt/esp/vendor/libpp.a \
  --bind source-artifact:libpp=/path/to/linked-libpp.elf
```

`local.toml` is local/private. Target, platform, memory, reviewed packs, and
accepted evidence remain project files with different ownership.

The raw `.a` is authoritative inventory and origin evidence. A fully linked
ELF is the authoritative runtime-address/call-selection view. `project files
--details` lists the corresponding inventory, artifact and companion roles for
every configured source; do not invent role names from library filenames.

## 2. Validate before analysis

```console
cargo vendor-binary-workbench project doctor \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

`BLOCKED` means configuration or a required input is invalid. `VALID` means the
project can run; warnings may still indicate outputs that have not been
generated. `--details` expands capabilities and every artifact binding.

## 3. Generate analysis evidence

Use the resource-limited optimized binary for the cold artifact-wide pass:

```console
tools/vendor-binary-workbench/scripts/run-limited \
  project analyze \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --jobs 1
```

This composes symbol inventory, executable boundaries, MMIO discovery,
interface discovery, linked IR, navigation, and review projections. Progress is
shown on stderr; the final result on stdout is intentionally short.

## 4. Read the result

```console
cargo vendor-binary-workbench project status \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

Read the output in this order:

1. overall state;
2. blocking problems;
3. exact `Next` commands;
4. phase summary;
5. optional `--details` component inventory.

Use `project browse` for interactive navigation or `inspect function` for a
lossless focused code/CFG/semantic report.

## 5. MMIO to reviewed Rust API

The project pipeline finds physical accesses inside declared MMIO regions. The
generated facts contain addresses, access widths, masks, call sites, functions,
and provenance. They deliberately do not invent register names or W1C/FIFO/reset
semantics.

Review those facts:

```console
cargo vendor-binary-workbench registers review \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo vendor-binary-workbench registers validate \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

Edit the reviewed TOML register model and API pack. Publication then derives a
clean SVD, an internal raw PAC, and the restricted public bindings:

```console
cargo vendor-binary-workbench project publish \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

HAL code consumes named restricted bindings. Raw addresses and unrestricted
integer register writes remain inside the generated private PAC boundary.

## 6. Verification and CI

```console
cargo vendor-binary-workbench project verify \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo vendor-binary-workbench project check \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

`project check` is the non-mutating CI entry point. It reproduces analysis,
verification, and publication outputs and rejects stale reviewed evidence.
