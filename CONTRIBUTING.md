# Contributing

## Claude architectural review

The [review workflow](.github/workflows/claude-review.yml) uses the Claude
Messages API directly, authenticated through GitHub Actions OIDC and
[Anthropic Workload Identity Federation](https://platform.claude.com/docs/en/manage-claude/wif-providers/github-actions).
In the Claude Console organization holding your API credits, open **Settings
→ Workload identity → Connect workload → GitHub Actions**. Create a service
account and federation rule for this reviewer, scoped to `workspace:developer`
in the workspace that should pay for inference. Set a workspace spend limit
in Console to control monthly spend.

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
and this review workflow.

Add the resulting IDs as repository **Actions variables**, not secrets:
`ANTHROPIC_FEDERATION_RULE_ID` (`fdrl_…`), `ANTHROPIC_ORGANIZATION_ID`,
`ANTHROPIC_SERVICE_ACCOUNT_ID` (`svac_…`) and `ANTHROPIC_WORKSPACE_ID` if
the rule covers more than one workspace. The workflow grants `id-token: write`;
the controller fetches a GitHub assertion, exchanges it at `/v1/oauth/token`
and sends the temporary token as a Bearer credential. Refresh fetches a fresh
single-use assertion. No persistent Anthropic API key or Claude GitHub App is
needed. The optional variable `CLAUDE_REVIEW_MODEL` selects the model;
the default is `claude-opus-5-5`, with `high` effort. Sonnet 5.5 is available
as an explicit lower-cost choice. The default prioritizes cross-component
judgment; it is not a claim that a public architecture-specific benchmark
has established a winner.

For public repositories, configure **Settings → Actions → Policies** with an
active event policy scoped only to `.github/workflows/claude-review.yml`, allowing
`pull_request_target`, `workflow_run` and `workflow_dispatch`. Without an
applicable explicit policy, GitHub's default policy will block
`pull_request_target` from November 2, 2026. Keep the exception scoped to this
trusted controller; see [GitHub's event-policy documentation](https://docs.github.com/en/actions/reference/security/securely-using-pull_request_target#default-policy-for-pull_request_target).

Opening or updating a non-draft PR from a branch in this repository invalidates
the previous verdict and waits for its latest `CI` push run. When CI finishes,
the reviewer reads the complete changed-file list and patches, explicitly
referenced and GitHub-linked closing issues with their discussions and relationship
(closing commitment or reference-only context), and the
source files and callers it requests at immutable SHAs. The `base` source tool
revision means the merge base used by the PR's three-dot diff, so unrelated
changes already on main are not mistaken for the PR's before state.
The review checks architectural ownership, dependency direction, contracts,
data/control flow and concrete runtime regressions, with triggers, consequences,
fixes and links to source lines. Style preferences are outside this review.
Source reads return numbered slices; literal search scans one explicit source
path and declares when its result exceeds 100 matches. Neither tool executes code.
Cargo.lock patches remain in the initial diff so dependency changes are visible;
full lock-file reads are excluded from source investigation. Claude reads the
owning Cargo.toml when it needs dependency context. A lockfile finding must
anchor a line present at the correct revision in its complete provided patch;
missing or truncated patches cannot satisfy this evidence check. Findings in
ordinary source files require the anchor line read in that pass. Generated
publications are excluded from patches and full-file reads; investigate their
reviewed source inputs instead.

Each completed review or reviewer failure adds a new `github-actions[bot]`
comment. Previous reports are never edited or replaced, so findings remain
visible after fixes and subsequent pushes. Each report identifies its head/base
commits and links to the exact Actions attempt. Waiting for CI and review
progress use the pending commit status without adding intermediate comments.
A completed report explicitly says **Архитектурное ревью** and names the model.
A second model conversation verifies the candidate report
with fresh context and its own source reads, challenging both reported defects
and a clean verdict. This can reduce false positives; the two passes use the
same model and can still share blind spots.
The controller returns rejected report validation as a tool error, so the model
can read missing evidence and repair the report within the same call/token
budget. An invalid report never becomes the final verdict; exhausting the
budget still reports incomplete analysis.

Independent pre-existing defects encountered during the review appear in a
separate short summary (at most five) for human verification and possible future
issues. Each needs source evidence read at both the merge base and head, a
reachable failure scenario and a reason it is outside the PR. These findings
do not block this PR or create issues. A pre-existing defect made reachable or
worsened by the PR belongs in the blocking findings instead.

API failures, missing diffs, unavailable
issues and analysis limits cannot yield approval. The controller rechecks the
PR revisions and CI before publishing. The controller also sets the commit
status `claude-runtime-review` (its branch-protection identifier): pending while waiting/working, success only
after complete analysis and successful CI, failure for defects or coverage
gaps, and error when the reviewer fails. Require this status alongside `ci-ok`
in `main` branch protection to prevent auto-merge from overtaking the reviewer.
Enable that requirement only after this workflow is on the default branch and
federation variables are configured, so the setup PR is not blocked by a check
that cannot run yet. Source analysis
does not establish hardware readiness or replace qualification/HIL evidence.

The controller is checked out from the current trusted default branch, including
when a PR's base snapshot predates this workflow; it never
checks out or executes the PR's code. Claude can only read repository source
and return findings. Only the controller publishes the comment. Automatic
review covers same-repository branches, matching CI's push trigger. Fork PRs
need a separate CI policy before this workflow can give a merge verdict.

To repeat a review after issue requirements change, use **Actions → Claude
architectural review → Run workflow**, selecting the default branch and the PR
number. To verify federation during setup, select `verify_auth` instead; that
mode exchanges an OIDC token and makes one minimal Messages request (up to 16
output tokens) without reading or commenting on a PR. Each analysis is limited
to 20 model calls per pass, with 2,000,000 cumulative input
tokens and 48,000 output tokens shared across both passes; reaching a limit
reports incomplete analysis. The workflow has a 45-minute timeout.
The larger input budget covers repeated source investigation across large PRs;
the preflight counts each request before paid inference. Automatic prompt
caching reuses growing message prefixes within each pass. Input accounting
includes ordinary tokens, cache writes and cache reads; cached inputs are not
free and do not bypass the analysis budget. Context and linked
issues are kept complete, and logs show per-step token usage without source or
credentials. PRs whose context exceeds 1,000,000 characters require splitting or
an explicit limit change. GitHub Actions runner minutes have separate billing.

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
