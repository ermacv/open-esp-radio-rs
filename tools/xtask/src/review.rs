//! The local Claude review of this branch before it is pushed.
//!
//! A review finding is cheapest before the push: once pushed, its fix costs
//! a full CI round, and the Claude review in CI
//! (`.github/workflows/claude-review.yml`) only audits each pull request once
//! within the API budget. This review runs the same instructions (root
//! `CLAUDE.md` and `REVIEW.md`, read from `origin/main` as CI reads them from
//! main) and report schema in a separate, headless Claude Code process
//! (`claude -p`) on the committed branch, with
//! read-only tools and no repository settings, hooks or MCP servers, so the
//! session that wrote the change never reviews it in its own context.
//!
//! A verdict is kept per diff: its identity is the verbatim patch id of the
//! diff between the merge base with `origin/main` and `HEAD`, so a rebase
//! that changes nothing in the diff, or a reworded commit, keeps it. Which
//! review a push gets, the full one or a fix round's, is [`round`]'s
//! decision; each is handed the findings of the branch's previous verdict,
//! and a round takes at most [`PASSES`] passes. 🔴 important and 🟡 nit
//! findings block the push, a
//! nit being any real defect, text that misstates behavior included; 🔵
//! wording findings, about the phrasing, naming or clarity of correct text,
//! do not: `cargo xtask push` files them as `review: …` issues of the branch
//! once its pull request exists ([`file_findings`]), as the CI review files
//! its findings. 🟣 pre-existing ones are only printed. This is the only
//! blocking review: the one in CI reviews a pull request once and files its
//! findings as issues.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use oer_process::{self as process, Checkout, git};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Result, push};

pub mod round;

use round::{Mode, Plan};

/// The file in the review's checkout that holds a fix round's diff.
const ROUND_DIFF: &str = "review-round.diff";

/// The model unless `OER_REVIEW_MODEL` names another; the CI review's default.
const MODEL: &str = "claude-opus-5-5";
/// What one review may spend, as in CI.
const MAX_BUDGET_USD: &str = "5";
/// The CI review job's limit.
const TIMEOUT: Duration = Duration::from_secs(45 * 60);
const TOOLS: &str = "Read,Grep,Glob,Agent,Bash";
const ALLOWED_TOOLS: &[&str] = &[
    "Read",
    "Grep",
    "Glob",
    "Agent",
    "Bash(git diff *)",
    "Bash(git log *)",
    "Bash(git show *)",
    // A pull request's inventory with proof names a search the reviewer
    // reruns (docs/issues.md, review and readiness), with Grep or these
    // listings. Never `git grep`: its `-O` runs a program.
    "Bash(git ls-files *)",
    "Bash(git ls-tree *)",
    "Bash(gh pr view *)",
    "Bash(gh pr diff *)",
    "Bash(gh issue view *)",
];

/// How the local review differs from the CI review `REVIEW.md` describes.
const LOCAL: &str = "\
# Local review

This is the local review before the branch is pushed, not the CI review.
The branch is checked out at the repository root, not in `pr/`: run `git`
without `-C pr` and read files from the root. There is no `review-inputs/`:
diff against the merge base named in the prompt and read the PR and its
issues, if any, with `gh`. Bash runs only `git diff …`, `git log …`,
`git show …`, `git ls-files …`, `git ls-tree …`, `gh pr view …`,
`gh pr diff …` and `gh issue view …`, without
`cd`, pipes or redirections; any other call is denied and fails the review.
Rerun a pull request's inventory search with the Grep and Glob tools.
In a fix round the prompt names `review-round.diff` at the root: the diff
since the previous pushed head, which you read with Read; it is not part
of the branch. CI has not run; the push gate's fast checks (tidy,
formatting, Clippy and tests of the changed packages) passed. The branch may
have no pull request yet; then no earlier review exists on GitHub, and the
prompt lists the findings of the previous local review, if any. The prompt
lists no filed issues: set `issue` to 0 for every finding. `cargo xtask push`
files the wording findings as issues after this review: report a wording
finding the previous review listed with the same path and title, so it is
not filed twice.";

/// What each severity means, in the report schema: the line between a nit and
/// wording is whether the text is correct.
pub const SEVERITY: &str = "important: a reachable failure or a violated contract, with \
     its trigger. nit: a real but minor defect the change introduces or touches, text that \
     misstates behavior included (documentation, comments, messages, names that say \
     something false). wording: only the phrasing, naming or clarity of text that is \
     correct; text that misstates behavior is never wording. pre-existing: a defect \
     outside the change. An important or nit finding blocks the pull request's \
     readiness; a wording one rides along or is filed.";

/// The script that files review findings as issues, for the CI review and
/// for the local one's wording findings and, past the pass cap, its blocking
/// ones.
const FILE_FINDINGS: &str = ".github/scripts/review_findings.py";

/// One finding, as the CI review's schema reports it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Finding {
    pub severity: String,
    pub path: String,
    pub line: u64,
    pub title: String,
    pub body: String,
    #[serde(flatten)]
    pub rest: serde_json::Map<String, Value>,
}

