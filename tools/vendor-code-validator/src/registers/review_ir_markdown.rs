//! Markdown rendering for linked-IR field and semantic evidence.

use std::fmt::Write as _;

use super::review_ir::ReviewIrRegister;

pub(super) fn write_ir_evidence(output: &mut String, register: &ReviewIrRegister) {
    writeln!(
        output,
        "Linked-IR names: {}. Users: {}.",
        code_list(register.names.iter().map(String::as_str)),
        code_list(register.functions.iter().map(String::as_str))
    )
    .expect("writing to String cannot fail");
    if register.fields.is_empty() {
        output.push_str("Linked IR contains no subregister field candidate for this width.\n");
        return;
    }
    output.push_str("\nLinked-IR field candidates:\n\n");
    output.push_str("| Bits | Mask | Writes | Predicates | Polls | Functions | Access functions | Predicate functions | Semantic operations / roots |\n");
    output.push_str("| --- | --- | ---: | ---: | ---: | --- | --- | --- | --- |\n");
    for field in register.fields.values() {
        writeln!(
            output,
            "| `{}:{}` | `{:#010x}` | {} | {} | {} | {} | {} | {} | {} / {} |",
            field.most_significant_bit,
            field.least_significant_bit,
            field.mask,
            field.write_shapes,
            field.predicate_shapes,
            field.poll_shapes,
            code_list(field.functions.iter().map(String::as_str)),
            code_list(field.access_functions.iter().map(String::as_str)),
            code_list(field.predicate_functions.iter().map(String::as_str)),
            code_list(field.semantic_operations.iter().map(String::as_str)),
            code_list(field.semantic_roots.iter().map(String::as_str)),
        )
        .expect("writing to String cannot fail");
    }
    for field in register.fields.values() {
        if field.predicate_evidence.is_empty() && field.semantic_evidence.is_empty() {
            continue;
        }
        writeln!(
            output,
            "\nEvidence for bits `{}:{}`:",
            field.most_significant_bit, field.least_significant_bit
        )
        .expect("writing to String cannot fail");
        for evidence in &field.predicate_evidence {
            writeln!(
                output,
                "- Predicate `{}` in `{}`: `{}`{}{}; producer path: {}.",
                markdown_code(&evidence.kind),
                markdown_code(&evidence.function),
                markdown_code(&evidence.condition),
                evidence.effective_operation.as_ref().map_or_else(
                    String::new,
                    |operation| format!(", effective operation `{}`", markdown_code(operation))
                ),
                evidence
                    .register_comparison_value
                    .map_or_else(String::new, |value| format!(
                        ", register comparison `{value:#010x}`"
                    )),
                code_list(evidence.producer_path.iter().map(String::as_str)),
            )
            .expect("writing to String cannot fail");
        }
        for evidence in &field.semantic_evidence {
            writeln!(
                output,
                "- Semantic link `{}` / `{}` / `{}`: action `{}` from `{}`, predicate `{}` (`{}` -> `{}`), path `{}`, residual `{}`.",
                markdown_code(&evidence.kind),
                markdown_code(&evidence.root),
                markdown_code(&evidence.operation),
                markdown_code(&evidence.action_target),
                markdown_code(&evidence.action_origin),
                markdown_code(&evidence.predicate_function),
                markdown_code(&evidence.condition),
                markdown_code(&evidence.effective_operation),
                markdown_code(&evidence.path_expression),
                markdown_code(&evidence.residual_path_expression),
            )
            .expect("writing to String cannot fail");
        }
    }
}

fn code_list<'a>(values: impl IntoIterator<Item = &'a str>) -> String {
    let values = values
        .into_iter()
        .map(|value| format!("`{}`", markdown_code(value)))
        .collect::<Vec<_>>();
    if values.is_empty() {
        "-".to_owned()
    } else {
        values.join(", ")
    }
}

fn markdown_code(value: &str) -> String {
    value.replace('`', "'").replace('|', "\\|")
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use crate::registers::review_ir::{ReviewFieldEvidence, ReviewPredicateEvidence};

    #[test]
    fn renders_field_functions_and_predicate_details() {
        let field = ReviewFieldEvidence {
            least_significant_bit: 4,
            most_significant_bit: 7,
            mask: 0xf0,
            predicate_shapes: 1,
            access_functions: ["rom:read".to_owned()].into(),
            predicate_functions: ["rom:dispatch".to_owned()].into(),
            predicate_evidence: [ReviewPredicateEvidence {
                kind: "direct-mmio".to_owned(),
                function: "rom:dispatch".to_owned(),
                producer_path: vec!["rom:read".to_owned()],
                condition: "field != 0".to_owned(),
                effective_operation: Some("not-equal".to_owned()),
                register_comparison_value: Some(0),
            }]
            .into(),
            ..ReviewFieldEvidence::default()
        };
        let register = ReviewIrRegister {
            address: 0x1010,
            width: 32,
            functions: ["rom:dispatch".to_owned()].into(),
            fields: BTreeMap::from([((4, 7, 0xf0), field)]),
            names: BTreeSet::new(),
        };
        let mut output = String::new();
        write_ir_evidence(&mut output, &register);
        assert!(output.contains("`rom:read`"));
        assert!(output.contains("effective operation `not-equal`"));
        assert!(output.contains("register comparison `0x00000000`"));
    }
}
