# ESP32-S31 vendor verification target

Commands in this target guide use the explicit
`cargo vendor-binary-workbench-esp32s31` alias so the compiled ESP32-S31
harness is present. The ordinary `vendor-binary-workbench` build is generic
and intentionally has an empty harness registry.

`vendor-project.toml` is the preferred project entry point. It composes the
existing target pack with `memory.toml`, whose MMIO regions are independent of
SVD register names.

## Normal workflow

Most users need four project operations:

1. `project inputs init` once per machine, to bind private artifacts;
2. `project analyze --jobs 2`, to refresh all reverse-engineering evidence;
3. `project verify`, to execute every configured vendor/Rust proof suite;
4. `project check` as the complete non-mutating CI gate.

The checked project deliberately omits private artifact paths. Initialize an
ignored sibling `local.toml` from authenticated local artifacts:

```console
cargo vendor-binary-workbench-esp32s31 project inputs init \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --bind source-artifact:rom=/path/to/esp32s31_rev0_rom.elf \
  --bind source-artifact:archive=/path/to/linked-libphy.elf \
  --bind source-inventory:archive=/path/to/libphy.a \
  --bind source-companion:rom=/path/to/linked-libphy.elf \
  --bind source-companion:archive=/path/to/esp32s31_rev0_rom.elf \
  --bind source-artifact:libpp=/path/to/linked-libpp.elf \
  --bind source-inventory:libpp=/path/to/libpp.a \
  --bind source-companion:libpp=/path/to/esp32s31_rev0_rom.elf \
  --bind source-artifact:libnet80211=/path/to/linked-libnet80211.elf \
  --bind source-inventory:libnet80211=/path/to/libnet80211.a \
  --bind source-companion:libnet80211=/path/to/esp32s31_rev0_rom.elf \
  --bind rust-artifact=/path/to/rust-trace-probes.elf
```

`project inputs init` checks the role names, required profile sources and
ELF/archive container types before writing. Use `--check` to verify the local
file or `--force` to replace it deliberately. `--target-spec` examples below
remain valid as direct, single-command invocations.

The public project configuration can be checked without proprietary inputs:

```console
cd verification/vendor/targets/esp32s31
cargo vendor-binary-workbench-esp32s31 project doctor

cargo vendor-binary-workbench-esp32s31 project status \
  --project vendor-project.toml
```

The missing run spec is a readiness warning rather than a configuration error.
The status report distinguishes parseable verification suites, missing suite
input roles and the last aggregate verification result. A configured suite is
not reported as executed merely because its TOML files parse.

Once sibling `local.toml` exists, the complete generated-evidence workflow is:

```console
cargo vendor-binary-workbench-esp32s31 project analyze \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --jobs 2

cargo vendor-binary-workbench-esp32s31 project analyze --check \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo vendor-binary-workbench-esp32s31 project verify \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo vendor-binary-workbench-esp32s31 project verify --check \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo vendor-binary-workbench-esp32s31 project check \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --jobs 2
```

This generates or checks the complete symbol inventory, the cross-report
navigation index, MMIO/interface facts, all four linked-IR profiles, and the
register/function reviews, then validates the reviewed register, interface,
and function files. `project check` additionally reproduces all behavioral
suites and verifies the publication outputs without changing them.
`project verify` executes the twelve checked proof boundaries with their own
source selection, probe prefix, profiles, dispositions, baselines and gate,
then writes one `generated/reports/verification.json`. Use `--suite ID` for a
focused non-publishing run; partial selection never replaces the aggregate
report. `--candidate-evidence-dir /tmp/esp32s31-evidence` writes separate
review-only baseline candidates and refuses to overwrite accepted baselines.
The aggregate report includes a deduplicated replacement graph connecting
`(source, symbol)` to reviewed Rust components, probe symbols and every suite
proof. `--jobs 2` bounds both independent MMIO function analysis and
artifact-wide linked-IR workers. Omit it for safe automatic worker selection.
It deliberately does not update `svd/esp32s31-radio.svd` or production PAC
code. The public register release gate needs no private run spec:

```console
cargo vendor-binary-workbench-esp32s31 project publish \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --check
```

