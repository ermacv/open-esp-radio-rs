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
as an explicit lower-cost choice (`claude-sonnet-5-5`). Both models have explicit
prices in the controller; selecting another model requires adding its tariff
before it can run. The default prioritizes cross-component
judgment; it is not a claim that a public architecture-specific benchmark
has established a winner.

For public repositories, configure **Settings → Actions → Policies** with an
active event policy scoped only to `.github/workflows/claude-review.yml`, allowing
`pull_request_target`, `workflow_run` and `workflow_dispatch`. Without an
applicable explicit policy, GitHub's default policy will block
`pull_request_target` from November 2, 2026. Keep the exception scoped to this
trusted controller; see [GitHub's event-policy documentation](https://docs.github.com/en/actions/reference/security/securely-using-pull_request_target#default-policy-for-pull_request_target).

Only open PRs marked ready for review are eligible. Draft and closed PRs are
skipped for PR events, CI completion and manual review requests. Marking a PR
ready triggers review even when its CI already finished. The controller checks
the live PR state before each Claude request and after each inference response;
returning to draft, closing the PR or changing its head/base, title or description
stops obsolete analysis without posting a verdict. Publication also rechecks
that the PR is still open and non-draft with the same inputs.

Opening, marking ready or updating a non-draft PR from a branch in this
repository invalidates the previous verdict and waits for its latest `CI` push
run to succeed. Failed CI blocks merging without any Claude requests or review
comments; a successful CI rerun can start the analysis. Manual review requests
also require successful CI. The reviewer then reads the complete changed-file
list and patches, explicitly
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
Automatic repeats reuse the latest completed result for identical head/base
SHAs, PR and issue requirements, model and controller version, without another
paid analysis or duplicate report. The fingerprint covers the complete initial
context and controller contents; only completed statuses from
`github-actions[bot]` are accepted. The required status links to the original
run, and reuse rechecks current PR eligibility and CI. An explicit manual
request always starts a new analysis and adds a new report, including when its
inputs are unchanged. A GitHub rerun of an automatic event remains automatic.
The routing job has only read permissions. Review jobs share a per-PR concurrency
group with `queue: max` and no cancellation of the running job, so automatic and
manual requests cannot spend concurrently on the same PR; GitHub permits up to
100 pending jobs in that group.
A completed report explicitly says **Архитектурное ревью** and names the model.
A second model conversation verifies the candidate report
with a separate history and its own source reads, challenging both reported defects
and a clean verdict. This can reduce false positives; the two passes use the
same model and can still share blind spots.
Read-only tools use strict schemas; the final report uses
[JSON structured outputs](https://platform.claude.com/docs/en/build-with-claude/structured-outputs)
through `output_config.format`. Normal `end_turn` responses are parsed and
validated as reports, rather than requiring a final tool call. Claude 5.5 does
not support forced tool calls. Semantic validation failures return controller
feedback, so the model can read missing evidence and repair the report within
the same call, cost and output-token budgets. Signed thinking blocks remain
unchanged in the append-only history. An invalid report never becomes the final verdict;
exhausting the budget still reports incomplete analysis. Truncated responses,
refusals, malformed JSON and inconsistent protocol responses fail closed.

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
output tokens) without reading or commenting on a PR. A review has an estimated
**$5 API budget shared by analysis and verification**, 20 model calls per pass
and 48,000 output tokens shared across both passes. Reaching a limit reports
incomplete analysis. The workflow has a 45-minute timeout.
Each request permits at most 24,000 output tokens, capped by the remaining shared
output-token and dollar budgets. Output includes adaptive thinking as well as
the visible response;
the cap must leave room for both investigation and the report. A truncated
response never supplies findings or approval, even if it contains parseable JSON.
Before each paid request, the controller counts its input and reserves its cost
at the more expensive of the ordinary-input and cache-write rates, treating the
whole input as a cache miss. It does not assume that an earlier cache hit will
repeat. The remaining dollar budget caps that request's possible output. Actual
usage then charges ordinary input, cache writes, cache reads and output at their
separate rates; cheap repeated cache reads do not exhaust a cumulative-input
token cap. Both passes share the same cost accounting. An actual cost above the
budget cannot produce approval or start another request. The token-counting
endpoint supplies an estimate, so this local guard is not an exact provider-side
billing limit; use the workspace spend limit for provider-enforced billing control.
An explicit 5-minute prompt-cache breakpoint covers the identical tools, system instructions and
initial PR context shared by both passes. Their different tasks and the candidate
report follow that breakpoint; the verifier never receives the analyst's
conversation history. Automatic caching additionally reuses growing prefixes
within each pass. Cache hits depend on the prefix remaining unchanged and its
TTL; neither a verifier hit nor reuse across separate workflow runs is guaranteed.
Cached inputs are not free and remain part of the dollar budget. Context and linked
issues are kept complete. JSON log records separate ordinary input, cache writes,
cache reads, output and estimated USD for each step and each pass's total, even
when analysis fails, without logging source or credentials. Response metadata
records the stop reason, content-block types and request/output token limits,
without text, thinking, signatures or tool arguments. Completed and failed review
comments include the same usage, cost and budget breakdown when the reviewer was
initialized. Only responses with available usage are counted; a request without
a received response can incur costs not reflected in the table. Estimates use
the direct API tariffs dated in the controller and default 5-minute cache writes; final
billing is in Claude Console. The $5 budget applies to one review run; an explicit
manual repeat has its own budget, while reuse of a completed verdict makes no API calls.
PRs whose context exceeds 1,000,000 characters require splitting or an explicit
limit change. GitHub Actions runner minutes have separate billing.

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
