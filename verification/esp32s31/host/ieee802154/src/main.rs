//! Run one catalog scenario against the compiled vendor driver and print its
//! boundary trace, one record per line, or compare it with the production
//! engine and print the verdict.

use std::process::ExitCode;

use std::panic::{AssertUnwindSafe, catch_unwind};

use oer_esp32s31_ieee802154_vendor_host::{Verdict, catalog, claim, compare, port};

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    match (arguments.next().as_deref(), arguments.next()) {
        (Some("list"), None) => {
            for scenario in catalog::SCENARIOS {
                println!("{}", scenario.name);
            }
            ExitCode::SUCCESS
        }
        (Some(command @ ("run" | "compare")), Some(name)) => {
            let Some(scenario) = catalog::SCENARIOS
                .iter()
                .find(|scenario| scenario.name == name)
            else {
                eprintln!("unknown scenario: {name}");
                return ExitCode::FAILURE;
            };
            let vendor = claim(scenario);
            if command == "run" {
                for record in vendor {
                    println!("{record}");
                }
                return ExitCode::SUCCESS;
            }
            // A port panic is a vendor assertion the port adds; the panic
            // message goes to stderr and the empty trace fails the comparison.
            let port = catch_unwind(AssertUnwindSafe(|| port::run(scenario)))
                .unwrap_or_else(|_| Ok(Vec::new()));
            let verdict = compare(&vendor, port);
            println!("{verdict}");
            match verdict {
                Verdict::Match => ExitCode::SUCCESS,
                Verdict::Diff { .. } => ExitCode::from(1),
                Verdict::Incomplete(_) => ExitCode::from(2),
            }
        }
        _ => {
            eprintln!(
                "usage: oer-esp32s31-ieee802154-vendor-host list | run <scenario> | compare <scenario>"
            );
            ExitCode::FAILURE
        }
    }
}