This strictly validates the model, API, lint and evidence packs, then verifies
the configured SVD, PAC and binding index as one preflighted publication.

The many leaf commands documented below are inspection and repair tools. They
are not additional required stages: use them when working on one model or when
`project analyze` identifies the exact failing component.

## Project files at a glance

| Kind | Examples | Ownership |
| --- | --- | --- |
| project composition | `vendor-project.toml`, `target.toml`, `platform.toml`, `memory.toml` | tracked, generic workflow and target facts |
| private inputs | `local.toml` | ignored, machine-local artifact paths |
| reviewed knowledge | `registers/`, `functions/reviewed.toml`, `interfaces/reviewed.toml`, `code/boundaries.toml` | tracked, edited and reviewed by a person |
| generated evidence | `generated/` | ignored locally, recreated by `project analyze` |

`vendor-project.toml` is the only normal entry point. The other tracked TOML
files are composed through it and should not be passed individually on the
command line.

## Register project

The checked `registers/device.toml` and its peripheral fragments are the
workbench's editable ESP32-S31 radio register model. The workbench loads this
model directly; generated XML is not required before MMIO discovery, IR
export, or verification. The separate
`../../../../svd/esp32s31-platform-radio-deps.svd` project input contributes
official platform registers used by the radio call graph without transferring
their runtime ownership to this project.

Inspect the model and generated review with:

