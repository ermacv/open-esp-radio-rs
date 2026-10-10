use super::{TableFacts, jump_tables};
use crate::*;

const TEXT: u32 = 1;
const RODATA: u32 = 3;
const RELA_RODATA: u32 = 5;
const SYMTAB: u32 = 10;
const TABLE: u64 = 7;

fn table_symbol() -> SymbolId {
    SymbolId {
        object: ObjectId {
            artifact: ArtifactId::of_bytes(b"object"),
            location: ObjectLocation::Standalone,
        },
        table: SymbolTableKind::Static,
        table_section: SYMTAB,
        index: TABLE,
    }
}

/// `bltu 2, a1` (when `bounded`); `a5 = load4[(a1 << 2) + .Ltable]`;
/// `jalr zero, 0(a5)` at 0x14 in a block from 0 to 0x18.
fn records(bounded: bool) -> Vec<FunctionRecord> {
    let expression = |id, expression| FunctionRecord::Expression {
        id,
        offset: 0,
        expression,
    };
    let mut records = vec![
        FunctionRecord::Block {
            id: 0,
            start: 0,
            end: 0x18,
        },
        expression(0, Expression::EntryRegister { register: 11 }),
        expression(
            1,
            Expression::Integer {
                op: IntegerOp::Shl,
                left: AbstractValue::Expression { id: 0 },
                right: AbstractValue::Constant { value: 2 },
            },
        ),
        expression(
            2,
            Expression::Integer {
                op: IntegerOp::Add,
                left: AbstractValue::Expression { id: 1 },
                right: AbstractValue::Symbol {
                    symbol: table_symbol(),
                    addend: 0,
                },
            },
        ),
        expression(
            3,
            Expression::Load {
                address: AbstractValue::Expression { id: 2 },
                width: 4,
                signed: true,
            },
        ),
        FunctionRecord::Value {
            offset: 0x10,
            register: 15,
            value: AbstractValue::Expression { id: 3 },
            relocation: None,
        },
        FunctionRecord::Instruction {
            offset: 0x14,
            bytes: vec![0x67, 0x80, 0x07, 0x00],
            decoded: DecodedOp {
                length: 4,
                text: "jalr zero, 0(a5)".into(),
                flow: InstructionFlow::Indirect {
                    base: 15,
                    offset: 0,
                    link: false,
                },
            },
        },
    ];
    if bounded {
        records.push(FunctionRecord::Condition {
            offset: 4,
            test: BranchTest::Ltu,
            left: AbstractValue::Constant { value: 2 },
            right: AbstractValue::Expression { id: 0 },
        });
    }
    records
}

/// `.Ltable` at offset 0 of `.rodata`; entries 0..`entries` relocated to
/// the case labels at 0x20, 0x30, 0x20, … of `.text` (`target` for the last).
fn facts(entries: u64, last: u64) -> TableFacts {
    let mut facts = TableFacts::default();
    facts.relocated.insert(RELA_RODATA, RODATA);
    facts.symbols.insert((SYMTAB, TABLE), (Some(RODATA), 0));
    facts.symbols.insert((SYMTAB, 8), (Some(TEXT), 0x20));
    facts.symbols.insert((SYMTAB, 9), (Some(TEXT), 0x30));
    facts.symbols.insert((SYMTAB, 11), (Some(TEXT), last));
    for entry in 0..entries {
        let label = if entry + 1 == entries {
            11
        } else if entry % 2 == 0 {
            8
        } else {
            9
        };
        facts
            .words
            .insert((RELA_RODATA, 4 * entry), (SYMTAB, label, 0));
    }
    facts
}

const EXTENT: CodeRange = CodeRange {
    start: 0,
    length: 0x40,
};

#[test]
fn a_bounded_relocated_table_yields_its_cases_in_index_order() {
    let tables = jump_tables(&records(true), TEXT, EXTENT, &facts(3, 0x38));
    assert_eq!(
        tables,
        [JumpTable {
            site: 0x14,
            entries: vec![0x20, 0x30, 0x38],
        }],
        "`bltu 2, a1` admits indices 0..=2"
    );
}

#[test]
fn an_unbounded_index_stays_an_indirect_gap() {
    assert!(jump_tables(&records(false), TEXT, EXTENT, &facts(3, 0x38)).is_empty());
}

#[test]
fn a_selected_entry_without_its_relocation_stays_an_indirect_gap() {
    assert!(jump_tables(&records(true), TEXT, EXTENT, &facts(2, 0x30)).is_empty());
}

#[test]
fn an_entry_outside_the_function_stays_an_indirect_gap() {
    assert!(jump_tables(&records(true), TEXT, EXTENT, &facts(3, 0x80)).is_empty());
}
