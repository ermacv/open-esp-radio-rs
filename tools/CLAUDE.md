# tools/

Repository tooling, grouped by the contract each tool owns: [README](README.md).

| Path | Owns |
| --- | --- |
| `xtask/` | The entry command line: the check registry (`check tier`, the gate of `check changed` and `push`, CI's jobs), CI state, locks, worktrees, sweeps and the repository checks ([commands](xtask/README.md)); domain work is called from its owners; `cargo hil` is `hil/host/cli` |
| `repo/` | `oer-repo`: the one model of the repository — file inventory, manifests, workspaces, chips, typed classification, dependency policy, `owner(path)` and path-package closures |
| `tidy/` | `oer-tidy`: fast text policy over that model (`cargo tidy check`) |
| `registers/` | `oer-register-tool`: register model validation and PAC/SVD publication, the PAC, shared-word and inventory checks (the inventory run included); `bindings/` (`oer-register-bindings`) is the binding index format every host layer reads |
| `blobray/` | Binary analysis; a separate workspace with its own lock file and target |
| `elf/` | `oer-elf`: the one ELF/archive view (symbols with aliases, sections, segments, relocations, DWARF `Symbolizer`) and the one RV32 relocation table (`rv32`) |
| `vendor-provenance/`, `symbol-lineage/`, `phy-archive/` | Verification: the `SOURCE` citation recogniser (tidy uses it), vendor function fingerprints, the provenance registry and check, `vendor-diff`, vendor symbol names across releases and the source-only PHY archive's compiled-symbol audit |
| `riscv/` | RV32 layers shared by Blobray: `decode/` (instructions), `model/`, `program/`, `analysis/` and `lift/` (the program model); `stack/` (stack bounds, the only stack analyzer) |
| `process/`, `durable/`, `toolchain/` | Foundation: child processes, Git and the checkout (`oer-process`); atomic files, digests, timestamps and XDG state directories (`oer-durable`); host tool lookup, recorded tool versions, the image compiler and the Blobray workspace's Cargo commands (`oer-toolchain`). Nothing else re-implements them |
| `image/` | Image packages: `pipeline/` (`oer-image`, `build(ImageSpec) -> ImageBundle`), `bundle/`, `encode/`, `policy/`, `checks/`, `check/{interrupts,placement,stack}/`, `compare/` and `linker/` |
| `chip-profile/`, `command-tree/` | Chip profiles with their flash map and start policy, CLI command trees |
| `fw/` | `oer-fw`, `cargo fw`: the dev kit (build, flash, monitor, devices); links images, devices, chip profiles and the foundation only |
| `device/` | `devices/` (`oer-devices`): discovery, port, console, reset and OpenOCD modules, plus flash, receipted image writes and the held-board facade with feature `image`; `mac/`, `lock/` and `peer-line/` retain their independent dependency boundaries |
| `esp-idf/` | ESP-IDF build environment and pinned catalog projects |
| `vendor-pins/`, `markers/`, `stats/` | Vendor pin reader, the `SOURCE`/`CAPABILITY` marker recognisers and host statistics |
| `vendor-artifacts/` | `oer-vendor-artifacts`: the only reader of `verification/<chip>/artifacts.toml`, the store and fetch, and the layout of a chip's verification project (`project::Project`); depends only on toml, serde and the foundation so build scripts and Blobray use it |

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
  and closures have one owner, `oer-repo`; tidy, xtask, the evaluator, the
  HIL source snapshot and the vendor harness read them through it instead
  of listing files, parsing manifests or running `cargo metadata`.
- Blobray needs `--manifest-path tools/blobray/Cargo.toml` for every Cargo
  command, or `cargo blobray`.
- A command or flag change updates the docs that name it: `cargo xtask check
  docs` checks every documented xtask, fw, stand, tidy, hil, qualification,
  registers, verification and blobray command against that tool's command
  tree.
- Every host package declares `open-radio.host-app`, `host-boundary` and
  `host-layer`; app boundaries are enforced independently of layers. It depends only
  down the [host layers](../docs/architecture.md#host-layers); only entry
  crates spawn `cargo hil` or `cargo xtask`.
- What runs where is the check registry's (`cargo xtask check tier
  --list`): a new check is a registry entry, never a workflow step.
- Tests use real temporary Cargo graphs and processes, never source-spelling
  or regex checks for Rust identifiers.
- Output stays a summary; full logs go to `target/xtask/logs/`.

## Commands (in the background)

- `cargo test -p oer-xtask`, `cargo test --manifest-path tools/tidy/Cargo.toml -p oer-tidy` or `cargo test -p oer-repo`.
- `cargo tidy check` and `cargo xtask check docs`.
- `cargo xtask check tier --list` prints the registry; `cargo xtask check
  tier full --job host` runs one CI job locally.
- `cargo tidy workspaces --json` and `cargo tidy chips --json` print the repository model.
