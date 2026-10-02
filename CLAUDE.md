# open-esp-radio-rs

A Rust 2024 radio stack for the ESP32-S31 and ESP32-C5 (Wi-Fi, Bluetooth LE,
IEEE 802.15.4) with its HIL stand, vendor verification and qualification.
Claude Code agents do almost all the work; the owner sets the tasks.

## Rules

- **Ask, never fall back.** When a requirement or rule admits materially different readings that change the implementation, drop required data, add a fallback or leave work unfinished, explain the ambiguity and ask the user; continue only independent work until they answer. Never silently pick a conservative reading, older behavior, approximation or omission. An explicit user decision stays authoritative: do not ask it again.
- **No compatibility layers.** Change an interface together with every caller: no deprecated aliases, re-export shims, legacy readers or fallback paths for an old shape.
- **Background work.** Builds, tests, checks and HIL runs (`cargo build|test|clippy|check|doc`, `cargo xtask check`, `cargo xtask push`, `cargo xtask build`, `cargo hil run`, `cargo hil wait`) run with `run_in_background: true`; act on the completion notification and keep answering meanwhile. Greps and reads stay in the foreground. No foreground `sleep` polling. Never wait for or signal processes by name (`pgrep -f`, `pkill -f`): the pattern matches the waiting shell too. Chain dependent steps in one command or wait on a PID. A hook enforces this.
- **zsh.** An unquoted `$VAR` holding several words stays one argument: pass lists of scenarios or paths literally or as an array.
- **Tests.** Every behavioral change gets a focused regression test, beside its module (`#[cfg(test)]`) or in the crate's `tests/`. See [testing rules](crates/CLAUDE.md#tests).
- **MMIO and unsafe.** Handwritten code reaches MMIO only through typed PAC accessors; publish a missing field in the SVD/PAC instead of a local mask or shift. Keep `unsafe` narrow and documented ([crates/UNSAFE.md](crates/UNSAFE.md)). Lint policy lives in `[lints]` tables and crate-root attributes, not tool flags.
- **Generated files.** Never read the raw PACs, `pac/src/generated.rs`, published SVD/bindings or `verification/*/facts` TOML: grep them with an explicit path (tree-wide searches skip them through `.ignore`). Regenerate, never hand-edit.
- **Sources.** Never commit vendor binaries, disassembly dumps, credentials, unreviewed extraction artifacts or anything from `_oracles/`. Recovered tables and coefficients belong in production source with their provenance: [source policy](docs/source-policy.md#recovered-tables-and-coefficients).
- **Docs current.** Update the owning README or rustdoc in the same change ([documentation policy](docs/documentation.md)). Never add audit reports, work plans, migration histories, experiment diaries or test-run summaries; generated reports stay in ignored outputs.
- Preserve unrelated changes in an already-dirty worktree.

## Gate, push, commit

- Iterate with `cargo test -p <package> <test>`; gate uncommitted work with `cargo xtask check changed`, under a minute warm (`--full` adds CI's heavy set). Output is a summary; logs are in `target/xtask/logs/`.
- Work on a branch in its own worktree. `main` changes only through pull requests: `cargo xtask push` gates exactly `HEAD`, pushes the branch, opens its PR and enables auto-merge (`--draft` leaves merging to a person). CI on the PR is the full check; do not wait for it. A red `main` comes before other work. Skill: `push-and-ci`; reference: [the push gate](tools/xtask/README.md#the-push-gate).
- After a pull that changed a lock file: `cargo tidy fetch` (Cargo is offline). After a dependency change: `cargo xtask lock`. A second checkout: `cargo xtask worktree add` (its own warm `target/`; never share a target directory).
- Commits use Conventional subjects (`feat(blobray): …`, `fix(esp32s31): …`), scoped and imperative. A production change that Blobray or verification work needs lands in its own product-scoped commit. A PR names the ownership boundary, the checks run, any qualification/HIL evidence and every generated SVD/PAC change.

## Map

Each directory's `CLAUDE.md` holds its layout, local rules and commands.

| Path | Owns |
| --- | --- |
| `crates/` | Production libraries and the `oer` facade: [crates/CLAUDE.md](crates/CLAUDE.md) |
| `registers/` | Reviewed register models and generated SVD/bindings: [registers/CLAUDE.md](registers/CLAUDE.md) |
| `hil/` | HIL protocol, runner, targets, scenarios and evidence: [hil/CLAUDE.md](hil/CLAUDE.md) |
| `verification/` | Vendor pins, recovered facts and comparisons: [verification/CLAUDE.md](verification/CLAUDE.md) |
| `qualification/` | Capability catalogs, programs and the evaluator: [qualification/CLAUDE.md](qualification/CLAUDE.md) |
| `tools/` | xtask, tidy, Blobray, register tool, memory report: [tools/CLAUDE.md](tools/CLAUDE.md) |
| `platform/` | Board boot, staged runtime entry, linker placement, chip profiles: [platform/esp32s31/README.md](platform/esp32s31/README.md) |
| `examples/`, `experiments/` | Buildable applications; the experimental network engine no production package uses |
| `docs/` | Cross-owner contracts: [architecture](docs/architecture.md), [documentation](docs/documentation.md), [verification and qualification](docs/verification-and-qualification.md) |

Task workflows live in `.claude/skills/`: `protocol-change`,
`driver-or-hardware-change`, `new-package`, `qualification-entry`,
`vendor-evidence`, `hil-run` and `push-and-ci`.
