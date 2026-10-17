//! `check references`: the evidence archive pin, rule 1 and rule 2, offline (#761
//! Q7; owner decision
//! [6225000924](https://github.com/ermacv/open-esp-radio-rs/issues/761#issuecomment-6225000924)).
//!
//! This check reads no runs and needs no access to the private archive: the pin repeats its version's manifest, so its
//! digest and every cited run are checked against the pin alone: every
//! cited run is in the pinned version with its digest (rule 2), and the
//! version holds no run nothing cites (rule 1). It also loads every program
//! and its catalogs without evidence.

use std::path::Path;

use oer_hil_evidence_archive::{PIN, Pin, Refused, RunRef, pin::unpinned_in};

use crate::{Result, model};

/// What the check found.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Checked {
    /// The pinned version, if any, unless a candidate was checked.
    pub(crate) version: Option<String>,
    /// The candidate checked instead of the pinned version, if any.
    pub(crate) candidate: Option<String>,
    /// The version the rules held for, as messages name it.
    pub(crate) label: String,
    pub(crate) programs: usize,
    pub(crate) citations: usize,
    /// Each citation the checked version (the candidate, else the pinned
    /// one) does not hold with its digest.
    pub(crate) unpinned: Vec<String>,
    /// Each run the checked version holds that no record cites.
    pub(crate) uncited: Vec<String>,
}

/// A version to check instead of the pinned one: the runs `cargo hil
/// archive pack` or `publish` is about to put in a version, checked before
/// it creates anything (`--candidate FILE`).
#[derive(Debug)]
pub(crate) struct Candidate {
    pub(crate) path: String,
    pub(crate) runs: Vec<RunRef>,
}

impl Candidate {
    /// The `runs` of a references file (TOML or JSON) or of a version's
    /// `manifest.json`.
    pub(crate) fn read(path: &Path) -> Result<Self> {
        #[derive(serde::Deserialize)]
        struct Runs {
            runs: Vec<RunRef>,
        }
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let runs: Runs = if text.trim_start().starts_with('{') {
            serde_json::from_str(&text).map_err(|error| error.to_string())
        } else {
            toml::from_str(&text).map_err(|error| error.to_string())
        }
        .map_err(|error| Refused(format!("{}: {error}", path.display())))?;
        Ok(Self {
            path: path.display().to_string(),
            runs: runs.runs,
        })
    }
}

/// Check the repository at `root` against the run references `citations`
/// (what cites each, the reference) its tracked records make, and rules 1
/// and 2 against `candidate` when given, else against the pinned version.
pub(crate) fn check(
    root: &Path,
    citations: &[(String, RunRef)],
    candidate: Option<&Candidate>,
) -> Result<Checked> {
    let pin = Pin::load(&root.join(PIN)).map_err(crate::boxed)?;
    let programs = model::program_paths(root)?;
    for program in &programs {
        oer_process::check_cancelled().map_err(crate::boxed)?;
        // A program or catalog that does not load breaks a rule (exit 1),
        // a file it names missing included; a failure to read one is the
        // check's (exit 3).
        model::CatalogView::load_for_program(root, &root.join(program)).map_err(
            |error| -> Box<dyn std::error::Error> {
                if crate::hil::environment_error(&*error) {
                    error
                } else {
                    Box::new(Refused(format!("{}: {error}", program.display())))
                }
            },
        )?;
    }
    // The version the rules hold for: the candidate, else the pinned one.
    let (label, version, held) = match (candidate, pin.as_ref()) {
        (Some(candidate), _) => (
            format!("the candidate version in {}", candidate.path),
            None,
            Some(candidate.runs.as_slice()),
        ),
        (None, Some(pin)) => (
            format!("the pinned version {}", pin.version()),
            Some(pin.version().to_string()),
            Some(pin.manifest().runs.as_slice()),
        ),
        (None, None) => (String::from("the pinned version"), None, None),
    };
    oer_process::check_cancelled().map_err(crate::boxed)?;
    let unpinned = unpinned_in(
        held,
        &label,
        citations
            .iter()
            .map(|(citation, reference)| (citation.clone(), reference)),
    );
    let cited = citations
        .iter()
        .map(|(_, reference)| &reference.run)
        .collect::<std::collections::BTreeSet<_>>();
    let uncited = held
        .unwrap_or_default()
        .iter()
        .filter(|held| !cited.contains(&held.run))
        .map(|held| format!("{label} holds run {}, which no record cites", held.run))
        .collect();
    Ok(Checked {
        version,
        candidate: candidate.map(|candidate| candidate.path.clone()),
        label,
        programs: programs.len(),
        citations: citations.len(),
        unpinned: unpinned.iter().map(ToString::to_string).collect(),
        uncited,
    })
}

