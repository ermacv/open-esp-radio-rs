//! Small, presentation-only table helper for human command output.

use tabled::{builder::Builder, settings::Style};

pub(super) fn render<const COLUMNS: usize>(
    headers: [&str; COLUMNS],
    rows: impl IntoIterator<Item = [String; COLUMNS]>,
) -> String {
    let mut builder = Builder::default();
    builder.push_record(headers);
    for row in rows {
        builder.push_record(row);
    }
    let mut table = builder.build();
    table.with(Style::rounded());
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
}
