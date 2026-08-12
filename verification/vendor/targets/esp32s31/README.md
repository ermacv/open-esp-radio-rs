# ESP32-S31 vendor-analysis project

This directory is a complete Vendor Binary Workbench project for the
ESP32-S31 rev0 radio stack. The normal repository binary includes its compiled
platform harness. Build with `--no-default-features` only when developing the
platform-neutral backend.

`vendor-project.toml` is the only normal entry point. Do not pass the target,
memory map, SVD, interface pack, or function pack separately: the project
manifest composes them.

## First use

Start with the three read-only project views:

```console
cargo vendor-binary-workbench project doctor \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo vendor-binary-workbench project files \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo vendor-binary-workbench project status \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

- `doctor` validates configuration and local inputs;
- `files` explains ownership and identifies missing external artifacts;
- `status` reports analysis, review, verification, and publication readiness.

Use `--details` only for the expanded evidence inventory.

## Local artifacts

`local.toml` is ignored and machine-local. Create or validate it with
`project inputs init`; never add private artifact paths to the tracked project
manifest.

```console
cargo vendor-binary-workbench project inputs init \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --bind source-artifact:rom=/path/to/esp32s31_rev0_rom.elf \
  --bind source-inventory:archive=/path/to/libphy.a \
  --bind source-inventory:libpp=/path/to/libpp.a \
  --bind source-inventory:libnet80211=/path/to/libnet80211.a \
  --bind source-inventory:coex=/path/to/libcoexist.a \
  --bind rust-artifact=/path/to/rust-trace-probes.elf
```

The project also uses fully linked, source-scoped oracle ELF files for exact
call selection and concrete comparison. `project files` lists every required
role and resolved path. The local input schema is documented by
`local.example.toml`.

The checked helper crates are:

- `oracle-firmware/` — builds source-scoped vendor link units;
- `probes/` — builds Rust verification probes.

Their own READMEs own build details. The Workbench treats the resulting files
as caller-owned inputs; it does not emulate the linker or silently build them.

## Analyze safely

Cold artifact-wide analysis must use the optimized, resource-limited binary:

```console
CARGO_BUILD_JOBS=2 cargo build --profile workbench \
  -p open-radio-vendor-binary-workbench --bin vendor-binary-workbench

tools/vendor-binary-workbench/scripts/run-limited \
  project analyze \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --jobs 1
```

The wrapper applies a 1-GiB/no-swap limit and a 15-minute timeout. One worker
is the safe default; higher values are an explicit measured opt-in.

After analysis:

```console
cargo vendor-binary-workbench project status \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo vendor-binary-workbench project browse \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

Use `inspect function SOURCE:SYMBOL` for one lossless body and `inspect flow`
for a bounded target/effect query. The first reviewed asynchronous route is:

```console
tools/vendor-binary-workbench/scripts/run-limited \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec verification/vendor/targets/esp32s31/local.toml \
  inspect flow --event-route rx-success-to-pp-task
```

The checked replay now proves the concrete route from `pp_post(0x19)` through
the reviewed FIFO, counted latch, `ppTask` dequeue/indexed dispatch and the
`wdevProcessRxSucDataAll` boundary in one persistent execution session. The
report deliberately keeps `path-feasibility=false` for the complete IRQ route:
the executable replay starts at `pp_post`, so the higher IRQ-to-post prefix is
structural evidence rather than a concrete end-to-end replay. Low-level
engines live under `advanced` and are not a second required workflow.

## Registers and the closed PAC

The data flow is:

```text
declared MMIO regions
  → generated access facts
  → reviewed registers/device.toml + peripheral fragments
  → clean SVD + private pac-raw
  → reviewed registers/api.toml
  → restricted public PAC API
```

Generated facts never invent register names, W1C behavior, reset values, or
field semantics. Review and publication are separate operations:

```console
cargo vendor-binary-workbench registers review \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo vendor-binary-workbench registers validate \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo vendor-binary-workbench project publish \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

The HAL consumes named restricted bindings. Physical addresses and arbitrary
integer writes remain inside the generated private raw-PAC boundary.

## Verification and CI

```console
cargo vendor-binary-workbench project verify \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo vendor-binary-workbench project check \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

Verification suites are declared in `vendor-project.toml`; scenarios,
dispositions, and accepted baselines live in their corresponding TOML
directories. Those files, rather than prose command lists, are the source of
truth for current coverage. Candidate evidence is always separate from
accepted baselines.

For the generic workflow and schema rationale, see
[`tools/vendor-binary-workbench/docs/getting-started.md`](../../../../tools/vendor-binary-workbench/docs/getting-started.md).
