//! Run one catalog scenario against the compiled vendor driver and print its
//! boundary trace, one record per line.

use std::process::ExitCode;

use oer_esp32s31_ieee802154_vendor_host::{catalog, claim};

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    match (arguments.next().as_deref(), arguments.next()) {
        (Some("list"), None) => {
            for scenario in catalog::SCENARIOS {
                println!("{}", scenario.name);
            }
            ExitCode::SUCCESS
        }
        (Some("run"), Some(name)) => {
            let Some(scenario) = catalog::SCENARIOS
                .iter()
                .find(|scenario| scenario.name == name)
            else {
                eprintln!("unknown scenario: {name}");
                return ExitCode::FAILURE;
            };
            for record in claim(scenario) {
                println!("{record}");
            }
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: oer-esp32s31-ieee802154-vendor-host list | run <scenario>");
            ExitCode::FAILURE
        }
    }
}
