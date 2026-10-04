//! Fast, fail-closed repository integrity checks.
//!
//! The checks read the repository as text, without Cargo or rustc, so the
//! whole tier runs in seconds:
//!
//! - [`sources`]: every Rust file is reachable from a crate root, and
//!   capability anchors and vendor citations sit only in reachable files;
//! - [`records`]: every repository path a qualification or evidence record
//!   names exists;
//! - [`workspaces`]: every package belongs to a discovered workspace, and
//!   every workspace has its lock file;
//! - [`dependencies`]: every declared dependency is named by its package;
//! - [`classification`]: every package declares a known layer and platform,
//!   names an existing chip or family, follows the name rule, and keeps the
//!   evidence and HIL role edges;
//! - [`zeroed`]: no `link_section` names a zeroed region outside
//!   `oer_memory::zeroed_static!`;
//! - [`layouts`]: code relying on a foreign type's layout names the release
//!   it was reviewed at, and its workspace still locks that release.
//!
//! The same model answers what other tooling would otherwise rediscover:
//! [`workspaces`] lists every Cargo workspace and [`chips`] every chip
//! profile, for `oer-xtask` and CI alike.
//!
//! Each check returns its problems; an empty report is a pass.

pub mod allowlist;
pub mod chips;
pub mod classification;
pub mod dependencies;
pub mod fetch;
pub mod interrupts;
pub mod layouts;
pub mod manifest;
pub mod reachability;
pub mod records;
pub mod repo;
pub mod sources;
pub mod workspaces;
pub mod zeroed;

#[cfg(test)]
mod testing;

use std::collections::{BTreeMap, BTreeSet};

use allowlist::Allowlist;
use manifest::Manifests;
use reachability::{Reach, Walker};
use repo::Repo;

pub type Result<T> = std::result::Result<T, String>;

/// Everything the checks share, read once.
pub struct Context<'a> {
    pub repo: &'a Repo,
    pub manifests: Manifests,
    pub allowlist: Allowlist,
    /// What each package's roots reach, by manifest path.
    pub reach: BTreeMap<String, Reach>,
    /// Every Rust file some crate root reaches.
    pub reachable: BTreeSet<String>,
}

impl<'a> Context<'a> {
    pub fn load(repo: &'a Repo) -> Result<Self> {
        let manifests = Manifests::load(repo)?;
        let allowlist = if repo.is_file(allowlist::PATH) {
            Allowlist::parse(&repo.read(allowlist::PATH)?)?
        } else {
            Allowlist::default()
        };
        let mut walker = Walker::default();
        let mut reach = BTreeMap::new();
        let mut reachable = BTreeSet::new();
        for package in &manifests.packages {
            let found = walker.reach(repo, &package.roots)?;
            reachable.extend(found.files.iter().cloned());
            reach.insert(package.manifest.clone(), found);
        }
        Ok(Self {
            repo,
            manifests,
            allowlist,
            reach,
            reachable,
        })
    }
}

/// One check's name and its problems.
pub struct Outcome {
    pub check: &'static str,
    pub problems: Vec<String>,
}

/// Runs every check.
pub fn run(repo: &Repo) -> Result<Vec<Outcome>> {
    let context = Context::load(repo)?;
    let chips = chips::Chips::load(repo)?;
    Ok(vec![
        Outcome {
            check: "orphan sources",
            problems: sources::orphans(&context),
        },
        Outcome {
            check: "anchors and citations",
            problems: sources::markers(&context)?,
        },
        Outcome {
            check: "record paths",
            problems: records::check(&context)?,
        },
        Outcome {
            check: "workspaces",
            problems: workspaces::check(&context),
        },
        Outcome {
            check: "unused dependencies",
            problems: dependencies::check(&context)?,
        },
        Outcome {
            check: "classification",
            problems: classification::check(&context, &chips),
        },
        Outcome {
            check: "zeroed statics",
            problems: zeroed::check(&context)?,
        },
        Outcome {
            check: "reviewed layouts",
            problems: layouts::check(&context)?,
        },
        Outcome {
            check: "static interrupts",
            problems: interrupts::check(&context),
        },
    ])
}
