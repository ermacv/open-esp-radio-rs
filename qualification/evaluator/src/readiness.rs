//! The readiness of every program, and what dropped since an earlier
//! evaluation: Nightly evaluates every program against the checkout's
//! evidence and compares the reports with the previous night's.
//!
//! A capability's readiness drops when one of its axes was terminal and is
//! no longer, or when it was proof-ready or ready and is no longer. A
//! capability or program the earlier reports lack, or one the checkout no
//! longer selects, is a reviewed catalog change, not a drop.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::Result;
use crate::model::{
    self, AsyncProof, HilProof, HostProof, ImplementationProof, Qualification, VendorProof,
};

/// The report file of `program` (relative to the root) in a readiness
/// directory: its path below the programs' directory, `/` replaced by `-`.
fn report_name(program: &Path) -> Result<String> {
    let relative = program
        .strip_prefix(model::PROGRAMS)
        .map_err(|_| format!("{} is not a qualification program", program.display()))?;
    Ok(relative
        .with_extension("json")
        .to_string_lossy()
        .replace(['/', '\\'], "-"))
}

/// What an earlier or current report says of one capability.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
struct Capability {
    id: String,
    implementation: String,
    host: String,
    vendor: String,
    hil: String,
    r#async: String,
    proof_ready: bool,
    ready: bool,
}

/// The capabilities of a JSON report, read from the fields above only, so a
/// report an earlier evaluator wrote stays comparable.
fn capabilities(path: &Path) -> Result<BTreeMap<String, Capability>> {
    #[derive(Deserialize)]
    struct Report {
        capabilities: Vec<Capability>,
    }
    let report: Report = serde_json::from_slice(&std::fs::read(path)?)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(report
        .capabilities
        .into_iter()
        .map(|capability| (capability.id.clone(), capability))
        .collect())
}

/// Whether `value` of `axis` is terminal, as the evaluator's proof states
/// decide it.
fn terminal(axis: &str, value: &str) -> bool {
    let terminal: Vec<&str> = match axis {
        "implementation" => [
            ImplementationProof::Complete,
            ImplementationProof::Incomplete,
        ]
        .into_iter()
        .filter(|p| p.is_terminal())
        .map(|p| p.label())
        .collect(),
        "host" => [HostProof::Covered, HostProof::Incomplete]
            .into_iter()
            .filter(|p| p.is_terminal())
            .map(|p| p.label())
            .collect(),
        "vendor" => [
            VendorProof::Qualified,
            VendorProof::Mapped,
            VendorProof::Unmapped,
            VendorProof::NotApplicable,
        ]
        .into_iter()
        .filter(|p| p.is_terminal())
        .map(|p| p.label())
        .collect(),
        "hil" => [
            HilProof::Qualified,
            HilProof::Missing,
            HilProof::NotApplicable,
        ]
        .into_iter()
        .filter(|p| p.is_terminal())
        .map(|p| p.label())
        .collect(),
        "async" => [
            AsyncProof::Bounded,
            AsyncProof::Incomplete,
            AsyncProof::NotApplicable,
        ]
        .into_iter()
        .filter(|p| p.is_terminal())
        .map(|p| p.label())
        .collect(),
        _ => vec![],
    };
    terminal.contains(&value)
}

/// One capability's readiness that dropped: what it was and is now.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Regression {
    pub(crate) program: String,
    pub(crate) capability: String,
    pub(crate) what: String,
    pub(crate) before: String,
    pub(crate) now: String,
}

