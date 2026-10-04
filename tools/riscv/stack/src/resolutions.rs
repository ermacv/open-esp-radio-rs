//! Targets that facts outside the machine code give the indirect sites the
//! analysis left unresolved, with the fact each set comes from.
//!
//! Every fact gives a superset of what its sites can reach, so the facts of
//! one site unite. An empty set resolves a site only where the fact is
//! complete for it ([`Fact::proves_empty`]); otherwise it is a hole
//! ([`crate::Reason::NoCandidate`]), never a site that reaches nothing.
use std::collections::{BTreeMap, BTreeSet};

/// Where a site's targets come from.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Fact {
    /// The image's static interrupt table: a dispatcher reaches the
    /// handlers of its level and core.
    InterruptTable,
    /// The handler arguments of every call that posts an IPC callback.
    IpcPosts,
    /// The waker vtables of the image.
    WakerVtables,
    /// The functions whose type can be a static field's function pointer.
    FieldType,
}

impl Fact {
    /// Whether an empty set from this fact proves the site reaches nothing:
    /// the interrupt table lists every handler, and the IPC posts are every
    /// call of the one function that posts. Waker vtables and field types
    /// are found, not listed, so their empty set is a hole.
    pub fn proves_empty(self) -> bool {
        matches!(self, Self::InterruptTable | Self::IpcPosts)
    }
}

/// One site's targets and the facts that gave them.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Resolution {
    pub targets: BTreeSet<u32>,
    pub facts: BTreeSet<Fact>,
}

impl Resolution {
    /// Whether the site reaches exactly `targets`: a nonempty set, or an
    /// empty one a fact proves.
    pub fn resolves(&self) -> bool {
        !self.targets.is_empty() || self.facts.iter().any(|fact| fact.proves_empty())
    }
}

/// The resolutions of an image's indirect sites, by site.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Resolutions {
    sites: BTreeMap<u32, Resolution>,
}

impl Resolutions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add the `targets` that `fact` gives `site`, to those other facts give.
    pub fn add(&mut self, site: u32, fact: Fact, targets: impl IntoIterator<Item = u32>) {
        let resolution = self.sites.entry(site).or_default();
        resolution.facts.insert(fact);
        resolution.targets.extend(targets);
    }

    /// Add every resolution of `other`.
    pub fn extend(&mut self, other: Resolutions) {
        for (site, resolution) in other.sites {
            let into = self.sites.entry(site).or_default();
            into.facts.extend(resolution.facts);
            into.targets.extend(resolution.targets);
        }
    }

    pub fn get(&self, site: u32) -> Option<&Resolution> {
        self.sites.get(&site)
    }

    pub fn iter(&self) -> impl Iterator<Item = (u32, &Resolution)> {
        self.sites
            .iter()
            .map(|(&site, resolution)| (site, resolution))
    }

    pub fn len(&self) -> usize {
        self.sites.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sites.is_empty()
    }
}
