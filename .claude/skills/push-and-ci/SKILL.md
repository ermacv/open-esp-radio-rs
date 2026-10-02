---
name: push-and-ci
description: Use before committing or pushing in this repository, when running the gate (cargo xtask check changed, cargo xtask push), when a check or CI job fails, when main CI is red, or when choosing which repository checks a change needs.
---

# Gate, push and CI

Read first (about 2k tokens): [gate and push](../../../tools/xtask/README.md#gate-and-push),
[the push gate](../../../tools/xtask/README.md#the-push-gate) and
[output](../../../tools/xtask/README.md#output).

## Checklist

1. **Iterate** with `cargo test -p <package> <test>` in the background.
2. **Gate uncommitted work** with `cargo xtask check changed`
   (`run_in_background: true`). It runs tidy, fmt of the selected packages,
   `lock --check`, docs and capability checks when relevant, and Clippy plus
   tests of the changed packages and their reverse dependents. Add `--full`
   when CI's heavy set matters: API docs, image type-checks, PHY, network,
   register and provenance audits.
3. **Read failures from the summary.** It prints at most 60 lines of
   diagnostics and the log path under `target/xtask/logs/`; open that log with
   a bounded read (`tail -n 200`, Grep) rather than rerunning with `--verbose`.
4. **Commit** scoped Conventional commits: `feat(scope): …`, `fix(esp32s31): …`.
   A production change needed by Blobray or verification work is its own commit.
5. **Push** with `cargo xtask push` in the background: it gates exactly
   `HEAD`, rebases onto `origin/main` and reruns the gate only for packages
   both sides touched. Never `git push` to `main` directly and never chain
   `check changed | tail && git push` (a pipeline returns its last status).
6. **CI** runs on every branch push; `.github/workflows/ci.yml` is the full
   checkpoint and each job is one command in [the xtask reference](../../../tools/xtask/README.md).
   The gate prints any workflow whose newest run on `main` failed.
7. **Red `main` first.** If `main` CI is red, fixing it comes before other
   work: `gh run list --branch main --limit 5`, then
   `gh run view <id> --log-failed`, reproduce the job's command locally.

## Other workspaces and dependencies

- `cargo xtask lock` after a dependency or pin change; `cargo tidy fetch`
  after a pull that changed a lock file.
- Examples: `cargo xtask build firmware <example> --type-check`.
- Image classes: `cargo xtask check firmware --list`, then
  `cargo xtask check firmware --class <class> --type-check`.
- Blobray: `--manifest-path tools/blobray/Cargo.toml`.
- `cargo xtask sweep` lists rebuildable caches when the disk runs low.
