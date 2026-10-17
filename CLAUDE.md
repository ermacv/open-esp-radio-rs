# open-esp-radio-rs

A Rust 2024 radio stack for the ESP32-S31 and ESP32-C5 (Wi-Fi, Bluetooth LE,
IEEE 802.15.4) with its HIL stand, vendor verification and qualification.
Coding agents do almost all the work; the owner sets the tasks.

**Chips.** ESP32-S31 is the production target: every layer, the
qualification catalogs. ESP32-C5 has
investigation inputs only: its register model and PAC, HAL, platform,
esp-hal SoC adapter, HIL target and vendor verification; it has no radio
driver, role, runtime, composition, qualification catalog or example yet.
Commands written with `<chip>` take either chip where that input exists.

## Rules

- **Ask, never fall back.** A question is material when its readings differ in a shared crate, drop required data, add a fallback, leave required work unfinished, change on-hardware behavior or qualification, or change a block's public API. Explain the ambiguity, recommend an option and ask the user; continue only independent work until they answer, and batch questions that block nothing per milestone. Never silently pick a conservative reading, older behavior, approximation or omission. An explicit user decision stays authoritative: do not ask it again.
- **No compatibility layers.** Change an interface together with every caller: no deprecated aliases, re-export shims, legacy readers or fallback paths for an old shape.
- **zsh.** An unquoted `$VAR` holding several words stays one argument: pass lists of scenarios or paths literally or as an array.
- **Tests.** Every behavioral change gets a focused regression test, beside its module (`#[cfg(test)]`) or in the crate's `tests/`. See [testing rules](crates/CLAUDE.md#tests).
- **MMIO and unsafe.** Handwritten code reaches MMIO only through typed PAC accessors; publish a missing field in the SVD/PAC instead of a local mask or shift. Keep `unsafe` narrow and documented ([crates/UNSAFE.md](crates/UNSAFE.md)). Lint policy lives in `[lints]` tables and crate-root attributes, not tool flags.
- **Generated files.** Never read the raw PACs, `pac/src/generated.rs`, published SVD/bindings or `verification/*/facts` TOML: grep them with an explicit path (tree-wide searches skip them through `.ignore`). Regenerate, never hand-edit; the pre-tool hook refuses such reads and edits.
- **Own GitHub account only.** Never act on GitHub outside the owner's account: no pushes, pull requests, issues, comments, reviews or any other message in another project, upstream included (`esp-rs/*` and every other third-party repository). Fixes to a dependency go to the owner's fork (for esp-hal, `ermacv/esp-hal` `oer/main`, see its `UPSTREAM.md`); reading other projects is fine.
- **Sources.** Never commit vendor binaries, disassembly dumps, credentials, unreviewed extraction artifacts or anything from `_oracles/`. Recovered tables and coefficients belong in production source with their provenance: [source policy](docs/source-policy.md#recovered-tables-and-coefficients).
- Preserve unrelated changes in an already-dirty worktree.

## Agent entry points

`AGENTS.md` beside each `CLAUDE.md` and `.agents/skills` are relative
symlinks to the canonical `CLAUDE.md` and `.claude/skills/`: edit only the
canonical files, and keep an `AGENTS.md` link beside any new scoped
`CLAUDE.md`.
HIL source snapshots exclude `AGENTS.md` and `.agents/` from their inputs
([source capture](hil/host/README.md#build-and-run)). `.claude/settings.json`
registers the permissions and the generated-file guard in `.claude/hooks/`.

## Build, push, commit

- Build and test with Cargo: `cargo test -p <package> <test>`. Repository commands a person runs by hand: `cargo xtask check ui` (compile-fail contract tests), `cargo xtask check feature-sets` (each package's declared feature sets), `cargo xtask doc` (API documentation and doctests), `cargo xtask lock` (every workspace's lock after a dependency or pin change; `--check` verifies offline) and `cargo xtask worktree add` (a second checkout with its own warm host `target/`).
- After a pull that changed a lock file: `cargo tidy fetch` (Cargo is offline). If tidy's own registry dependencies are missing, bootstrap with `cargo --config net.offline=false tidy fetch`. `cargo tidy workspaces` and `cargo tidy chips` print the repository model.
- Work on a branch in its own worktree. `main` changes only through pull requests: `cargo xtask push` pushes the branch with `git push` and opens its pull request as a draft, with no checks and no review; a person marks it ready and merges it.
- Commits use Conventional subjects (`feat(blobray): …`, `fix(esp32s31): …`), scoped and imperative. A production change that Blobray or verification work needs lands in its own product-scoped commit. A PR names every generated SVD/PAC change.

## Map

Each directory's `CLAUDE.md` holds its layout, local rules and commands.

| Path | Owns |
| --- | --- |
| `crates/` | Production libraries and the `oer` facade: [crates/CLAUDE.md](crates/CLAUDE.md) |
| `registers/` | Reviewed register models and generated SVD/bindings: [registers/CLAUDE.md](registers/CLAUDE.md) |
| `hil/` | HIL protocol, runner, targets, scenarios and evidence: [hil/CLAUDE.md](hil/CLAUDE.md) |
| `verification/` | `cargo verification` (own workspace): vendor pins, recovered facts and comparisons: [verification/CLAUDE.md](verification/CLAUDE.md) |
| `qualification/` | Capability catalogs, programs and the evaluator: [qualification/CLAUDE.md](qualification/CLAUDE.md) |
| `tools/` | Repository commands (xtask, tidy), dev kit (`cargo fw`: flash an image bundle or an ESP-IDF catalog image such as a `hil/peers` reference, monitor, devices, compare), images, devices, Blobray, register tool: [tools/CLAUDE.md](tools/CLAUDE.md) |
| `stand/` | `cargo stand` and the shared stand's packages: [stand guide](hil/host/stand.md) |
| `platform/` | Board boot, staged runtime entry, linker placement, chip profiles: [platform/esp32s31/README.md](platform/esp32s31/README.md) |
| `docs/` | Cross-owner contracts and reference: [architecture](docs/architecture.md), [source policy](docs/source-policy.md), [hardware errata](docs/hardware-errata.md), [protocol naming](docs/protocol-naming.md), PHY and vendor notes |

A block guide skill (`block-wifi`,
`block-bluetooth`, `block-ieee802154`) maps
a block's packages across layers, its shared contracts, traps and checks; the
agent profile of the same name in `.claude/agents/` starts a session with it
(`@block-wifi` in agent view, `claude --agent block-wifi`). A block decides
inside its own crates with the repository owner directly. A crate is shared
when it is outside every block and the production packages of two or more
blocks reach it; [the derived list](docs/architecture.md#changing-a-shared-contract) names
them, and `registers/` follows the PAC. A shared crate takes only what at
least two blocks have proven in code. Changing or removing existing public API
of a shared crate follows the
[shared-change procedure](docs/architecture.md#changing-a-shared-contract): a
prototype in a block that shows the failing case and a cost table; the
repository owner decides. An additive change (a new PAC or SVD field, a new HAL accessor,
nothing existing changed) is the block's own; its pull request names the
shared crate it extends. Bounds, cancellation and each terminal's slot are specified before a
contract's shape.
