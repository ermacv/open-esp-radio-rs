//! Copyable register and field skeletons derived from MMIO evidence.

use std::fmt::Write as _;

use super::RegisterFact;

pub(super) fn write_draft(output: &mut String, fact: &RegisterFact, range_start: u32) {
    let offset = fact.address - range_start;
    output.push_str(
        "\nDraft to copy into the appropriate reviewed peripheral fragment and edit:\n\n```toml\n",
    );
    output.push_str("[[peripherals.registers]]\n\n[peripherals.registers.register]\n");
    writeln!(output, "name = \"{}\"", candidate_name(fact, offset))
        .expect("writing to String cannot fail");
    output.push_str("description = \"TODO: reviewed hardware meaning\"\n");
    writeln!(output, "addressOffset = {offset:#X}").expect("writing to String cannot fail");
    writeln!(output, "size = {}", fact.width).expect("writing to String cannot fail");
    writeln!(output, "access = \"{}\"", inferred_access(fact))
        .expect("writing to String cannot fail");
    let fields = candidate_fields(fact);
    if !fields.is_empty() {
        output.push_str("\n# Mechanical partition induced by partial-write masks. Split, merge, rename or delete after review.\n");
        for (lsb, width) in fields {
            output.push_str("[[peripherals.registers.register.fields]]\n");
            writeln!(
                output,
                "name = \"FIELD_{}_{}\"",
                u16::from(lsb) + u16::from(width) - 1,
                lsb
            )
            .expect("writing to String cannot fail");
            writeln!(output, "bitOffset = {lsb}").expect("writing to String cannot fail");
            writeln!(output, "bitWidth = {width}\n").expect("writing to String cannot fail");
        }
    }
    output.push_str("```\n");
}

pub(super) fn candidate_fields(fact: &RegisterFact) -> Vec<(u8, u8)> {
    let full_mask = width_mask(fact.width);
    let masks = fact
        .candidate_masks
        .iter()
        .copied()
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

fn candidate_name(fact: &RegisterFact, offset: u32) -> String {
    if fact.catalog_name != "UNMAPPED" {
        let leaf = fact.catalog_name.rsplit('.').next().unwrap_or_default();
        let name = identifier(leaf);
        if !name.is_empty() {
            return name;
        }
    }
    format!("REG_{offset:08X}_W{}", fact.width)
}

fn identifier(value: &str) -> String {
    let mut output = String::new();
    let mut separator = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !output.is_empty() {
                output.push('_');
            }
            output.push(character.to_ascii_uppercase());
            separator = false;
        } else {
            separator = true;
        }
    }
    if output.as_bytes().first().is_some_and(u8::is_ascii_digit) {
        output.insert_str(0, "REG_");
    }
    output
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

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
            write_patterns: vec![],
            candidate_masks: vec![0x0f, 0xf0, 0xff],
        };
        assert_eq!(candidate_fields(&fact), [(0, 4), (4, 4)]);

        fact.width = 32;
        fact.candidate_masks = vec![u32::MAX];
        assert!(candidate_fields(&fact).is_empty());
    }
}
