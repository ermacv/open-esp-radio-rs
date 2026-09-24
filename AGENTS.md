# Repository Guidelines

## Ambiguous Requirements

If requirements or repository rules admit materially different interpretations
that change implementation, exclude required data, introduce a fallback, or
leave requested work unfinished, explicitly explain the ambiguity and ask the
user which interpretation to apply. Wait for their answer before making the
dependent decision; continue independent work where possible. Do not silently
choose a conservative interpretation, older behavior, approximation or omission
as a fallback. Existing explicit user decisions remain authoritative and must
not be requested again.

## Project Structure & Module Organization

This Rust 2024 workspace separates shipping code from evidence and tooling:

- `crates/` contains production libraries and the thin `oer` facade. Portable
  protocols live in `crates/protocols/`; ESP32-S31 PAC, HAL, PHY and radio
  backends live under `crates/hardware/esp32s31/`. Executor and board bindings
  live in `crates/adapters/` and `crates/composition/`. Stable-memory contracts
  live in `crates/memory/`; network values live in `crates/network/interface/`
  and stack adapters in `crates/adapters/{embassy-net,xarxa}/`.
  `experiments/network-engine/` owns the experimental network engine and may
  be used by production tests, never by production dependencies.
  Concrete Wi-Fi and Bluetooth
  radio execution lives in `crates/runtime/embassy/esp32s31/`; the Embassy
  executor/time platform backend remains in `crates/adapters/`.
  Every package declares `package.metadata.open-radio` scope, layer and
  platform (`portable`, `host` or `chip`), with a separate `chip` identifier
  when platform is `chip`. Architecture checks enforce the dependency graph
  independently of source location. Internal libraries never depend on the `oer` facade.
- `platform/esp32s31/` owns shared board boot, staged runtime entry and linker
  placement for HIL and standalone examples. `tools/firmware/` owns host image
  packing and structural checks.
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

Use `cargo xtask check docs` for Markdown/catalog changes and
`cargo xtask check docs --package <package>` for API changes. Add `--private`
when private API is affected. `cargo xtask check docs --full` and
`cargo xtask check source-only` are full checkpoints, not the default after
an individual edit. Run the relevant target, architecture, safety and artifact
checks when changing their ownership boundaries; a partial check is not full
repository coverage.

Use `cargo test -p <package> <test_name>` for focused iteration. Blobray and
its vendor providers form the separate `tools/blobray` workspace with its own
lockfile and target directory; pass `--manifest-path tools/blobray/Cargo.toml`
to Cargo for them. Build the Blobray host with `cargo build --manifest-path
tools/blobray/Cargo.toml --profile blobray -p blobray-next --bin blobray`
(or `cargo blobray`). Real analyses use its built-in supervisor; select `--limit-mode
watchdog` explicitly when kernel cgroup delegation is unavailable. Register
publication uses `cargo registers generate --manifest
registers/esp32s31/publication/registers.toml --check`. HIL commands require attached hardware;
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

The user-authorized exception is [the Blobray completion plan](tools/blobray/PLAN.md).
It is the sole tracked work plan: maintain its stage obligations and status when
working on Blobray. Complete an active stage, including acceptance checks, before
advancing. Do not request confirmation between authorized stages. An infeasible
stage requires an explicit `needs-replan` entry preserving every unmet obligation;
partial implementation is not completion. Logs, measurements and private inputs
remain in ignored outputs. This exception does not authorize other work logs.

## Commit & Pull Request Guidelines

History follows Conventional Commit-style subjects such as
`feat(blobray): ...`, `fix(esp32s31): ...`, and `refactor(blobray): ...`.
Keep commits scoped and imperative. A production-crate change needed by
Blobray or verification work lands in its own product-scoped commit, never
inside a `blobray` commit: production owners are HIL evidence inputs. PRs
should explain the affected ownership boundary, list checks run, link qualification/HIL evidence where applicable,
and call out generated SVD/PAC changes. Never commit vendor binaries,
disassembly dumps, credentials, or unreviewed extraction artifacts. Necessary
recovered hardware tables and calibration coefficients are explicitly allowed
in production source, including values recovered from binaries. Record their
source identity, purpose, representation and applicable hardware/profile;
verify their use against the real source artifact. Binary origin alone is
never a reason to omit required data or substitute an older profile. See
[docs/source-policy.md](docs/source-policy.md) for the canonical rule. Preserve
unrelated changes in an already-dirty worktree.
