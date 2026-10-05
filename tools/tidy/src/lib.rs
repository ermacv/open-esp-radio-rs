//! Fast, fail-closed repository integrity checks.
//!
//! The checks read the repository as text, without Cargo or rustc, so the
//! whole tier runs in seconds:
//!
//! - [`sources`]: every Rust file is reachable from a crate root, and
//!   capability anchors and vendor `SOURCE` citations (`oer_markers`, the
//!   one recogniser of each grammar) sit only in reachable files;
//! - [`records`]: every repository path a qualification or evidence record
//!   names exists;
//! - [`workspaces`]: every package belongs to a discovered workspace, and
//!   every workspace has its lock file, and every firmware workspace has
//!   the same release profile;
//! - [`dependencies`]: every declared dependency is named by its package;
//! - [`classification`]: every package's `[package.metadata.open-radio]`
//!   classifies it, names an existing chip or family and input files, and
//!   its name follows the rule; every dependency keeps the layer, platform
//!   and role rules of [`oer_repo::policy`];
//! - [`spawns`]: only entry crates run `cargo hil` or `cargo xtask`;
//! - [`layouts`]: code relying on a foreign type's layout names the release
//!   it was reviewed at, and its workspace still locks that release.
//!
//! The checks are text policy over the repository model of `oer-repo`,
//! which also answers the `workspaces` and `chips` commands.
//!
//! Each check returns its problems; an empty report is a pass.

pub mod allowlist;
pub mod classification;
pub mod dependencies;
pub mod fetch;
pub mod layouts;
pub mod reachability;
pub mod records;
pub mod sources;
pub mod spawns;
pub mod workspaces;

#[cfg(test)]
mod testing;

use std::collections::{BTreeMap, BTreeSet};

use allowlist::Allowlist;
use oer_repo::{Model, Repo};
use reachability::{Reach, Walker};

pub type Result<T> = std::result::Result<T, String>;

/// Everything the checks share, read once.
pub struct Context<'a> {
    pub repo: &'a Repo,
    pub model: Model,
    pub allowlist: Allowlist,
    /// What each package's roots reach, by manifest path.
    pub reach: BTreeMap<String, Reach>,
    /// Every Rust file some crate root reaches.
    pub reachable: BTreeSet<String>,
}

impl<'a> Context<'a> {
    pub fn load(repo: &'a Repo) -> Result<Self> {
        let model = Model::load(repo)?;
        let allowlist = if repo.is_file(allowlist::PATH) {
            Allowlist::parse(&repo.read(allowlist::PATH)?)?
        } else {
            Allowlist::default()
        };
        let mut walker = Walker::default();
        let mut reach = BTreeMap::new();
        let mut reachable = BTreeSet::new();
        for package in model.packages() {
            let found = walker.reach(repo, &package.roots)?;
            reachable.extend(found.files.iter().cloned());
            reach.insert(package.manifest.clone(), found);
        }
        Ok(Self {
            repo,
            model,
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
            check: "firmware release profiles",
            problems: workspaces::release_profiles(&context),
        },
        Outcome {
            check: "unused dependencies",
            problems: dependencies::check(&context)?,
        },
        Outcome {
            check: "classification",
            problems: classification::check(&context),
        },
        Outcome {
            check: "layer dependencies",
            problems: oer_repo::policy::check(&context.model),
        },
        Outcome {
            check: "command-line spawns",
            problems: spawns::check(&context)?,
        },
        Outcome {
            check: "reviewed layouts",
            problems: layouts::check(&context)?,
        },
    ])
}
