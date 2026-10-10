//! A readable view of a jump table the analysis followed.
use blobray_domain::JumpTable;
use std::collections::BTreeMap;

/// `jump table +176: +1bc <- cases 0, 3; +2de <- case 1`, targets ascending,
/// each with the case values (`first_case` plus the entry index) selecting it.
pub fn human(table: &JumpTable) -> String {
    let mut cases: BTreeMap<u64, Vec<i64>> = BTreeMap::new();
    for (index, target) in table.entries.iter().enumerate() {
        cases
            .entry(*target)
            .or_default()
            .push(table.first_case + index as i64);
    }
    let targets: Vec<String> = cases
        .iter()
        .map(|(target, cases)| {
            let list: Vec<String> = cases.iter().map(i64::to_string).collect();
            let word = if cases.len() == 1 { "case" } else { "cases" };
            format!("+{target:x} <- {word} {}", list.join(", "))
        })
        .collect();
    format!("jump table +{:x}: {}", table.site, targets.join("; "))
}

#[cfg(test)]
mod tests {
    use super::human;
    use blobray_domain::JumpTable;

    #[test]
    fn targets_list_the_cases_that_select_them() {
        let table = JumpTable {
            site: 0x176,
            first_case: 0,
            entries: vec![0x41a, 0x2de, 0x41a, 0x1bc],
        };
        assert_eq!(
            human(&table),
            "jump table +176: +1bc <- case 3; +2de <- case 1; +41a <- cases 0, 2"
        );
        let rebased = JumpTable {
            first_case: 8,
            ..table
        };
        assert_eq!(
            human(&rebased),
            "jump table +176: +1bc <- case 11; +2de <- case 9; +41a <- cases 8, 10"
        );
    }
}
