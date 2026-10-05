//! The `// CAPABILITY:` anchor grammar: the one recogniser of a capability
//! anchor line and of the catalog ids it names. The qualification evaluator
//! binds catalog entries to code through it (`cargo qualification catalog
//! anchors`), and [`crate::sources::markers`] keeps anchors in compiled
//! files.

/// The marker that opens an anchor line, after optional indentation.
pub const MARKER: &str = "// CAPABILITY:";

/// The ids an anchor `line` names: `None` when the line is no anchor,
/// otherwise the ids, or why they are malformed.
pub fn capability(line: &str) -> Option<Result<Vec<String>, String>> {
    let rest = line.trim_start().strip_prefix(MARKER)?;
    Some(ids(rest))
}

/// The comma-separated catalog ids after the marker: lowercase words joined
/// by single hyphens, each named once.
fn ids(rest: &str) -> Result<Vec<String>, String> {
    let mut ids = Vec::new();
    for id in rest.split(',').map(str::trim) {
        let valid = id.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
            && id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !id.ends_with('-')
            && !id.contains("--");
        if !valid {
            return Err(format!("`{id}` is not a catalog id"));
        }
        if ids.iter().any(|known| known == id) {
            return Err(format!("`{id}` is repeated"));
        }
        ids.push(id.to_owned());
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_anchor_names_catalog_ids_once() {
        assert_eq!(
            capability("    // CAPABILITY: wpa2, ble-legacy-advertising"),
            Some(Ok(vec![
                "wpa2".to_owned(),
                "ble-legacy-advertising".to_owned()
            ]))
        );
        assert_eq!(capability("/// CAPABILITY: wpa2"), None);
        assert_eq!(capability("// capability: wpa2"), None);
        assert_eq!(capability("let x = 1; // CAPABILITY: wpa2"), None);
        for malformed in [
            "// CAPABILITY: Wpa2",
            "// CAPABILITY: a--b",
            "// CAPABILITY: a, a",
            "// CAPABILITY:",
        ] {
            assert!(capability(malformed).unwrap().is_err(), "{malformed}");
        }
    }
}
