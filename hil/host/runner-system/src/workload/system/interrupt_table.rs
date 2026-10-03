//! An interrupt-table source stays silent until its owner enables it and
//! after its owner disables it again.

use oer_hil_execution::context::Context;
use std::{fs, path::Path, time::Duration};

use oer_hil_protocol::system::SourceGated;

use crate::Result;
use oer_hil_link::SerialCapture;

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

pub fn run(probes: u8, output: &Path, context: &Context<'_>) -> Result<()> {
    fs::create_dir_all(output)?;
    let results = context.with_capture(output, |capture| probe(capture, probes))?;
    fs::write(
        output.join("interrupt-table.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": 1,
            "status": "passed",
            "probes": results
                .iter()
                .map(|gated| serde_json::json!({
                    "before_enable": gated.before_enable,
                    "enabled": gated.enabled,
                    "after_disable": gated.after_disable,
                }))
                .collect::<Vec<_>>(),
        }))?,
    )?;
    eprintln!("interrupt-table=PASS probes={probes}");
    Ok(())
}

fn probe(capture: &SerialCapture, probes: u8) -> Result<Vec<SourceGated>> {
    let capabilities = capture.request_image_keys(Duration::from_secs(10))?;
    if !capabilities.has::<oer_hil_protocol::system::SourceGate>() {
        return Err("firmware does not advertise the interrupt-table probe".into());
    }
    let mut results = Vec::with_capacity(usize::from(probes));
    for probe in 1..=probes {
        let gated = capture.probe_source_gate(PROBE_TIMEOUT)?;
        validate(&gated).map_err(|error| format!("probe {probe}: {error}"))?;
        results.push(gated);
    }
    Ok(results)
}

/// The handler ran exactly once, while its source was enabled.
fn validate(gated: &SourceGated) -> std::result::Result<(), String> {
    if gated.before_enable != 0 {
        return Err(format!(
            "the handler ran {} times before its source was enabled",
            gated.before_enable
        ));
    }
    if gated.enabled != 1 {
        return Err(format!(
            "the handler ran {} times for one pending alarm once its source was enabled",
            gated.enabled
        ));
    }
    if gated.after_disable != 0 {
        return Err(format!(
            "the handler ran {} times after its source was disabled",
            gated.after_disable
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_handler_runs_only_while_its_source_is_enabled() {
        let gated = SourceGated {
            before_enable: 0,
            enabled: 1,
            after_disable: 0,
        };
        assert!(validate(&gated).is_ok());
        for leak in [
            SourceGated {
                before_enable: 1,
                ..gated
            },
            SourceGated {
                enabled: 0,
                ..gated
            },
            SourceGated {
                after_disable: 1,
                ..gated
            },
        ] {
            assert!(validate(&leak).is_err(), "{leak:?}");
        }
    }
}
