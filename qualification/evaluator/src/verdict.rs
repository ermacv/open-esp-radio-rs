//! `verdict --run PATH`: one run bundle judged against the HIL requirements
//! of every program by the same evaluator, for work on the stand (owner
//! decision
//! [6225298347](https://github.com/ermacv/open-esp-radio-rs/issues/761#issuecomment-6225298347)).
//!
//! The run is not in the pinned evidence archive: every line says it is not
//! published and not evidence, and nothing here reaches `status`, `next`,
//! `plan`, `readiness` or `evaluate`, which read only the archive.

use std::{fmt, path::Path};

use crate::{
    Result,
    hil::EvidenceStatus,
    model::{self, Qualification},
};

/// A requirement the run observed and did not meet.
#[derive(Debug)]
pub(crate) struct Unmet(pub(crate) usize);

impl fmt::Display for Unmet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the run does not meet {} requirement(s) it observed",
            self.0
        )
    }
}

impl std::error::Error for Unmet {}

/// There is nothing to judge: no run bundle at the path, or a run that
/// observed no scenario a program requires or is of no program's chip.
#[derive(Debug)]
pub(crate) struct Unjudged(pub(crate) String);

impl fmt::Display for Unjudged {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Unjudged {}

/// The run bundle failed validation, so nothing can be judged on it.
#[derive(Debug)]
pub(crate) struct Invalid(pub(crate) String);

impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the run bundle is not valid: {}", self.0)
    }
}

impl std::error::Error for Invalid {}

/// One requirement the run observed: the program, the capability and the
/// evaluator's decision.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Judged {
    pub(crate) program: String,
    pub(crate) capability: String,
    pub(crate) scenario: String,
    pub(crate) status: &'static str,
    pub(crate) passed: usize,
    pub(crate) failed: usize,
    pub(crate) excluded: usize,
}

/// Every requirement of every program below `root` that the run at `run`
/// observed, as the evaluator decides it on that run alone; [`Invalid`],
/// with the reason, when its bundle fails validation.
pub(crate) fn judge(root: &Path, run: &Path) -> Result<Vec<Judged>> {
    if !run.is_dir() {
        return Err(Box::new(Unjudged(format!(
            "{} holds no run bundle",
            run.display()
        ))));
    }
    let mut judged = Vec::new();
    for program in model::program_paths(root)? {
        oer_process::check_cancelled().map_err(crate::boxed)?;
        let qualification = Qualification::load_and_judge(&root.join(&program), root, run)?;
        if let Some(invalid) = qualification.evidence_inputs.hil.invalid.first() {
            return Err(Box::new(Invalid(invalid.reason.clone())));
        }
        for capability in qualification.capabilities.values() {
            for decision in &capability.hil_decisions {
                let counts = decision.observation_counts();
                if counts.total == 0 {
                    continue;
                }
                judged.push(Judged {
                    program: program.display().to_string(),
                    capability: capability.id.clone(),
                    scenario: decision.scenario.clone(),
                    status: decision.status.label(),
                    passed: counts.passed,
                    failed: counts.failed,
                    excluded: counts.excluded,
                });
            }
        }
    }
    // The last program's evaluation checked for cancellation only between
    // runs: check once more before a verdict.
    oer_process::check_cancelled().map_err(crate::boxed)?;
    Ok(judged)
}

/// `cargo qualification verdict --run PATH`.
pub(crate) fn run(root: &Path, run: &Path) -> Result<()> {
    let run = root.join(run);
    let judged = judge(root, &run)?;
    let (lines, outcome) = report(&run, &judged);
    for line in lines {
        println!("{line}");
    }
    outcome
}

/// The lines `verdict` prints for the run at `run`, every one marked not
/// evidence, and its outcome: [`Unmet`] when the run does not meet a
/// requirement it observed, an error when it observed none.
pub(crate) fn report(run: &Path, judged: &[Judged]) -> (Vec<String>, Result<()>) {
    let unmet = judged
        .iter()
        .filter(|judged| judged.status != EvidenceStatus::Satisfied.label())
        .count();
    let mut lines = vec![format!(
        "VERDICT\tnot-published\tnot-evidence\trun={}",
        run.display()
    )];
    lines.extend(judged.iter().map(|judged| {
        format!(
            "VERDICT-REQUIREMENT\t{}\t{}\tscenario={}\tstatus={}\tpassed={}\tfailed={}\texcluded={}\tnot-evidence",
            judged.program,
            judged.capability,
            judged.scenario,
            judged.status,
            judged.passed,
            judged.failed,
            judged.excluded
        )
    }));
    lines.push(format!(
        "VERDICT-SUMMARY\trequirements={}\tunmet={unmet}\tnot-published\tnot-evidence",
        judged.len()
    ));
    let outcome = if judged.is_empty() {
        Err(Box::new(Unjudged(format!(
            "{} gives no verdict: it observed no scenario a program requires, or it is of no program's chip, of another run schema or not complete",
            run.display()
        ))) as Box<dyn std::error::Error>)
    } else if unmet > 0 {
        Err(Box::new(Unmet(unmet)) as Box<dyn std::error::Error>)
    } else {
        Ok(())
    };
    (lines, outcome)
}
