//! Minimal deterministic JSON rendering for CLI reports.

use std::{fmt::Write as _, path::Path};

use crate::{Result, artifact_sha256};

pub(crate) fn write_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                write!(output, "\\u{:04x}", character as u32)
                    .expect("writing to String cannot fail");
            }
            character => output.push(character),
        }
    }
    output.push('"');
}

pub(crate) fn write_strings(
    output: &mut String,
    values: impl IntoIterator<Item = impl AsRef<str>>,
) {
    output.push('[');
    for (index, value) in values.into_iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        write_string(output, value.as_ref());
    }
    output.push(']');
}

pub(crate) fn write_artifact(output: &mut String, path: &Path) -> Result<()> {
    output.push_str("{\"path\": ");
    write_string(output, &path.display().to_string());
    output.push_str(", \"sha256\": ");
    write_string(output, &artifact_sha256(path)?);
    output.push('}');
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_escape_json_control_characters() {
        let mut output = String::new();
        write_string(&mut output, "a\t\"b\\c\n");
        assert_eq!(output, "\"a\\t\\\"b\\\\c\\n\"");
    }
}
