//! A readable view of a jump table the analysis followed.
use blobray_domain::JumpTable;
use std::collections::BTreeMap;

/// `jump table +176: +1bc <- cases 0, 3; +2de <- case 1`, targets ascending,
/// each with the case indices whose entry holds it.
pub fn human(table: &JumpTable) -> String {
    let mut cases: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for (case, target) in table.entries.iter().enumerate() {
        cases.entry(*target).or_default().push(case);
    }
    let targets: Vec<String> = cases
        .iter()
        .map(|(target, cases)| {
            let list: Vec<String> = cases.iter().map(usize::to_string).collect();
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
            entries: vec![0x41a, 0x2de, 0x41a, 0x1bc],
        };
        assert_eq!(
            human(&table),
            "jump table +176: +1bc <- case 3; +2de <- case 1; +41a <- cases 0, 2"
        );
    }
}
