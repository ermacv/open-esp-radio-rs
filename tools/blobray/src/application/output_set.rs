//! Retained destination bindings and completion accounting for one operation.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use super::generated_file::{ContentIdentity, GeneratedOutput};
use crate::{Result, artifacts::StagedLinkedIrBundle};

struct Slot {
    path: PathBuf,
    claimed: AtomicBool,
    completed: OnceLock<ContentIdentity>,
}

/// Exact content observed by a completed emission, comparison or cache reuse.
/// Fields are private: callers cannot substitute a digest for another output.
#[derive(Clone, Debug)]
pub(super) struct OutputReceipt {
    path: PathBuf,
    content: ContentIdentity,
}

impl OutputReceipt {
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
    pub(super) fn sha256(&self) -> &str {
        &self.content.sha256
    }
    pub(super) fn bytes(&self) -> u64 {
        self.content.bytes
    }
    pub(super) fn validate(&self) -> Result<()> {
        let file = fs::File::open(&self.path).map_err(|error| {
            crate::Error::invalid(format!(
                "completed output {} is unavailable: {error}",
                self.path.display()
            ))
        })?;
        if ContentIdentity::read(file)? != self.content {
            return Err(crate::Error::invalid(format!(
                "completed output {} changed after emission",
                self.path.display()
            )));
        }
        Ok(())
    }
}

/// Clones share the same one-use slots; cloning does not duplicate permission.
/// The run coordinator admits these paths before creating the set. Standalone
/// application workflows create their own explicit declaration.
#[derive(Clone)]
pub(crate) struct OutputSet {
    slots: Arc<[Slot]>,
    check: bool,
}

impl OutputSet {
    pub(super) fn new(paths: &[PathBuf], check: bool) -> Result<Self> {
        let mut seen = BTreeSet::new();
        for path in paths {
            if !seen.insert(path) {
                return Err(crate::Error::invalid(format!(
                    "duplicate output binding {}",
                    path.display()
                )));
            }
        }
        Ok(Self {
            slots: paths
                .iter()
                .map(|path| Slot {
                    path: path.clone(),
                    claimed: AtomicBool::new(false),
                    completed: OnceLock::new(),
                })
                .collect(),
            check,
        })
    }

    pub(super) fn paths(&self) -> Vec<PathBuf> {
        self.slots.iter().map(|slot| slot.path.clone()).collect()
    }

    pub(super) fn check(&self) -> bool {
        self.check
    }

    fn claim(&self, index: usize) -> Result<&Slot> {
        let slot = self
            .slots
            .get(index)
            .ok_or_else(|| crate::Error::invalid(format!("output slot {index} is not declared")))?;
        if slot.claimed.swap(true, Ordering::AcqRel) {
            return Err(crate::Error::invalid(format!(
                "output {} was already claimed",
                slot.path.display()
            )));
        }
        Ok(slot)
    }

    pub(super) fn file<'a>(&'a self, index: usize, kind: &'a str) -> Result<GeneratedOutput<'a>> {
        let slot = self.claim(index)?;
        Ok(GeneratedOutput::tracked(
            &slot.path,
            self.check,
            kind,
            &slot.completed,
        ))
    }

    pub(super) fn require_complete(&self) -> Result<()> {
        for slot in self.slots.iter() {
            if slot.completed.get().is_none() {
                return Err(crate::Error::invalid(format!(
                    "declared output {} did not complete",
                    slot.path.display()
                )));
            }
        }
        Ok(())
    }

    pub(super) fn receipts(&self) -> Result<Vec<OutputReceipt>> {
        self.require_complete()?;
        Ok(self
            .slots
            .iter()
            .map(|slot| OutputReceipt {
                path: slot.path.clone(),
                content: slot.completed.get().expect("completion checked").clone(),
            })
            .collect())
    }

    /// Admit an unchanged cached file only after observing its actual content.
    /// A miss leaves its slot unclaimed so restoration can acquire it instead.
    pub(super) fn reuse(&self, index: usize, digest: &str) -> Result<bool> {
        let slot = self
            .slots
            .get(index)
            .ok_or_else(|| crate::Error::invalid("undeclared cached output"))?;
        let file = match fs::File::open(&slot.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        let identity = ContentIdentity::read(file)?;
        if identity.sha256 != digest {
            return Ok(false);
        }
        self.claim(index)?
            .completed
            .set(identity)
            .map_err(|_| crate::Error::invalid("cached output receipt was already completed"))?;
        Ok(true)
    }

    /// The directory adapter consumes the exact declared bundle members. It
    /// retains the existing bundle swap/check operation; it does not claim
    /// that several work items publish as one transaction.
    pub(super) fn bundle(
        &self,
        destination: &Path,
        bundle: StagedLinkedIrBundle,
    ) -> Result<Vec<PathBuf>> {
        let members = crate::artifacts::bundle_files(destination)
            .chain(std::iter::once(destination.join(super::coverage::FILE)))
            .collect::<Vec<_>>();
        let indices = members
            .iter()
            .map(|path| {
                self.slots
                    .iter()
                    .position(|slot| slot.path == *path)
                    .ok_or_else(|| {
                        crate::Error::invalid(format!(
                            "bundle member {} is not a declared output",
                            path.display()
                        ))
                    })
            })
            .collect::<Result<Vec<_>>>()?;
        for path in &members {
            let name = path.file_name().expect("bundle member name");
            if !bundle.path().join(name).is_file() {
                return Err(crate::Error::invalid(format!(
                    "staged bundle member {name:?} is missing"
                )));
            }
        }
        let identities = members
            .iter()
            .map(|path| {
                ContentIdentity::read(fs::File::open(
                    bundle.path().join(path.file_name().expect("bundle member")),
                )?)
            })
            .collect::<Result<Vec<_>>>()?;
        for &index in &indices {
            self.claim(index)?;
        }
        if self.check {
            let stale = bundle.compare(destination)?;
            if !stale.is_empty() {
                return Ok(stale);
            }
        } else {
            bundle.publish(destination)?;
        }
        for (index, identity) in indices.into_iter().zip(identities) {
            self.slots[index].completed.set(identity).map_err(|_| {
                crate::Error::invalid("bundle output receipt was already completed")
            })?;
        }
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests;
