//! `SOURCE:` recovered-fact comment blocks and the chips they belong to.
//!
//! Production records a recovered vendor fact in a comment block opened by
//! the marker `SOURCE`. The marker may name the chips whose pinned artifacts
//! the fact describes, in parentheses directly after it:
//!
//! ```text
//! // SOURCE(esp32s31): complete `libpp.a[trc.o]::rcGetRate` …
//! /// SOURCE(esp32s31,esp32c5)[EVIDENCE_ID]: …
//! ```
//!
//! Any bracketed evidence tag keeps following the marker unchanged. A block
//! under a chip's own directory belongs to that chip and may omit the list;
//! a block under a chip-neutral path that cites a vendor function must name
//! its chips. This module is the one parser of that grammar.
use std::path::Path;

/// The marker that opens a recovered-fact comment block.
pub const MARKER: &str = "SOURCE";

/// One recovered-fact comment block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Block {
    /// One-based line of the marker.
    pub line: usize,
    /// The chips the marker names, when it names any.
    pub chips: Option<Vec<String>>,
    /// The block's comment text, without the chip list.
    pub text: String,
}

/// A marker whose chip list does not parse.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MalformedMarker {
    pub line: usize,
    pub reason: &'static str,
}

fn is_comment(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("//") || line.starts_with("/*") || line.starts_with('*')
}

/// The chip list after the marker on `line`, and the line without it.
fn marker_chips(line: &str) -> Result<(Option<Vec<String>>, String), &'static str> {
    let Some(at) = line.find(MARKER) else {
        return Ok((None, line.to_owned()));
    };
    let after = at + MARKER.len();
    let Some(list) = line[after..].strip_prefix('(') else {
        return Ok((None, line.to_owned()));
    };
    let close = list.find(')').ok_or("unclosed chip list")?;
    let chips: Vec<String> = list[..close]
        .split(',')
        .map(|chip| chip.trim().to_owned())
        .collect();
    if chips.iter().any(String::is_empty) {
        return Err("empty chip name");
    }
    let rest = &list[close + 1..];
    Ok((Some(chips), format!("{}{MARKER}{rest}", &line[..at])))
}

/// Every recovered-fact comment block of `text`, or the first malformed
/// marker.
pub fn blocks(text: &str) -> Result<Vec<Block>, MalformedMarker> {
    let mut blocks: Vec<Block> = vec![];
    let mut open = false;
    for (index, line) in text.lines().enumerate() {
        if !is_comment(line) {
            open = false;
            continue;
        }
        if line.contains(MARKER) {
            let (chips, stripped) = marker_chips(line).map_err(|reason| MalformedMarker {
                line: index + 1,
                reason,
            })?;
            blocks.push(Block {
                line: index + 1,
                chips,
                text: stripped,
            });
            open = true;
        } else if open {
            let block = blocks.last_mut().expect("an open block");
            block.text.push('\n');
            block.text.push_str(line);
        }
    }
    Ok(blocks)
}

/// Where a block lies relative to the scanned chip.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Place {
    /// Under the scanned chip's own directory.
    OwnChip,
    /// Under no chip's directory.
    Neutral,
}

/// Whether a block cites the scanned `chip`'s pins.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Attribution {
    Cited,
    NotCited,
    /// A chip-neutral block that names no chip: a violation when it cites a
    /// vendor function, since its chip is then unknown.
    Uncharted,
    /// A violation, with its reason.
    Invalid(String),
}

/// The attribution of `block` at `place` for a scan of `chip`, whose names
/// must be among `known`.
pub fn attribute(block: &Block, place: Place, chip: &str, known: &[String]) -> Attribution {
    if let Some(unknown) = block.chips.iter().flatten().find(|c| !known.contains(c)) {
        return Attribution::Invalid(format!("unknown chip `{unknown}`"));
    }
    match (place, &block.chips) {
        (Place::OwnChip, None) => Attribution::Cited,
        (Place::OwnChip, Some(chips)) if chips.iter().any(|c| c == chip) => Attribution::Cited,
        (Place::OwnChip, Some(_)) => {
            Attribution::Invalid(format!("names other chips under the {chip} directory"))
        }
        (Place::Neutral, None) => Attribution::Uncharted,
        (Place::Neutral, Some(chips)) if chips.iter().any(|c| c == chip) => Attribution::Cited,
        (Place::Neutral, Some(_)) => Attribution::NotCited,
    }
}

