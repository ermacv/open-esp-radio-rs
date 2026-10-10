//! The local Claude review of this branch before it is pushed.
//!
//! The Claude review in CI (`.github/workflows/claude-review.yml`) starts
//! only after CI passed, so every finding it reports costs a full CI round
//! before the fix is reviewed again. This review runs the same instructions
//! (root `CLAUDE.md` and `REVIEW.md`, read from `origin/main` as CI reads
//! them from main) and report schema in a separate,
//! headless Claude Code process (`claude -p`) on the committed branch, with
//! read-only tools and no repository settings, hooks or MCP servers, so the
//! session that wrote the change never reviews it in its own context.
//!
//! A verdict is kept per diff: its identity is the verbatim patch id of the
//! diff between the merge base with `origin/main` and `HEAD`, so a rebase
//! that changes nothing in the diff, or a reworded commit, keeps it. A
//! re-review of the branch hands Claude the findings of the branch's previous
//! verdict to check. 🔴 important and 🟡 nit findings block, as in CI; 🟣
//! pre-existing ones are only printed, since CI files them as issues.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use oer_process::{self as process, Checkout, git};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Result, push};

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
    "Bash(gh pr view *)",
    "Bash(gh pr diff *)",
    "Bash(gh issue view *)",
];

/// How the local review differs from the CI review `REVIEW.md` describes.
const LOCAL: &str = "\
# Local review

This is the local review before the branch is pushed, not the CI review.
The branch is checked out at the repository root, not in `pr/`: run `git`
without `-C pr` and read files from the root. Diff against the merge base
named in the prompt. CI has not run; the push gate's fast checks (tidy,
formatting, Clippy and tests of the changed packages) passed. The branch may
have no pull request yet; then no earlier review exists on GitHub, and the
prompt lists the findings of the previous local review, if any. The prompt
lists no filed issues: set `issue` to 0 for every finding.";

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
    pub fn blocking(&self) -> bool {
        matches!(self.severity.as_str(), "important" | "nit")
    }

    fn mark(&self) -> &'static str {
        match self.severity.as_str() {
            "important" => "🔴",
            "nit" => "🟡",
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
}

impl Verdict {
    pub fn blocking(&self) -> usize {
        self.report.findings.iter().filter(|f| f.blocking()).count()
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
                    "severity": {"enum": ["important", "nit", "pre-existing"]},
                    "path": {"type": "string"},
                    "line": {"type": "integer", "minimum": 1},
                    "title": {"type": "string"},
                    "body": {"type": "string"},
                    "area": {"enum": names("area:")},
                    "priority": {"enum": names("priority:")},
                    "issue": {"type": "integer", "minimum": 0}}}}}
    }))
}

/// The prompt for `branch` at `head` against `base`, with the findings the
/// branch's previous verdict reported.
pub fn prompt(branch: &str, head: &str, base: &str, previous: Option<&Verdict>) -> String {
    let mut prompt = format!(
        "Review branch {branch} at head {head}, checked out at the repository root, \
         against its merge base {base} with origin/main (`git diff {base}...HEAD`), \
         following the review instructions."
    );
    match previous {
        Some(verdict) if !verdict.report.findings.is_empty() => {
            prompt.push_str(&format!(
                "\n\nThe previous local review of this branch, at head {}, reported the \
                 findings below. Check whether each is fixed, report every one still open \
                 again, and review the whole diff, not only the fixes: a fix often leaves \
                 the same defect elsewhere or brings a new one.\n",
                verdict.head
            ));
            for finding in &verdict.report.findings {
                prompt.push_str(&format!(
                    "\n- {} {} ({}:{}): {}",
                    finding.severity, finding.title, finding.path, finding.line, finding.body
                ));
            }
        }
        _ => {}
    }
    prompt
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

/// The newest stored verdict of `branch`.
fn previous(ctx: &Checkout, branch: &str) -> Option<Verdict> {
    let mut verdicts: Vec<(std::time::SystemTime, Verdict)> = fs::read_dir(directory(ctx))
        .ok()?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, load(&entry.path())?))
        })
        .filter(|(_, verdict)| verdict.branch == branch)
        .collect();
    verdicts.sort_by_key(|(modified, _)| *modified);
    verdicts.pop().map(|(_, verdict)| verdict)
}

/// Run `claude -p` and return its report and cost.
fn claude(ctx: &Checkout, prompt: &str) -> Result<(Report, f64)> {
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
        .current_dir(&ctx.root)
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
    if result["is_error"].as_bool() != Some(false) || result["structured_output"].is_null() {
        return Err(format!(
            "review: claude produced no report ({}): {}",
            result["subtype"].as_str().unwrap_or("unknown"),
            result["result"].as_str().unwrap_or("no message")
        )
        .into());
    }
    let report = serde_json::from_value(result["structured_output"].clone())?;
    Ok((report, result["total_cost_usd"].as_f64().unwrap_or(0.0)))
}

/// The verdict on `branch`'s diff from `base` to `head`, the checked-out
/// commit: the stored one when the diff was reviewed already, otherwise a
/// new review, refused when `HEAD` moved while Claude read the tree.
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
    let previous = previous(ctx, branch);
    println!(
        "review: reviewing {branch} at {} with a separate Claude Code run{}",
        &head[..12],
        if previous.is_some() {
            "; it checks the previous local findings"
        } else {
            ""
        }
    );
    let (report, cost_usd) = claude(ctx, &prompt(branch, head, base, previous.as_ref()))?;
    let now = git::text(&ctx.root, ["rev-parse", "HEAD"])?;
    if now != head {
        return Err(format!(
            "review: HEAD moved from {} to {} during the review, which read the working tree; run it again",
            &head[..12],
            &now[..now.len().min(12)]
        )
        .into());
    }
    let verdict = Verdict {
        branch: branch.to_owned(),
        head: head.to_owned(),
        base: base.to_owned(),
        identity,
        report,
        cost_usd,
    };
    fs::create_dir_all(directory(ctx))?;
    fs::write(&path, serde_json::to_string_pretty(&verdict)?)?;
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
        }
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
    fn the_schema_takes_areas_and_priorities_from_the_label_catalog() {
        let labels = r#"[{"name":"kind:bug"},{"name":"area:hil"},{"name":"priority:p1"}]"#;
        let schema = schema(labels).unwrap();
        let item = &schema["properties"]["findings"]["items"]["properties"];
        assert_eq!(item["area"]["enum"], json!(["area:hil"]));
        assert_eq!(item["priority"]["enum"], json!(["priority:p1"]));
    }

    #[test]
    fn a_re_review_checks_the_previous_findings() {
        let first = prompt("fix/a", "h", "b", None);
        assert!(first.contains("git diff b...HEAD") && !first.contains("previous"));
        let previous = verdict(vec![finding("nit")]);
        let again = prompt("fix/a", "h", "b", Some(&previous));
        assert!(
            again.contains("nit Lost error (crates/a/src/lib.rs:7)"),
            "{again}"
        );
        assert!(again.contains("same defect elsewhere"), "{again}");
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
