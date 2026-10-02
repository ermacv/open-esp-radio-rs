//! The fast integrity tier: every `oer-tidy` check over this checkout, run
//! in-process.

use crate::{Context, Result};

/// Run every `oer-tidy` check over this checkout; fails listing every
/// problem.
pub fn run(ctx: &Context) -> Result<()> {
    let repo = oer_tidy::repo::Repo::from_git(&ctx.root)?;
    let mut problems = Vec::new();
    for outcome in oer_tidy::run(&repo)? {
        for problem in outcome.problems {
            problems.push(format!("tidy {}: {problem}", outcome.check));
        }
    }
    if problems.is_empty() {
        return Ok(());
    }
    Err(format!(
        "{} problem(s) (exceptions: tools/tidy/allowlist.toml):\n{}",
        problems.len(),
        problems.join("\n")
    )
    .into())
}
