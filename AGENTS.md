# Repository Guidelines

## Project Structure & Module Organization

This Rust 2024 workspace separates shipping code from evidence and tooling:

- `driver/` contains production radio code. Cross-chip protocols live in
  `driver/ieee80211/`; ESP32-S31 PAC, HAL, PHY, DMA, and MAC implementations live
  under `driver/chips/esp32s31/`; executor and board bindings live in
  `driver/adapters/` and `driver/integration/`. Stable-memory contracts live in
  `driver/memory/`; network values, stack adapters and the experimental
  research engine live under `driver/network/`. Concrete Wi-Fi and Bluetooth
  radio execution lives in `driver/runtime/embassy/esp32s31/`; the Embassy
  executor/time platform backend remains in `driver/adapters/`.
- `hil/` contains the typed HIL protocol, host runner, targets, and scenarios.
- `verification/vendor/` holds reviewed vendor-comparison inputs; `_oracles/`
  is private input and must never be committed.
- `qualification/` owns capability programs and their independent evaluator.
  `registers/` owns reviewed hardware models, publication policy and generated
  SVD/bindings. `tools/` contains Blobray, memory analysis and repository checks
  under `tools/repo/`. Vendor investigation compositions live under
  `verification/vendor/projects/`.

Keep tests beside their Rust modules (`#[cfg(test)]`) or in a crate's `tests/`
directory. Do not place production behavior in verification probes.

## Build, Test, and Development Commands

```console
cargo check --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
cargo qualification validate --manifest qualification/targets/esp32s31/wifi-sta.toml
cargo xtask check source-only
```

Use `cargo test -p <package> <test_name>` for focused iteration. Build the
Blobray host with `cargo build --profile blobray -p blobray-esp32s31 --bin
blobray` and its limiter with `cargo build --profile blobray -p blobray --bin
blobray-run`. Run real analyses through `target/blobray/blobray-run`
to enforce its memory and time limits. HIL commands require attached hardware;
follow `hil/targets/esp32s31/README.md`.

## Coding Style & Naming Conventions

Use standard `rustfmt` formatting (four-space indentation). Follow Rust naming:
`snake_case` functions/modules, `UpperCamelCase` types, and `SCREAMING_SNAKE_CASE`
constants. Prefer typed ownership/state transitions over raw addresses or
integer register images. Handwritten code outside the generated/restricted PAC
must access MMIO only through typed PAC accessors; if an accessor is missing,
review and publish the field in the SVD/PAC instead of adding a local mask or
shift. Keep `unsafe` narrowly scoped and documented; the workspace denies
`unsafe_op_in_unsafe_fn` and mutable calls in `debug_assert!`.

## Testing Guidelines

Every behavioral change needs a focused regression test. Hardware-facing
changes should pair host tests with dated HIL evidence when qualification is
claimed. Vendor comparison must fail closed (`MATCH`, `DIFF`, or `INCOMPLETE`)
and must exercise compiled production code, not a shadow implementation. Do
not test generated register addresses, masks, shifts, field positions, or PAC
type names. Tests for memory protocols should verify behavior and ownership,
not reproduce the same raw image or layout constants as the implementation.

## Documentation Guidelines

Keep tracked documentation current: describe implemented interfaces, ownership,
usage and limitations. Follow [docs/documentation.md](docs/documentation.md).
Do not add audit reports, work plans, migration histories, experiment diaries
or test-run summaries. Store generated reports with their owner's ignored
outputs. Capability matrices may describe source coverage and hardware limits;
qualification remains the readiness authority. Preserve reviewed machine
provenance and schema inputs when removing narrative history.

## Commit & Pull Request Guidelines

History follows Conventional Commit-style subjects such as
`feat(blobray): ...`, `fix(esp32s31): ...`, and `refactor(blobray): ...`.
Keep commits scoped and imperative. PRs should explain the affected ownership
boundary, list checks run, link qualification/HIL evidence where applicable,
and call out generated SVD/PAC changes. Never commit vendor binaries,
disassembly dumps, credentials, or proprietary extracted tables. Preserve
unrelated changes in an already-dirty worktree.
