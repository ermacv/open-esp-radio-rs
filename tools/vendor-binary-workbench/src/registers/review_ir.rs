//! Selected schema-v32 linked-IR evidence used by the manual register report.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use super::review_ir_parse::parse_report;
use crate::Result;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct ReviewPredicateEvidence {
    pub(super) kind: String,
    pub(super) function: String,
    pub(super) producer_path: Vec<String>,
    pub(super) condition: String,
    pub(super) effective_operation: Option<String>,
    pub(super) register_comparison_value: Option<u32>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct ReviewSemanticEvidence {
    pub(super) kind: String,
    pub(super) root: String,
    pub(super) operation: String,
    pub(super) action_target: String,
    pub(super) action_origin: String,
    pub(super) predicate_function: String,
    pub(super) path_expression: String,
    pub(super) residual_path_expression: String,
    pub(super) condition: String,
    pub(super) effective_operation: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct ReviewFieldEvidence {
    pub(super) least_significant_bit: u8,
    pub(super) most_significant_bit: u8,
    pub(super) mask: u32,
    pub(super) write_shapes: usize,
    pub(super) predicate_shapes: usize,
    pub(super) poll_shapes: usize,
    pub(super) functions: BTreeSet<String>,
    pub(super) access_functions: BTreeSet<String>,
    pub(super) predicate_functions: BTreeSet<String>,
    pub(super) predicate_evidence: BTreeSet<ReviewPredicateEvidence>,
    pub(super) semantic_operations: BTreeSet<String>,
    pub(super) semantic_roots: BTreeSet<String>,
    pub(super) semantic_evidence: BTreeSet<ReviewSemanticEvidence>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct ReviewIrRegister {
    pub(super) address: u32,
    pub(super) width: u8,
    pub(super) names: BTreeSet<String>,
    pub(super) functions: BTreeSet<String>,
    pub(super) fields: BTreeMap<(u8, u8, u32), ReviewFieldEvidence>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct RegisterReviewIr {
    pub(super) reports: Vec<PathBuf>,
    pub(super) registers: BTreeMap<(u32, u8), ReviewIrRegister>,
}

impl RegisterReviewIr {
    pub(super) fn load_all(paths: &[PathBuf]) -> Result<Self> {
        let mut unique = BTreeSet::new();
        let mut output = Self::default();
        for path in paths {
            if !unique.insert(path) {
                return Err(format!("duplicate linked-IR review report {}", path.display()).into());
            }
            output.merge_file(path)?;
            output.reports.push(path.clone());
        }
        Ok(output)
    }

    fn merge_file(&mut self, path: &Path) -> Result<()> {
        for register in parse_report(path)? {
            let key = (register.address, register.width);
            merge_register(self.registers.entry(key).or_default(), register)?;
        }
        Ok(())
    }

    pub(super) fn register(&self, address: u32, width: u8) -> Option<&ReviewIrRegister> {
        self.registers.get(&(address, width))
    }

    pub(super) fn field_count(&self) -> usize {
        self.registers
            .values()
            .map(|register| register.fields.len())
            .sum()
    }
}

fn merge_register(target: &mut ReviewIrRegister, source: ReviewIrRegister) -> Result<()> {
    if target.width == 0 {
        target.address = source.address;
        target.width = source.width;
    }
    target.names.extend(source.names);
    target.functions.extend(source.functions);
    for (key, source) in source.fields {
        let target = target
            .fields
            .entry(key)
            .or_insert_with(|| ReviewFieldEvidence {
                least_significant_bit: source.least_significant_bit,
                most_significant_bit: source.most_significant_bit,
                mask: source.mask,
                ..ReviewFieldEvidence::default()
            });
        target.write_shapes = target
            .write_shapes
            .checked_add(source.write_shapes)
            .ok_or("linked-IR write shape count overflow")?;
        target.predicate_shapes = target
            .predicate_shapes
            .checked_add(source.predicate_shapes)
            .ok_or("linked-IR predicate shape count overflow")?;
        target.poll_shapes = target
            .poll_shapes
            .checked_add(source.poll_shapes)
            .ok_or("linked-IR poll shape count overflow")?;
        target.functions.extend(source.functions);
        target.access_functions.extend(source.access_functions);
        target
            .predicate_functions
            .extend(source.predicate_functions);
        target.predicate_evidence.extend(source.predicate_evidence);
        target
            .semantic_operations
            .extend(source.semantic_operations);
        target.semantic_roots.extend(source.semantic_roots);
        target.semantic_evidence.extend(source.semantic_evidence);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(write_shapes: usize, operation: &str) -> String {
        format!(
            r#"{{
  "schema_version": 32,
  "command": "ir export",
  "completeness_claim": false,
  "mmio_field_semantics_claim": false,
  "mmio_registers": [{{
    "address": "0x00001010",
    "width": 32,
    "names": ["RADIO.CONTROL"],
    "functions": ["rom:init"],
    "field_candidates": [{{
      "least_significant_bit": 4,
      "most_significant_bit": 7,
      "mask": "0x000000f0",
      "write_shapes": {write_shapes},
      "predicate_shapes": 1,
      "poll_shapes": 0,
      "functions": ["rom:init"],
      "access_functions": ["rom:read_control"],
      "predicate_functions": ["rom:init"],
      "predicate_evidence": [{{
        "kind": "direct-mmio",
        "function": "rom:init",
        "producer_path": ["rom:read_control"],
        "condition": "field != 0",
        "effective_operation": "not-equal",
        "register_comparison_value": "0x00000000"
      }}],
      "semantic_operations": ["{operation}"],
      "semantic_roots": ["rom:init"],
      "semantic_evidence": []
    }}]
  }}]
}}"#
        )
    }

    #[test]
    fn loads_and_merges_schema_v32_field_evidence() {
        let base = std::env::temp_dir().join(format!(
            "vendor-workbench-register-review-ir-{}",
            std::process::id()
        ));
        let first = base.with_extension("first.json");
        let second = base.with_extension("second.json");
        std::fs::write(&first, report(1, "rtos.event.send")).unwrap();
        std::fs::write(&second, report(2, "delay.blocking")).unwrap();
        let evidence = RegisterReviewIr::load_all(&[first.clone(), second.clone()]).unwrap();
        std::fs::remove_file(first).unwrap();
        std::fs::remove_file(second).unwrap();
        let register = evidence.register(0x1010, 32).unwrap();
        let field = register.fields.values().next().unwrap();
        assert_eq!(field.write_shapes, 3);
        assert_eq!(field.predicate_shapes, 2);
        assert_eq!(field.semantic_operations.len(), 2);
        assert_eq!(field.predicate_evidence.len(), 1);
    }

    #[test]
    fn rejects_schema_drift_and_invalid_field_masks() {
        let path = std::env::temp_dir().join(format!(
            "vendor-workbench-register-review-ir-invalid-{}.json",
            std::process::id()
        ));
        std::fs::write(
            &path,
            report(1, "rtos.event.send").replacen(
                "\"schema_version\": 32",
                "\"schema_version\": 31",
                1,
            ),
        )
        .unwrap();
        let error = RegisterReviewIr::load_all(std::slice::from_ref(&path)).unwrap_err();
        assert!(error.to_string().contains("requires linked-IR schema 32"));

        std::fs::write(
            &path,
            report(1, "rtos.event.send").replacen("0x000000f0", "0x000000e0", 1),
        )
        .unwrap();
        let error = RegisterReviewIr::load_all(std::slice::from_ref(&path)).unwrap_err();
        std::fs::remove_file(path).unwrap();
        assert!(
            error
                .to_string()
                .contains("invalid field bit range or mask")
        );
    }

    #[test]
    fn rejects_the_same_report_twice() {
        let path = std::env::temp_dir().join(format!(
            "vendor-workbench-register-review-ir-duplicate-{}.json",
            std::process::id()
        ));
        std::fs::write(&path, report(1, "rtos.event.send")).unwrap();
        let error = RegisterReviewIr::load_all(&[path.clone(), path.clone()]).unwrap_err();
        std::fs::remove_file(path).unwrap();
        assert!(
            error
                .to_string()
                .contains("duplicate linked-IR review report")
        );
    }
}