/// The place of `path` for a scan of `chip`, or `None` under another
/// supported chip's directory.
pub fn place(path: &Path, chip: &str, supported: &[String]) -> Option<Place> {
    let mut own = false;
    for component in path.components() {
        let component = component.as_os_str().to_str().unwrap_or_default();
        if component == chip {
            own = true;
        } else if supported.iter().any(|c| c == component) {
            return None;
        }
    }
    Some(if own { Place::OwnChip } else { Place::Neutral })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> Vec<String> {
        vec!["esp32c5".into(), "esp32s31".into()]
    }

    #[test]
    fn a_marker_names_its_chips_before_any_evidence_tag() {
        let text = "// SOURCE(esp32s31, esp32c5)[TAG]: `phy_i2c_init1`\n\
                    // continues `phy_rf_init`\nfn x() {}\n\
                    /// SOURCE: plain\n";
        let blocks = blocks(text).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(
            blocks[0].chips,
            Some(vec!["esp32s31".to_owned(), "esp32c5".to_owned()])
        );
        assert!(blocks[0].text.contains("SOURCE[TAG]"));
        assert!(!blocks[0].text.contains("esp32c5"));
        assert!(blocks[0].text.contains("phy_rf_init"));
        assert_eq!(blocks[1].chips, None);
        assert_eq!(blocks[1].line, 4);
    }

    #[test]
    fn a_malformed_chip_list_is_rejected_with_its_line() {
        assert_eq!(
            blocks("fn x() {}\n// SOURCE(esp32s31: open\n"),
            Err(MalformedMarker {
                line: 2,
                reason: "unclosed chip list"
            })
        );
        assert!(blocks("// SOURCE(esp32s31,): x\n").is_err());
    }

    #[test]
    fn neutral_blocks_must_name_their_chips() {
        let neutral = |chips: Option<Vec<&str>>| Block {
            line: 1,
            chips: chips.map(|c| c.into_iter().map(str::to_owned).collect()),
            text: String::new(),
        };
        let known = known();
        assert_eq!(
            attribute(
                &neutral(Some(vec!["esp32s31"])),
                Place::Neutral,
                "esp32s31",
                &known
            ),
            Attribution::Cited
        );
        assert_eq!(
            attribute(
                &neutral(Some(vec!["esp32s31"])),
                Place::Neutral,
                "esp32c5",
                &known
            ),
            Attribution::NotCited
        );
        assert_eq!(
            attribute(&neutral(None), Place::Neutral, "esp32c5", &known),
            Attribution::Uncharted
        );
        assert_eq!(
            attribute(&neutral(None), Place::OwnChip, "esp32c5", &known),
            Attribution::Cited
        );
        assert!(matches!(
            attribute(
                &neutral(Some(vec!["esp32s31"])),
                Place::OwnChip,
                "esp32c5",
                &known
            ),
            Attribution::Invalid(_)
        ));
        assert!(matches!(
            attribute(
                &neutral(Some(vec!["esp32"])),
                Place::Neutral,
                "esp32c5",
                &known
            ),
            Attribution::Invalid(_)
        ));
    }

    #[test]
    fn a_path_is_own_neutral_or_another_chips() {
        let known = known();
        assert_eq!(
            place(
                Path::new("crates/hardware/esp32c5/pac/src/a.rs"),
                "esp32c5",
                &known
            ),
            Some(Place::OwnChip)
        );
        assert_eq!(
            place(
                Path::new("crates/protocols/ieee80211/src/a.rs"),
                "esp32c5",
                &known
            ),
            Some(Place::Neutral)
        );
        assert_eq!(
            place(
                Path::new("crates/hardware/esp32s31/hal/src/a.rs"),
                "esp32c5",
                &known
            ),
            None
        );
    }
}
