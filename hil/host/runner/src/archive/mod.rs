//! Durable evidence packages. Export/import are offline; GitHub is a transport.
use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde::{Deserialize, Serialize};

use crate::Result;

mod github;
mod install;
mod package;
#[cfg(test)]
mod tests;

const TARGET: &str = "esp32s31";

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Package sealed runs and optional analysis without changing their contents.
    Export {
        id: String,
        #[arg(long = "run", required = true)]
        runs: Vec<String>,
        /// Explicit analysis directory, copied under supplement/.
        #[arg(long)]
        supplement: Option<PathBuf>,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Validate the archive inventory and every contained HIL run offline.
    Verify {
        archive: PathBuf,
        #[arg(long)]
        sha256: Option<String>,
    },
    /// Restore runs into the local HIL store without overwriting different data.
    Import {
        archive: PathBuf,
        #[arg(long)]
        sha256: Option<String>,
    },
    /// Upload a verified package to a new release in a private repository.
    Publish {
        archive: PathBuf,
        #[arg(long)]
        repo: String,
    },
    /// Download, verify and import a private release by its archive ID.
    Fetch {
        id: String,
        #[arg(long)]
        repo: String,
        /// Optional digest pinned independently of the release metadata.
        #[arg(long)]
        sha256: Option<String>,
    },
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u16,
    id: String,
    target: String,
    runs: Vec<String>,
    files: Vec<Member>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Member {
    path: String,
    size_bytes: u64,
    sha256: String,
}

fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 128
        || !id.as_bytes()[0].is_ascii_alphanumeric()
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
    {
        return Err(
            "archive and run IDs must be portable names starting with a letter or digit".into(),
        );
    }
    Ok(())
}

pub(crate) fn run(root: &Path, command: Command) -> Result<()> {
    match command {
        Command::Export {
            id,
            runs,
            supplement,
            output,
        } => {
            validate_id(&id)?;
            let output = output
                .unwrap_or_else(|| root.join(format!("target/hil/{TARGET}/exports/{id}.tar.gz")));
            let digest = package::export(root, &id, runs, supplement.as_deref(), &output)?;
            crate::emit_json(
                &serde_json::json!({"archive": output, "sha256": digest, "id": id}),
                false,
            )
        }
        Command::Verify { archive, sha256 } => {
            let verified = package::open(&archive, sha256.as_deref())?;
            crate::emit_json(
                &serde_json::json!({"status": "verified", "id": verified.manifest.id,
                "runs": verified.manifest.runs, "sha256": verified.sha256}),
                false,
            )
        }
        Command::Import { archive, sha256 } => {
            let verified = package::open(&archive, sha256.as_deref())?;
            let destination = install::install(root, &verified)?;
            crate::emit_json(
                &serde_json::json!({"status": "imported", "directory": destination,
                "id": verified.manifest.id, "runs": verified.manifest.runs, "sha256": verified.sha256}),
                false,
            )
        }
        Command::Publish { archive, repo } => github::publish(&archive, &repo),
        Command::Fetch { id, repo, sha256 } => github::fetch(root, &id, &repo, sha256.as_deref()),
    }
}
