//! Vendor objects reported by the calibration firmware on its console.
use crate::Result;
use std::collections::BTreeMap;

/// Prefix of every report line (`verification/esp32s31/hil-vendor/calibration`).
pub const REPORT_PREFIX: &str = "oer-vendor-calibration";
/// Suffix of the line that closes a boot's report.
const END_SUFFIX: &str = "-end";
/// Suffix of a register reply prefix, and the request the firmware answers.
const REGISTER_SUFFIX: &str = "-register";
pub const REGISTER_REQUEST: &str = "r";

/// Suffix of the host's record of a register whose read reset the chip.
const UNREADABLE_SUFFIX: &str = "-unreadable";

/// The host's record that reading `address` reset the chip.
pub fn unreadable_line(address: u32) -> String {
    format!("{REPORT_PREFIX}{UNREADABLE_SUFFIX} {address:08x}\n")
}

/// The addresses `console` records as unreadable.
pub fn unreadable(console: &str) -> Result<std::collections::BTreeSet<u32>> {
    let prefix = format!("{REPORT_PREFIX}{UNREADABLE_SUFFIX} ");
    console
        .lines()
        .filter_map(|line| line.trim().strip_prefix(&prefix))
        .map(|address| Ok(u32::from_str_radix(address, 16)?))
        .collect()
}

/// Complete boot reports in `console`.
pub fn reports(console: &str) -> usize {
    let end = format!("{REPORT_PREFIX}{END_SUFFIX}");
    console.lines().filter(|line| line.trim() == end).count()
}

/// One register reply line: the firmware's answer format, which the host
/// also writes for the production register image.
pub fn register_line(address: u32, value: u32) -> String {
    format!("{REPORT_PREFIX}{REGISTER_SUFFIX} {address:08x} {value:08x}\n")
}

/// The request line for the word at `address`.
pub fn register_request(address: u32) -> String {
    format!("{REGISTER_REQUEST} {address:08x}\n")
}

/// `(address, value)` of each complete register reply line of `console`; a
/// line still arriving, without its newline, is not read yet.
pub fn registers(console: &str) -> Result<BTreeMap<u32, u32>> {
    let prefix = format!("{REPORT_PREFIX}{REGISTER_SUFFIX} ");
    let mut values = BTreeMap::new();
    let complete = console.rfind('\n').map_or("", |end| &console[..end]);
    for line in complete.lines().map(str::trim) {
        let Some(rest) = line.strip_prefix(&prefix) else {
            continue;
        };
        let (address, value) = rest
            .split_once(' ')
            .ok_or_else(|| format!("malformed register reply: {line}"))?;
        let (address, value) = (
            u32::from_str_radix(address, 16)?,
            u32::from_str_radix(value, 16)?,
        );
        if values.insert(address, value).is_some() {
            return Err(format!("register {address:#010x} answered twice").into());
        }
    }
    Ok(values)
}

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
    fn register_replies_are_read_by_address() {
        let console = "r 20100434\noer-vendor-calibration-register 20100434 0000abcd\n";
        assert_eq!(registers(console).unwrap()[&0x2010_0434], 0xabcd);
        assert_eq!(register_request(0x2010_0434), "r 20100434\n");
    }

    #[test]
    fn unreadable_registers_and_reports_are_counted() {
        let console = format!(
            "oer-vendor-calibration-end\n{}oer-vendor-calibration-end\n",
            unreadable_line(0x2010_2800)
        );
        assert_eq!(reports(&console), 2);
        assert!(unreadable(&console).unwrap().contains(&0x2010_2800));
    }

    #[test]
    fn a_reply_still_arriving_is_not_read() {
        let console = "oer-vendor-calibration-register 20100434 0000abcd\n\
                       oer-vendor-calibration-register 2010";
        let values = registers(console).unwrap();
        assert_eq!(values.len(), 1);
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