/// The run references tracked records cite. No tracked record cites a run
/// yet. The stand profile's type already holds its spans' `evidence`
/// references, but no profile is tracked: the PR that tracks the first one
/// adds its reader here. Catalog decisions and measured bounds get their
/// citing field with their first citing record, with its reader here and its
/// test, in one PR (owner decision 6226361520 on #761). Until then catalogs, programs
/// and the stand profile refuse unknown fields, so no reference is written
/// past this reader.
fn citations(_root: &Path) -> Vec<(String, RunRef)> {
    Vec::new()
}

/// The lines `check references` prints: each unpinned citation, then the
/// summary, which always states how many citations were checked.
pub(crate) fn lines(checked: &Checked) -> Vec<String> {
    let mut lines = checked
        .unpinned
        .iter()
        .map(|line| format!("UNPINNED\t{line}"))
        .chain(
            checked
                .uncited
                .iter()
                .map(|line| format!("UNCITED\t{line}")),
        )
        .collect::<Vec<_>>();
    lines.push(format!(
        "REFERENCES\t{}\tprograms={}\tcitations={}\tunpinned={}\tuncited={}",
        match &checked.candidate {
            Some(candidate) => format!("candidate={candidate}"),
            None => format!("pin={}", checked.version.as_deref().unwrap_or("none")),
        },
        checked.programs,
        checked.citations,
        checked.unpinned.len(),
        checked.uncited.len()
    ));
    lines
}

