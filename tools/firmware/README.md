# Staged firmware tooling

`oer-esp32s31-firmware` is the host implementation of the ESP32-S31 image contract.
It packs runtime checksums, validates ELF placement and interrupt entry instructions, checks
compiler stack metadata, validates the ROM image checksum/digest, prepares the
OTA selector, and configures bootstrap/image/flash commands.
ELF inspection tools run under the shared process supervisor, with cancellation,
owned descendants and a two-minute deadline per invocation.

Its `interrupt_stack` module is the interrupt-stack gate every image build
runs before the task-stack audit: it bounds each hart's interrupt stack from
the runtime ELF, the pinned ROM ELF (`rom` of
`verification/esp32s31/artifacts.toml`, from the vendor store: `cargo xtask
vendor-fetch esp32s31 --artifact rom`; a missing or changed ROM is an error)
and its reviewed summaries, with the platform's interrupt contract
(`oer-esp32s31-platform-layout`'s `interrupts`), and fails unless every bound
is known and, with the contract's margin, fits the usable stack, and unless
every table entry's slot symbol and the handler it calls lie in SRAM (the
slot's own direct calls: a call site no inlined function owns, by the DWARF).
It writes each hart's levels and their critical paths to
`interrupt-stack.txt`.

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
`-Z emit-stack-sizes`, `-Z move-size-limit` with `-D large-assignments`,
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
