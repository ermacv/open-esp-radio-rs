# open-esp-radio-rs

A Rust 2024 radio stack for the ESP32-S31 and ESP32-C5 (Wi-Fi, Bluetooth LE,
IEEE 802.15.4) with its HIL stand, vendor verification and qualification.
Coding agents do almost all the work; the owner sets the tasks.

**Chips.** ESP32-S31 is the production target: every layer, the
qualification catalogs and programs, and the examples. ESP32-C5 has
investigation inputs only: its register model and PAC, HAL, platform,
esp-hal SoC adapter, HIL target and vendor verification; it has no radio
driver, role, runtime, composition, qualification catalog or example yet.
Commands written with `<chip>` take either chip where that input exists.

## Rules

- **Ask, never fall back.** When a requirement or rule admits materially different readings that change the implementation, drop required data, add a fallback or leave work unfinished, explain the ambiguity and ask the user; continue only independent work until they answer. Never silently pick a conservative reading, older behavior, approximation or omission. An explicit user decision stays authoritative: do not ask it again.
- **No compatibility layers.** Change an interface together with every caller: no deprecated aliases, re-export shims, legacy readers or fallback paths for an old shape.
- **Background work.** Builds, tests, checks and HIL runs (`cargo build|test|clippy|check|doc`, `cargo xtask check`, `cargo xtask push`, `cargo fw build`, `cargo hil run`, `cargo hil wait`) run asynchronously (`run_in_background: true`); act on completion and keep answering meanwhile. Greps and reads stay in the foreground. No foreground `sleep` polling. Never wait for or signal processes by name (`pgrep -f`, `pkill -f`): the pattern matches the waiting shell too. Chain dependent steps in one command or wait on a PID. Claude Code's pre-tool hook enforces this.
- **zsh.** An unquoted `$VAR` holding several words stays one argument: pass lists of scenarios or paths literally or as an array.
- **Tests.** Every behavioral change gets a focused regression test, beside its module (`#[cfg(test)]`) or in the crate's `tests/`. See [testing rules](crates/CLAUDE.md#tests).
- **MMIO and unsafe.** Handwritten code reaches MMIO only through typed PAC accessors; publish a missing field in the SVD/PAC instead of a local mask or shift. Keep `unsafe` narrow and documented ([crates/UNSAFE.md](crates/UNSAFE.md)). Lint policy lives in `[lints]` tables and crate-root attributes, not tool flags.
- **Generated files.** Never read the raw PACs, `pac/src/generated.rs`, published SVD/bindings or `verification/*/facts` TOML: grep them with an explicit path (tree-wide searches skip them through `.ignore`). Regenerate, never hand-edit; the pre-tool hook refuses such reads and edits.
- **Own GitHub account only.** Never act on GitHub outside the owner's account: no pushes, pull requests, issues, comments, reviews or any other message in another project, upstream included (`esp-rs/*` and every other third-party repository). Fixes to a dependency go to the owner's fork (for esp-hal, `ermacv/esp-hal` `oer/main`, see its `UPSTREAM.md`); reading other projects is fine.
- **Decisions.** An owner decision is only what the repository owner said or approved, recorded with a link; the owner may revise it at any time. Never record your own choice as an "owner decision": propose options and ask. See [decisions and approval](docs/issues.md#decisions-and-approval).
- **Issue creation.** Before creating any issue through a plugin, API or CLI, validate its explicit labels with `python3 .github/scripts/issue_labels.py validate`: exactly one `kind:*`, at least one `area:*`, and exactly one `priority:*` for an executable issue (tracking may omit priority). Supply that complete set in the creation request and verify the returned labels; never defer them to triage or report creation complete while classification is invalid. See [issue management](docs/issues.md#creation-and-automatic-control).
- **Sources.** Never commit vendor binaries, disassembly dumps, credentials, unreviewed extraction artifacts or anything from `_oracles/`. Recovered tables and coefficients belong in production source with their provenance: [source policy](docs/source-policy.md#recovered-tables-and-coefficients).
- **Docs current.** Update the owning README or rustdoc in the same change ([documentation policy](docs/documentation.md)). Never add audit reports, work plans, migration histories, experiment diaries or test-run summaries; generated reports stay in ignored outputs.
- **Block knowledge.** A non-obvious fact about a block found during a task (an invariant, a trap, where a value really comes from) goes into that block's guide skill or owning README in the same PR.
- Preserve unrelated changes in an already-dirty worktree.

## Agent entry points

`AGENTS.md` beside each `CLAUDE.md` and `.agents/skills` are relative
symlinks to the canonical `CLAUDE.md` and `.claude/skills/`: edit only the
canonical files, and keep an `AGENTS.md` link beside any new scoped
`CLAUDE.md` (`cargo tidy check` holds the links and skill frontmatter).
HIL source snapshots exclude `AGENTS.md` and `.agents/` from their inputs
([source capture](hil/host/README.md#build-and-run)). `.claude/settings.json`
registers the permissions and the hooks in `.claude/hooks/`.

## Gate, push, commit

- Iterate with `cargo test -p <package> <test>`; gate uncommitted work with `cargo xtask check changed`, under a minute warm (`--full` adds CI's full tier). One check registry decides what the gate, push and every CI job run (`cargo xtask check tier --list`; a CI job is `cargo xtask check tier full --job <job>`). Output is a summary; logs are in `target/xtask/logs/`.
- Work on a branch in its own worktree. `main` changes only through pull requests: `cargo xtask push` gates exactly `HEAD`, pushes the branch, opens its PR and enables auto-merge (`--draft` leaves merging to a person). CI on the PR is the full check; do not wait for it. A red `main` comes before other work, and so does your own PR a Claude review blocks (🔴 or 🟡); another session's blocked PR is only reported, never pushed to. Skill: `push-and-ci`; reference: [the push gate](tools/xtask/README.md#the-push-gate).
- After a pull that changed a lock file: `cargo tidy fetch` (Cargo is offline). If tidy's own registry dependencies are missing, bootstrap with `cargo --config net.offline=false tidy fetch`; its host-only workspace does not require firmware Git sources to start, including after a moved Git pin. After a dependency change: `cargo xtask lock`. A second checkout: `cargo xtask worktree add` (its own warm host `target/`). Host workspaces keep private target directories; firmware images and vendor probes use the one host-wide compile cache owned by `oer-toolchain`, with its build lock held until artifacts are copied out.
- Commits use Conventional subjects (`feat(blobray): …`, `fix(esp32s31): …`), scoped and imperative. A production change that Blobray or verification work needs lands in its own product-scoped commit. A PR names the ownership boundary, the checks run, any qualification/HIL evidence and every generated SVD/PAC change.

## Map

Each directory's `CLAUDE.md` holds its layout, local rules and commands.

| Path | Owns |
| --- | --- |
| `crates/` | Production libraries and the `oer` facade: [crates/CLAUDE.md](crates/CLAUDE.md) |
| `registers/` | Reviewed register models and generated SVD/bindings: [registers/CLAUDE.md](registers/CLAUDE.md) |
| `hil/` | HIL protocol, runner, targets, scenarios and evidence: [hil/CLAUDE.md](hil/CLAUDE.md) |
| `verification/` | `cargo verification` (own workspace): vendor pins, recovered facts and comparisons: [verification/CLAUDE.md](verification/CLAUDE.md) |
| `qualification/` | Capability catalogs, programs and the evaluator: [qualification/CLAUDE.md](qualification/CLAUDE.md) |
| `tools/` | Gate (xtask, tidy), dev kit (`cargo fw`), images, devices, Blobray, register tool: [tools/CLAUDE.md](tools/CLAUDE.md) |
| `stand/` | `cargo stand` and the shared stand's packages: [stand guide](hil/host/stand.md) |
| `platform/` | Board boot, staged runtime entry, linker placement, chip profiles: [platform/esp32s31/README.md](platform/esp32s31/README.md) |
| `examples/` | Buildable applications |
| `docs/` | Cross-owner contracts: [architecture](docs/architecture.md), [documentation](docs/documentation.md), [verification and qualification](docs/verification-and-qualification.md) |

Task workflows live in `.claude/skills/`: `protocol-change`,
`driver-or-hardware-change`, `new-package`, `qualification-entry`,
`vendor-evidence`, `hil-run` and `push-and-ci`.

One task is one session and one PR. A block guide skill (`block-wifi`,
`block-bluetooth`, `block-ieee802154`) maps
a block's packages across layers, its shared contracts, traps and checks; the
agent profile of the same name in `.claude/agents/` starts a session with it
(`@block-wifi` in agent view, `claude --agent block-wifi`). A shared
contract does not change while a block that depends on it has work in flight.
