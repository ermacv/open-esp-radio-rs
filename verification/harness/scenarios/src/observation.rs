//! Production PHY source lines the compared observations depend on.
//!
//! Blobray reports which executed production instructions a compared
//! observation depends on. Debug line information of the probe ELF maps them
//! to production hardware source lines (PHY, HAL, PAC and MAC driver crates
//! of the chip, and the chip-neutral hardware crates it shares), including
//! inlined frames: a line is
//! observed when any of its executed instructions is. Every executed but
//! unobserved line is either reviewed by a decision below, with its reason, or
//! reported as untriaged in the evidence index. A decision that matches no
//! unobserved line fails the run, so the table cannot outlive the code it
//! describes.
use crate::harness::{Result, invalid};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Production hardware sources of the installed chip, relative to the
/// repository root.
fn hardware_scope() -> &'static str {
    crate::chip().hardware_scope
}

/// Every production hardware source tree attributed to the installed chip:
/// its own, then the chip-neutral ones it shares.
fn scopes() -> impl Iterator<Item = &'static str> {
    std::iter::once(hardware_scope()).chain(crate::chip().shared_scopes.iter().copied())
}

/// The repository path of a decision's file: below the chip's hardware
/// scope, or below the repository root when it names a shared tree.
fn decision_path(file: &str) -> PathBuf {
    if crate::chip()
        .shared_scopes
        .iter()
        .any(|scope| file.starts_with(scope))
    {
        PathBuf::from(file)
    } else {
        Path::new(hardware_scope()).join(file)
    }
}

/// Repository root: this package lives three directories below it.
pub fn root() -> Result<PathBuf> {
    Ok(Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()?)
}

/// A source line of one file, relative to the repository root.
pub type SourceLine = (PathBuf, u32);

/// A reviewed decision on executed lines no compared observation depends on.
/// Each place is a file below the chip's hardware scope (or a path of a shared
/// hardware tree from the repository root) and a line's trimmed text, so the
/// decision survives unrelated line shifts.
#[derive(Clone, Copy, Debug)]
pub struct Decision {
    pub reason: &'static str,
    pub places: &'static [(&'static str, &'static str)],
}

/// Executed and observed production hardware lines.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Lines {
    pub executed: BTreeSet<SourceLine>,
    pub observed: BTreeSet<SourceLine>,
    /// Lines an emitted event depends on, compared or not.
    pub effect: BTreeSet<SourceLine>,
    /// Lines the memory state at a case end depends on.
    pub state: BTreeSet<SourceLine>,
}

impl Lines {
    pub fn extend(&mut self, other: &Lines) {
        self.executed.extend(other.executed.iter().cloned());
        self.observed.extend(other.observed.iter().cloned());
        self.effect.extend(other.effect.iter().cloned());
        self.state.extend(other.state.iter().cloned());
    }

    pub fn unobserved(&self) -> BTreeSet<SourceLine> {
        self.executed.difference(&self.observed).cloned().collect()
    }
}

/// Maps probe instructions to production hardware source lines.
pub struct LineMap {
    lines: BTreeMap<u32, Vec<SourceLine>>,
}

impl LineMap {
    /// The lines within the chip's hardware scope of every instruction in `pcs` of `elf`.
    pub fn new(elf: &[u8], pcs: &BTreeSet<u32>, root: &Path) -> Result<Self> {
        use object::{Object, ObjectSection};
        let file = object::File::parse(elf)?;
        let endian = gimli::RunTimeEndian::Little;
        let dwarf = gimli::Dwarf::load(|id| {
            let data = file
                .section_by_name(id.name())
                .and_then(|section| section.data().ok())
                .unwrap_or(&[]);
            Ok::<_, gimli::Error>(gimli::EndianSlice::new(data, endian))
        })?;
        let context = addr2line::Context::from_dwarf(dwarf)?;
        let root = root.canonicalize()?;
        let mut lines = BTreeMap::new();
        for pc in pcs {
            let mut located = vec![];
            let mut frames = context
                .find_frames(u64::from(*pc))
                .skip_all_loads()
                .map_err(|e| invalid(format!("debug information at {pc:#x}: {e}")))?;
            while let Some(frame) = frames
                .next()
                .map_err(|e| invalid(format!("debug information at {pc:#x}: {e}")))?
            {
                let Some(location) = frame.location else {
                    continue;
                };
                let (Some(path), Some(line)) = (location.file, location.line) else {
                    continue;
                };
                if let Some(file) = scopes().find_map(|scope| {
                    Path::new(path)
                        .strip_prefix(root.join(scope))
                        .ok()
                        .map(|relative| Path::new(scope).join(relative))
                }) {
                    located.push((file, line));
                }
            }
            lines.insert(*pc, located);
        }
        Ok(Self { lines })
    }

    /// Lines of `executed` instructions, observed when any of `observed` is.
    pub fn lines(
        &self,
        instructions: &blobray_application::in_process::ObservedInstructions,
    ) -> Lines {
        let mut result = Lines::default();
        for pc in &instructions.executed {
            for line in self.lines.get(pc).into_iter().flatten() {
                result.executed.insert(line.clone());
                for (set, lines) in [
                    (&instructions.observed, &mut result.observed),
                    (&instructions.effect, &mut result.effect),
                    (&instructions.state, &mut result.state),
                ] {
                    if set.contains(pc) {
                        lines.insert(line.clone());
                    }
                }
            }
        }
        result
    }
}

