use std::sync::Arc;

use oer_riscv_model::{
    ArtifactId, DecodedOp, FunctionRecord, FunctionRelocation, InstructionFlow, ObjectId,
    ObjectLocation, ReferenceKind, ReferenceTarget, SymbolDefinition, SymbolId, SymbolTableKind,
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
