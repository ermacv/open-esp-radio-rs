//! Host qualification for complete station dematerialization.

use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use crate::{
    Result,
    controlled_ap::{
        ControlledAp, require_controlled_ap_credentials_environment,
        require_station_credentials_environment,
    },
    traffic_capture::SerialCapture,
};

const DEFAULT_SERIAL: &str = "/dev/ttyACM0";
// The replacement station has its own bounded 30-second connected-entry
// deadline. Leave enough host-side time for two monitor quiescence edges and
// the final PAC/IRQ/resource report after that target deadline expires.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

struct Options {
    serial: PathBuf,
    timeout: Duration,
    access_point: AccessPointOwnership,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AccessPointOwnership {
    /// The runner starts the repository HE20 fixture and restores the previous
    /// host networking state when qualification finishes.
    Runner,
    /// The caller owns an already-running access point. The runner must not
    /// invoke privileged host-network helpers or change that access point.
    External,
}

pub(crate) fn run(arguments: Vec<String>, root: &Path) -> Result<()> {
    if arguments
        .first()
        .is_some_and(|value| matches!(value.as_str(), "help" | "--help" | "-h"))
    {
        print_help();
        return Ok(());
    }
    let options = parse_options(&arguments)?;
    let _access_point = match options.access_point {
        AccessPointOwnership::Runner => {
            require_controlled_ap_credentials_environment()?;
            Some(ControlledAp::start()?)
        }
        AccessPointOwnership::External => {
            require_station_credentials_environment()?;
            None
        }
    };
    let output = root.join("target/hil/esp32s31/qualification/station-stop");
    fs::create_dir_all(&output)?;
    let capture = SerialCapture::start_with_reset(&options.serial);
    let result = qualify(&capture, options.timeout);
    let log = capture.finish();
    fs::write(output.join("uart.log"), &log)?;
    result?;
    println!("station_stop=PASS");
    println!("uart_log={}", output.join("uart.log").display());
    Ok(())
}

fn qualify(capture: &SerialCapture, timeout: Duration) -> Result<()> {
    let capabilities = capture.prepare_protocol()?;
    if !capabilities.features.station_stop_control {
        return Err("firmware does not advertise complete station stop control".into());
    }
    let handle = capture.request_station_stop()?;
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(evidence) = capture.observed_station_stop(handle) {
            if !evidence.is_complete() {
                return Err(format!(
                    "station stopped with incomplete owner evidence: {evidence:?}"
                )
                .into());
            }
            return Ok(());
        }
        thread::sleep(Duration::from_millis(20));
    }
    Err(format!(
        "target did not reconstruct WifiStopped within {} seconds",
        timeout.as_secs()
    )
    .into())
}

fn parse_options(arguments: &[String]) -> Result<Options> {
    let mut options = Options {
        serial: PathBuf::from(DEFAULT_SERIAL),
        timeout: DEFAULT_TIMEOUT,
        access_point: AccessPointOwnership::Runner,
    };
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--serial" => {
                index += 1;
                options.serial = PathBuf::from(
                    arguments
                        .get(index)
                        .ok_or("--serial requires a device path")?,
                );
            }
            "--timeout-seconds" => {
                index += 1;
                let seconds = arguments
                    .get(index)
                    .ok_or("--timeout-seconds requires a value")?
                    .parse::<u64>()?;
                if !(10..=120).contains(&seconds) {
                    return Err("--timeout-seconds must be in 10..=120".into());
                }
                options.timeout = Duration::from_secs(seconds);
            }
            "--external-ap" => {
                options.access_point = AccessPointOwnership::External;
            }
            argument => return Err(format!("unknown station stop option `{argument}`").into()),
        }
        index += 1;
    }
    Ok(options)
}

fn print_help() {
    println!(
        "cargo hil station stop [options]\n\n\
         --serial <path>          diagnostics device (default /dev/ttyACM0)\n\
         --timeout-seconds <n>    complete role-stop deadline, 10..=120 (default 60)\n\
         --external-ap            use a caller-owned, already-running access point"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bounded_stop_options() {
        let options = parse_options(&[
            "--serial".into(),
            "/dev/test-radio".into(),
            "--timeout-seconds".into(),
            "45".into(),
        ])
        .unwrap();
        assert_eq!(options.serial, PathBuf::from("/dev/test-radio"));
        assert_eq!(options.timeout, Duration::from_secs(45));
        assert_eq!(options.access_point, AccessPointOwnership::Runner);
    }

    #[test]
    fn external_access_point_does_not_require_runner_ownership() {
        let options = parse_options(&["--external-ap".into()]).unwrap();
        assert_eq!(options.access_point, AccessPointOwnership::External);
    }

    #[test]
    fn rejects_unbounded_stop_timeout() {
        assert!(parse_options(&["--timeout-seconds".into(), "121".into()]).is_err());
    }

    #[test]
    fn allocation_handles_cannot_replace_returned_role_resources() {
        let evidence = open_esp_radio_hil_protocol::StationStopEvidence {
            role_resources_reclaimed: false,
            ..open_esp_radio_hil_protocol::StationStopEvidence::COMPLETE
        };
        assert!(!evidence.is_complete());
    }

    #[test]
    fn reconstructed_wifi_without_a_subsequent_role_round_trip_is_incomplete() {
        let evidence = open_esp_radio_hil_protocol::StationStopEvidence {
            subsequent_role_quiesced: false,
            ..open_esp_radio_hil_protocol::StationStopEvidence::COMPLETE
        };
        assert!(!evidence.is_complete());
    }

    #[test]
    fn one_monitor_epoch_cannot_replace_monitor_restart_evidence() {
        let evidence = open_esp_radio_hil_protocol::StationStopEvidence {
            subsequent_role_restarted: false,
            ..open_esp_radio_hil_protocol::StationStopEvidence::COMPLETE
        };
        assert!(!evidence.is_complete());

        let evidence = open_esp_radio_hil_protocol::StationStopEvidence {
            subsequent_role_restart_quiesced: false,
            ..open_esp_radio_hil_protocol::StationStopEvidence::COMPLETE
        };
        assert!(!evidence.is_complete());
    }

    #[test]
    fn monitor_return_without_station_rematerialization_is_incomplete() {
        let evidence = open_esp_radio_hil_protocol::StationStopEvidence {
            station_rematerialized: false,
            ..open_esp_radio_hil_protocol::StationStopEvidence::COMPLETE
        };
        assert!(!evidence.is_complete());
    }

    #[test]
    fn replacement_station_must_reach_connected_entry() {
        let evidence = open_esp_radio_hil_protocol::StationStopEvidence {
            station_connected: false,
            ..open_esp_radio_hil_protocol::StationStopEvidence::COMPLETE
        };
        assert!(!evidence.is_complete());
    }

    #[test]
    fn replacement_station_must_return_its_exact_owner_graph() {
        let evidence = open_esp_radio_hil_protocol::StationStopEvidence {
            station_requiesced: false,
            ..open_esp_radio_hil_protocol::StationStopEvidence::COMPLETE
        };
        assert!(!evidence.is_complete());
    }
}
