use std::sync::Arc;

use oer_riscv_model::{
    AbstractValue, ArtifactId, DecodedOp, FunctionRecord, FunctionRelocation, InstructionFlow,
    MemoryKind, ObjectId, ObjectLocation, ReferenceKind, ReferenceTarget, SymbolDefinition,
    SymbolId, SymbolTableKind, ValueAlternative, ValueAlternatives,
};

use super::{SymbolReference, listing, references_to};

fn reference(offset: u64, name: &str, addend: i64, kind: ReferenceKind) -> FunctionRecord {
    let target = Arc::new(ReferenceTarget {
        binding: 1,
        definition: SymbolDefinition::Undefined,
        symbol: SymbolId {
            object: ObjectId {
                artifact: ArtifactId::of_bytes(b"library"),
                location: ObjectLocation::ArchiveMember { ordinal: 3 },
            },
            table: SymbolTableKind::Static,
            table_section: 9,
            index: 1,
        },
        name: name.as_bytes().to_vec(),
        section: None,
        offset: 0,
        symbol_type: 2,
    });
    FunctionRecord::Reference {
        raw: Box::new(FunctionRelocation {
            section: 1,
            index: 0,
            offset,
            relocation_type: 19,
            addend: Some(addend),
            target: target.clone(),
        }),
        reference_kind: kind,
        target,
        addend: Some(addend),
        paired: None,
        known: true,
    }
}

fn instruction(offset: u64, text: &str) -> FunctionRecord {
    FunctionRecord::Instruction {
        offset,
        bytes: vec![0; 4],
        decoded: DecodedOp {
            length: 4,
            text: text.into(),
            flow: InstructionFlow::Next,
        },
    }
}

fn records() -> Vec<FunctionRecord> {
    vec![
        instruction(0, "lui a5, 0"),
        reference(0, "g_pm", 52, ReferenceKind::Address),
        instruction(4, "auipc ra, 0"),
        reference(4, "pm_scale_listen_interval", 0, ReferenceKind::Call),
        instruction(8, "ret"),
    ]
}

#[test]
fn the_listing_names_each_instructions_relocated_symbols() {
    assert_eq!(
        listing(&records()),
        [
            format!("{:6x}  {:<40} g_pm+52", 0, "lui a5, 0"),
            format!("{:6x}  {:<40} pm_scale_listen_interval", 4, "auipc ra, 0"),
            format!("{:6x}  ret", 8),
        ]
    );
}

#[test]
fn references_to_select_the_named_targets_with_their_kind() {
    let found = references_to(&records(), &["pm_scale_listen_interval".into()]);
    assert_eq!(
        found,
        [SymbolReference {
            offset: 4,
            kind: ReferenceKind::Call,
            target: b"pm_scale_listen_interval".to_vec(),
        }]
    );
    assert!(references_to(&records(), &["pm_start".into()]).is_empty());
}

fn value(offset: u64, register: u8, value: AbstractValue) -> FunctionRecord {
    FunctionRecord::Value {
        offset,
        register,
        value,
        relocation: None,
    }
}

fn access(
    offset: u64,
    access: MemoryKind,
    address: AbstractValue,
    stored: Option<AbstractValue>,
) -> FunctionRecord {
    FunctionRecord::MemoryAccess {
        offset,
        access,
        width: 4,
        address,
        value: stored,
        relocation: None,
    }
}

#[test]
fn the_listing_annotates_exact_values_and_addresses_in_hexadecimal() {
    let constant = |value| AbstractValue::Constant { value };
    let records = vec![
        instruction(0, "lui a4, 131337"),
        value(0, 14, constant(0x2010_9000)),
        instruction(4, "lw a5, 4(a4)"),
        access(4, MemoryKind::Load, constant(0x2010_9004), None),
        value(4, 15, AbstractValue::Unknown),
        instruction(8, "sw a3, 24(a4)"),
        access(
            8,
            MemoryKind::Store,
            constant(0x2010_9018),
            Some(constant(0x1b)),
        ),
        instruction(12, "sw ra, 60(sp)"),
        access(
            12,
            MemoryKind::Store,
            AbstractValue::EntryStack { offset: -4 },
            None,
        ),
        instruction(14, "ret"),
        value(14, 0, constant(0)),
        instruction(16, "addi sp, sp, -64"),
        value(16, 2, AbstractValue::EntryStack { offset: -64 }),
        instruction(20, "addi a0, sp, 12"),
        value(20, 10, AbstractValue::EntryStack { offset: -52 }),
        instruction(24, "mv a1, a2"),
        value(
            24,
            11,
            AbstractValue::Alternatives {
                values: ValueAlternatives::new(vec![
                    ValueAlternative::Constant { value: 2 },
                    ValueAlternative::Constant { value: 0x40 },
                ])
                .unwrap(),
            },
        ),
    ];
    assert_eq!(
        listing(&records),
        [
            format!("{:6x}  {:<40}  # a4=0x20109000", 0, "lui a4, 131337"),
            format!("{:6x}  {:<40}  # [0x20109004]", 4, "lw a5, 4(a4)"),
            format!("{:6x}  {:<40}  # [0x20109018] <- 0x1b", 8, "sw a3, 24(a4)"),
            format!("{:6x}  sw ra, 60(sp)", 12),
            format!("{:6x}  ret", 14),
            format!("{:6x}  addi sp, sp, -64", 16),
            format!("{:6x}  {:<40}  # a0=sp-0x34", 20, "addi a0, sp, 12"),
            format!("{:6x}  {:<40}  # a1=one-of{{0x2 | 0x40}}", 24, "mv a1, a2"),
        ]
    );
}

#[test]
fn a_symbol_value_is_named_with_its_addend() {
    let symbol = match reference(0, "phy_param", 0, ReferenceKind::Address) {
        FunctionRecord::Reference { target, .. } => target.symbol.clone(),
        _ => unreachable!(),
    };
    let records = vec![
        instruction(0, "lui s0, 0"),
        reference(0, "phy_param", 0, ReferenceKind::Address),
        value(
            0,
            8,
            AbstractValue::Symbol {
                symbol,
                addend: 239,
            },
        ),
    ];
    assert_eq!(
        listing(&records),
        [format!(
            "{:6x}  {:<40} phy_param  # s0=phy_param+0xef",
            0, "lui s0, 0"
        )]
    );
}