impl Finding {
    /// A finding that holds the push: a defect, never only wording.
    pub fn blocking(&self) -> bool {
        matches!(self.severity.as_str(), "important" | "nit")
    }

    /// A finding about the phrasing, naming or clarity of correct text: it is
    /// filed as an issue instead of holding the push.
    pub fn wording(&self) -> bool {
        self.severity == "wording"
    }

    fn mark(&self) -> &'static str {
        match self.severity.as_str() {
            "important" => "🔴",
            "nit" => "🟡",
            "wording" => "🔵",
            _ => "🟣",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Report {
    pub summary: String,
    pub findings: Vec<Finding>,
}

/// A stored verdict on one diff of a branch.
#[derive(Debug, Deserialize, Serialize)]
pub struct Verdict {
    pub branch: String,
    pub head: String,
    pub base: String,
    pub identity: String,
    pub report: Report,
    pub cost_usd: f64,
    /// The review it was ([`round`]) and why.
    #[serde(default = "full")]
    pub mode: Mode,
    #[serde(default)]
    pub reason: String,
    /// How long the review ran, as `claude -p` reports it.
    #[serde(default)]
    pub seconds: f64,
    /// The branch's head on `origin` when it ran, `None` before its first
    /// push: the round its pass counts toward ([`PASSES`]).
    #[serde(default)]
    pub pushed: Option<String>,
    /// A push went through with it past the pass cap: its findings are
    /// filed, so a retried or rebase-only push that reuses it proceeds too.
    #[serde(default)]
    pub capped: bool,
}

/// Local review passes a round may take: when the last one still blocks, the
/// push proceeds without it and files its findings.
pub const PASSES: usize = 2;

fn full() -> Mode {
    Mode::Full
}

impl Verdict {
    pub fn blocking(&self) -> usize {
        self.report.findings.iter().filter(|f| f.blocking()).count()
    }

    /// The report `review_findings.py` files for this verdict: its wording
    /// findings, or none when it has none.
    pub fn wording(&self) -> Option<Report> {
        let findings: Vec<Finding> = self
            .report
            .findings
            .iter()
            .filter(|finding| finding.wording())
            .cloned()
            .collect();
        (!findings.is_empty()).then(|| Report {
            summary: self.report.summary.clone(),
            findings,
        })
    }

    /// The report that files its blocking findings, for a push the pass cap
    /// let through ([`PASSES`]).
    pub fn blocking_report(&self) -> Report {
        Report {
            summary: self.report.summary.clone(),
            findings: self
                .report
                .findings
                .iter()
                .filter(|finding| finding.blocking())
                .cloned()
                .collect(),
        }
    }
}

/// The report schema of `.github/scripts/claude_review.py`, with the areas and
/// priorities of the label catalog.
pub fn schema(labels: &str) -> Result<Value> {
    let labels: Vec<Value> = serde_json::from_str(labels)?;
    let names = |prefix: &str| -> Vec<String> {
        labels
            .iter()
            .filter_map(|label| label["name"].as_str())
            .filter(|name| name.starts_with(prefix))
            .map(str::to_owned)
            .collect()
    };
    Ok(json!({
        "type": "object", "additionalProperties": false, "required": ["summary", "findings"],
        "properties": {
            "summary": {"type": "string"},
            "findings": {"type": "array", "items": {
                "type": "object", "additionalProperties": false,
                "required": ["severity", "path", "line", "title", "body", "area", "priority", "issue"],
                "properties": {
                    "severity": {"enum": ["important", "nit", "wording", "pre-existing"],
                                 "description": SEVERITY},
                    "path": {"type": "string"},
                    "line": {"type": "integer", "minimum": 1},
                    "title": {"type": "string"},
                    "body": {"type": "string"},
                    "area": {"enum": names("area:")},
                    "priority": {"enum": names("priority:")},
                    "issue": {"type": "integer", "minimum": 0}}}}}
    }))
}

/// The prompt for `branch` at `head` against `base` under `plan`, with the
/// findings the branch's previous verdict reported.
pub fn prompt(
    branch: &str,
    head: &str,
    base: &str,
    plan: &Plan,
    previous: Option<&Verdict>,
) -> String {
    let mut prompt = match plan.mode {
        Mode::Full => format!(
            "Review branch {branch} at head {head}, checked out at the repository root, \
             against its merge base {base} with origin/main (`git diff {base}...HEAD`), \
             following the review instructions. This is a full review: {}.",
            plan.reason
        ),
        Mode::Round => format!(
            "Review the fix round of branch {branch} at head {head}, checked out at the \
             repository root, following the review instructions: {}. Review the round's \
             diff, in {ROUND_DIFF} (from the previous pushed head {} to HEAD), check whether \
             each finding of the previous review below is fixed, and check end to end every owner decision \
             the pull request implements, which its body links (`gh pr view {branch}`). The \
             diff against the merge base {base} (`git diff {base}...HEAD`) is the context the \
             round lands in.",
            plan.reason,
            plan.pushed.as_deref().unwrap_or_default()
        ),
    };
    if let Some(verdict) = previous.filter(|verdict| !verdict.report.findings.is_empty()) {
        prompt.push_str(&format!(
            "\n\nThe previous local review of this branch, at head {}, reported the \
             findings below. Report every one still open again, and review the whole round, \
             not only the fixes: a fix often leaves the same defect elsewhere or brings a new \
             one.\n",
            verdict.head
        ));
        for finding in &verdict.report.findings {
            prompt.push_str(&format!(
                "\n- {} {} ({}:{}): {}",
                finding.severity, finding.title, finding.path, finding.line, finding.body
            ));
        }
    }
    prompt
}

/// The line of a file at the new side of a `git diff -U0` (`diff`) that
/// stands for `line` at its old side: shifted by the hunks before it, or the
/// first line of the hunk that changed it.
pub fn moved_line(diff: &str, line: u64) -> u64 {
    let range = |text: &str| -> Option<(u64, u64)> {
        let (start, length) = text.split_once(',').unwrap_or((text, "1"));
        Some((start.parse().ok()?, length.parse().ok()?))
    };
    let mut moved = line;
    for header in diff.lines().filter(|text| text.starts_with("@@ ")) {
        let mut fields = header.split_whitespace().skip(1);
        let (Some(Some((old, old_length))), Some(Some((new, new_length)))) = (
            fields
                .next()
                .and_then(|text| text.strip_prefix('-'))
                .map(range),
            fields
                .next()
                .and_then(|text| text.strip_prefix('+'))
                .map(range),
        ) else {
            continue;
        };
        // An empty side names the line before the hunk.
        let old_after = if old_length == 0 {
            old + 1
        } else {
            old + old_length
        };
        let new_after = if new_length == 0 {
            new + 1
        } else {
            new + new_length
        };
        if line >= old_after {
            moved = line + new_after - old_after;
        } else {
            if old_length > 0 && line >= old {
                moved = new.max(1);
            }
            break;
        }
    }
    moved
}

/// The environment `review_findings.py` files the findings of `report`
/// with, as issues of pull request `pr` on `branch` linking the pushed
/// commit `head`, or none when it has none. A verdict reused after a rebase
/// reviewed another commit, which may never have been pushed: `line` gives
/// each finding's line at `head`.
pub fn filing(
    report: Option<Report>,
    pr: &str,
    branch: &str,
    head: &str,
    mut line: impl FnMut(&Finding) -> Result<u64>,
) -> Result<Option<Vec<(&'static str, String)>>> {
    let Some(mut report) = report.filter(|report| !report.findings.is_empty()) else {
        return Ok(None);
    };
    for finding in &mut report.findings {
        finding.line = line(finding)?;
    }
    Ok(Some(vec![
        ("RESULT", serde_json::to_string(&report)?),
        ("PR", pr.to_owned()),
        ("HEAD", head.to_owned()),
        ("BRANCH", branch.to_owned()),
    ]))
}

/// File the findings of `report`, from `verdict`, ([`filing`]) through
/// `review_findings.py`, linking the pushed commit `head`, which validates
/// their labels and skips a finding already filed; returns its line per
/// finding.
pub fn file_findings(
    ctx: &Checkout,
    verdict: &Verdict,
    report: Option<Report>,
    pr: &str,
    branch: &str,
    head: &str,
) -> Result<Vec<String>> {
    let line = |finding: &Finding| -> Result<u64> {
        if verdict.head == head {
            return Ok(finding.line);
        }
        let diff = git::output(
            &ctx.root,
            [
                "diff",
                "-U0",
                "--no-ext-diff",
                &verdict.head,
                head,
                "--",
                &finding.path,
            ],
        )?;
        Ok(moved_line(&String::from_utf8_lossy(&diff), finding.line))
    };
    let Some(filing) = filing(report, pr, branch, head, line)? else {
        return Ok(Vec::new());
    };
    let token = gh(ctx, &["auth", "token"])?;
    let repository = gh(
        ctx,
        &[
            "repo",
            "view",
            "--json",
            "nameWithOwner",
            "--jq",
            ".nameWithOwner",
        ],
    )?;
    let mut command = process::command("python3");
    command
        .current_dir(&ctx.root)
        .arg(FILE_FINDINGS)
        .envs(filing)
        .env("GH_TOKEN", token)
        .env("GH_REPO", repository)
        .env("PYTHONDONTWRITEBYTECODE", "1");
    let output = process::capture(&mut command)?;
    Ok(String::from_utf8(output.stdout)?
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

/// The output of the GitHub CLI, whose login files the issues.
fn gh(ctx: &Checkout, arguments: &[&str]) -> Result<String> {
    let output = process::capture(ctx.command("gh").args(arguments))?;
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

/// The text a terminal shows for `verdict`.
pub fn render(verdict: &Verdict) -> String {
    let mut text = format!("review: {}\n", verdict.report.summary.trim());
    for finding in &verdict.report.findings {
        text.push_str(&format!(
            "\n{} {} ({}:{})\n  {}\n",
            finding.mark(),
            finding.title,
            finding.path,
            finding.line,
            finding.body.trim().replace('\n', "\n  ")
        ));
    }
    text
}

fn directory(ctx: &Checkout) -> PathBuf {
    ctx.root.join("target/xtask/review")
}

/// The patch id of the diff between `base` and `head`: independent of line
/// numbers and commit messages, but not of whitespace (`--verbatim`), since
/// a fix may change only whitespace.
pub fn identity(root: &Path, base: &str, head: &str) -> Result<String> {
    let diff = git::output(root, ["diff", "--binary", "--no-ext-diff", base, head])?;
    let mut file = tempfile::NamedTempFile::new()?;
    std::io::Write::write_all(&mut file, &diff)?;
    let output = process::command("git")
        .current_dir(root)
        .args(["patch-id", "--verbatim"])
        .stdin(fs::File::open(file.path())?)
        .output()?;
    if !output.status.success() {
        return Err(format!("review: git patch-id failed with {}", output.status).into());
    }
    let text = String::from_utf8(output.stdout)?;
    text.split_whitespace()
        .next()
        .map(str::to_owned)
        .ok_or_else(|| "review: the branch changes nothing against origin/main".into())
}

fn load(path: &Path) -> Option<Verdict> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// Run `claude -p` in `tree`, the reviewed commit's checkout, and return its
/// report and cost.
fn claude(ctx: &Checkout, tree: &Path, prompt: &str) -> Result<(Report, f64, f64)> {
    // The instructions and label catalog of `origin/main`, as CI reads them
    // from trusted main: the reviewed branch cannot relax its own review.
    let main = |name: &str| git::text(&ctx.root, ["show", &format!("origin/main:{name}")]);
    let guidance = format!(
        "{}\n\n{}\n\n{LOCAL}",
        main("CLAUDE.md")?,
        main("REVIEW.md")?
    );
    let schema = schema(&main(".github/labels.json")?)?;
    let model = std::env::var("OER_REVIEW_MODEL").unwrap_or_else(|_| MODEL.to_owned());
    let mut command = process::command("claude");
    command
        .current_dir(tree)
        .args(["-p", prompt])
        .args(["--model", &model, "--effort", "high"])
        .args(["--max-budget-usd", MAX_BUDGET_USD])
        .args(["--tools", TOOLS])
        .args(["--allowedTools", &ALLOWED_TOOLS.join(",")])
        .args(["--permission-mode", "dontAsk"])
        // No repository settings, hooks, agents or MCP servers: guidance
        // comes only from the files appended below, as in CI.
        .args(["--setting-sources", ""])
        .arg("--strict-mcp-config")
        .arg("--no-session-persistence")
        .args(["--output-format", "json"])
        .args(["--json-schema", &schema.to_string()])
        .args(["--append-system-prompt", &guidance]);
    let output = process::output(&mut command, Some(TIMEOUT))
        .map_err(|error| format!("review: could not run `claude` (Claude Code CLI): {error}"))?;
    let result: Value = serde_json::from_slice(&output.stdout).map_err(|_| {
        format!(
            "review: claude ended with {} and no result:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )
    })?;
    report(&result)
}

/// The report and cost of a `claude -p` JSON result: none for an error, a
/// missing report or any tool call the `dontAsk` policy denied, since a
/// report written past a denial may rest on evidence the review never saw.
fn report(result: &Value) -> Result<(Report, f64, f64)> {
    if result["is_error"].as_bool() != Some(false) || result["structured_output"].is_null() {
        return Err(format!(
            "review: claude produced no report ({}): {}",
            result["subtype"].as_str().unwrap_or("unknown"),
            result["result"].as_str().unwrap_or("no message")
        )
        .into());
    }
    if let Some(denials) = result["permission_denials"]
        .as_array()
        .filter(|denials| !denials.is_empty())
    {
        let denied: Vec<String> = denials
            .iter()
            .map(|denial| {
                let input = &denial["tool_input"];
                format!(
                    "{}: {}",
                    denial["tool_name"].as_str().unwrap_or("tool"),
                    input["command"]
                        .as_str()
                        .map_or_else(|| input.to_string(), str::to_owned)
                )
            })
            .collect();
        return Err(format!("review: tool calls were denied: {}", denied.join("; ")).into());
    }
    let report = serde_json::from_value(result["structured_output"].clone())?;
    Ok((
        report,
        result["total_cost_usd"].as_f64().unwrap_or(0.0),
        result["duration_ms"].as_f64().unwrap_or(0.0) / 1000.0,
    ))
}

/// The head of `branch` on `origin`, `None` before its first push.
pub fn pushed_head(ctx: &Checkout, branch: &str) -> Result<Option<String>> {
    Ok(git::text(
        &ctx.root,
        ["ls-remote", "origin", &format!("refs/heads/{branch}")],
    )?
    .split_whitespace()
    .next()
    .map(str::to_owned))
}

/// The stored verdicts of `branch`, oldest first.
fn verdicts(ctx: &Checkout, branch: &str) -> Vec<Verdict> {
    let Ok(entries) = fs::read_dir(directory(ctx)) else {
        return Vec::new();
    };
    let mut verdicts: Vec<(std::time::SystemTime, Verdict)> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, load(&entry.path())?))
        })
        .filter(|(_, verdict)| verdict.branch == branch)
        .collect();
    verdicts.sort_by_key(|(modified, _)| *modified);
    verdicts.into_iter().map(|(_, verdict)| verdict).collect()
}

/// The local review passes of a branch's round: the pushed head it runs
/// under and how many reviews ran there. After a push, a rebase keeps the
/// pushed head and so the round. Before the first push the round is also
/// bound to its merge base: a branch name reused by a later pull request
/// starts on another base, and so a new round.
#[derive(Debug, Default, Deserialize, Serialize)]
struct Round {
    pushed: Option<String>,
    /// The merge base of a round before the first push: a later pull
    /// request under the same branch name starts on another.
    #[serde(default)]
    base: Option<String>,
    passes: usize,
}

fn round_file(ctx: &Checkout, branch: &str) -> PathBuf {
    directory(ctx).join(format!("round-{}.json", branch.replace('/', "--")))
}

/// The round of `branch` under the pushed head `pushed`: the stored one
/// when it runs under that head (and, before the first push, on the merge
/// base `base`), else a new one. A rebase before the first push starts a
/// new round, which only allows a pass more.
fn round(ctx: &Checkout, branch: &str, pushed: Option<&str>, base: &str) -> Round {
    let base = pushed.is_none().then(|| base.to_owned());
    fs::read_to_string(round_file(ctx, branch))
        .ok()
        .and_then(|text| serde_json::from_str::<Round>(&text).ok())
        .filter(|round| round.pushed.as_deref() == pushed && round.base == base)
        .unwrap_or(Round {
            pushed: pushed.map(str::to_owned),
            base,
            passes: 0,
        })
}

/// Count one more review pass of `branch`'s round under `pushed`.
fn count_pass(ctx: &Checkout, branch: &str, pushed: Option<&str>, base: &str) -> Result<()> {
    let mut round = round(ctx, branch, pushed, base);
    round.passes += 1;
    fs::create_dir_all(directory(ctx))?;
    oer_durable::atomic_write(
        &round_file(ctx, branch),
        serde_json::to_string(&round)?.as_bytes(),
    )?;
    Ok(())
}

/// Whether the round of `branch` since its pushed head `pushed` used its
/// last local review pass ([`PASSES`]).
pub fn capped(ctx: &Checkout, branch: &str, pushed: Option<&str>, base: &str) -> bool {
    round(ctx, branch, pushed, base).passes >= PASSES
}

/// Mark the stored verdict at `path` as one a push went through with past
/// the pass cap.
pub fn mark_capped(path: &Path, verdict: &mut Verdict) -> Result<()> {
    verdict.capped = true;
    oer_durable::atomic_write(path, serde_json::to_string_pretty(verdict)?.as_bytes())?;
    Ok(())
}

/// The verdict on `branch`'s diff from `base` to the commit `head`: the
/// stored one when the diff was reviewed already, otherwise a new review in
/// the mode [`round::plan`] decides, handed the previous verdict's findings.
pub fn verdict(ctx: &Checkout, branch: &str, base: &str, head: &str) -> Result<(Verdict, PathBuf)> {
    let identity = identity(&ctx.root, base, head)?;
    let path = directory(ctx).join(format!("{identity}.json"));
    if let Some(verdict) = load(&path) {
        println!(
            "review: this diff was reviewed at {}; reusing that verdict",
            &verdict.head[..12]
        );
        return Ok((verdict, path));
    }
    let pushed = pushed_head(ctx, branch)?;
    if pushed.is_some() {
        // The pushed head of another checkout of the branch; without it the
        // review is full.
        let _ = git::output(
            &ctx.root,
            [
                "fetch",
                "--quiet",
                "origin",
                &format!("refs/heads/{branch}"),
            ],
        );
    }
    let previous = verdicts(ctx, branch).pop();
    let plan = round::plan(
        &ctx.root,
        base,
        head,
        pushed.as_deref(),
        &crate::checks::shared::owners(ctx)?,
    )?;
    println!(
        "review: reviewing {branch} at {} with a separate Claude Code run: {} review, {}{}",
        &head[..12],
        match plan.mode {
            Mode::Full => "full",
            Mode::Round => "fix-round",
        },
        plan.reason,
        if previous.is_some() {
            "; it checks the previous local findings"
        } else {
            ""
        }
    );
    // Claude reads a detached checkout of `head`, never the working tree, which
    // the session keeps editing while the review runs.
    let tree = git::Worktree::detached(
        &ctx.root,
        &directory(ctx).join(format!("tree-{identity}")),
        head,
    )?;
    if let Some(diff) = &plan.diff {
        fs::write(tree.path().join(ROUND_DIFF), diff)?;
    }
    let reviewed = claude(
        ctx,
        tree.path(),
        &prompt(branch, head, base, &plan, previous.as_ref()),
    );
    tree.remove()?;
    let (report, cost_usd, seconds) = reviewed?;
    let verdict = Verdict {
        branch: branch.to_owned(),
        head: head.to_owned(),
        base: base.to_owned(),
        identity,
        report,
        cost_usd,
        mode: plan.mode,
        reason: plan.reason,
        seconds,
        pushed,
        capped: false,
    };
    println!(
        "review: {} review took {seconds:.0} s, ${cost_usd:.2}",
        match verdict.mode {
            Mode::Full => "the full",
            Mode::Round => "the fix-round",
        }
    );
    fs::create_dir_all(directory(ctx))?;
    fs::write(&path, serde_json::to_string_pretty(&verdict)?)?;
    count_pass(ctx, branch, verdict.pushed.as_deref(), base)?;
    Ok((verdict, path))
}

/// `cargo xtask review`: print the verdict and fail on a blocking finding.
pub fn run(ctx: &Checkout) -> Result<()> {
    let tracked: Vec<String> =
        git::lines(&ctx.root, ["status", "--porcelain", "--untracked-files=no"])?
            .iter()
            .map(|line| line.get(3..).unwrap_or(line).to_owned())
            .collect();
    let untracked = git::lines(&ctx.root, ["ls-files", "--others", "--exclude-standard"])?;
    if let Some(reason) = push::unpushable(&tracked, &untracked) {
        return Err(format!("review: {reason}").into());
    }
    let branch = git::text(&ctx.root, ["rev-parse", "--abbrev-ref", "HEAD"])?;
    if let Some(reason) = push::unreviewable(&branch) {
        return Err(format!("review: {reason}").into());
    }
    git::output(&ctx.root, ["fetch", "--quiet", "origin", "main"])?;
    let base = git::text(&ctx.root, ["merge-base", "HEAD", "origin/main"])?;
    let head = git::text(&ctx.root, ["rev-parse", "HEAD"])?;
    let (verdict, path) = verdict(ctx, &branch, &base, &head)?;
    print!("{}", render(&verdict));
    match verdict.blocking() {
        0 => {
            println!("review: no blocking findings");
            if let Some(wording) = verdict.wording() {
                println!(
                    "review: {} wording finding(s) do not block; `cargo xtask push` files \
                     them as issues",
                    wording.findings.len()
                );
            }
            Ok(())
        }
        count => Err(format!(
            "review: {count} blocking finding(s); fix them and run it again (report: {})",
            path.display()
        )
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(severity: &str) -> Finding {
        Finding {
            severity: severity.to_owned(),
            path: String::from("crates/a/src/lib.rs"),
            line: 7,
            title: String::from("Lost error"),
            body: String::from("The error is dropped."),
            rest: serde_json::Map::new(),
        }
    }

    fn verdict(findings: Vec<Finding>) -> Verdict {
        Verdict {
            branch: String::from("fix/a"),
            head: String::from("0123456789abcdef"),
            base: String::from("fedcba9876543210"),
            identity: String::from("id"),
            report: Report {
                summary: String::from("Checked the error path."),
                findings,
            },
            cost_usd: 0.0,
            mode: Mode::Full,
            reason: String::new(),
            seconds: 0.0,
            pushed: None,
            capped: false,
        }
    }

    /// A pull request's inventory with proof names a search the reviewer
    /// reruns, so the read-only Git listings are allowed and named, and
    /// `git grep`, whose `-O` runs a program, is not.
    #[test]
    fn the_review_can_rerun_an_inventorys_search() {
        for command in ["git ls-files", "git ls-tree"] {
            assert!(ALLOWED_TOOLS.contains(&format!("Bash({command} *)").as_str()));
            assert!(LOCAL.contains(&format!("`{command} …`")), "{command}");
        }
        assert!(!ALLOWED_TOOLS.iter().any(|tool| tool.contains("git grep")));
        assert!(LOCAL.contains("Grep and Glob"));
    }

    #[test]
    fn a_denied_tool_call_fails_the_review() {
        let mut result = serde_json::json!({
            "is_error": false,
            "total_cost_usd": 0.5,
            "structured_output": {"summary": "Not verified.", "findings": []},
            "permission_denials": [{
                "tool_name": "Bash",
                "tool_use_id": "t1",
                "tool_input": {"command": "cd pr && git diff"}
            }]
        });
        let error = report(&result).unwrap_err().to_string();
        assert_eq!(
            error,
            "review: tool calls were denied: Bash: cd pr && git diff"
        );
        result["permission_denials"] = serde_json::json!([]);
        result["duration_ms"] = serde_json::json!(96_000);
        let (report, cost, seconds) = report(&result).unwrap();
        assert_eq!(report.summary, "Not verified.");
        assert_eq!(cost, 0.5);
        assert_eq!(seconds, 96.0);
    }

    #[test]
    fn important_and_nit_findings_block_and_pre_existing_ones_do_not() {
        let verdict = verdict(vec![
            finding("important"),
            finding("nit"),
            finding("pre-existing"),
        ]);
        assert_eq!(verdict.blocking(), 2);
        let text = render(&verdict);
        assert!(
            text.contains("🔴 Lost error (crates/a/src/lib.rs:7)"),
            "{text}"
        );
        assert!(text.contains("🟣"), "{text}");
    }

    #[test]
    fn a_wording_only_report_passes_and_its_findings_are_filed() {
        let wording = verdict(vec![finding("wording"), finding("pre-existing")]);
        assert_eq!(wording.blocking(), 0);
        let filed = wording.wording().unwrap();
        assert_eq!(filed.findings, vec![finding("wording")]);
        assert_eq!(filed.summary, wording.report.summary);
        assert!(render(&wording).contains("🔵 Lost error"));
        // Nothing to file without a wording finding.
        assert_eq!(verdict(vec![finding("pre-existing")]).wording(), None);
    }

    #[test]
    fn wording_findings_link_the_pushed_commit_at_their_moved_lines() {
        // A verdict reused after a rebase reviewed another commit, which may
        // never have been pushed: the link names the pushed one, at the line
        // the finding moved to.
        let environment = filing(
            verdict(vec![finding("wording")]).wording(),
            "715",
            "fix/b",
            "pushed",
            |finding| Ok(finding.line + 3),
        )
        .unwrap()
        .unwrap();
        let value = |name: &str| {
            environment
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.clone())
                .unwrap()
        };
        assert_eq!(value("HEAD"), "pushed");
        assert_eq!(value("PR"), "715");
        assert_eq!(value("BRANCH"), "fix/b");
        let filed: Report = serde_json::from_str(&value("RESULT")).unwrap();
        assert_eq!(filed.findings.len(), 1);
        assert_eq!(filed.findings[0].line, 10);
        assert_eq!(
            filing(
                verdict(vec![finding("nit")]).wording(),
                "715",
                "fix/b",
                "pushed",
                |finding| Ok(finding.line)
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn a_line_moves_with_the_hunks_before_it() {
        // Three lines inserted after line 2, line 5 rewritten into two, line
        // 9 deleted.
        let diff = "diff --git a/f b/f\n--- a/f\n+++ b/f\n\
                    @@ -2,0 +3,3 @@\n+a\n+b\n+c\n\
                    @@ -5 +9,2 @@\n-x\n+y\n+z\n\
                    @@ -9 +13,0 @@\n-gone\n";
        assert_eq!(moved_line(diff, 1), 1);
        assert_eq!(moved_line(diff, 2), 2);
        assert_eq!(moved_line(diff, 3), 6);
        assert_eq!(moved_line(diff, 5), 9);
        assert_eq!(moved_line(diff, 6), 11);
        assert_eq!(moved_line(diff, 9), 13);
        assert_eq!(moved_line(diff, 10), 14);
        assert_eq!(moved_line("", 7), 7);
    }

    #[test]
    fn a_rebased_line_is_found_in_the_pushed_commit() {
        let repository = tempfile::tempdir().unwrap();
        let root = repository.path();
        let git = |arguments: &[&str]| {
            git::text(
                root,
                [
                    &[
                        "-c",
                        "user.name=t",
                        "-c",
                        "user.email=t@t",
                        "-c",
                        "commit.gpgsign=false",
                    ],
                    arguments,
                ]
                .concat(),
            )
            .unwrap()
        };
        git(&["init", "--quiet"]);
        fs::write(root.join("f.txt"), "one\ntwo\nreviewed\n").unwrap();
        git(&["add", "f.txt"]);
        git(&["commit", "--quiet", "-m", "reviewed"]);
        let reviewed = git(&["rev-parse", "HEAD"]);
        fs::write(root.join("f.txt"), "zero\nhalf\none\ntwo\nreviewed\n").unwrap();
        git(&["commit", "--quiet", "-am", "rebased"]);
        let diff = git::output(root, ["diff", "-U0", &reviewed, "HEAD", "--", "f.txt"]).unwrap();
        assert_eq!(moved_line(&String::from_utf8_lossy(&diff), 3), 5);
    }

    #[test]
    fn a_nit_beside_wording_still_blocks() {
        let verdict = verdict(vec![finding("wording"), finding("nit")]);
        assert_eq!(verdict.blocking(), 1);
        assert_eq!(verdict.wording().unwrap().findings.len(), 1);
    }

    #[test]
    fn text_that_misstates_behavior_is_never_wording() {
        let schema = schema("[]").unwrap();
        let severity = &schema["properties"]["findings"]["items"]["properties"]["severity"];
        assert_eq!(
            severity["enum"],
            json!(["important", "nit", "wording", "pre-existing"])
        );
        let description = severity["description"].as_str().unwrap();
        assert!(
            description.contains("nit: a real but minor defect")
                && description.contains("text that misstates behavior included"),
            "{description}"
        );
        assert!(
            description.contains("text that misstates behavior is never wording"),
            "{description}"
        );
    }

    #[test]
    fn the_schema_takes_areas_and_priorities_from_the_label_catalog() {
        let labels = r#"[{"name":"kind:bug"},{"name":"area:hil"},{"name":"priority:p1"}]"#;
        let schema = schema(labels).unwrap();
        let item = &schema["properties"]["findings"]["items"]["properties"];
        assert_eq!(item["area"]["enum"], json!(["area:hil"]));
        assert_eq!(item["priority"]["enum"], json!(["priority:p1"]));
    }

    fn plan(mode: Mode) -> Plan {
        Plan {
            mode,
            reason: String::from("the reason"),
            pushed: (mode == Mode::Round).then(|| String::from("pushed")),
            diff: None,
        }
    }

    #[test]
    fn a_first_review_is_full_and_a_fix_round_reviews_its_diff_and_the_previous_findings() {
        let first = prompt("fix/a", "h", "b", &plan(Mode::Full), None);
        assert!(first.contains("git diff b...HEAD") && first.contains("full review: the reason"));
        assert!(!first.contains("previous"), "{first}");
        let previous = verdict(vec![finding("nit")]);
        let round = prompt("fix/a", "h", "b", &plan(Mode::Round), Some(&previous));
        assert!(
            round.contains(ROUND_DIFF) && round.contains("previous pushed head pushed"),
            "{round}"
        );
        assert!(round.contains("every owner decision"), "{round}");
        assert!(
            round.contains("nit Lost error (crates/a/src/lib.rs:7)"),
            "{round}"
        );
        // A full review gets the previous findings too.
        assert!(
            prompt("fix/a", "h", "b", &plan(Mode::Full), Some(&previous))
                .contains("nit Lost error")
        );
    }

    /// The second pass under one pushed head caps its round; a new pushed
    /// head, or a reused branch name's, starts a new round.
    #[test]
    fn the_second_pass_under_one_pushed_head_caps_its_round() {
        let directory = tempfile::tempdir().unwrap();
        let ctx = Checkout {
            root: directory.path().to_owned(),
        };
        count_pass(&ctx, "fix/a", None, "b").unwrap();
        assert!(!capped(&ctx, "fix/a", None, "b"));
        count_pass(&ctx, "fix/a", None, "b").unwrap();
        assert!(capped(&ctx, "fix/a", None, "b"));
        // A later pull request under the same name starts on another base.
        assert!(!capped(&ctx, "fix/a", None, "c"));
        // A push starts the next round, whatever the base.
        assert!(!capped(&ctx, "fix/a", Some("p1"), "b"));
        count_pass(&ctx, "fix/a", Some("p1"), "b").unwrap();
        count_pass(&ctx, "fix/a", Some("p1"), "c").unwrap();
        assert!(capped(&ctx, "fix/a", Some("p1"), "d"));
        assert!(!capped(&ctx, "fix/b", Some("p1"), "b"));
        // A verdict a push went through with past the cap says so.
        let path = directory.path().join("v.json");
        let mut stored = verdict(vec![finding("nit")]);
        mark_capped(&path, &mut stored).unwrap();
        assert!(load(&path).unwrap().capped);
    }

    #[test]
    fn a_whitespace_fix_is_a_new_diff_and_a_rebase_is_not() {
        let repository = tempfile::tempdir().unwrap();
        let root = repository.path();
        let git = |arguments: &[&str]| {
            git::text(
                root,
                [
                    &[
                        "-c",
                        "user.name=t",
                        "-c",
                        "user.email=t@t",
                        "-c",
                        "commit.gpgsign=false",
                    ],
                    arguments,
                ]
                .concat(),
            )
            .unwrap()
        };
        let commit = |name: &str, text: &str| {
            fs::write(root.join(name), text).unwrap();
            git(&["add", name]);
            git(&["commit", "--quiet", "-m", name]);
            git(&["rev-parse", "HEAD"])
        };
        git(&["init", "--quiet"]);
        let base = commit("a.txt", "one\n");
        let spaced = commit("b.txt", "failed: {e}\n");
        let first = identity(root, &base, &spaced).unwrap();
        git(&["reset", "--quiet", "--hard", &base]);
        let unspaced = commit("b.txt", "failed:{e}\n");
        assert_ne!(identity(root, &base, &unspaced).unwrap(), first);
        // The same change on a later base keeps its identity.
        git(&["reset", "--quiet", "--hard", &base]);
        let later = commit("c.txt", "two\n");
        let rebased = commit("b.txt", "failed: {e}\n");
        assert_eq!(identity(root, &later, &rebased).unwrap(), first);
    }

    #[test]
    fn a_report_keeps_the_classification_it_does_not_interpret() {
        let report: Report = serde_json::from_value(json!({
            "summary": "s",
            "findings": [{"severity": "nit", "path": "a", "line": 1, "title": "t",
                          "body": "b", "area": "area:hil", "priority": "priority:p2", "issue": 0}]
        }))
        .unwrap();
        assert_eq!(report.findings[0].rest["area"], json!("area:hil"));
    }
}
