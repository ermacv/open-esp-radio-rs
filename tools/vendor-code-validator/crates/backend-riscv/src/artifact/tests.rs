use super::*;

#[test]
fn decoder_reads_mixed_width_code_without_objdump() {
    let symbol = ArtifactSymbolDefinition {
        member: None,
        name: "synthetic_mixed_width".to_owned(),
        address: 0x1000,
        bytes: vec![
            0x13, 0x00, 0x00, 0x00, // addi zero, zero, 0
            0x01, 0x00, // c.nop
        ],
        addresses_resolved: true,
        memory_regions: Vec::new(),
        relocations: Vec::new(),
    };
    let decoded = decode_symbol(&symbol).unwrap();
    assert_eq!(decoded.len(), 2);
    assert_eq!(decoded.first().unwrap().width, 4);
    assert_eq!(decoded.last().unwrap().width, 2);
}

#[test]
fn compressed_andi_immediate_is_sign_extended() {
    // c.andi a5, -2
    let (instruction, width) = Inst::decode(0x9b_f9, Xlen::Rv32).unwrap();
    let Inst::Andi { imm, .. } = instruction else {
        panic!("expected C.ANDI, got {instruction}");
    };
    assert_eq!(width, IsCompressed::Yes);
    assert_eq!(imm.as_u32(), 0x3e); // rv-asm 0.2.1 decoder behavior
    assert_eq!(andi_immediate(imm, 2), 0xffff_fffe);
}

#[test]
fn memory_region_requires_a_complete_supported_width() {
    let region = MemoryRegion {
        start: 0x1000,
        length: 4,
        writable: true,
        name: ".data".to_owned(),
    };
    assert!(region.contains(0x1000, 32));
    assert!(region.contains(0x1003, 8));
    assert!(!region.contains(0x1003, 16));
    assert!(!region.contains(0x1000, 24));
}

#[test]
fn recognizes_both_riscv_call_relocation_kinds() {
    assert_eq!(
        riscv_relocation_kind(object::elf::R_RISCV_CALL),
        Some(RelocationKind::Call)
    );
    assert_eq!(
        riscv_relocation_kind(object::elf::R_RISCV_CALL_PLT),
        Some(RelocationKind::CallPlt)
    );
}
