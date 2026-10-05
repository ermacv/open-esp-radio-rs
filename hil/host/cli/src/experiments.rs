//! `cargo hil ab` and `cargo hil bisect`: their arguments, and calls into
//! `oer-hil-experiment`.

use std::num::NonZeroU32;

use oer_hil_experiment::{ab, bisect, launch::Runner};
use oer_hil_run_bundle::RunId;
use oer_hil_schema::run::Outcome;
use oer_process::Checkout;

use crate::Result;

#[derive(clap::Args)]
pub(crate) struct AbCli {
    /// Variant A: `rev=<revision>`, then `;override:<esp-hal|embassy|xarxa>=<path>`
    /// for each replaced dependency; `rev=` defaults to HEAD.
    #[arg(long)]
    a: String,
    /// Variant B, in the same form.
    #[arg(long)]
    b: String,
    #[arg(long = "scenario", required = true)]
    scenarios: Vec<String>,
    /// Runs of each arm per layout seed.
    #[arg(long, default_value_t = 3)]
    repetitions: u32,
    /// Layout seeds 1..=K, each built and run for both arms.
    #[arg(long, default_value_t = 1)]
    layout_seeds: u32,
    #[command(flatten)]
    boards: crate::jobs::BoardChoiceArgs,
    /// The stand makes the comparison a job as it does a run; these are
    /// taken from the arguments before `cli` is read.
    #[command(flatten)]
    _job: crate::jobs::JobArgs,
}

/// Run the comparison `args` describe with `runner`; returns each created
/// run with its outcome.
pub(crate) fn ab(
    ctx: &Checkout,
    owner: &str,
    runner: &Runner,
    cli: AbCli,
) -> Result<Vec<(RunId, Option<Outcome>)>> {
    let finished = ab::run(
        &ctx.root,
        owner,
        runner,
        &ab::Spec {
            a: cli.a.parse()?,
            b: cli.b.parse()?,
            scenarios: cli.scenarios,
            repetitions: cli.repetitions,
            layout_seeds: cli.layout_seeds,
            boards: cli.boards.arguments(),
        },
    )?;
    print!("{}", ab::summary(&finished.report));
    println!("report: {}", finished.path.display());
    Ok(finished.runs)
}

#[derive(clap::Args)]
pub(crate) struct BisectCli {
    /// A commit at which the scenario passes.
    #[arg(long)]
    good: String,
    /// A later commit, descending from GOOD, at which it does not.
    #[arg(long)]
    bad: String,
    #[arg(long)]
    scenario: String,
    /// Build every tested image with this code layout seed.
    #[arg(long, value_name = "SEED")]
    layout_seed: Option<NonZeroU32>,
    #[command(flatten)]
    boards: crate::jobs::BoardChoiceArgs,
}

pub(crate) fn bisect(
    ctx: &Checkout,
    owner: &str,
    cli: BisectCli,
) -> Result<std::process::ExitCode> {
    let finished = bisect::run(
        &ctx.root,
        owner,
        &bisect::Spec {
            good: cli.good,
            bad: cli.bad,
            scenario: cli.scenario,
            layout_seed: cli.layout_seed,
            boards: cli.boards.arguments(),
        },
        |line| eprintln!("hil: {line}"),
    )?;
    print!("{}", bisect::summary(&finished.report));
    Ok(std::process::ExitCode::from(finished.code))
}
