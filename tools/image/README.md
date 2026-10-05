# Image pipeline

`oer-image` is the one image pipeline of the repository: `build(ImageSpec)
-> ImageBundle`. Standalone examples (`cargo xtask build firmware`) and HIL
image classes (`oer-hil-image`, through `cargo hil image build`, the runner
and `cargo xtask check firmware`) build every image here; nothing else
compiles, gates or encodes one.

## Contract

An `ImageSpec` names the tree (`root`: a checkout or a source snapshot), the
chip, the application crate (workspace, package, binary, features), the
stack policy, what the interrupt-stack gate requires (`Required::Proven`, or
`Partial` for diagnostic images), the layout seed, local dependency
`Overrides` (`ESP_HAL_ROOT`, `EMBASSY_ROOT`, `OPEN_RADIO_XARXA_ROOT`), the
caller's builder packages and extra policy files, the bundle directory, the
compile cache and an optional audit of the runtime ELF (HIL's observer
placement).

The chip profile's boot kind selects the pipeline:

| Boot | Module | Bootloader and partition table |
| --- | --- | --- |
| `staged` (esp32s31) | `staged` | The ROM-readable DIO bootloader from the `espflash` library's resources, the partition table of the flash map's CSV, the `ota_0` selection |
| `esp-idf-bootloader` (esp32c5) | `esp_idf` | The catalog bootloader of `hil/bootloaders/<chip>` and its partition table, built against the pinned ESP-IDF (`esp_idf::catalog`, `esp_idf::idf`); its `flasher_args.json` must place them at the flash map's offsets |

An `ImageBundle` is one directory with a `bundle.json`:

