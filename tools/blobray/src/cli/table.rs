//! Small, presentation-only table helper for human command output.

use tabled::{
    builder::Builder,
    settings::{Style, Width, peaker::PriorityRight},
};

/// Bound a human-only table cell without changing the structured report.
///
/// This is character based rather than byte based so paths and recovered
/// symbols containing Unicode remain valid UTF-8. The final character is an
/// ellipsis and is included in `max_chars`.
pub(super) fn compact(value: impl AsRef<str>, max_chars: usize) -> String {
    let value = value.as_ref();
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    if max_chars == 0 {
        return String::new();
    }
    value
        .chars()
        .take(max_chars.saturating_sub(1))
        .chain(std::iter::once('…'))
        .collect()
}

pub(super) fn render<const COLUMNS: usize>(
    headers: [&str; COLUMNS],
    rows: impl IntoIterator<Item = [String; COLUMNS]>,
) -> String {
    render_at_width(headers, rows, super::output::human_width())
}

fn render_at_width<const COLUMNS: usize>(
    headers: [&str; COLUMNS],
    rows: impl IntoIterator<Item = [String; COLUMNS]>,
    width: usize,
) -> String {
    let mut builder = Builder::default();
    builder.push_record(headers);
    for row in rows {
        builder.push_record(row);
    }
    let mut table = builder.build();
    table.with(Style::rounded());
    // Preserve short identity/status columns and spend the wrapping budget on
    // the widest descriptive column (normally a path or explanation).
    table.with(
        Width::wrap(width)
            .keep_words(true)
            .priority(PriorityRight::new()),
    );
    table.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_rounded_header_and_rows() {
        let table = render(["Name", "Status"], [["radio".into(), "ready".into()]]);
        assert!(table.contains("Name"));
        assert!(table.contains("radio"));
        assert!(table.starts_with('╭'));
    }

    #[test]
    fn compact_bounds_cells_by_characters() {
        assert_eq!(compact("radio", 8), "radio");
        assert_eq!(compact("register", 5), "regi…");
        assert_eq!(compact("радио", 4), "рад…");
        assert_eq!(compact("radio", 0), "");
    }

    #[test]
    fn narrow_tables_preserve_short_identity_columns() {
        let table = render_at_width(
            ["Role", "State", "Path"],
            [[
                "rust-artifact".into(),
                "missing".into(),
                "/a/very/long/path/to/a/generated/verification/artifact".into(),
            ]],
            40,
        );
        assert!(table.contains("Role"));
        assert!(table.contains("State"));
        assert!(table.contains("rust-artifact"));
        assert!(table.contains("missing"));
    }
}
