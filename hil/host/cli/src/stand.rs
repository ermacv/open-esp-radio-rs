//! `cargo hil stand discover|doctor`: the stand file against the host,
//! through `oer-hil-stand-host`.

use oer_hil_stand_host::{discover, doctor};
use oer_hil_stand_model::StandFile;

use crate::Result;
use oer_process::Checkout;

#[derive(clap::Subcommand)]
pub(crate) enum StandCli {
    /// Map each attached board to its hub port and button and compare the
    /// result with the stand file, which it never changes.
    Discover {
        /// Switch HUB:PORT (a stand-file hub id and port) off for five
        /// seconds under a lease, to see which button it is.
        #[arg(long, value_name = "HUB:PORT", conflicts_with = "verify_power")]
        blink: Option<String>,
        /// Cycle BOARD's hub port under its lease and require the board to
        /// leave USB while the port is off and to return once it is on.
        #[arg(long, value_name = "BOARD")]
        verify_power: Option<String>,
    },
    /// Check the host around the stand file: the file itself, `uhubctl`
    /// without sudo and NetworkManager leaving `wlan0` alone.
    Doctor,
}

pub(crate) fn stand(
    ctx: &Checkout,
    owner: impl FnOnce() -> Result<String>,
    cli: StandCli,
) -> Result<std::process::ExitCode> {
    let path = oer_hil_stand_model::paths::stand_file()?;
    match cli {
        StandCli::Discover {
            blink: Some(target),
            ..
        } => {
            let stand = StandFile::load(&path)?;
            let (hub, port) = discover::blink(&stand, &target, owner()?)?;
            println!(
                "{}:{port} was off for {} s",
                hub.id,
                discover::BLINK.as_secs()
            );
        }
        StandCli::Discover {
            verify_power: Some(board),
            ..
        } => {
            // The ROM prints its reset reason before the board's own USB
            // enumerates, so the board leaving USB and returning is the proof.
            let stand = StandFile::load(&path)?;
            let after = discover::verify_power(&ctx.root, &stand, &board, owner()?)?
                .verdict()
                .map_err(|why| format!("{board}: {why}"))?;
            println!(
                "{board} lost its power: it left USB and returned {:.1} s after its port's power",
                after.as_secs_f64()
            );
        }
        StandCli::Discover { .. } => {
            let stand = StandFile::load(&path)?;
            print!("{}", discover::describe(&stand, &discover::report(&stand)?));
        }
        StandCli::Doctor => {
            let mut failed = false;
            for check in doctor::doctor(&ctx.root, &path) {
                match &check.failure {
                    None => println!("PASS {}", check.name),
                    Some(failure) => {
                        failed = true;
                        println!("FAIL {}: {failure}", check.name);
                    }
                }
            }
            if failed {
                return Ok(std::process::ExitCode::FAILURE);
            }
        }
    }
    Ok(std::process::ExitCode::SUCCESS)
}
