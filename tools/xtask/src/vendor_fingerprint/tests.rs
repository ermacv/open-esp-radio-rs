use super::*;

fn relocation(offset: usize, kind: u32, target: Target) -> Relocation {
    Relocation {
        offset,
        kind,
        target,
    }
}

/// `lui a5, IMM; lw a4, IMM(a5)` with both immediates relocated.
fn load(high: u32, low: u32, name: &str) -> Function {
    let lui = 0x0000_07b7 | (high << 12);
    let lw = 0x0007_a703 | (low << 20);
    let code = [lui.to_le_bytes(), lw.to_le_bytes()].concat();
    fingerprint(
        "a.o",
        "f",
        code,
        vec![
            relocation(0, elf::R_RISCV_HI20, Target::External(name.into())),
            relocation(4, elf::R_RISCV_LO12_I, Target::External(name.into())),
        ],
    )
}

#[test]
fn relocated_fields_do_not_enter_the_fingerprint() {
    let a = load(0x12345, 0x678, "sym_a");
    let b = load(0x54321, 0x123, "sym_a");
    assert_eq!(a.code, b.code);
    assert_eq!(a.named, b.named);
}

#[test]
fn a_renamed_target_changes_only_the_named_fingerprint() {
    let a = load(0, 0, "sym_a");
    let b = load(0, 0, "sym_b");
    assert_eq!(a.code, b.code);
    assert_ne!(a.named, b.named);
}

#[test]
fn unrelocated_code_changes_the_code_fingerprint() {
    // `li a5, 2` against `li a5, 6`.
    let a = fingerprint("a.o", "f", vec![0x89, 0x47], vec![]);
    let b = fingerprint("a.o", "f", vec![0x99, 0x47], vec![]);
    assert_ne!(a.code, b.code);
    assert_eq!(a.tokens.len(), 1);
}

#[test]
fn similarity_counts_common_instructions() {
    assert_eq!(similarity(&[1, 2, 3, 4], &[1, 2, 3, 4]), 1.0);
    assert_eq!(similarity(&[1, 2, 3, 4], &[1, 9, 3, 4]), 0.75);
    assert_eq!(similarity(&[1, 2], &[3, 4]), 0.0);
}
