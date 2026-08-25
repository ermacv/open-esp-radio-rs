//! Copyable register and field skeletons derived from MMIO evidence.

use std::fmt::Write as _;

use super::{RegisterFact, review_ir::ReviewIrRegister};

pub(super) fn write_draft(output: &mut String, fact: &RegisterFact, address_space: &str) {
    output.push_str(
        "\nSparse reviewed-knowledge template. Copy only facts proven by manual review; replace every `REVIEW_REQUIRED` value and keep exact applicability/evidence:\n\n```toml\n",
    );
    if fact.catalog_name != "UNMAPPED" {
        writeln!(
            output,
            "# Generated catalog candidate (not accepted knowledge): {}",
            fact.catalog_name
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
        "[[assertions]]\nid = \"REVIEW_REQUIRED.register-declaration\"\nsubject = \"mmio:{address_space}:{:#010x}/{}\"\nkind = \"register-declaration\"\nvalue = \"REVIEW_REQUIRED_EXISTING_REGION\"\nnote = \"Declares geometry only; observed accesses do not prove hardware semantics.\"\n",
        fact.address, fact.width
    )
    .expect("writing to String cannot fail");
    output.push_str(
        "[assertions.applies-to]\nchips = [\"REVIEW_REQUIRED_CHIP\"]\nchip-revisions = [\"REVIEW_REQUIRED_REVISION\"]\n\n[[assertions.evidence]]\nsource = \"REVIEW_REQUIRED_EVIDENCE_ID\"\nlocator = \"REVIEW_REQUIRED_DECLARATION_LOCATOR\"\n\n",
    );
    writeln!(
        output,
        "[[assertions]]\nid = \"REVIEW_REQUIRED.register-name\"\nsubject = \"mmio:{address_space}:{:#010x}/{}\"\nkind = \"register-name\"\nvalue = \"REVIEW_REQUIRED_REGISTER_NAME\"\n",
        fact.address, fact.width
    )
    .expect("writing to String cannot fail");
    output.push_str(
        "[assertions.applies-to]\nchips = [\"REVIEW_REQUIRED_CHIP\"]\nchip-revisions = [\"REVIEW_REQUIRED_REVISION\"]\n\n[[assertions.evidence]]\nsource = \"REVIEW_REQUIRED_EVIDENCE_ID\"\nlocator = \"REVIEW_REQUIRED_NAME_LOCATOR\"\n",
    );
    output.push_str("```\n");
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
