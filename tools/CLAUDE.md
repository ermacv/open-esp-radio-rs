# tools/

Repository tooling, grouped by the contract each tool owns: [README](README.md).

| Path | Owns |
| --- | --- |
| `xtask/` | Gate, push, locks, architecture/docs/firmware checks, vendor commands ([commands](xtask/README.md)); `cargo hil` is `hil/host/cli` |
| `tidy/` | `oer-tidy`: fast text checks and the one model of workspaces, chips and package classification |
| `registers/` | `oer-register-tool`: register model validation and PAC/SVD publication |
| `blobray/` | Binary analysis; a separate workspace with its own lock file and target |
| `memory-report/`, `symbol-lineage/` | ELF memory and linked-code analysis; vendor symbol names across releases |
| `riscv/` | RV32 layers shared by Blobray: `decode/` (instructions), `model/`, `program/`, `analysis/` and `lift/` (the program model); `stack/` (stack bounds, the only stack analyzer) |
| `firmware/`, `process/`, `chip-profile/`, `command-tree/` | Image operations, child processes, chip profiles, CLI command trees |
| `vendor-artifacts/` | `oer-vendor-artifacts`: the pinned vendor artifacts of `verification/<chip>/artifacts.toml`, their store and fetch |
| `image-linker/` | `oer-image-linker`: every image's linker; refuses an input section bound for a zeroed region (the platform layout's list) that holds a non-zero byte or a relocation, then runs `rust-lld` unchanged |

## Rules

- Classification, workspaces and chips have one owner, `oer-tidy`; xtask and
  CI read them through it instead of re-deriving them.
- Blobray needs `--manifest-path tools/blobray/Cargo.toml` for every Cargo
  command, or `cargo blobray`.
- A command or flag change updates the docs that name it: `cargo xtask check
  docs` checks every documented xtask, hil, qualification, registers and
  blobray command against that tool's command tree.
- Tests use real temporary Cargo graphs and processes, never source-spelling
  or regex checks for Rust identifiers.
- Output stays a summary; full logs go to `target/xtask/logs/`.

## Commands (in the background)

- `cargo test -p oer-xtask` or `cargo test -p oer-tidy`.
- `cargo xtask check tidy` and `cargo xtask check docs`.
- `cargo tidy workspaces --json` and `cargo tidy chips --json` print the repository model.
