//! Production PHY source lines the compared observations depend on.
//!
//! Blobray reports which executed production instructions a compared
//! observation depends on. Debug line information of the probe ELF maps them
//! to production PHY source lines, including inlined frames: a line is
//! observed when any of its executed instructions is. Every executed but
//! unobserved line is either reviewed by a decision below, with its reason, or
//! reported as untriaged in the evidence index. A decision that matches no
//! unobserved line fails the run, so the table cannot outlive the code it
//! describes.
use crate::harness::{Result, invalid};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Production PHY sources, relative to the repository root.
pub const SCOPE: &str = "crates/hardware/esp32s31/phy/src";

/// Repository root: this package lives five directories below it.
pub fn root() -> Result<PathBuf> {
    Ok(Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../..")
        .canonicalize()?)
}

/// A source line of one file, relative to the repository root.
pub type SourceLine = (PathBuf, u32);

/// A reviewed decision on an executed line no compared observation depends on.
/// `source_line` is the line's trimmed text, so the decision survives
/// unrelated line shifts.
#[derive(Clone, Copy, Debug)]
pub struct Decision {
    pub file: &'static str,
    pub source_line: &'static str,
    pub reason: &'static str,
}

/// Reviewed unobserved lines.
pub const DECISIONS: &[Decision] = &[];

/// Executed and observed production PHY lines.
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

/// Maps probe instructions to production PHY source lines.
pub struct LineMap {
    lines: BTreeMap<u32, Vec<SourceLine>>,
}

impl LineMap {
    /// The lines within `SCOPE` of every instruction in `pcs` of `elf`.
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
        let scope = root.join(SCOPE);
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
                if let Ok(relative) = Path::new(path).strip_prefix(&scope) {
                    located.push((Path::new(SCOPE).join(relative), line));
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
            if decisions
                .iter()
                .any(|d| line.0 == Path::new(d.file) && text == d.source_line)
            {
                reviewed.insert(line.clone());
            } else {
                untriaged.insert(line.clone());
            }
        }
        Ok((reviewed, untriaged))
    }

    /// Every decision must still match an unobserved line.
    pub fn check(
        &mut self,
        root: &Path,
        decisions: &[Decision],
        unobserved: &BTreeSet<SourceLine>,
    ) -> Result<()> {
        let mut matched = BTreeSet::new();
        for line in unobserved {
            let text = self.text(root, line)?.to_owned();
            for (index, decision) in decisions.iter().enumerate() {
                if line.0 == Path::new(decision.file) && text == decision.source_line {
                    matched.insert(index);
                }
            }
        }
        match decisions
            .iter()
            .enumerate()
            .find(|(index, _)| !matched.contains(index))
        {
            Some((_, stale)) => Err(invalid(format!(
                "observation decision for `{}` in {} matches no unobserved line; \
                 the line is observed or no longer executed",
                stale.source_line, stale.file
            ))),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "crates/hardware/esp32s31/phy/src/example.rs";

    fn root() -> tempfile::TempDir {
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
        file: FILE,
        source_line: "let unused = 1;",
        reason: "test",
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
