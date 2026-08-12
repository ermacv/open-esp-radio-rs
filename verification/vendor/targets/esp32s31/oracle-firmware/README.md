# ESP32-S31 vendor PHY oracle

This is an intentionally isolated hardware oracle. It links Espressif's PHY
implementation, performs vendor cold PHY initialization, and then hands the
radio peripheral to the open MAC/channel code for differential observation.

It is a separate Cargo workspace on purpose:

- the root driver workspace and `hil/targets/esp32s31` remain source-only;
- normal builds, tests, metadata, and publication do not resolve vendor PHY
  packages;
- the vendor and fully open drivers are never linked into the same firmware
  as competing MAC owners;
- normal `cargo hil` images never include this vendor workspace.

The checked-in source records each wrapped blob/ROM boundary. Binary evidence
stays in the ignored repository-local `_oracles/` directory. The caller owns
artifact acquisition, revision selection and authentication before building
this isolated workspace; the HIL runner does not contain a digest allow-list.

The current host runner deliberately has no `cargo hil oracle` compatibility
command. Historical qualification records mentioning it predate the isolated
project workflow and are not current instructions. The runtime can be checked
without touching hardware with:

```console
cargo check \
  --manifest-path verification/vendor/targets/esp32s31/oracle-firmware/Cargo.toml \
  -p open-esp-radio-vendor-oracle-hil-esp32s31 \
  --target riscv32imafc-unknown-none-elf --release
```

Building and flashing the vendor runtime is an explicit hardware-oracle
operation and is not part of `project analyze`, `project verify`, or the normal
source-only HIL image classes.

## Build the Workbench analysis inputs

From the repository root, build every linked analysis ELF and the Rust
comparison probe with the target-owned, sequential helper:

```console
verification/vendor/targets/esp32s31/build-analysis-inputs
```

The helper does not acquire vendor binaries and does not modify `local.toml`.
It requires the authenticated archives and ROM in `_oracles/`, then writes
only Cargo outputs below `target/verification/` at the paths used by the local
run spec. Set `OPEN_RADIO_ANALYSIS_BUILD_JOBS` explicitly to opt into more
than one Cargo build job.
