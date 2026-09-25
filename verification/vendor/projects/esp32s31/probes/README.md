# ESP32-S31 compiled verification probes

This isolated workspace builds retained Rust entry points for instruction- and
effect-level comparison with caller-owned vendor artifacts. The probes depend
on production driver crates, but no production crate or HIL firmware depends
on the probes.

Build the comparison ELF from the repository root:

```console
CARGO_TARGET_DIR="$PWD/target/verification/esp32s31-probes" \
cargo build --manifest-path verification/vendor/projects/esp32s31/probes/Cargo.toml \
  -p oer-verification-esp32s31-probes-elf \
  --target riscv32imafc-unknown-none-elf --release

CARGO_TARGET_DIR="$PWD/target/verification/esp32s31-register-probes" \
cargo build --manifest-path verification/vendor/projects/esp32s31/probes/Cargo.toml \
  -p oer-verification-esp32s31-register-probes-elf \
  --target riscv32imafc-unknown-none-elf --release

CARGO_TARGET_DIR="$PWD/target/verification/esp32s31-bluetooth-probes" \
cargo build --manifest-path verification/vendor/projects/esp32s31/probes/Cargo.toml \
  -p oer-verification-esp32s31-bluetooth-probes-elf \
  --target riscv32imafc-unknown-none-elf --release
```

`library/` owns stable retained wrappers and explicit C-layout projections.
`elf/` owns only the comparison image entry point and linker layout. Neither is
a board test, runtime adapter or public driver API. Bluetooth production probes
live in the separate `bluetooth-library/` and `bluetooth-elf/` pair so BLE
verification never depends on Wi-Fi/PHY probe wrappers.

## Declaration and validated build

Use the [shared probe compiler](../../../harness/README.md) to declare each
entry once with `oer_probe_macros::probe!`. It generates the C export, linker root
and embedded ABI catalog. Simple adapters use `=> expression;`; complex and naked
adapters retain explicit bodies. Symbol names and C projections are the
boundary the scenarios select.

The primary build command validates the resulting executable entries as well as
compiling the three images:

```console
cargo xtask build vendor-probes --chip esp32s31
```

Every ELF contains `.blobray.probes`. Build scripts derive roots from the same
Rust declarations used by the macro frontend; there is no retention list to edit
when adding an entry. The catalog is part of the captured production artifact.

The main library's `with_phy` helper keeps the Wi-Fi register and interrupt owners
alive while lending the PHY partition to an adapter. Full platform construction,
Bluetooth setup, async scheduling and assembly trampolines retain their explicit
ownership rules. No private Rust owner layout is inferred from an ABI parameter.

`ets_delay_us` and the post-delay barrier in the software-frequency adapter retain
their optimization semantics. The current ESP-HAL link resolves the delay name
to its ROM address; registration does not override that binding. The owned
software-frequency probe lets the Next acceptance scenario check an ordinary
call to the captured ROM veneer, its tail transfer to `ets_delay_us`, and the
two-microsecond modeled delay. These checks concern software boundaries, not
physical timing.

No entry adapter exists: Blobray enters every vendor root and production probe
directly. Explicit words fill the argument registers and the stack, and a root's
callee-saved registers start at zero, so evidence and effect contracts bind the
real functions. Bluetooth's
pure calculation and combined calculation/publication probes share one array
projection around shipping `calculate_bluetooth_tx_gain`; only publication acquires
a radio owner. Their full behavior is exercised by the [native gain matrix](../README.md#wi-fi-and-bluetooth-gain-arithmeticpublication).
