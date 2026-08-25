//! Copyable register and field skeletons derived from MMIO evidence.

use std::fmt::Write as _;

use super::{RegisterFact, review_ir::ReviewIrRegister};

pub(super) fn write_draft(
    output: &mut String,
    fact: &RegisterFact,
    chip: &str,
    address_space: &str,
) {
    output.push_str(
        "\nSparse reviewed-knowledge template. Copy only facts proven by manual review; replace every `REVIEW_REQUIRED` value and keep exact applicability/evidence:\n\n```toml\n",
    );
    output.push_str(&render_sparse_review_draft(fact, chip, address_space));
    output.push_str("```\n");
}

/// Render a copyable assertion fragment without Markdown framing. Every
/// semantic value remains explicitly unresolved until a human supplies exact
/// applicability and durable evidence.
pub(crate) fn render_sparse_review_draft(
    fact: &RegisterFact,
    chip: &str,
    address_space: &str,
) -> String {
    let mut output = String::new();
    if fact.catalog_name != "UNMAPPED" {
        let catalog_name = fact.catalog_name.replace(['\r', '\n'], " ");
        writeln!(
            output,
            "# Generated catalog candidate (not accepted knowledge): {}",
            catalog_name
        )
        .expect("writing to String cannot fail");
    }
    writeln!(
        output,
        "# Generated access observation (not hardware semantics): {}",
        inferred_access(fact)
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "[[assertions]]\nid = \"REVIEW_REQUIRED.register-identity\"\nsubject = \"register:{chip}/{address_space}/{:#x}/{}\"\nkind = \"register-identity\"\nvalue = \"REVIEW_REQUIRED_REGION.REVIEW_REQUIRED_REGISTER_NAME\"\nnote = \"Identifies geometry only; observed accesses do not prove hardware semantics.\"\n",
        fact.address, fact.width
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "[assertions.applies-to]\nchips = [\"{chip}\"]\nchip-revisions = [\"REVIEW_REQUIRED_REVISION\"]\n\n[[assertions.evidence]]\nsource = \"review-required-evidence-id\"\nlocator = \"REVIEW_REQUIRED_IDENTITY_LOCATOR\""
    )
    .expect("writing to String cannot fail");
    output
}

pub(super) fn candidate_fields(
    fact: &RegisterFact,
    ir: Option<&ReviewIrRegister>,
) -> Vec<(u8, u8)> {
    let full_mask = width_mask(fact.width);
    let masks = ir
        .filter(|ir| !ir.fields.is_empty())
        .map(|ir| ir.fields.values().map(|field| field.mask).collect())
        .unwrap_or_else(|| fact.candidate_masks.clone())
        .into_iter()
        .filter(|mask| *mask != 0 && *mask != full_mask)
        .collect::<Vec<_>>();
    let mut output = Vec::new();
    let mut bit = 0_u8;
    while bit < fact.width {
        let signature = mask_signature(&masks, bit);
        if !signature.iter().any(|present| *present) {
            bit += 1;
            continue;
        }
        let start = bit;
        while bit + 1 < fact.width && mask_signature(&masks, bit + 1) == signature {
            bit += 1;
        }
        output.push((start, bit - start + 1));
        bit += 1;
    }
    output
}

fn mask_signature(masks: &[u32], bit: u8) -> Vec<bool> {
    masks
        .iter()
        .map(|mask| mask & (1_u32 << bit) != 0)
        .collect()
}

fn width_mask(width: u8) -> u32 {
    if width == 32 {
        u32::MAX
    } else {
        (1_u32 << width) - 1
    }
}

pub(super) fn inferred_access(fact: &RegisterFact) -> &'static str {
    match (fact.reads != 0, fact.writes != 0) {
        (true, true) => "read-write",
        (true, false) => "read-only",
        (false, true) => "write-only",
        (false, false) => "read-write",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use crate::registers::review_ir::ReviewFieldEvidence;

    #[test]
    fn sparse_draft_is_raw_parseable_and_does_not_infer_hardware_semantics() {
        let fact = RegisterFact {
            address: 0x2010_3100,
            width: 32,
            catalog_name: "RADIO.REG_20103100\n[[assertions]]".to_owned(),
            reads: 3,
            writes: 0,
            read_functions: BTreeSet::new(),
            write_functions: BTreeSet::new(),
            read_sites: BTreeSet::new(),
            write_sites: BTreeSet::new(),
            write_patterns: Vec::new(),
            candidate_masks: Vec::new(),
        };

        let draft = render_sparse_review_draft(&fact, "esp32s31", "cpu");
        let parsed = draft.parse::<toml_edit::DocumentMut>().unwrap();

        assert_eq!(parsed["assertions"].as_array_of_tables().unwrap().len(), 1);
        assert!(draft.contains("subject = \"register:esp32s31/cpu/0x20103100/32\""));
        assert!(draft.contains("REVIEW_REQUIRED.register-identity"));
        assert!(draft.contains("kind = \"register-identity\""));
        assert!(draft.contains("value = \"REVIEW_REQUIRED_REGION.REVIEW_REQUIRED_REGISTER_NAME\""));
        assert!(draft.contains("RADIO.REG_20103100 [[assertions]]"));
        assert_eq!(
            draft
                .lines()
                .filter(|line| *line == "[[assertions]]")
                .count(),
            1
        );
        assert!(!draft.contains("kind = \"register-access\""));
        assert!(!draft.contains("hardware-write-semantics"));
        assert!(!draft.contains("```"));
    }

    #[test]
    fn field_drafts_partition_partial_masks_and_ignore_whole_register_writes() {
        let mut fact = RegisterFact {
            address: 0x1010,
            width: 8,
            catalog_name: "UNMAPPED".to_owned(),
            reads: 0,
            writes: 1,
            read_functions: BTreeSet::new(),
            write_functions: BTreeSet::new(),
            read_sites: BTreeSet::new(),
            write_sites: BTreeSet::new(),
            write_patterns: vec![],
            candidate_masks: vec![0x0f, 0xf0, 0xff],
        };
        assert_eq!(candidate_fields(&fact, None), [(0, 4), (4, 4)]);

        fact.width = 32;
        fact.candidate_masks = vec![u32::MAX];
        assert!(candidate_fields(&fact, None).is_empty());
    }

    #[test]
    fn linked_ir_field_boundaries_take_precedence_over_write_only_masks() {
        let fact = RegisterFact {
            address: 0x1010,
            width: 32,
            catalog_name: "UNMAPPED".to_owned(),
            reads: 1,
            writes: 1,
            read_functions: BTreeSet::new(),
            write_functions: BTreeSet::new(),
            read_sites: BTreeSet::new(),
            write_sites: BTreeSet::new(),
            write_patterns: vec![],
            candidate_masks: vec![0xf0],
        };
        let ir = ReviewIrRegister {
            address: fact.address,
            width: fact.width,
            fields: BTreeMap::from([(
                (0, 1, 0x3),
                ReviewFieldEvidence {
                    least_significant_bit: 0,
                    most_significant_bit: 1,
                    mask: 0x3,
                    ..ReviewFieldEvidence::default()
                },
            )]),
            ..ReviewIrRegister::default()
        };
        assert_eq!(candidate_fields(&fact, Some(&ir)), [(0, 2)]);
    }
}