/// `cargo qualification check references`.
pub(crate) fn run(root: &Path, candidate: Option<&Path>) -> Result<()> {
    let candidate = candidate
        .map(|path| Candidate::read(&root.join(path)))
        .transpose()?;
    let checked = check(root, &citations(root), candidate.as_ref())?;
    for line in lines(&checked) {
        println!("{line}");
    }
    if checked.unpinned.is_empty() && checked.uncited.is_empty() {
        Ok(())
    } else {
        let mut broken = Vec::new();
        if !checked.unpinned.is_empty() {
            broken.push(format!(
                "{} cited runs are not in {} (rule 2)",
                checked.unpinned.len(),
                checked.label
            ));
        }
        if !checked.uncited.is_empty() {
            broken.push(format!(
                "{} holds {} runs no record cites (rule 1)",
                checked.label,
                checked.uncited.len()
            ));
        }
        Err(Box::new(Refused(broken.join("; "))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_hil_evidence_archive::{Manifest, Payload, Repository, RunId, Sha256};

    fn root() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(model::PROGRAMS)).unwrap();
        root
    }

    fn reference(run: &str, seal: &[u8]) -> RunRef {
        RunRef {
            run: RunId::parse(run).unwrap(),
            sha256: Sha256::of(seal),
        }
    }

    fn pin(root: &Path, runs: Vec<RunRef>) -> Pin {
        let manifest = Manifest::new(
            Payload {
                sha256: Sha256::of(b"payload"),
                bytes: 7,
            },
            runs,
        )
        .unwrap();
        let pin = Pin::new(Repository::parse("o/evidence").unwrap(), manifest);
        pin.write(&root.join(PIN)).unwrap();
        pin
    }

    #[test]
    fn without_a_pin_no_citation_holds() {
        let root = root();
        let cited = [(String::from("profile"), reference("1-a", b"seal"))];
        let checked = check(root.path(), &cited, None).unwrap();
        assert_eq!(checked.version, None);
        assert_eq!(checked.unpinned.len(), 1);
        assert!(check(root.path(), &[], None).unwrap().unpinned.is_empty());
    }

    /// Rule 2 on synthetic catalog citations: one the pinned version holds
    /// with its digest, one with another SHA-256, one outside the manifest.
    #[test]
    fn rule_two_holds_catalog_citations_to_the_pin() {
        let root = root();
        let pin = pin(root.path(), vec![reference("1-a", b"seal")]);
        let cite = |what: &str, run: &str, seal: &[u8]| {
            (
                format!("qualification/catalog/c.toml: {what}"),
                reference(run, seal),
            )
        };
        let citations = [
            cite("decision `base`", "1-a", b"seal"),
            cite("bound `base.rate`", "1-a", b"other"),
            cite("decision `edge`", "2-b", b"seal"),
        ];
        let checked = check(root.path(), &citations, None).unwrap();
        assert_eq!(checked.version, Some(pin.version().to_string()));
        assert_eq!(checked.citations, 3);
        assert_eq!(checked.unpinned.len(), 2);
        assert!(checked.unpinned[0].contains("bound `base.rate` cites run 1-a with SHA-256"));
        assert!(checked.unpinned[0].contains(&format!(
            "but the pinned version {} holds it with",
            pin.version()
        )));
        assert!(checked.unpinned[1].contains(&format!(
            "decision `edge` cites run 2-b, which the pinned version {} does not hold",
            pin.version()
        )));
        let lines = lines(&checked);
        assert_eq!(lines.len(), 3);
        assert!(
            lines[2].contains("\tcitations=3\tunpinned=2\tuncited=0"),
            "{lines:?}"
        );
    }

    /// `pack` and `publish` check a candidate before creating anything:
    /// the same rules, against its runs instead of the pinned version.
    #[test]
    fn a_candidate_is_held_to_both_rules_instead_of_the_pin() {
        let root = root();
        pin(root.path(), vec![reference("1-a", b"seal")]);
        let cited = [(String::from("decision `base`"), reference("2-b", b"seal"))];
        let candidate = |runs: Vec<RunRef>| Candidate {
            path: String::from("refs.toml"),
            runs,
        };
        // The pinned version fails both rules; the candidate that holds
        // exactly the citation passes.
        let pinned = check(root.path(), &cited, None).unwrap();
        assert_eq!((pinned.unpinned.len(), pinned.uncited.len()), (1, 1));
        let exact = check(
            root.path(),
            &cited,
            Some(&candidate(vec![reference("2-b", b"seal")])),
        )
        .unwrap();
        assert!(
            exact.unpinned.is_empty() && exact.uncited.is_empty(),
            "{exact:?}"
        );
        assert_eq!(exact.version, None);
        assert_eq!(exact.candidate.as_deref(), Some("refs.toml"));
        assert!(
            lines(&exact)
                .last()
                .unwrap()
                .starts_with("REFERENCES\tcandidate=refs.toml\t")
        );
        // A candidate missing the citation, or holding an uncited run, fails.
        let short = check(
            root.path(),
            &cited,
            Some(&candidate(vec![reference("1-a", b"seal")])),
        )
        .unwrap();
        assert_eq!((short.unpinned.len(), short.uncited.len()), (1, 1));
        assert!(short.unpinned[0].contains("the candidate version in refs.toml does not hold"));
        // A references file and a manifest both read as candidates.
        let file = root.path().join("refs.json");
        std::fs::write(
            &file,
            serde_json::json!({"schema": 1, "runs": [reference("2-b", b"seal")]}).to_string(),
        )
        .unwrap();
        assert_eq!(
            Candidate::read(&file).unwrap().runs,
            [reference("2-b", b"seal")]
        );
    }

    /// Rule 1: the pinned version holds exactly the cited runs.
    #[test]
    fn rule_one_refuses_a_pinned_run_no_record_cites() {
        let root = root();
        pin(
            root.path(),
            vec![reference("1-a", b"seal"), reference("2-b", b"seal")],
        );
        let cited = [(String::from("decision `base`"), reference("1-a", b"seal"))];
        let checked = check(root.path(), &cited, None).unwrap();
        assert!(checked.unpinned.is_empty());
        assert_eq!(checked.uncited.len(), 1);
        assert!(checked.uncited[0].contains("holds run 2-b, which no record cites"));
        assert!(lines(&checked)[0].starts_with("UNCITED\t"));
        // Nothing cited: every pinned run is uncited.
        assert_eq!(check(root.path(), &[], None).unwrap().uncited.len(), 2);
    }

    #[test]
    fn the_summary_states_the_citation_count_even_when_none() {
        let root = root();
        let checked = check(root.path(), &citations(root.path()), None).unwrap();
        assert_eq!(
            lines(&checked),
            ["REFERENCES\tpin=none\tprograms=0\tcitations=0\tunpinned=0\tuncited=0"]
        );
    }

    #[test]
    fn a_program_that_does_not_load_is_a_refusal() {
        let root = root();
        std::fs::write(
            root.path().join(model::PROGRAMS).join("broken.toml"),
            "schema = 5\n",
        )
        .unwrap();
        let error = check(root.path(), &[], None).unwrap_err();
        assert!(error.is::<Refused>(), "{error}");
        assert!(error.to_string().contains("broken.toml"), "{error}");
    }

    /// A checkout without a programs directory selects no program: the check
    /// holds the pin to rule 2 alone.
    #[test]
    fn no_programs_directory_is_no_program() {
        let root = root();
        std::fs::remove_dir_all(root.path().join(model::PROGRAMS)).unwrap();
        assert!(model::program_paths(root.path()).unwrap().is_empty());
        check(root.path(), &[], None).unwrap();
    }

    /// A file a program names that is missing breaks a rule (exit 1), even
    /// reported as an I/O error; any other I/O failure is the check's.
    #[test]
    fn a_missing_file_is_a_rule_other_io_the_checks() {
        use std::io::{Error, ErrorKind};
        let missing: Box<dyn std::error::Error> = Box::new(Error::new(ErrorKind::NotFound, "x"));
        assert!(!crate::hil::environment_error(&*missing));
        let denied: Box<dyn std::error::Error> =
            Box::new(Error::new(ErrorKind::PermissionDenied, "x"));
        assert!(crate::hil::environment_error(&*denied));
    }

    /// A program that cannot be read fails the check (exit 3), not a rule.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_program_is_the_environment_not_a_refusal() {
        use std::os::unix::fs::PermissionsExt;
        let root = root();
        let program = root.path().join(model::PROGRAMS).join("p.toml");
        std::fs::write(&program, "schema = 5\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o000)).unwrap();
        let readable = std::fs::read(&program).is_ok();
        let result = check(root.path(), &[], None);
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o644)).unwrap();
        if !readable {
            let error = result.unwrap_err();
            assert!(!error.is::<Refused>(), "{error}");
            assert!(error.is::<std::io::Error>(), "{error}");
        }
        // Readable but broken: a rule.
        assert!(check(root.path(), &[], None).unwrap_err().is::<Refused>());
        // Content that is not UTF-8 is a broken program too, not a failure
        // to read it.
        std::fs::write(&program, b"\xff\xfe").unwrap();
        let error = check(root.path(), &[], None).unwrap_err();
        assert!(error.is::<Refused>(), "{error}");
    }

    #[test]
    fn an_edited_pin_is_a_refusal() {
        let root = root();
        pin(root.path(), vec![reference("1-a", b"seal")]);
        let path = root.path().join(PIN);
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replace("bytes = 7", "bytes = 8")).unwrap();
        let error = check(root.path(), &[], None).unwrap_err();
        assert!(error.is::<Refused>(), "{error}");
    }
}
