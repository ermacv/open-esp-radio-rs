# tools/

Repository tooling, grouped by the contract each tool owns: [README](README.md).

| Path | Owns |
| --- | --- |
| `xtask/` | The repository commands a person runs by hand: `doc`, `lock`, `worktree`, `vendor fetch`, `push` (git push and a draft pull request) and the `ui` and `feature-sets` tests ([commands](xtask/README.md)); domain work is called from its owners; `cargo hil` is `hil/host/cli` |
| `repo/` | `oer-repo`: the one model of the repository — file inventory, manifests, workspaces, chips, typed classification, dependency policy, `owner(path)` and path-package closures |
| `tidy/` | `oer-tidy`: the repository model as text and the Cargo cache fetch (`cargo tidy workspaces`, `chips` and `fetch`) |
| `registers/` | `oer-register-tool`: register model validation and PAC/SVD publication, the PAC, shared-word and inventory checks (the inventory run included); `bindings/` (`oer-register-bindings`) is the binding index format every host layer reads |
| `blobray/` | Binary analysis; a separate workspace with its own lock file and target |
| `elf/` | `oer-elf`: the one ELF/archive view (symbols with aliases, sections, segments, relocations, DWARF `Symbolizer`) and the one RV32 relocation table (`rv32`) |
| `symbol-lineage/` | `oer-symbol-lineage`: the one relocation-normalized vendor function fingerprint and vendor symbol names across obfuscated archive releases |
| `riscv/` | RV32 layers shared by Blobray: `decode/` (instructions), `model/`, `program/`, `analysis/` and `lift/` (the program model); `stack/` (stack bounds, the only stack analyzer) |
| `process/`, `durable/`, `toolchain/` | Foundation: child processes, Git and the checkout (`oer-process`); atomic files, digests, timestamps and XDG state directories (`oer-durable`); host tool lookup, recorded tool versions, the image compiler and the Blobray workspace's Cargo commands (`oer-toolchain`). Nothing else re-implements them |
| `image/` | Image packages: `pipeline/` (`oer-image`, `build(ImageSpec) -> ImageBundle`), `bundle/`, `encode/`, `policy/`, `checks/`, `check/{interrupts,placement,stack}/`, `compare/` and `linker/` |
| `chip-profile/`, `command-tree/` | Chip profiles with their flash map and start policy, CLI command trees |
| `fw/` | `oer-fw`, `cargo fw`: the dev kit (flash, monitor, devices, compare); links images, devices, chip profiles and the foundation only |
| `device/` | `devices/` (`oer-devices`): discovery, port, console, reset and OpenOCD modules, plus flash, receipted image writes and the held-board facade with feature `image`; `mac/`, `lock/` and `peer-line/` retain their independent dependency boundaries |
| `esp-idf/` | ESP-IDF build environment and pinned catalog projects |
| `vendor-pins/`, `stats/` | Vendor pin reader and host statistics |
| `vendor-artifacts/` | `oer-vendor-artifacts`: the only reader of `verification/<chip>/artifacts.toml`, the host-wide store and fetch (`cargo xtask vendor fetch`); depends only on toml, serde and the foundation so build scripts and the register inventory use it |

## Rules

- ELF files and archives are read through `oer-elf` and RV32 relocations
  classified by its `rv32` table; no tool reads an ELF through `llvm-nm` or
  `llvm-objdump` output or keeps its own relocation list. Function listings
  come from `oer_riscv_lift::listing`. (`llvm-objdump` stays only the
  decoder's conformance oracle, `llvm-nm` only the reader of bitcode.)
- Every firmware image is built by `oer-image` (`build(ImageSpec) ->
  ImageBundle`); no other tool compiles, gates, packs or encodes an image or
  its bootloader, partition table or OTA data, and a flash only writes a
  bundle's snapshot, through the device image write (`oer_devices::image`,
  which records a receipt per board).
  Flash offsets and the start policy come from `platform/<chip>/chip.toml`'s
  `[flash]` map.
- Files, manifests, workspaces, chips, classification, the dependency rules
  and closures have one owner, `oer-repo`; tidy, xtask and the
  HIL source snapshot read them through it instead
  of listing files, parsing manifests or running `cargo metadata`.
- Blobray needs `--manifest-path tools/blobray/Cargo.toml` for every Cargo
  command, or `cargo blobray`.
- Every host package declares `open-radio.host-app`, `host-boundary` and
  `host-layer`; app boundaries are enforced independently of layers. It depends only
  down the [host layers](../docs/architecture.md#host-layers); only entry
  crates spawn `cargo hil` or `cargo xtask`.
- Tests use real temporary Cargo graphs and processes, never source-spelling
  or regex checks for Rust identifiers.

## Commands

- `cargo test -p oer-xtask`, `cargo test --manifest-path tools/tidy/Cargo.toml -p oer-tidy` or `cargo test -p oer-repo`.
- `cargo tidy workspaces --json` and `cargo tidy chips --json` print the repository model.
