//! `cargo hil evidence`: this checkout's pending evidence.
//!
//! `cargo hil run` never writes tracked files: a clean run's passed
//! scenarios are noted in this checkout's pending list
//! ([`oer_hil_run_bundle::store::pending`]), and the qualification
//! evaluator records them as tracked shards with
//! `cargo qualification hil-evidence --hil-target CHIP --pending`, which
//! reads the runs itself; the stand never runs the evaluator.

use oer_hil_run_bundle::{RunStore, store::pending};

use crate::Result;
use oer_process::Checkout;

/// `cargo hil evidence`.
#[derive(clap::Subcommand)]
pub(crate) enum EvidenceCli {
    /// List this checkout's clean runs whose evidence is not recorded;
    /// `cargo qualification hil-evidence --hil-target CHIP --pending`
    /// records them as tracked shards in hil/evidence/<chip>/.
    Pending,
    /// Drop runs whose evidence will not be recorded, such as runs whose
    /// inputs changed since, from the pending list.
    Dismiss {
        #[arg(long = "run", value_name = "ID", required = true)]
        runs: Vec<String>,
    },
}

/// `cargo hil evidence ...`.
pub(crate) fn command(ctx: &Checkout, cli: EvidenceCli) -> Result<std::process::ExitCode> {
    match cli {
        EvidenceCli::Dismiss { runs } => {
            pending::forget(&ctx.root, &runs)?;
            println!("hil: dismissed {} pending run(s)", runs.len());
        }
        EvidenceCli::Pending => {
            let store = RunStore::shared()?;
            let pending = pending::load(&ctx.root)?
                .into_iter()
                // Pruned runs cannot be recorded.
                .filter(|entry| store.run(&entry.run).is_dir())
                .collect::<Vec<_>>();
            if pending.is_empty() {
                println!("no pending HIL evidence in this checkout");
            }
            for entry in pending {
                println!(
                    "{}  {}  ({})",
                    entry.run,
                    entry.scenarios.join(", "),
                    entry.owner
                );
            }
        }
    }
    Ok(std::process::ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dismissing_names_at_least_one_run() {
        use clap::Parser as _;
        #[derive(clap::Parser)]
        struct Wrap {
            #[command(subcommand)]
            command: EvidenceCli,
        }
        assert!(Wrap::try_parse_from(["x", "dismiss"]).is_err());
        let Wrap {
            command: EvidenceCli::Dismiss { runs },
        } = Wrap::try_parse_from(["x", "dismiss", "--run", "r1", "--run", "r3"]).unwrap()
        else {
            panic!("dismiss parses as dismiss");
        };
        assert_eq!(runs, ["r1", "r3"]);
        assert!(Wrap::try_parse_from(["x", "pending"]).is_ok());
    }
}
