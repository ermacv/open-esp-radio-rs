//! Run every mutant of executed production code against the scenarios that
//! reach it.
//!
//! Each worker owns a detached git worktree of `HEAD` with its own Cargo
//! target directory, so probe rebuilds stay incremental and workers never
//! share a build. A baseline run of every scenario, from worker zero's probe,
//! records which production instructions each scenario reaches; the mutants
//! and the scenarios each one runs follow from that reach. A mutant is
//! `killed` by the first scenario that fails, `survived` when all of them
//! pass, `equivalent` when its executable code equals the baseline's, and
//! `unviable` when it does not build.
use crate::harness::{Result, invalid};
use crate::mutation::{Mutant, executed_lines, generate};
use crate::{PROBES_MANIFEST, PROBES_PACKAGE, PROBES_TARGET};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

pub struct Campaign {
    /// Repository root; its tracked files must equal `HEAD`.
    pub root: PathBuf,
    /// Production source directory to mutate, relative to `root`.
    pub scope: PathBuf,
    /// Ignored output root for worktrees, logs and the report.
    pub output: PathBuf,
    pub workers: usize,
    /// This scenario executable.
    pub scenarios: PathBuf,
    /// Arguments every scenario receives, except `--production` and `--output`.
    pub common: Vec<OsString>,
    /// Each scenario's own arguments.
    pub suites: BTreeMap<String, Vec<OsString>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Outcome {
    Killed { scenario: String },
    Survived,
    Equivalent,
    Unviable,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MutantResult {
    pub id: String,
    pub mutant: Mutant,
    /// Scenarios whose baseline reached the mutated lines.
    pub scenarios: Vec<String>,
    pub outcome: Outcome,
    pub seconds: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Report {
    pub commit: String,
    pub scope: PathBuf,
    pub executed_lines: usize,
    pub results: Vec<MutantResult>,
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(invalid(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

struct Worker {
    tree: PathBuf,
    target: PathBuf,
}

impl Worker {
    /// A clean detached worktree of `commit`, reused between runs.
    fn prepare(root: &Path, output: &Path, index: usize, commit: &str) -> Result<Self> {
        let tree = std::path::absolute(output.join(format!("worker-{index}")))?;
        if tree.join(".git").exists() {
            git(&tree, &["checkout", "--detach", "--force", commit])?;
            git(&tree, &["reset", "--hard", commit])?;
        } else {
            git(
                root,
                &[
                    "worktree",
                    "add",
                    "--detach",
                    &tree.to_string_lossy(),
                    commit,
                ],
            )?;
        }
        Ok(Self {
            target: tree.join("target/mutation-probes"),
            tree,
        })
    }

    fn elf(&self) -> PathBuf {
        self.target
            .join(PROBES_TARGET)
            .join("release")
            .join(PROBES_PACKAGE)
    }

    /// Build the probe ELF; false when the source does not compile.
    fn build(&self, log: &Path) -> Result<bool> {
        let status = Command::new("cargo")
            .args(["build", "--release", "--locked", "--manifest-path"])
            .arg(self.tree.join(PROBES_MANIFEST))
            .args(["--package", PROBES_PACKAGE, "--target", PROBES_TARGET])
            .env("CARGO_TARGET_DIR", &self.target)
            .stdout(std::fs::File::create(log)?)
            .stderr(std::fs::File::create(log.with_extension("stderr"))?)
            .status()?;
        Ok(status.success())
    }
}

/// Bytes of every executable section, which a mutant must change to matter.
fn code(elf: &Path) -> Result<Vec<u8>> {
    use object::{Object, ObjectSection, SectionFlags};
    let bytes = std::fs::read(elf)?;
    let file = object::File::parse(&*bytes)?;
    let mut code = vec![];
    for section in file.sections() {
        if matches!(section.flags(), SectionFlags::Elf { sh_flags }
            if sh_flags & u64::from(object::elf::SHF_EXECINSTR) != 0)
        {
            code.extend_from_slice(section.data()?);
        }
    }
    Ok(code)
}

impl Campaign {
    fn scenario(
        &self,
        suite: &str,
        production: &Path,
        output: &Path,
        reach: Option<&Path>,
        log: &Path,
    ) -> Result<bool> {
        let mut command = Command::new(&self.scenarios);
        command
            .arg(suite)
            .args(&self.common)
            .args(&self.suites[suite])
            .arg("--production")
            .arg(production)
            .arg("--output")
            .arg(output);
        if let Some(reach) = reach {
            command.arg("--reach").arg(reach);
        }
        let status = command
            .stdout(std::fs::File::create(log)?)
            .stderr(std::fs::File::create(log.with_extension("stderr"))?)
            .status()?;
        let _ = std::fs::remove_dir_all(output);
        Ok(status.success())
    }

    pub fn run(&self) -> Result<Report> {
        if !git(
            &self.root,
            &["status", "--porcelain", "--untracked-files=no"],
        )?
        .is_empty()
        {
            return Err(invalid(
                "mutation workers check out HEAD: commit or stash tracked changes first",
            ));
        }
        let commit = git(&self.root, &["rev-parse", "HEAD"])?;
        let logs = self.output.join("logs");
        std::fs::create_dir_all(&logs)?;
        let workers = (0..self.workers.max(1))
            .map(|i| Worker::prepare(&self.root, &self.output, i, &commit))
            .collect::<Result<Vec<_>>>()?;
        eprintln!("building {} baseline probes", workers.len());
        std::thread::scope(|scope| -> Result<()> {
            let builds: Vec<_> = workers
                .iter()
                .enumerate()
                .map(|(i, w)| {
                    let log = logs.join(format!("baseline-{i}.log"));
                    scope.spawn(move || w.build(&log))
                })
                .collect();
            for build in builds {
                if !build
                    .join()
                    .map_err(|_| invalid("baseline build panicked"))??
                {
                    return Err(invalid("baseline probe does not build"));
                }
            }
            Ok(())
        })?;
        let baseline = workers[0].elf();
        let baseline_code = code(&baseline)?;
        eprintln!("running {} baseline scenarios", self.suites.len());
        let reach: BTreeMap<String, BTreeSet<u32>> = std::thread::scope(|scope| {
            let runs: Vec<_> = self
                .suites
                .keys()
                .map(|suite| {
                    let baseline = &baseline;
                    let output = self.output.join(format!("baseline-{suite}"));
                    let path = self.output.join(format!("reach-{suite}.json"));
                    let log = logs.join(format!("baseline-{suite}.log"));
                    scope.spawn(move || -> Result<BTreeMap<String, BTreeSet<u32>>> {
                        if !self.scenario(suite, baseline, &output, Some(&path), &log)? {
                            return Err(invalid(format!(
                                "baseline scenario {suite} failed; see {}",
                                log.display()
                            )));
                        }
                        Ok(serde_json::from_slice(&std::fs::read(&path)?)?)
                    })
                })
                .collect();
            let mut reach = BTreeMap::new();
            for run in runs {
                reach.extend(run.join().map_err(|_| invalid("baseline panicked"))??);
            }
            Ok::<_, crate::harness::Error>(reach)
        })?;
        let lines = executed_lines(&baseline, &reach, &workers[0].tree, &self.scope)?;
        let mut by_file: BTreeMap<PathBuf, BTreeMap<u32, BTreeSet<String>>> = BTreeMap::new();
        for ((file, line), suites) in &lines {
            by_file
                .entry(file.clone())
                .or_default()
                .insert(*line, suites.clone());
        }
        let mut queue = VecDeque::new();
        for (file, executed) in &by_file {
            let source = std::fs::read_to_string(workers[0].tree.join(file))?;
            let numbers: BTreeSet<u32> = executed.keys().copied().collect();
            let file_suites: BTreeSet<String> = executed.values().flatten().cloned().collect();
            for mutant in generate(file, &source, &numbers)? {
                let last = source[..mutant.end].matches('\n').count() as u32 + 1;
                let mut suites: BTreeSet<String> = executed
                    .range(mutant.line..=last)
                    .flat_map(|(_, s)| s.iter().cloned())
                    .collect();
                if suites.is_empty() {
                    suites = file_suites.clone();
                }
                queue.push_back((mutant, suites));
            }
        }
        let total = queue.len();
        eprintln!("{} executed lines, {total} mutants", lines.len());
        let queue = Mutex::new(queue);
        let results = Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            for (index, worker) in workers.iter().enumerate() {
                let (queue, results, baseline_code, logs) =
                    (&queue, &results, &baseline_code, &logs);
                scope.spawn(move || -> Result<()> {
                    for attempt in 0.. {
                        let Some((mutant, suites)) = queue.lock().unwrap().pop_front() else {
                            return Ok(());
                        };
                        let start = std::time::Instant::now();
                        let path = worker.tree.join(&mutant.file);
                        let original = std::fs::read_to_string(&path)?;
                        std::fs::write(&path, mutant.apply(&original))?;
                        let tag = format!("{index}-{attempt}");
                        let outcome = (|| -> Result<Outcome> {
                            if !worker.build(&logs.join(format!("{tag}-build.log")))? {
                                return Ok(Outcome::Unviable);
                            }
                            if code(&worker.elf())? == *baseline_code {
                                return Ok(Outcome::Equivalent);
                            }
                            for suite in &suites {
                                let output = worker.tree.join("target/mutation-run");
                                let log = logs.join(format!("{tag}-{suite}.log"));
                                if !self.scenario(suite, &worker.elf(), &output, None, &log)? {
                                    return Ok(Outcome::Killed {
                                        scenario: suite.clone(),
                                    });
                                }
                            }
                            Ok(Outcome::Survived)
                        })();
                        std::fs::write(&path, original)?;
                        let outcome = outcome?;
                        let mut results = results.lock().unwrap();
                        eprintln!(
                            "[{}/{total}] {} {:?}",
                            results.len() + 1,
                            mutant.id(),
                            outcome
                        );
                        results.push(MutantResult {
                            id: mutant.id(),
                            mutant,
                            scenarios: suites.into_iter().collect(),
                            outcome,
                            seconds: start.elapsed().as_secs_f64(),
                        });
                    }
                    Ok(())
                });
            }
        });
        let mut results = results.into_inner().unwrap();
        results.sort_by(|a, b| {
            (&a.mutant.file, a.mutant.start).cmp(&(&b.mutant.file, b.mutant.start))
        });
        if results.len() != total {
            return Err(invalid(
                "a mutation worker stopped before the queue emptied",
            ));
        }
        Ok(Report {
            commit,
            scope: self.scope.clone(),
            executed_lines: lines.len(),
            results,
        })
    }
}

/// Lines each mutant touches, for tests.
#[cfg(test)]
fn touched(mutant: &Mutant, source: &str) -> (u32, u32) {
    (
        mutant.line,
        source[..mutant.end].matches('\n').count() as u32 + 1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_multi_line_mutant_covers_every_touched_line() {
        let source = "fn f(b: &B) {\n    b.a.write(1);\n    b.b.write(2);\n}\n";
        let order = generate(Path::new("f.rs"), source, &BTreeSet::from([2, 3]))
            .unwrap()
            .into_iter()
            .find(|m| m.operator == crate::mutation::Operator::Order)
            .unwrap();
        assert_eq!(touched(&order, source), (2, 3));
    }
}