```console
cargo vendor-binary-workbench-esp32s31 registers validate \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo vendor-binary-workbench-esp32s31 registers review \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

The clean SVD is written to `svd/esp32s31-radio.svd`. Discovery evidence
remains in the ignored `generated/findings/mmio.json`. `registers review`
writes the ignored `generated/reports/register-review.md`, joining addresses
to read/write functions, write masks and current model identities and emitting
copyable drafts for gaps. Users edit reviewed names, fields, access rules,
reset values and enumerations only in `registers/peripherals/*.toml`; the
generated report never feeds SVD or PAC generation.

The project treats `archive:phy_reg_check` as a reviewed diagnostic-only
reader. Registers seen only in that complete dump remain raw evidence under
the `non-operational-only` state and do not enter the public PAC. Any address
also touched by operational ROM or archive code still requires a reviewed
identity. MODEM_SYSCON, MODEM_LPCON and I2C_ANA_MST are separate platform-owned
memory regions and are likewise visible to discovery without being owned by
the radio SVD.

The checked project defines artifact-wide `rom-all` and `archive-all`
linked-IR profiles. Every named code symbol is a root; one unsupported
instruction reached by the conservative function CFG becomes a per-PC decode
blocker instead of discarding the whole function. Unsupported bytes after a
return or other path terminator do not pollute function review. Illegal
all-zero halfwords remain explicit
`zero-fill-or-illegal-trap` evidence because ROM functions use them both as
trap encodings and unreachable fill; they are not treated as false function
boundaries. Each primary input receives the other linked ELF as its reviewed
companion through `local.toml`, then register review merges both reports:

```console
cargo vendor-binary-workbench-esp32s31 ir build \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo vendor-binary-workbench-esp32s31 registers review \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

This adds poll and predicate field candidates plus links to guarded RTOS,
delay, NVS and logging operations. Those operation names remain navigation
evidence and are not promoted to SVD semantics. Use `ir build --check` to
verify both generated views, or `registers review --no-ir-reports` when only
the base MMIO-discovery report is wanted.

Private artifact paths remain in the local run spec. The generic profile
format and companion rules are documented in
[`project-ir-build.md`](../../../../tools/vendor-binary-workbench/docs/project-ir-build.md).

`registers/api.toml` owns the reviewed ESP32-S31 safe compound transactions,
ownership split and device-access helper. `project publish` is the normal
production gate. The PAC can still be checked directly when diagnosing that
single stage:

```console
cargo vendor-binary-workbench-esp32s31 registers generate-pac-raw \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --check
```

This leaf check verifies only raw-PAC freshness. Use `project publish --check`
for the production gate: it validates the reviewed publication scopes and
then checks SVD, raw PAC, generated closed-PAC domains and binding outputs
together. `--deny-unreviewed` on a leaf register command intentionally audits
every discovery observation, including findings outside those publication
scopes.

The pack is cross-validated against the clean schema-2 register model and
produces the checked-in PAC byte-for-byte. Use `--no-api-pack --output PATH`
when a plain architecture-neutral svd2rust output is useful for inspection.
Reviewed provenance, confidence vocabulary and coarse dump ranges now live in
the functional catalogs under `registers/evidence/`. Validation resolves every
source used by the model and API pack, and checks evidence ranges plus all
modeled registers against `memory.toml`. `registers/lints.toml` retains the
ESP32-S31 policy against synthetic `PRESERVED` fields without imposing that
naming rule on generic projects. The retired generator migration and current
publication ownership are recorded in
[`pac-gen-migration.md`](../../../../tools/vendor-binary-workbench/docs/history/pac-gen-migration.md).

The project also owns the neutral PAC address/path index. Diagnose that stage
independently of the production PAC with:

```console
cargo vendor-binary-workbench-esp32s31 registers generate-bindings \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --check --deny-unreviewed
```

This produces `svd/esp32s31-radio.bindings.toml` from the same schema-2 model and
records the Rust PAC crate name used by `driver generate`.

The project also configures the generic interface workspace. Generate facts
from a caller-owned run spec, initialize the reviewed pack once, and validate
it after edits or vendor updates:

```console
cargo vendor-binary-workbench-esp32s31 interfaces discover \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo vendor-binary-workbench-esp32s31 interfaces init-pack \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo vendor-binary-workbench-esp32s31 interfaces validate \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

Generated facts are ignored because they expose local paths and artifact
digests. The reviewed `interfaces/reviewed.toml` is intended to become a
shareable project asset after manual review. Reusable RTOS, NVS, logging, and
delay operations come from the tool's semantic catalog; the project pack owns
only ESP32-S31 anchors, layout versions, and slot ABI. Validation also retains
the generic discovery evidence for each concrete call site and recovered
argument expression; those facts do not make the reviewed semantic a runtime
execution claim.
The schema-v2 interface pack is a sparse overlay: generated unreviewed
anchors/slots remain in facts and validation output, while the TOML contains
only reviewed or ignored decisions. Vendor updates never rewrite it.

The first reviewed table is the ROM Wi-Fi OS adapter v9 reached through
`g_osi_funcs_p`: the pack records its 0x200-byte layout, version and magic
guards, 42 observed ABI slots, 12 documented manual slots and 18 explicit
execution-model links. The ABI vocabulary includes RV32 `i64`/`u64` returns,
so `_esp_timer_get_time` is represented without pretending that its return or
clock effects are executable. Interface validation currently resolves 176
concrete ROM call sites against this table.

Resolved interface contracts are also the structural ABI registry used by
project IR builds. A reviewed slot is rendered as a named opaque call even on
an alternative CFG path that was absent from discovery facts. Only an explicit
execution-model link permits modeled continuation; otherwise the call retains
`unmodeled-reviewed-external-call`. The real `rom-all` profile currently has
20 such named calls and no `unregistered-external-abi-slot`. The remaining
cleanup is to remove layout/version/magic and ABI duplication from the compiled
ESP32-S31 harness, leaving it responsible only for executable behavior.

The same project configures a reviewed function/context workspace over both IR
profiles. Generate IR, initialize the pack once, then edit names and roles in
`functions/reviewed.toml` and regenerate the reading view:

```console
cargo vendor-binary-workbench-esp32s31 functions init-pack \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo vendor-binary-workbench-esp32s31 functions validate \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo vendor-binary-workbench-esp32s31 functions review \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

The ignored `generated/reports/function-review.md` puts reviewed roles and
context field names beside detailed pseudo-code, exact validated interface
call sites, recovered call arguments, linked-IR CFG guards,
RTOS/NVS/logging/delay links, trampoline counts, and closure blockers. Omitted
facts stay in a compact inventory; their complete pseudo-code remains in the
per-profile reports and TUI. It is not source reconstruction and
does not feed the register SVD. Register names remain in `registers/`, and
external table ABI/semantics remain in `interfaces/` plus the reusable catalog.

This directory owns target-specific input for compiled vendor-to-Rust
verification. It is deliberately outside the generic verification engine.

- `target.toml` selects the generic RISC-V 32-bit backend, ILP32 calling
  convention and Rust recompilation target.
- `platform.toml` is the project-mode platform pack: it composes the
  ESP32-S31 radio harness with reusable RTOS/NVS/logging/delay vocabulary.
- `interfaces/reviewed.toml` alone binds concrete observed table slots to that
  vocabulary; the platform pack does not identify vendor layouts.
- `local.example.toml` documents caller-owned artifact roles; `project inputs
  init` creates and validates the untracked `local.toml`.
- `profiles/` contains concrete compiled-equivalence scenarios.
- `dispositions/` maps vendor inventory symbols to Rust components and
  executable contracts.
- `baselines/` contains expected evidence classifications.

No file here selects a proprietary artifact path or authenticates one. The
caller validates the desired vendor revision and passes absolute paths at run
time, either as command options, an explicit run spec, or the automatically
discovered untracked sibling `local.toml`. Private integration tests
recognize these explicit variables:

- `OPEN_ESP_RADIO_ESP32S31_ROM_ELF`
- `OPEN_ESP_RADIO_ESP32S31_LIBPHY_ARCHIVE`
- `OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE`

Executable call models and lifecycle entry contracts remain compiled in the
ESP32-S31 semantic harness. The reviewed interface pack is the only owner of
callback-table anchors, layout guards and slot ABI; compiled harnesses expose
behavior-only models joined by explicit model IDs. Typed executable contracts
live in the generic semantic crate and ESP32-S31 verification adapters live in
the target semantic harness. See
[`docs/VENDOR_BINARY_WORKBENCH_ARCHITECTURE.md`](../../../../docs/VENDOR_BINARY_WORKBENCH_ARCHITECTURE.md).

## COEX and BLE bring-up slice

The project has artifact-wide `coex-all` and `btbb-all` profiles for the
private `libcoexist.a` and `libbtbb.a` inputs. They are normal analysis inputs,
not platform semantics compiled into the generic backend. Refresh only this
slice while implementing coexistence with:

```console
cargo vendor-binary-workbench-esp32s31 ir build \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --profile coex-all --profile btbb-all --jobs 4
```

The 2026-08-10 real artifacts produce 134 COEX functions with 29 register
identities and 186 BT baseband functions with 111 register identities, with no
remaining instruction-decode blockers in either archive. The reviewed
`COEX_HW_TIMER` bank covers five timer instances at `0x2010_f400`, stride
`0x10`. The linked IR retains `arg0 <= 4` on configuration, secondary-target,
enable and disable accesses instead of converting the selector to one guessed
register.

Schema-v42 linked IR also exposes COEX static data directly. The current
archive contains 119 named data objects plus 42 initialized or zeroed section
objects retained under their only available compiler anchor. Important starting points include the
48-byte `coex_pti_tab`, the 20-byte `g_coex_param`, the 64-byte
`coex_schm_env`, and the family of 6/10/14/22-byte scheduler schemes. Exact
initializer bytes remain uninterpreted evidence. `.LANCHOR*` aliases join
recovered function reads and writes to the named object where ELF proves the
same member, section and offset.

Use the configured non-release review scopes `coex-core`, `coex-timer`,
`coex-scheduler` and `ble-advertising` to keep implementation work focused.
They deliberately do not gate the existing Wi-Fi publication scopes yet. The
checked COEX link view is built with:

```console
OPEN_RADIO_LINKED_ORACLE_SPEC="$PWD/verification/vendor/targets/esp32s31/oracle-firmware/trace-elf/linked-oracle-coex.toml" \
CARGO_TARGET_DIR="$PWD/target/verification/vendor-linked-coex" \
cargo build --manifest-path verification/vendor/targets/esp32s31/oracle-firmware/Cargo.toml \
  -p open-esp-radio-vendor-oracle-esp32s31-trace-elf \
  --target riscv32imafc-unknown-none-elf --release
```

The linked ELF owns final addresses and calls; the archive remains the
authority for inventory, initializers and origin. `g_coa_funcs_p` has concrete
storage in the ELF, while runtime profiles remain the only owner of the table
instance contents. The standard `__udivdi3` dependency uses an exact long
division implementation instead of a dummy return-value stub.

The first production COEX gate covers all five hardware-timer entries for the
four instruction-exact leaves. It executes 20 cases and checks the ordered
fresh-read RMW traces for enable, disable, force and unforce:

```console
CARGO_TARGET_DIR="$PWD/target/verification/esp32s31-probes" \
cargo build --manifest-path verification/vendor/targets/esp32s31/probes/Cargo.toml \
  -p open-esp-radio-verification-esp32s31-probes-elf \
  --target riscv32imafc-unknown-none-elf --release

cargo vendor-binary-workbench-esp32s31 project verify \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec verification/vendor/targets/esp32s31/local.toml \
  --suite coex-timer
```

`open-esp-radio-esp32s31-coex` owns the reviewed priority/timer maps and the
executor-neutral request/release state machine. The separate
`open-esp-radio-esp32s31-coex-embassy` crate serializes commands through one
hardware owner. Scheduler policy, Bluetooth lifecycle and the remaining RTOS
callback behavior are not claimed by this first gate.

The reviewed `coex-adapter-v2` interface is taken from the matching open
ESP-IDF adapter header and guarded by the exact archive hash plus runtime
version and magic fields. The current archive resolves 39 call sites to 15
reusable operations: semaphore lifecycle/use, ISR detection, allocation,
monotonic time, timer lifecycle, chip detection, diagnostics and crystal
frequency. Recognition is intentionally not execution authority. Calls such
as `esp_timer_get_time`, semaphore creation and allocation remain completeness
blockers until the COEX profiles supply explicit behavior models and concrete
table-instance state. Diagnostics and version checks remain visibly isolated
behind non-executable link stubs and are not valid execution paths.

The current focused queues are:

| Scope | Roots / closure | Complete | MMIO | Adapter calls | Immediate gap |
| --- | ---: | ---: | ---: | ---: | --- |
| `coex-core` | 8 / 22 | 9 | 21 | 11 | execution models, six direct unresolved calls |
| `coex-timer` | 5 / 6 | 4 | 21 | 2 | 64-bit division/link composition and timer models |
| `coex-scheduler` | 4 / 6 | 0 | 0 | 2 | timer/semaphore behavior and reviewed state layout |
| `ble-advertising` | 4 / 27 | 22 | 65 | 0 | one linked PHY dependency and four root replacements |

These are analysis/implementation queues, not pass percentages. A private
helper may remain incomplete while a reviewed root is eventually qualified as
one Rust composition; closure blockers and root replacement coverage therefore
remain separate dimensions.

## libpp interrupt pilot

The first Wi-Fi vertical slice verifies two generated PAC leaves and their
composition in the production MAC IRQ path. Build the caller-owned linked view
and the Rust probes first:

```console
OPEN_RADIO_LINKED_ORACLE_SPEC="$PWD/verification/vendor/targets/esp32s31/oracle-firmware/trace-elf/linked-oracle-libpp.toml" \
cargo build --manifest-path verification/vendor/targets/esp32s31/oracle-firmware/Cargo.toml \
  -p open-esp-radio-vendor-oracle-esp32s31-trace-elf \
  --target riscv32imafc-unknown-none-elf --release

CARGO_TARGET_DIR="$PWD/target/verification/esp32s31-probes" \
cargo build --manifest-path verification/vendor/targets/esp32s31/probes/Cargo.toml \
  -p open-esp-radio-verification-esp32s31-probes-elf \
  --target riscv32imafc-unknown-none-elf --release
```

Then run the focused regression gate. The caller supplies and authenticates
all three vendor inputs; no artifact path or hash is embedded in the tool:

```console
cargo vendor-binary-workbench-esp32s31 verify inventory \
  --target-spec verification/vendor/targets/esp32s31/target.toml \
  --source-artifact "libpp=$ESP32S31_LIBPP_LINKED_ELF" \
  --source-inventory "libpp=$OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE" \
  --source-companion "libpp=$OPEN_ESP_RADIO_ESP32S31_ROM_ELF" \
  --rust-artifact "$ESP32S31_RUST_TRACE_PROBES_ELF" \
  --rust-prefix open_libpp_trace_ \
  --dispositions verification/vendor/targets/esp32s31/dispositions/libpp-interrupt.toml \
  --gate regression --match-floor 3 \
  --evidence-baseline verification/vendor/targets/esp32s31/baselines/libpp-interrupt.toml
```

The dedicated Rust prefix is part of the focused gate boundary. The same ELF
also contains PHY probes, but they are neither candidates nor orphans in this
libpp run. The expected result is three exact effect-contract matches, no
mismatch/incomplete/orphan row, and a passing evidence baseline.

## WDEVPWR interrupt boundary

The power-interrupt gate verifies only the masked STATUS read and exact CLEAR
write. Production carries the acknowledged image into a separate Embassy
signal without decoding unverified cause bits. HIL keeps the complete
WDEVPWR enable mask at zero, so this boundary is ready for later power policy
but does not enable modem sleep.

```console
cargo vendor-binary-workbench-esp32s31 verify inventory \
  --target-spec verification/vendor/targets/esp32s31/target.toml \
  --source-artifact "libpp=$ESP32S31_LIBPP_LINKED_ELF" \
  --source-inventory "libpp=$OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE" \
  --source-companion "libpp=$OPEN_ESP_RADIO_ESP32S31_ROM_ELF" \
  --rust-artifact "$ESP32S31_RUST_TRACE_PROBES_ELF" \
  --rust-prefix open_libpp_power_irq_trace_ \
  --dispositions verification/vendor/targets/esp32s31/dispositions/libpp-power-interrupt.toml \
  --gate regression --match-floor 2 \
  --evidence-baseline verification/vendor/targets/esp32s31/baselines/libpp-power-interrupt.toml
```

The expected result is two exact-effect matches, no
mismatch/incomplete/orphan row, and a passing evidence baseline. Cause-bit
meaning, interrupt enable policy and the resulting RF/PHY/clock transition are
explicitly outside this gate.

## Connected modem wake counters

Ten safe typed PAC transactions cover the finite register sequence selected
by the connected vendor PM path: beacon-miss timeout/limit, both counter wake
gates, modem-state sleep limit, wake protection lead time, and optional TBTT
auto-period enable/disable/interval. `StaModemWakeConfig` bounds every field
before MMIO, and `RadioRegisters::configure_station_modem_wakeup` composes the
same operations in vendor order without importing vendor PM context.

```console
cargo vendor-binary-workbench-esp32s31 verify inventory \
  --target-spec verification/vendor/targets/esp32s31/target.toml \
  --source-artifact "libpp=$ESP32S31_LIBPP_LINKED_ELF" \
  --source-inventory "libpp=$OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE" \
  --source-companion "libpp=$OPEN_ESP_RADIO_ESP32S31_ROM_ELF" \
  --rust-artifact "$ESP32S31_RUST_TRACE_PROBES_ELF" \
  --rust-prefix open_libpp_power_trace_ \
  --dispositions verification/vendor/targets/esp32s31/dispositions/libpp-modem-wakeup.toml \
  --gate regression --match-floor 10 \
  --evidence-baseline verification/vendor/targets/esp32s31/baselines/libpp-modem-wakeup.toml
```

The expected result is ten exact-effect matches and a passing evidence
baseline. This is not a whole-function equivalence claim for vendor
`pm_sleep`: RF/PHY and clock gating, wake restoration, TIM/DTIM policy and
verified interrupt-cause decoding remain separate required slices.

The adjacent two-register station TSF wake transaction is verified
separately because it has a closed bool input domain and a non-symmetric
disable branch: both branches set bit 21 at `0x2010_d830`, while only bit 29 at
`0x2010_d858` follows the argument.

```console
cargo vendor-binary-workbench-esp32s31 verify inventory \
  --target-spec verification/vendor/targets/esp32s31/target.toml \
  --source-artifact "libpp=$ESP32S31_LIBPP_LINKED_ELF" \
  --source-inventory "libpp=$OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE" \
  --source-companion "libpp=$OPEN_ESP_RADIO_ESP32S31_ROM_ELF" \
  --rust-artifact "$ESP32S31_RUST_TRACE_PROBES_ELF" \
  --rust-prefix open_libpp_power_tsf_trace_ \
  --dispositions verification/vendor/targets/esp32s31/dispositions/libpp-sta-tsf-wakeup.toml \
  --profiles verification/vendor/targets/esp32s31/profiles/libpp-sta-tsf-wakeup.toml \
  --gate regression --match-floor 1 \
  --evidence-baseline verification/vendor/targets/esp32s31/baselines/libpp-sta-tsf-wakeup.toml
```

Both profile cases must have four ordered MMIO events and complete branch
coverage. The gate previously rejected an extra Rust `fence`; the production
method now retains exactly the vendor-observed transaction.

The planner also needs a live STA-TSF sample to reject a wake target that has
already passed while RX/control work was queued. The focused ROM gate closes
all four optional-output-pointer combinations of `hal_get_sta_tsf`; the
production `RadioRegisters::station_tsf` specializes the same safe PAC
register transaction to both output words.

```console
cargo vendor-binary-workbench-esp32s31 verify inventory \
  --target-spec verification/vendor/targets/esp32s31/target.toml \
  --source-artifact "rom=$OPEN_ESP_RADIO_ESP32S31_ROM_ELF" \
  --source-prefix rom=hal_get_sta_tsf \
  --rust-artifact "$ESP32S31_RUST_TRACE_PROBES_ELF" \
  --rust-prefix open_rom_power_tsf_trace_ \
  --dispositions verification/vendor/targets/esp32s31/dispositions/rom-sta-tsf-snapshot.toml \
  --profiles verification/vendor/targets/esp32s31/profiles/rom-sta-tsf-snapshot.toml \
  --gate regression --match-floor 1 \
  --evidence-baseline verification/vendor/targets/esp32s31/baselines/rom-sta-tsf-snapshot.toml
```

The expected result is four matching cases, complete coverage of both pointer
branches, `match=1`, and no mismatch, incomplete or orphan probe.

## Ordinary TX/DMA register slice

The next focused gate covers seven production operations: CCA publication,
trigger-flow sampling, finite enable/valid/invalid/disable queue access, and
the final TX queue doorbell. The four indexed profiles declare `arg-range 0 0
3`; all four logical queues must be executed, and the verifier proves the
reversed `CONTROL[3-queue]` mapping without treating the out-of-domain
assertion as an admissible vendor input.

`hal_mac_txq_enable` is intentionally not labeled whole-function equivalent.
The vendor root first performs the exact CONTROL read/write, then changes its
private queue context, has an HE trigger-based branch, and updates vendor
statistics. The checked adapter therefore verifies the register prefix,
requires `embassy-tx-queue-ownership`, records
`he-trigger-based-tx-disabled` as a current prerequisite, and allows only the
statistics suffix to be omitted as unused instrumentation.

```console
cargo vendor-binary-workbench-esp32s31 verify inventory \
  --target-spec verification/vendor/targets/esp32s31/target.toml \
  --source-artifact "libpp=$ESP32S31_LIBPP_LINKED_ELF" \
  --source-inventory "libpp=$OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE" \
  --source-companion "libpp=$OPEN_ESP_RADIO_ESP32S31_ROM_ELF" \
  --rust-artifact "$ESP32S31_RUST_TRACE_PROBES_ELF" \
  --rust-prefix open_libpp_tx_trace_ \
  --dispositions verification/vendor/targets/esp32s31/dispositions/libpp-tx-dma.toml \
  --profiles verification/vendor/targets/esp32s31/profiles/libpp-tx-dma.toml \
  --gate regression --match-floor 7 \
  --evidence-baseline verification/vendor/targets/esp32s31/baselines/libpp-tx-dma.toml
```

The expected focused result is `match=7`, `mismatch=0`, `incomplete=0`,
`orphan-rust-probe=0`, and a passing evidence baseline. The production
`start_prepared_mac_tx` calls the same verified safe PAC transaction between
its two device fences; vendor context layout and statistics are absent from
runtime code.

## RX descriptor-walker register slice

The RX gate covers eight finite leaves used by the production ring owner:
walker enable/disable, raw last/next reads, base publication, complete
last-pointer reconstruction, and reload-bit read/set. Safe typed PAC
transactions implement the same Effect Contracts exercised by the probes;
the handwritten `RxRingStopped`/`RxRingLive` types retain lifecycle and
descriptor memory ownership.

```console
cargo vendor-binary-workbench-esp32s31 verify inventory \
  --target-spec verification/vendor/targets/esp32s31/target.toml \
  --source-artifact "libpp=$ESP32S31_LIBPP_LINKED_ELF" \
  --source-inventory "libpp=$OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE" \
  --source-companion "libpp=$OPEN_ESP_RADIO_ESP32S31_ROM_ELF" \
  --rust-artifact "$ESP32S31_RUST_TRACE_PROBES_ELF" \
  --rust-prefix open_libpp_rx_trace_ \
  --dispositions verification/vendor/targets/esp32s31/dispositions/libpp-rx-dma.toml \
  --gate regression --match-floor 9 \
  --evidence-baseline verification/vendor/targets/esp32s31/baselines/libpp-rx-dma.toml
```

The expected result is eight exact PAC-leaf matches plus one compiled
composition match, with no mismatch/incomplete/orphan row. The
`wDev_AppendRxBlocks` adapter deliberately verifies an architectural
replacement rather than C-layout identity. It pins the vendor chain guard,
old-tail publication, leaf-call order and exact `0x186a1` reload bound, then
executes the production Rust descriptor/staging owner for immediate settle,
two one-microsecond Embassy edges, terminal-frontier base repair, and the full
100,001-sample timeout. Every scenario is repeated with two private-stack
padding fills and must retain identical MMIO, delay and return observables.
Vendor `wDevCtrl`, `g_osi` locking, linked-list diagnostics and optional
statistics are not imported into the runtime.

## Infrastructure-STA Authentication/Association slice

`ieee80211_sta_new_state` is deliberately verified as an architectural
replacement, not as whole-function or private-layout equivalence. The vendor
root combines ordinary station management with NVS/configuration reads,
`g_osi` timers and locks, diagnostics, power/coexistence, mesh branches and
private interface/node state. The open implementation instead owns typed
Authentication/Association protocol state, accepts station configuration from
its caller, and exposes the deadline to an Embassy executor.

```console
cargo vendor-binary-workbench-esp32s31 verify inventory \
  --target-spec verification/vendor/targets/esp32s31/target.toml \
  --source-artifact "libnet80211=$ESP32S31_LIBNET80211_LINKED_ELF" \
  --source-inventory "libnet80211=$OPEN_ESP_RADIO_ESP32S31_LIBNET80211_ARCHIVE" \
  --source-companion "libnet80211=$OPEN_ESP_RADIO_ESP32S31_ROM_ELF" \
  --rust-artifact "$ESP32S31_RUST_TRACE_PROBES_ELF" \
  --rust-prefix open_libnet80211_trace_ \
  --dispositions verification/vendor/targets/esp32s31/dispositions/libnet80211-sta-join.toml \
  --gate regression --match-floor 1 \
  --evidence-baseline verification/vendor/targets/esp32s31/baselines/libnet80211-sta-join.toml
```

`ESP32S31_LIBNET80211_LINKED_ELF` is a caller-built linked view of the same
authenticated archive, analogous to the libpp linked view above. The raw
archive remains the authoritative symbol inventory; an archive path is not a
substitute for the executable input.

The gate pins the vendor Authentication and Association management-send
branches, their timeout callbacks and the exact 1,000-ms state deadline. It
then executes the production `StaJoinRunner` with a finite PAC/DMA test adapter
and monotonic clock in four compiled scenarios: first-attempt Open
Authentication success, the Authentication attempt limit, Association
success, and Association retries through the exact deadline. Each scenario is
repeated with two private-stack fills and must produce the same result without
MMIO or blocking-delay effects. RX is serviced before timeout at an equal
deadline; successful Association transfers the still-live ring to the WPA2
phase instead of silently stopping and recreating DMA ownership.

The three-attempt Authentication limit and 160-ms Association retransmission
cadence are currently source-owned open-driver policies; the inspected vendor
root does not establish them. Only the 1,000-ms state deadline is claimed as a
vendor-anchored timing invariant. NVS, logging, RTOS synchronization, mesh and
power-state behavior are explicitly outside this verification boundary.