| File | Content |
| --- | --- |
| `runtime.elf` | The application ELF (the staged boot's stage-two runtime) |
| `application.bin` | The encoded ESP application image (the staged boot's bootstrap with the packed runtime embedded) |
| `bootloader.bin`, `partitions.bin` | The second-stage bootloader and the binary partition table |
| `otadata.bin` | The OTA selection of the first slot (staged only) |
| `runtime.bin`, `bootstrap.elf` | The packed runtime and the bootstrap (staged only) |
| `runtime-Cargo.lock`, `bootstrap-Cargo.lock` | The effective lock files |
| `placement.txt`, `runtime-stack.txt`, `interrupt-stack.txt`, `bootstrap-stack.txt` | Gate reports |
| `source-inputs.json` | Every repository file the build read |
| `build.log` | Every step's standard error |

`ImageBundle::segments` is what a flash writes: bootloader, partition table
and application at the flash map's offsets, the OTA selection last. Board
support writes these files and encodes nothing. `bundle::around` makes the
bundle of an application encoded earlier (a run's archive, a vendor
ESP-IDF build), so replays and foreign applications are flashed the same
way.

## Flash map

Each chip's `platform/<chip>/chip.toml` `[flash]` table
([`oer-chip-profile`](../chip-profile/src/lib.rs)'s `FlashMap`) holds the
bootloader, partition-table, application and OTA-data offsets and the
partition table CSVs (`platform/esp32s31/partitions/`). The flash map is
host data: no target code reads it (the bootstrap finds its payload through
the flash MMU), and the esp32c5, whose partition table comes from its
ESP-IDF build, has no layout crate. The staged pipeline refuses a flash map
whose application or OTA-data offset is not the partition table's.

## Encoding

Every flash file is encoded at build time with the `espflash` library
(`encode`): the application in QIO at 80 MHz for 16 MB with 64-KiB MMU
pages, the bootloader separately in the ROM's DIO (`rom_bootloader` checks
the segment checksum and digest), partition tables with `esp-idf-part`. The
build needs no `espflash` executable. The application must fit its
partition; at 90 % the build warns.

## Gates

Its `stack` module is the stack gate of every image build, with one function
for the runtime ELF (`audit_runtime_stacks`) and one for the bootstrap ELF
(`audit_bootstrap_stack`). Each analyses its ELF once with `oer-riscv-stack`,
the pinned ROM and its summaries, and resolves indirect sites with the same
facts (waker vtables, IPC posts, function-pointer field types). The policy is
a target's `stack.toml` (schema 5): `platform/esp32s31/stack.toml` for the
examples, extended by `hil/targets/esp32s31/stack.toml`, which adds the second
core's stack only HIL images start. Each stack names the function that runs on
it from its top (its root, by symbol), its storage (a sized symbol, or a
linker script's bottom and top symbols) and `minimum_free_bytes`: the root's
worst-case call chain (`Analysis::bound_with`) must leave the storage that
many bytes. The runtime's roots are `runtime_main` on CPU0's task stack and
`runtime_cpu1_psram_main` on CPU1's, which the platform runtime's assembly
enters at each stack's top; the bootstrap's is riscv-rt's `_start_rust`. A
root the image lacks fails, as does a non-optional stack without storage (an
`optional` stack, CPU1's in a single-core image, is reported as not linked).
A task stack's bound may be `partial + ?`: an executor's task polls and trait
objects leave sites no fact resolves. It passes with a warning when the part
it proves fits, and the report lists every unresolved site with its reason
and function; runtime stack painting, with the same reserves (the build hands
them to the firmware as `OPEN_RADIO_*_STACK_MINIMUM_FREE_BYTES`), checks the
exercised call chains. The gate also fails an ELF without `.stack_sizes`
records and every function of `oer-riscv-stack`'s inventory without a record
that `stack-coverage.toml` does not review exactly once (assembly, vector data,
compiler runtime or a linker script's section label; a review supplies no
frame). Reports go to
`runtime-stack.txt` and `bootstrap-stack.txt`: coverage, then per stack its
bound (proven, conditional or partial), budget, headroom, deepest path and
unresolved sites.

Its `interrupt_stack` module is the interrupt half of the runtime's gate: it
bounds each hart's interrupt stack from
the runtime ELF, the pinned ROM ELF (`rom` of
`verification/esp32s31/artifacts.toml`, from the vendor store: `cargo xtask
vendor-fetch esp32s31 --artifact rom`; a missing or changed ROM is an error)
and its reviewed summaries, with the platform's interrupt contract
(`oer-esp32s31-platform-layout`'s `interrupts`). The analysis reports every
hart as a bound or `partial + ?` with its holes; the gate's policy,
`Required`, decides what passes: `Proven` (product images and examples) fails
unless every bound is known, `Partial` (diagnostic image classes, whose
observers the product does not carry) also passes a `partial + ?` hart with a
warning. Either way the bound, or the proven part, must fit the usable stack
with the contract's margin, and every table entry's slot symbol and the
handler it calls must lie in SRAM (the slot's own direct calls: a call site no
inlined function owns, by the DWARF). A hart with a bound is `proven`, or
`conditional` on the assumptions it names; the gate admits only the executor
invariant (`ADMITTED`), each assumption until a change proves it. Every copy
of a CLIC level or route writer must run where the contract's
`LEVEL_WRITERS` allows, and no handler may lower the running level. Over the
code every interrupt level reaches, a floating-point instruction fails (the
handlers run with the FPU off); calls into the panic machinery and functions
in cached memory (flash, PSRAM) are listed. It writes all of this, each
hart's levels and their critical paths to `interrupt-stack.txt`.

The image compiler flags have one owner, `image::configure` of
[`oer-toolchain`](../toolchain/README.md), which a stack policy parameterizes
through `StackPolicy::image_compiler`:
`-Z emit-stack-sizes`, `-Z move-size-limit` (the policy's `max_move_bytes`) with `-D large-assignments`,
`-Z share-generics=y` and the C/C++ `-fstack-size-section`. No
`.cargo/config.toml` of the repository carries Rust flags (Cargo reads those
from the directory it runs in, not from the manifest's, so image builds from
the repository root would never see them). It sets `RUSTC_BOOTSTRAP=1` on the
image's Cargo command alone, only for those `-Z` flags on the exact stable
toolchain `rust-toolchain.toml` pins. Images build without frame pointers;
`esp-backtrace` then reports a panic's message and location without a
frame-pointer walk.

The stage-two header, checksum and address map come from the
[platform layout](../../platform/esp32s31/layout/README.md), which application
build scripts also use to configure the linker.

The staged runtime's placement audit (`staged::placement`) checks the
PSRAM/SRAM placement contract of the
[platform layout](../../platform/esp32s31/layout/README.md) and that each
PSRAM trap and interrupt entry first swaps to its SRAM stack; the caller's
audit runs beside it.

A stack policy that names no stacks sets only the move limit: the image is
compiled with every image flag and no stack gate runs. The esp32c5's
(`hil/targets/esp32c5/stack.toml`, 8192 bytes: its agent's main task moves a
4800-byte console future once) is one:
the stack analysis needs the chip's ROM ELF, which its `artifacts.toml` does
not pin yet.

## Comparing images

`compare::compare_elf` compares two linked RISC-V images function by
function modulo placement, over the `oer-elf` symbols and the decoded
listings of `oer_riscv_lift::listing`: every address an instruction forms
becomes symbol+offset, legacy mangling hashes and LLVM clone numbers are
dropped, identical-code-folded names pair by body, and a `Review` applies
reviewed aliases and scheduling ties. `cargo xtask compare elf` calls it;
`compare images` builds both sides first (`oer_hil_image::compare_images`).

## Source inputs and exclusion

`source-inputs.json` lists the compiled sources (dep-info and build-script
inputs), the workspaces' manifests and locks, Cargo configuration, the
policies, partition tables and chip profile, and the builder's own sources:
the dependency closure of `oer-image`, `oer-image-linker` and the caller's
builder packages, as `oer-repo` resolves it, so an analyzer or linker crate
is an input from the day it is linked.

Build exclusion has one mechanism, `exclusion::Lease` (an `flock`ed file):
one build per compile cache (`<cache>/build.lock`, waiting), one build per
private lock copy (`BuildLock`, refusing), and the host's image build slots
(`exclusion::slot` under `host_build_root()/tokens`: half the cores, at 2 GB
each).

Run host regressions with `cargo test -p oer-image`.