/// Trimmed text of source lines, read once per file.
#[derive(Default)]
pub struct Sources {
    files: BTreeMap<PathBuf, Vec<String>>,
}

impl Sources {
    fn text(&mut self, root: &Path, (file, line): &SourceLine) -> Result<&str> {
        if !self.files.contains_key(file) {
            let text = std::fs::read_to_string(root.join(file))?;
            self.files.insert(
                file.clone(),
                text.lines().map(|l| l.trim().to_owned()).collect(),
            );
        }
        self.files[file]
            .get(*line as usize - 1)
            .map(String::as_str)
            .ok_or_else(|| invalid(format!("{}:{line} is outside its file", file.display())))
    }

    /// Split `unobserved` into (reviewed, untriaged) under `decisions`.
    pub fn classify(
        &mut self,
        root: &Path,
        decisions: &[Decision],
        unobserved: &BTreeSet<SourceLine>,
    ) -> Result<(BTreeSet<SourceLine>, BTreeSet<SourceLine>)> {
        let mut reviewed = BTreeSet::new();
        let mut untriaged = BTreeSet::new();
        for line in unobserved {
            let text = self.text(root, line)?;
            if decisions.iter().any(|d| {
                d.places
                    .iter()
                    .any(|(file, source)| line.0 == decision_path(file) && text == *source)
            }) {
                reviewed.insert(line.clone());
            } else {
                untriaged.insert(line.clone());
            }
        }
        Ok((reviewed, untriaged))
    }

    /// Every place of every decision must still match an unobserved line.
    pub fn check(
        &mut self,
        root: &Path,
        decisions: &[Decision],
        unobserved: &BTreeSet<SourceLine>,
    ) -> Result<()> {
        let mut matched = BTreeSet::new();
        for line in unobserved {
            let text = self.text(root, line)?.to_owned();
            for decision in decisions {
                for place in decision.places {
                    if line.0 == decision_path(place.0) && text == place.1 {
                        matched.insert(*place);
                    }
                }
            }
        }
        let stale: Vec<String> = decisions
            .iter()
            .flat_map(|d| d.places)
            .filter(|place| !matched.contains(*place))
            .map(|(file, source)| format!("`{source}` in {file}"))
            .collect();
        if stale.is_empty() {
            Ok(())
        } else {
            Err(invalid(format!(
                "observation decisions match no unobserved line; each line is observed or \
                 no longer executed: {}",
                stale.join(", ")
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "crates/hardware/test/phy/src/example.rs";
    const PLACE: &str = "phy/src/example.rs";

    fn root() -> tempfile::TempDir {
        crate::install(&crate::chip::TEST);
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join(FILE);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "fn f() {\n    let unused = 1;\n    write();\n}\n").unwrap();
        root
    }

    fn line(number: u32) -> SourceLine {
        (PathBuf::from(FILE), number)
    }

    const DECIDED: &[Decision] = &[Decision {
        reason: "test",
        places: &[(PLACE, "let unused = 1;")],
    }];

    #[test]
    fn decisions_review_their_lines_and_leave_the_rest_untriaged() {
        let root = root();
        let unobserved = BTreeSet::from([line(2), line(3)]);
        let (reviewed, untriaged) = Sources::default()
            .classify(root.path(), DECIDED, &unobserved)
            .unwrap();
        assert_eq!(reviewed, BTreeSet::from([line(2)]));
        assert_eq!(untriaged, BTreeSet::from([line(3)]));
    }

    #[test]
    fn a_decision_whose_line_is_observed_or_not_executed_is_stale() {
        let root = root();
        let mut sources = Sources::default();
        sources
            .check(root.path(), DECIDED, &BTreeSet::from([line(2)]))
            .unwrap();
        assert!(
            sources
                .check(root.path(), DECIDED, &BTreeSet::from([line(3)]))
                .is_err()
        );
        assert!(
            sources
                .check(root.path(), DECIDED, &BTreeSet::new())
                .is_err()
        );
    }

    #[test]
    fn decisions_name_shared_trees_from_the_repository_root() {
        crate::install(&crate::chip::TEST);
        assert_eq!(decision_path(PLACE), PathBuf::from(FILE));
        assert_eq!(
            decision_path("crates/hardware/shared-test/src/lib.rs"),
            PathBuf::from("crates/hardware/shared-test/src/lib.rs")
        );
    }

    #[test]
    fn a_line_is_observed_when_any_of_its_instructions_is() {
        let map = LineMap {
            lines: BTreeMap::from([(0x10, vec![line(2)]), (0x14, vec![line(2), line(3)])]),
        };
        let lines = map.lines(&blobray_application::in_process::ObservedInstructions {
            executed: BTreeSet::from([0x10, 0x14]),
            observed: BTreeSet::from([0x10]),
            ..Default::default()
        });
        assert_eq!(lines.executed, BTreeSet::from([line(2), line(3)]));
        assert_eq!(lines.observed, BTreeSet::from([line(2)]));
        assert_eq!(lines.unobserved(), BTreeSet::from([line(3)]));
    }
}
