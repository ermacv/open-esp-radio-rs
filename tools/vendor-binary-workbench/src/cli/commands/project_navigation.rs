//! Navigation-only joins across independently generated project facts.

use std::path::Path;

use super::Result;
use crate::project::ProjectSpec;

pub(super) fn run(project: &ProjectSpec, output: &Path, check: bool) -> Result<bool> {
    let document = crate::navigation::build(project)?;
    let rendered = serde_json::to_string_pretty(&document)? + "\n";
    super::super::generated_output::write_or_check(output, &rendered, check, "navigation index")?;
    tracing::info!(
        status = if check { "verified" } else { "written" },
        path = %output.display(),
        "project navigation index"
    );
    Ok(true)
}
