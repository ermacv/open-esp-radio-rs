//! GitHub CLI transport; authentication is confined to child-process environments.
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use super::{install, package, validate_id};
use crate::Result;

const ASSET: &str = "evidence.tar.gz";
const REFERENCE: &str = "reference.json";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Reference {
    schema: u16,
    repository: String,
    id: String,
    asset: String,
    sha256: String,
}

struct GitHub {
    token: Option<Zeroizing<String>>,
}

impl GitHub {
    fn connect() -> Result<Self> {
        // gh auth or explicit environment tokens remain the normal interface.
        let mut probe = Command::new("gh");
        probe
            .args(["auth", "status", "--hostname", "github.com"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        if oer_process::output(&mut probe, Some(Duration::from_secs(30)))?
            .status
            .success()
        {
            return Ok(Self { token: None });
        }
        let mut command = Command::new("git");
        command
            .args(["-c", "credential.interactive=false", "credential", "fill"])
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = oer_process::owned::Child::spawn(&mut command)?;
        child
            .stdin
            .take()
            .ok_or("credential helper stdin is unavailable")?
            .write_all(b"protocol=https\nhost=github.com\n\n")?;
        let mut output = child.wait_with_output_timeout(Some(Duration::from_secs(30)))?;
        let token = if output.status.success() {
            std::str::from_utf8(&output.stdout).ok().and_then(|text| {
                text.lines()
                    .find_map(|line| line.strip_prefix("password="))
                    .filter(|s| !s.is_empty())
                    .map(|s| Zeroizing::new(s.to_owned()))
            })
        } else {
            None
        };
        output.stdout.zeroize();
        output.stderr.zeroize();
        let token =
            token.ok_or("GitHub authentication is required: use gh auth login or GH_TOKEN")?;
        Ok(Self { token: Some(token) })
    }

    fn command(&self) -> Command {
        let mut command = Command::new("gh");
        command.env("GH_HOST", "github.com").stdin(Stdio::null());
        if let Some(token) = &self.token {
            command.env("GH_TOKEN", token.as_str());
        }
        command
    }

    fn capture(&self, arguments: &[&str]) -> Result<Vec<u8>> {
        let mut command = self.command();
        command
            .args(arguments)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = oer_process::output(&mut command, Some(Duration::from_secs(120)))?;
        if !output.status.success() {
            return Err(format!(
                "GitHub operation failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        Ok(output.stdout)
    }

    fn require_private(&self, repo: &str) -> Result<()> {
        validate_repository(repo)?;
        let bytes = self.capture(&["api", &format!("repos/{repo}"), "--jq", ".private"])?;
        if bytes.trim_ascii() != b"true" {
            return Err("evidence archive transport requires a private GitHub repository".into());
        }
        Ok(())
    }
}

fn validate_repository(repo: &str) -> Result<()> {
    let parts: Vec<_> = repo.split('/').collect();
    if parts.len() != 2 {
        return Err("repository must be OWNER/NAME".into());
    }
    for part in parts {
        validate_id(part)?;
    }
    Ok(())
}

pub(super) fn publish(archive: &Path, repo: &str) -> Result<()> {
    let verified = package::open(archive, None)?;
    let github = GitHub::connect()?;
    github.require_private(repo)?;
    let reference = Reference {
        schema: 1,
        repository: repo.to_owned(),
        id: verified.manifest.id.clone(),
        asset: ASSET.to_owned(),
        sha256: verified.sha256.clone(),
    };
    let staging = tempfile::tempdir()?;
    fs::copy(archive, staging.path().join(ASSET))?;
    fs::write(
        staging.path().join(REFERENCE),
        serde_json::to_vec_pretty(&reference)?,
    )?;
    fs::write(
        staging.path().join("notes.md"),
        format!(
            "Verified HIL evidence archive `{}`.\n\nRuns: {}.\n\nSHA-256: `{}`.\n\nThis package preserves observations and their original status; publication does not confer hardware qualification.\n",
            reference.id,
            verified.manifest.runs.len(),
            reference.sha256
        ),
    )?;
    // A pre-existing tag/release is never overwritten. Keep incomplete uploads as drafts.
    let mut create = github.command();
    create
        .args([
            "release",
            "create",
            &reference.id,
            "--repo",
            repo,
            "--draft",
            "--title",
            &reference.id,
            "--notes-file",
        ])
        .arg(staging.path().join("notes.md"))
        .arg(staging.path().join(ASSET))
        .arg(staging.path().join(REFERENCE));
    oer_process::run(&mut create)?;
    // Independently download and verify remote bytes before exposing the release.
    let downloaded = tempfile::tempdir()?;
    let mut download = github.command();
    download
        .args([
            "release",
            "download",
            &reference.id,
            "--repo",
            repo,
            "--pattern",
            ASSET,
            "--dir",
        ])
        .arg(downloaded.path());
    oer_process::run(&mut download)?;
    let remote = package::open(&downloaded.path().join(ASSET), Some(&reference.sha256))?;
    if remote.manifest != verified.manifest {
        return Err("uploaded archive identity changed".into());
    }
    github.capture(&[
        "release",
        "edit",
        &reference.id,
        "--repo",
        repo,
        "--draft=false",
        "--latest=false",
    ])?;
    crate::emit_json(&reference, true)
}

pub(super) fn fetch(root: &Path, id: &str, repo: &str, pinned: Option<&str>) -> Result<()> {
    validate_id(id)?;
    let github = GitHub::connect()?;
    github.require_private(repo)?;
    let staging = tempfile::tempdir()?;
    let mut download = github.command();
    download
        .args([
            "release",
            "download",
            id,
            "--repo",
            repo,
            "--pattern",
            ASSET,
            "--pattern",
            REFERENCE,
            "--dir",
        ])
        .arg(staging.path());
    oer_process::run(&mut download)?;
    let reference: Reference = serde_json::from_slice(&fs::read(staging.path().join(REFERENCE))?)?;
    if reference.schema != 1
        || reference.repository != repo
        || reference.id != id
        || reference.asset != ASSET
    {
        return Err("downloaded reference does not match requested evidence".into());
    }
    if pinned.is_some_and(|sha| sha != reference.sha256) {
        return Err("release reference does not match pinned SHA-256".into());
    }
    let verified = package::open(&staging.path().join(ASSET), Some(&reference.sha256))?;
    if verified.manifest.id != id {
        return Err("downloaded archive ID does not match release".into());
    }
    let directory = install::install(root, &verified)?;
    crate::emit_json(
        &serde_json::json!({"status": "imported", "directory": directory, "reference": reference}),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repository_requires_two_unambiguous_components() {
        for repo in ["ermacv/evidence", "team-name/evidence_1"] {
            validate_repository(repo).unwrap();
        }
        for repo in ["--repo", "a/../b", "a/", "/b", "a/b/c", "a/b\\c"] {
            assert!(validate_repository(repo).is_err());
        }
    }
}
