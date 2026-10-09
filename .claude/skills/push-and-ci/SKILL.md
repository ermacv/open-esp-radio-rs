---
name: push-and-ci
description: Use before committing or pushing in this repository, when running the gate (cargo xtask check changed, cargo xtask push), when a check or CI job fails, when main CI is red, or when choosing which repository checks a change needs.
---

# Gate, push and CI

Read first (about 2k tokens): [gate and push](../../../tools/xtask/README.md#gate-and-push),
[the check registry](../../../tools/xtask/README.md#the-check-registry),
[the push gate](../../../tools/xtask/README.md#the-push-gate) and
[output](../../../tools/xtask/README.md#output).

## Checklist

1. **Branch.** Every change lives on its own branch in its own worktree
   (`cargo xtask worktree add`, or the session's worktree). `main` changes
   only through pull requests.
2. **Iterate** with `cargo test -p <package> <test>` in the background.
3. **Gate uncommitted work** with `cargo xtask check changed`
   (`run_in_background: true`), under a minute warm. It runs the fast checks
   of the registry the change selects: tidy, fmt of the selected packages,
   `lock --check` and capability checks when relevant, Clippy of the changed
   packages and their reverse dependents, the tests of the changed packages
   and, for chip code, image type-checks. `--full` adds the full-tier checks
   CI runs for the change: tests of the dependents, `check docs`, API docs,
   architecture, examples, PHY, network, register, qualification and
   provenance checks. Use it only when that set is the point.
   `cargo xtask check tier --list` prints every check with its tier and CI
   job.
4. **Read failures from the summary.** It prints at most 60 lines of
   diagnostics and the log path under `target/xtask/logs/`; open that log with
   a bounded read (`tail -n 200`, Grep) rather than rerunning with `--verbose`.
5. **Commit** scoped Conventional commits: `feat(scope): …`, `fix(esp32s31): …`.
   A production change needed by Blobray or verification work is its own commit.
   The pull request merges by rebase, so each commit lands on `main` as is.
6. **Push** with `cargo xtask push` in the background: it gates exactly
   `HEAD`, pushes the branch, opens the pull request titled by its first
   commit (`gh pr create --fill-first`)
   and enables auto-merge. Do not wait for CI: GitHub merges the branch once
   the required `ci-ok` check passes. `--draft` for work that needs the
   user's review first. To resolve a conflict with `main`, rebase the
   branch and push again: `push` rewrites only its own branch, with
   `--force-with-lease`. Never push to `main` directly.
7. **CI** runs on every branch push: xtask plans jobs by the complete Git tree and tool versions
   ([CI reuse](../../../tools/xtask/README.md#ci-input-reuse)). Each executed
   check job of `.github/workflows/ci.yml` runs
   `cargo xtask check tier full --job <job>`, so a failed check job is
   reproduced locally with the same command. A new check is a registry
   entry (`tools/xtask/src/registry.rs`), never a workflow step. A failed
   check on the pull request is fixed on the same branch and pushed again
   with `cargo xtask push`.
8. **Red `main` first.** The session start (`cargo xtask ci-status`) and the
   gate print a red `main`.
   Fixing it comes before other work: `gh run view <id> --log-failed`,
   reproduce the job's command locally, fix it on a branch.
9. **Review findings.** The Claude review blocks a PR on 🔴 important and 🟡
   nit findings and files 🟣 pre-existing ones as issues (`REVIEW.md`). The
   session start and the gate list every open PR a review blocks, with its
   branch; fixing it comes before other work too: `gh pr view <n> --comments`,
   fix on that branch (its worktree, or a new one from `origin/<branch>`) and
   `cargo xtask push` again. A PR without a review verdict gets a
   `@claude review` comment.

## Other workspaces and dependencies

- `cargo xtask lock` after a dependency or pin change; `cargo tidy fetch`
  after a pull that changed a lock file.
- Examples: `cargo fw build <example> --type-check`.
- Image classes: `cargo hil images check --list`, then
  `cargo hil images check --class <class> --type-check`; without
  `--type-check` the class's image bundle is built with every gate, and
  `performance`/`correctness` also pass Blobray's final radio target audit.
- Blobray: `--manifest-path tools/blobray/Cargo.toml`.
- `cargo hil sweep` lists rebuildable caches when the disk runs low.