/// What dropped from `before` to `now` in `program`'s capabilities.
fn drops(
    program: &str,
    before: &BTreeMap<String, Capability>,
    now: &BTreeMap<String, Capability>,
) -> Vec<Regression> {
    let mut drops = vec![];
    for (id, then) in before {
        let Some(current) = now.get(id) else { continue };
        let mut regress = |what: &str, before: String, now: String| {
            drops.push(Regression {
                program: program.to_owned(),
                capability: id.clone(),
                what: what.to_owned(),
                before,
                now,
            });
        };
        for (axis, before, now) in [
            (
                "implementation",
                &then.implementation,
                &current.implementation,
            ),
            ("host", &then.host, &current.host),
            ("vendor", &then.vendor, &current.vendor),
            ("hil", &then.hil, &current.hil),
            ("async", &then.r#async, &current.r#async),
        ] {
            if terminal(axis, before) && !terminal(axis, now) {
                regress(axis, before.clone(), now.clone());
            }
        }
        for (what, before, now) in [
            ("proof-ready", then.proof_ready, current.proof_ready),
            ("ready", then.ready, current.ready),
        ] {
            if before && !now {
                regress(what, "true".into(), "false".into());
            }
        }
    }
    drops
}

/// The readable list of `drops`, for an issue.
pub(crate) fn render(drops: &[Regression]) -> String {
    let mut text = String::from(
        "| Program | Capability | Axis | Before | Now |\n| --- | --- | --- | --- | --- |\n",
    );
    for regression in drops {
        let _ = writeln!(
            text,
            "| `{}` | `{}` | {} | `{}` | `{}` |",
            regression.program,
            regression.capability,
            regression.what,
            regression.before,
            regression.now
        );
    }
    text
}

/// Evaluate every program below `root`, write its JSON report into `out`,
/// and compare each with its report in `base` when that holds one; the
/// drops found, also written to `drops_file` as a table when there are any.
pub(crate) fn run(
    root: &Path,
    out: &Path,
    base: Option<&Path>,
    drops_file: Option<&Path>,
) -> Result<Vec<Regression>> {
    let mut found = vec![];
    let mut compared = 0;
    let programs = model::program_paths(root)?;
    for program in &programs {
        let qualification = Qualification::load_and_evaluate(&root.join(program), root)?;
        let name = report_name(program)?;
        let report = out.join(&name);
        crate::report::write_json(&qualification, &report)?;
        let Some(earlier) = base.map(|base| base.join(&name)).filter(|p| p.is_file()) else {
            continue;
        };
        compared += 1;
        found.extend(drops(
            &program.display().to_string(),
            &capabilities(&earlier)?,
            &capabilities(&report)?,
        ));
    }
    for regression in &found {
        println!(
            "READINESS-DROP\t{}\t{}\t{}\t{} -> {}",
            regression.program,
            regression.capability,
            regression.what,
            regression.before,
            regression.now
        );
    }
    println!(
        "READINESS\tprograms={}\tcompared={compared}\tdrops={}",
        programs.len(),
        found.len()
    );
    if let Some(path) = drops_file {
        if found.is_empty() {
            match std::fs::remove_file(path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    return Err(error.into());
                }
                _ => {}
            }
        } else {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, render(&found))?;
        }
    }
    Ok(found)
}

/// `path` below `root` when relative.
pub(crate) fn rooted(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        root.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capability(id: &str, vendor: &str, ready: bool) -> (String, Capability) {
        (
            id.to_owned(),
            Capability {
                id: id.to_owned(),
                implementation: "complete".into(),
                host: "covered".into(),
                vendor: vendor.into(),
                hil: "missing".into(),
                r#async: "not-applicable".into(),
                proof_ready: ready,
                ready,
            },
        )
    }

    #[test]
    fn readiness_drops_when_a_terminal_axis_or_readiness_is_lost() {
        let before = BTreeMap::from([
            capability("wifi.scan", "qualified", true),
            capability("wifi.join", "mapped", false),
            capability("wifi.removed", "qualified", true),
        ]);
        let now = BTreeMap::from([
            capability("wifi.scan", "mapped", false),
            // A non-terminal axis that changes, or gains, drops nothing.
            capability("wifi.join", "qualified", false),
            capability("wifi.added", "unmapped", false),
        ]);
        let found = drops("wifi-sta", &before, &now);
        let what: Vec<_> = found
            .iter()
            .map(|d| (d.capability.as_str(), d.what.as_str()))
            .collect();
        assert_eq!(
            what,
            [
                ("wifi.scan", "vendor"),
                ("wifi.scan", "proof-ready"),
                ("wifi.scan", "ready")
            ]
        );
        assert_eq!(
            (found[0].before.as_str(), found[0].now.as_str()),
            ("qualified", "mapped")
        );
        assert!(drops("wifi-sta", &before, &before).is_empty());
        assert!(
            render(&found)
                .contains("| `wifi-sta` | `wifi.scan` | vendor | `qualified` | `mapped` |")
        );
    }

    #[test]
    fn terminal_states_follow_the_evaluator() {
        for (axis, value, expected) in [
            ("implementation", "complete", true),
            ("implementation", "incomplete", false),
            ("host", "covered", true),
            ("vendor", "qualified", true),
            ("vendor", "not-applicable", true),
            ("vendor", "mapped", false),
            ("hil", "missing", false),
            ("async", "bounded", true),
            ("unknown", "complete", false),
        ] {
            assert_eq!(terminal(axis, value), expected, "{axis}={value}");
        }
    }

    #[test]
    fn a_report_is_named_by_its_program_path() {
        let program = Path::new(model::PROGRAMS).join("esp32s31/wifi-sta.toml");
        assert_eq!(report_name(&program).unwrap(), "esp32s31-wifi-sta.json");
        assert!(report_name(Path::new("elsewhere/a.toml")).is_err());
    }

    #[test]
    fn an_earlier_report_reads_from_its_capabilities_alone() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("a.json");
        std::fs::write(
            &path,
            r#"{"schema": 1, "gone": true, "capabilities": [{"id": "x", "implementation": "complete",
                "host": "covered", "vendor": "qualified", "hil": "missing", "async": "bounded",
                "proof_ready": false, "ready": false, "gone": []}]}"#,
        )
        .unwrap();
        assert_eq!(capabilities(&path).unwrap()["x"].vendor, "qualified");
    }
}
