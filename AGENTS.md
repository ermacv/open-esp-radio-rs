# Repository Guidelines

## Project Structure & Module Organization

This Rust 2024 workspace separates shipping code from evidence and tooling:

- `driver/` contains production radio code. Cross-chip protocols live in
  `driver/wifi/`; ESP32-S31 PAC, HAL, PHY, DMA, and MAC implementations live
  under `driver/chips/esp32s31/`; executor and board bindings live in
  `driver/adapters/` and `driver/integration/`.
- `hil/` contains the typed HIL protocol, host runner, targets, and scenarios.
- `verification/vendor/` holds reviewed vendor-comparison inputs; `_oracles/`
  is private input and must never be committed.
- `qualification/` is the machine-checked capability ledger. `svd/` contains
  reviewed hardware descriptions. `tools/` contains repository utilities and
  Blobray.

Keep tests beside their Rust modules (`#[cfg(test)]`) or in a crate's `tests/`
directory. Do not place production behavior in verification probes.

## Build, Test, and Development Commands

```console
cargo check --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
cargo qualification check --manifest qualification/targets/esp32s31/wifi-sta.ledger
tools/audit-source-only.sh
```

Use `cargo test -p <package> <test_name>` for focused iteration. Build the
Blobray with `cargo build --profile blobray -p
blobray-esp32s31 --bin blobray` and
run real analyses through `tools/blobray/scripts/run-limited`
to enforce its memory and time limits. HIL commands require attached hardware;
follow `hil/targets/esp32s31/README.md`.

## Coding Style & Naming Conventions

Use standard `rustfmt` formatting (four-space indentation). Follow Rust naming:
`snake_case` functions/modules, `UpperCamelCase` types, and `SCREAMING_SNAKE_CASE`
constants. Prefer typed ownership/state transitions over raw addresses or
integer register images. Keep `unsafe` narrowly scoped and documented; the
workspace denies `unsafe_op_in_unsafe_fn` and mutable calls in `debug_assert!`.

## Testing Guidelines

Every behavioral change needs a focused regression test. Hardware-facing
changes should pair host tests with dated HIL evidence when qualification is
claimed. Vendor comparison must fail closed (`MATCH`, `DIFF`, or `INCOMPLETE`)
and must exercise compiled production code, not a shadow implementation.

## Commit & Pull Request Guidelines

History follows Conventional Commit-style subjects such as
`feat(blobray): ...`, `fix(esp32s31): ...`, and `refactor(blobray): ...`.
Keep commits scoped and imperative. PRs should explain the affected ownership
boundary, list checks run, link qualification/HIL evidence where applicable,
and call out generated SVD/PAC changes. Never commit vendor binaries,
disassembly dumps, credentials, or proprietary extracted tables. Preserve
unrelated changes in an already-dirty worktree.
