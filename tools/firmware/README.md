# Staged firmware tooling

`oer-esp32s31-firmware` is the host implementation of the ESP32-S31 image contract.
It packs runtime checksums, validates ELF placement and interrupt entry instructions, bounds
every stack of an image, validates the ROM image checksum/digest, prepares the
OTA selector, and configures bootstrap/image/flash commands.
ELF inspection tools run under the shared process supervisor, with cancellation,
owned descendants and a two-minute deadline per invocation.

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

The default `device` feature provides serial-device selection, a lease shared
by xtask and HIL, and `write_segments`, which writes every flash segment
through one connection to the ROM stub and skips a segment whose flash contents
already match. The lease spans all writes and optional monitoring, uses USB
identity or the canonical serial path, and lives in the user's host cache so
separate checkouts cannot independently claim the same device. Without a port,
selection requires exactly one USB serial device; `cargo xtask build firmware
--flash` first picks the only attached board the HIL arbiter registers as the
chip.

Its `compiler` module is the single owner of the image compiler flags:
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

It does not execute scenarios, classify HIL images or configure network fixtures.
`cargo xtask build firmware` owns application build/flash orchestration. The
HIL runner uses the same mechanisms and adds observer checks, source snapshots,
replay and evidence. The [platform](../../platform/esp32s31/README.md) owns
embedded startup and the board profile.

Run host regressions with `cargo test -p oer-esp32s31-firmware`. Generated firmware and
reports remain in the invoking application's or HIL runner's ignored outputs.
