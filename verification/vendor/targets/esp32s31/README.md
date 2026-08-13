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

For one changed analysis profile, do not run the project-wide pipeline. Build
only that profile from its declared project inputs:

```console
tools/vendor-binary-workbench/scripts/run-limited \
  advanced ir build --profile libpp-all --jobs 1 \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

A partial profile build updates only its linked-IR bundle and deliberately
does not rebuild aggregate review scopes from unrelated stale profiles.
Focused compiled evidence is similarly run with `project verify --suite ID`;
it updates that suite report while leaving the aggregate verification report
for a complete project run.

Current same-channel AP+STA register evidence can be inspected directly:

```console
cargo vendor-binary-workbench project verify --suite wifi-interface-context \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo vendor-binary-workbench project feature wifi-ap-sta-interface-identity \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo vendor-binary-workbench project verify --suite wifi-sta-ap-receive \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec verification/vendor/targets/esp32s31/local.toml
```

The first suite proves the complete address/BSSID leaves for STA=0 and AP=1.
The second uses one cross-archive linked oracle to execute exactly the sparse
`wifi_set_rx_policy` cases 6 and 8 against the production HAL/PAC probe. It
also preserves both observed case-six submodes. The still-untyped
`g_ic + 0x74` word is deliberately named only as `Mode1`/`Mode2`; this evidence
does not identify either value as an AP+STA lifecycle state.

The reviewed logical object and its lifecycle consumers can be inspected
without scanning the full IR document:

```console
cargo vendor-binary-workbench inspect object \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  wifi-sta-lifecycle:g_ic --offset 0x74 \
  --flows-to wifi_set_rx_policy
```

The report proves reads, selector-bearing call routes, and two direct constant
zero writes: initialization in `ieee80211_ifattach` and clearing in
`ieee80211_mesh_quick_deinit`. No direct nonzero writer is currently observed.
Writers hidden behind argument aliases remain a blocker, so `Mode1`/`Mode2`
must not be renamed from function names or treated as an AP+STA lifecycle bit.

The low-level policy probe is intentionally isolated from the full STA/AP and
Embassy probe image. Rebuild it without pulling unfinished runtime adapters:

```console
cargo build \
  --manifest-path verification/vendor/targets/esp32s31/probes/register-elf/Cargo.toml \
  --target riscv32imafc-unknown-none-elf --release \
  --target-dir target/verification/esp32s31-register-probes
```

The current internal COEX core boundary is checked independently of the
unmodeled scheduler/runtime environment:

```console
cargo vendor-binary-workbench project verify --suite coex-core \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec verification/vendor/targets/esp32s31/local.toml

cargo vendor-binary-workbench project verify --suite coex-timer-control \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec verification/vendor/targets/esp32s31/local.toml

cargo vendor-binary-workbench project verify --suite coex-timer-set \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec verification/vendor/targets/esp32s31/local.toml
```

This proves PTI lookup, event-duration lookup and the complete release leaf.
All 100 valid-clock concrete request cases also match, but the request profile
remains incomplete because five Rust-only fail-closed clock error branches
are intentionally unreachable under the vendor valid-clock precondition. It
does not qualify the semaphore-backed scheduler or joint Wi-Fi/BLE hardware
behavior. Timer enable/disable/force/unforce are a separate 4/4 passing suite;
time conversion and value programming remain isolated in `coex-timer-set` as
one explicit incomplete boundary rather than making the qualified timer-control
surface appear incomplete.

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

The current AP/STA and COEX register frontier is intentionally split into
independent claims instead of one oversized "AP+STA ready" flag:

```console
cargo vendor-binary-workbench project feature wifi-ap-sta-interface-identity \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
cargo vendor-binary-workbench project feature wifi-ap-sta-receive-registers \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
cargo vendor-binary-workbench project feature wifi-ap-sta-key-role \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
cargo vendor-binary-workbench project feature wifi-mac-coex-register-programming \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

Their production path is `StaApRegisterHardware`/`WifiMacHal` and
`CoexTimerHardware`/`CoexTimerHal`. These APIs accept finite interface, timer,
PTI and policy types and expose no register addresses. This proves the named
register transactions only; shared-channel scheduling, beacon ownership and
runtime AP+STA arbitration remain separate lifecycle obligations.

Wi-Fi DMA descriptors are DMA-visible RAM, not PAC/MMIO registers. Raw
`word0 | BIT_30` expressions in host tests emulate hardware completion and do
not constitute a production register API; they should be migrated to a named
test fixture builder so descriptor ownership remains readable.

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
