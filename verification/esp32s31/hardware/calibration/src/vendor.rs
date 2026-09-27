//! Vendor objects reported by the calibration firmware on its console.
use crate::Result;
use std::collections::BTreeMap;

/// Prefix of every report line (`verification/esp32s31/hil-vendor/calibration`).
pub const REPORT_PREFIX: &str = "oer-vendor-calibration";
/// Suffix of the line that closes a boot's report.
const END_SUFFIX: &str = "-end";

/// The objects of one boot's complete report, by name. Fails when the report
/// is incomplete, repeats an object or carries malformed bytes.
pub fn parse(console: &str) -> Result<BTreeMap<String, Vec<u8>>> {
    let end = format!("{REPORT_PREFIX}{END_SUFFIX}");
    let mut objects = BTreeMap::new();
    for line in console.lines().map(str::trim) {
        if line == end {
            return Ok(objects);
        }
        let Some(rest) = line
            .strip_prefix(REPORT_PREFIX)
            .and_then(|r| r.strip_prefix(' '))
        else {
            continue;
        };
        let (name, hex) = rest
            .split_once(' ')
            .ok_or_else(|| format!("malformed report line: {line}"))?;
        let bytes = decode_hex(hex).ok_or_else(|| format!("malformed bytes of {name}"))?;
        if objects.insert(name.to_owned(), bytes).is_some() {
            return Err(format!("object {name} reported twice").into());
        }
    }
    Err("the report ended without its closing line".into())
}

fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Whether `console` holds a closed report.
pub fn complete(console: &str) -> bool {
    let end = format!("{REPORT_PREFIX}{END_SUFFIX}");
    console.lines().any(|line| line.trim() == end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_report_yields_its_objects_among_boot_logs() {
        let console = "I (278) phy_init: phy_version\n\
                       oer-vendor-calibration phy_param 0a00ff\n\
                       oer-vendor-calibration-end\n";
        let objects = parse(console).unwrap();
        assert_eq!(objects["phy_param"], [0x0a, 0x00, 0xff]);
        assert!(complete(console));
    }

    #[test]
    fn open_repeated_or_malformed_reports_fail() {
        assert!(parse("oer-vendor-calibration phy_param 00\n").is_err());
        assert!(
            parse(
                "oer-vendor-calibration a 00\noer-vendor-calibration a 01\n\
                 oer-vendor-calibration-end\n"
            )
            .is_err()
        );
        assert!(parse("oer-vendor-calibration a 0\noer-vendor-calibration-end\n").is_err());
    }
}
