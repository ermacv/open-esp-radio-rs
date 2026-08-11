//! Selected schema-v45 linked-IR evidence used by the manual register report.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use super::review_ir_parse::parse_report;
use crate::Result;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ReviewPredicateEvidence {
    pub(crate) kind: String,
    pub(crate) function: String,
    pub(crate) producer_path: Vec<String>,
    pub(crate) condition: String,
    pub(crate) effective_operation: Option<String>,
    pub(crate) register_comparison_value: Option<u32>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ReviewSemanticEvidence {
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
pub(crate) struct ReviewFieldEvidence {
    pub(crate) least_significant_bit: u8,
    pub(crate) most_significant_bit: u8,
    pub(crate) mask: u32,
    pub(crate) write_shapes: usize,
    pub(crate) predicate_shapes: usize,
    pub(crate) poll_shapes: usize,
    pub(crate) functions: BTreeSet<String>,
    pub(crate) access_functions: BTreeSet<String>,
    pub(crate) predicate_functions: BTreeSet<String>,
    pub(crate) predicate_evidence: BTreeSet<ReviewPredicateEvidence>,
    pub(crate) semantic_operations: BTreeSet<String>,
    pub(crate) semantic_roots: BTreeSet<String>,
    pub(crate) semantic_evidence: BTreeSet<ReviewSemanticEvidence>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ReviewIrRegister {
    pub(crate) address: u32,
    pub(crate) width: u8,
    pub(crate) names: BTreeSet<String>,
    pub(crate) functions: BTreeSet<String>,
    pub(crate) fields: BTreeMap<(u8, u8, u32), ReviewFieldEvidence>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct RegisterReviewIr {
    pub(crate) reports: Vec<PathBuf>,
    pub(crate) registers: BTreeMap<(u32, u8), ReviewIrRegister>,
}

impl RegisterReviewIr {
    pub(crate) fn load_all(paths: &[PathBuf]) -> Result<Self> {
        let mut unique = BTreeSet::new();
        let mut output = Self::default();
        for path in paths {
            if !unique.insert(path) {
                return Err(crate::Error::invalid(format!(
                    "duplicate linked-IR review report {}",
                    path.display()
                )));
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

    pub(crate) fn register(&self, address: u32, width: u8) -> Option<&ReviewIrRegister> {
        self.registers.get(&(address, width))
    }

    pub(crate) fn field_count(&self) -> usize {
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
            .ok_or("linked-IR write shape count overflow")
            .map_err(crate::Error::invalid)?;
        target.predicate_shapes = target
            .predicate_shapes
            .checked_add(source.predicate_shapes)
            .ok_or("linked-IR predicate shape count overflow")
            .map_err(crate::Error::invalid)?;
        target.poll_shapes = target
            .poll_shapes
            .checked_add(source.poll_shapes)
            .ok_or("linked-IR poll shape count overflow")
            .map_err(crate::Error::invalid)?;
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
        crate::artifacts::render_linked_ir_fixture(
            Vec::new(),
            vec![crate::LinkedMmioRegister {
                address: 4112,
                width: 32,
                names: vec!["RADIO.CONTROL".to_owned()],
                read_shapes: 0,
                write_shapes,
                poll_shapes: 0,
                predicate_shapes: 1,
                static_shapes: 0,
                indexed_candidate_shapes: 0,
                whole_register_write_shapes: 0,
                whole_register_predicate_shapes: 0,
                whole_register_poll_shapes: 0,
                read_modify_write_shapes: 0,
                write_masks: Vec::new(),
                predicate_masks: vec![240],
                poll_masks: Vec::new(),
                candidate_bit_ranges: Vec::new(),
                field_candidates: vec![crate::LinkedMmioFieldCandidate {
                    least_significant_bit: 4,
                    most_significant_bit: 7,
                    mask: 240,
                    write_shapes,
                    predicate_shapes: 1,
                    poll_shapes: 0,
                    functions: vec!["rom:init".to_owned()],
                    access_functions: vec!["rom:read_control".to_owned()],
                    predicate_functions: vec!["rom:init".to_owned()],
                    predicate_evidence: vec![crate::LinkedMmioFieldPredicateEvidence {
                        kind: "direct-mmio",
                        function: "rom:init".to_owned(),
                        producer: None,
                        producer_path: vec!["rom:read_control".to_owned()],
                        site: None,
                        path: None,
                        condition: "field != 0".to_owned(),
                        operation: "not-equal",
                        taken: None,
                        effective_operation: Some("not-equal"),
                        operand: None,
                        comparison_value: None,
                        register_comparison_value: Some(0),
                        inverted: false,
                    }],
                    semantic_operations: vec![operation.to_owned()],
                    semantic_roots: vec!["rom:init".to_owned()],
                    semantic_evidence: Vec::new(),
                }],
                functions: vec!["rom:init".to_owned()],
            }],
        )
    }

    fn write_report(path: &std::path::Path, input: &str) {
        crate::artifacts::write_fixture_bundle(path, input).unwrap();
    }

    #[test]
    fn loads_and_merges_schema_v45_field_evidence() {
        let base = std::env::temp_dir().join(format!(
            "vendor-workbench-register-review-ir-{}",
            std::process::id()
        ));
        let first = base.with_extension("first.ir");
        let second = base.with_extension("second.ir");
        write_report(&first, &report(1, "rtos.event.send"));
        write_report(&second, &report(2, "delay.blocking"));
        let evidence = RegisterReviewIr::load_all(&[first.clone(), second.clone()]).unwrap();
        std::fs::remove_dir_all(first).unwrap();
        std::fs::remove_dir_all(second).unwrap();
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
            "vendor-workbench-register-review-ir-invalid-{}",
            std::process::id()
        ));
        write_report(&path, &report(1, "rtos.event.send"));
        let manifest = path.join("manifest.json");
        let input = std::fs::read_to_string(&manifest).unwrap().replacen(
            "\"schema_version\": 45",
            "\"schema_version\": 32",
            1,
        );
        std::fs::write(&manifest, input).unwrap();
        let error = RegisterReviewIr::load_all(std::slice::from_ref(&path)).unwrap_err();
        assert!(error.to_string().contains("expected schema_version 45"));

        std::fs::remove_dir_all(&path).unwrap();
        write_report(
            &path,
            &report(1, "rtos.event.send").replacen("\"mask\":240", "\"mask\":224", 1),
        );
        let error = RegisterReviewIr::load_all(std::slice::from_ref(&path)).unwrap_err();
        std::fs::remove_dir_all(path).unwrap();
        assert!(
            error
                .to_string()
                .contains("invalid field bit range or mask")
        );
    }

    #[test]
    fn rejects_the_same_report_twice() {
        let path = std::env::temp_dir().join(format!(
            "vendor-workbench-register-review-ir-duplicate-{}",
            std::process::id()
        ));
        write_report(&path, &report(1, "rtos.event.send"));
        let error = RegisterReviewIr::load_all(&[path.clone(), path.clone()]).unwrap_err();
        std::fs::remove_dir_all(path).unwrap();
        assert!(
            error
                .to_string()
                .contains("duplicate linked-IR review report")
        );
    }
}
