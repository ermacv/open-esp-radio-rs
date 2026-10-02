//! The supported chips: every `platform/<id>/chip.toml`, read through
//! `oer-chip-profile`, the one parser of the profile.

use oer_chip_profile::Profile;

use crate::{Result, repo::Repo};

/// Every supported chip's profile, sorted by id.
pub struct Chips(Vec<Profile>);

impl Chips {
    pub fn load(repo: &Repo) -> Result<Self> {
        if !repo.exists("platform") {
            return Ok(Self(Vec::new()));
        }
        Profile::all(repo.root())
            .map(Self)
            .map_err(|error| format!("chip profiles: {error}"))
    }

    pub fn profiles(&self) -> &[Profile] {
        &self.0
    }

    /// Every chip id, ascending.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|profile| profile.id.as_str())
    }

    /// Every family some chip belongs to.
    pub fn families(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|profile| profile.family.as_str())
    }
}
