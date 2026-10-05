//! The one form of a board's MAC: upper case, colon separated, six bytes,
//! as Espressif USB Serial/JTAG ports report it as their USB serial number.

/// `text` (with or without `:` or `-` separators, any case) in the form the
/// ports report: `38:44:BE:AA:25:64`.
pub fn normalize(text: &str) -> Result<String, String> {
    let digits = text
        .chars()
        .filter(|character| !matches!(character, ':' | '-'))
        .collect::<String>();
    if digits.len() != 12 || !digits.chars().all(|digit| digit.is_ascii_hexdigit()) {
        return Err(format!("`{text}` is not a six-byte MAC address"));
    }
    Ok(digits
        .to_ascii_uppercase()
        .as_bytes()
        .chunks(2)
        .map(|pair| String::from_utf8_lossy(pair).into_owned())
        .collect::<Vec<_>>()
        .join(":"))
}

/// The MAC without separators, for file names: `3844BEAA2564`.
pub fn compact(mac: &str) -> String {
    mac.replace(':', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macs_are_normalized_and_invalid_ones_refused() {
        assert_eq!(normalize("38:44:be:aa:25:64").unwrap(), "38:44:BE:AA:25:64");
        assert_eq!(normalize("3844BEAA2564").unwrap(), "38:44:BE:AA:25:64");
        assert_eq!(normalize("38-44-be-aa-25-64").unwrap(), "38:44:BE:AA:25:64");
        assert_eq!(compact("38:44:BE:AA:25:64"), "3844BEAA2564");
        for invalid in ["", "38:44", "zz:44:be:aa:25:64", "/dev/ttyACM0"] {
            assert!(normalize(invalid).is_err(), "{invalid}");
        }
    }
}
