# Contributing

## Claude review

The [review workflow](.github/workflows/claude-review.yml) runs Claude Code
through the [Claude Agent SDK](https://code.claude.com/docs/en/agent-sdk/overview)
([script](.github/scripts/claude_review.py)) following the repository's
[review instructions](REVIEW.md). The Agent SDK, unlike Claude Code GitHub
Actions, is covered by the
[monthly API credits of Max and Team plans](https://platform.claude.com/docs/en/about-claude/api-credits-for-subscribers);
an uncovered request fails with `Credit balance too low`. It reviews each
open, ready, same-repository pull request into `main` after every successful
`CI` push run, when the PR opens or is marked ready with CI already green, and when a
collaborator comments `@claude review` on it. Draft PRs and PRs without green
CI are not reviewed.

Every event runs the workflow file from `main`. The trusted `main` checkout
supplies the script, `REVIEW.md` and the root `CLAUDE.md`, appended to the
system prompt; repository hooks, permission rules and agents are not loaded.
The PR head is checked out in `pr/` and only read. Claude gets read-only tools (file reads and search,
`git diff/log/show` in `pr/`, `gh pr view/diff`, `gh issue view`, subagents)
and a read-only GitHub token, and returns a structured report. A separate job
with write access posts it as a PR review with inline comments, and sets the
commit status `claude-runtime-review`: success without 🔴 findings, failure
with them, error when the review did not complete. A failed review's run log
names the result subtype, API status and error message. Require this status
alongside `ci-ok` in `main` branch protection so auto-merge cannot overtake
the reviewer. A newer review of the same PR cancels a running one. Re-reviews
read the previous Claude review and focus on the new commits.

The workflow authenticates through GitHub Actions OIDC and
[Anthropic Workload Identity Federation](https://platform.claude.com/docs/en/manage-claude/wif-providers/github-actions),
billed to Claude Console credits. In the Claude Console organization holding
your API credits, open **Settings → Workload identity → Connect workload →
GitHub Actions**. Create a service account and federation rule for this
reviewer, scoped to `workspace:developer` in the workspace that should pay for
inference. Set a workspace spend limit in Console to control monthly spend;
each review also stops at an estimated $5 (`max_budget_usd`). The script
refreshes the GitHub OIDC token file every four minutes, since the CLI re-reads
it whenever its access token expires. The fast host `claude-review` check runs
the script's offline regressions.

Configure the rule's match conditions as follows (the issuer is
`https://token.actions.githubusercontent.com` with discovery-mode JWKS):

```json
{
  "subject_prefix": "repo:ermacv@26526682/open-esp-radio-rs@1312931455:*",
  "audience": "https://api.anthropic.com",
  "claims": {
    "repository_id": "1312931455",
    "repository_owner_id": "26526682",
    "ref": "refs/heads/main",
    "workflow_ref": "ermacv/open-esp-radio-rs/.github/workflows/claude-review.yml@refs/heads/main"
  }
}
```

This repository uses GitHub's immutable subject format. Confirm the current
prefix with `gh api repos/ermacv/open-esp-radio-rs/actions/oidc/customization/sub`
before changing federation trust; a name-only subject does not match its JWTs.
The trailing wildcard accommodates the subject suffixes of different workflow
events; the exact `ref` and `workflow_ref` claims still restrict trust to main
and this review workflow. All three trigger events (`workflow_run`,
`pull_request_target`, `issue_comment`) run with `ref` set to `main`.

Add the resulting IDs as repository **Actions variables**, not secrets:
`ANTHROPIC_FEDERATION_RULE_ID` (`fdrl_…`), `ANTHROPIC_ORGANIZATION_ID`,
`ANTHROPIC_SERVICE_ACCOUNT_ID` (`svac_…`) and `ANTHROPIC_WORKSPACE_ID` if
the rule covers more than one workspace. No API key or Claude GitHub App is
needed. The optional variable `CLAUDE_REVIEW_MODEL` selects the model; the
default is `claude-opus-5-5` at `high` effort.

For public repositories, configure **Settings → Actions → Policies** with an
active event policy scoped only to `.github/workflows/claude-review.yml`, allowing
`pull_request_target`, `workflow_run` and `issue_comment`. Without an applicable explicit policy, GitHub's default
policy will block `pull_request_target` from November 2, 2026; see
[GitHub's event-policy documentation](https://docs.github.com/en/actions/reference/security/securely-using-pull_request_target#default-policy-for-pull_request_target).

The review is static analysis: it does not establish hardware readiness or
replace qualification/HIL evidence.

Contributors can work on portable Rust protocols, ownership tests, binary
analysis, documentation and host tools without a radio board. Hardware changes
also need the appropriate source/profile review and, when readiness is claimed,
dated applicable device evidence.

## Start with a task

Open or update an issue using the [issue management rules](docs/issues.md).
Choose its kind, area and executable-task priority before submission; the
[creation invariant](docs/issues.md#creation-and-automatic-control) applies to
forms, plugins, API calls and command-line creation.
Name one reviewable result, its defining owner and the evidence needed to
accept it. Use sub-issues for composition and dependencies for required
artifacts; neither a research result nor a merged implementation establishes
hardware readiness.

Read [From binary evidence to a Wi-Fi station](docs/binary-to-station.md), then
complete [the host tutorial](docs/first-contribution.md). For device work follow
[the ESP32-S31 route](docs/station-hardware.md). The
[capability map](qualification/README.md#everyday-status-and-next-work) connects
limitations and work candidates to their owners; it is not an aggregate
readiness verdict.

Use the toolchain in `rust-toolchain.toml`. Fetch public dependencies before
offline checks. Examples, platform, HIL, Blobray (`tools/blobray`) and
verification probes have separate workspaces and lockfiles; a root workspace
build does not cover them all.
The [repository guidelines](CLAUDE.md) and
[documentation policy](docs/documentation.md) define the contribution rules.

## Make one reviewable change

Find the defining owner and existing behavior tests. Internal libraries depend
on specific lower contracts, never on the public `oer` facade. Keep production
behavior in production crates and probes thin. Add a focused regression for a
behavioral change; verify resource return and failure paths as well as success.

For handwritten MMIO, use typed PAC capabilities. Missing fields require review
and publication in the hardware model/PAC. Recovered tables and coefficients
are allowed and may be necessary: preserve source identity, purpose,
representation and hardware/profile applicability, and verify against the real
artifact. See [source policy](docs/source-policy.md).

## Choose checks by the changed boundary

Run commands from the repository root unless the component guide says otherwise.
Changes reach `main` through pull requests. `cargo xtask push` runs the fast
gate on exactly the committed tree, pushes the branch, opens its pull request
and enables auto-merge, so the branch merges once CI passes. `cargo xtask check
changed` runs the same gate over uncommitted work, and `--full` adds what CI
checks on the pull request. The table lists what each boundary
adds when you iterate on it.

| Change | Checks |
| --- | --- |
| Markdown or catalog | `cargo xtask check docs`; for the guides in `docs/` also `mdbook build docs` (see [building the guides](docs/documentation.md#build-the-guides-and-api-documentation)) |
| Rust behavior | `cargo test -p PACKAGE FILTER --locked --offline`, confirming the selector executes tests; `cargo fmt --all -- --check` |
| Public API | `cargo xtask doc` plus focused behavior/compile tests; chip crates document their target through `[package.metadata.docs.rs]` |
| Hardware, ownership or dependency boundary | Relevant target, architecture, safety and artifact checks from [repository tooling](tools/xtask/README.md) |
| Register publication | `cargo registers generate --manifest registers/esp32s31/publication/registers.toml --check` |
| Hardware readiness claim | Applicable dated [HIL](hil/README.md) evidence and independent [qualification](qualification/README.md) |

`cargo xtask doc` is the complete API checkpoint;
the CI jobs in `.github/workflows/ci.yml` are the full source checkpoint. Run them when the
change or final checkpoint requires their coverage, not after every paragraph.
Passing a focused check is not full repository coverage. Documentation examples
must distinguish host execution, target compilation and attached-hardware runs.

## Prepare the pull request

Explain the problem, the resulting behavior and the affected ownership boundary.
List checks actually run and their scope. Link qualification/HIL evidence when
claiming hardware behavior, and call out generated SVD/PAC changes. Use a scoped
imperative subject such as `docs: explain the station contribution route` or
`fix(esp32s31): return scan owners after channel failure`.

Keep detailed contracts in one owner location and link to them. Do not commit
private binaries, dumps, credentials, run reports or new work plans. Generated
reports belong in ignored owner outputs; reviewed machine provenance remains
tracked. Preserve unrelated changes in an already-dirty checkout.
