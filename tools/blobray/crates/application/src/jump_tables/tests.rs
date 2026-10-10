use super::{SHT_RELA, SectionFact, TableFacts, jump_tables};
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

/// `bltu 2, a1` at 4 (when `bounded`), whose fall-through is the only way
/// into the dispatch block 8..0x18; there `a5 = load4[(a1 << 2) + .Ltable]`
/// and `jalr zero, 0(a5)` at 0x14.
fn records(bounded: bool) -> Vec<FunctionRecord> {
    let expression = |id, expression| FunctionRecord::Expression {
        id,
        offset: 0,
        expression,
    };
    let edge = |from, target, relation| FunctionRecord::Edge {
        from,
        target: Some(target),
        relation,
        external: false,
    };
    let mut records = vec![
        FunctionRecord::Block {
            id: 0,
            start: 0,
            end: 8,
        },
        FunctionRecord::Block {
            id: 1,
            start: 8,
            end: 0x18,
        },
        edge(4, 8, EdgeKind::Fallthrough),
        edge(4, 0x3c, EdgeKind::Taken),
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
fn memory() -> &'static WorkingMemory {
    Box::leak(Box::new(WorkingMemory::new(1 << 20).unwrap()))
}

fn section(index: u32, section_type: u32, info: u32, read_only_data: bool) -> SectionFact {
    SectionFact {
        index,
        section_type,
        info,
        read_only_data,
    }
}

/// `.Ltable` at offset 0 of `.rodata`; entries 0..`entries` relocated by
/// `.rela.rodata` (`relocations` of type `relocation_type`) to the case labels
/// at 0x20, 0x30, 0x20, … of `.text` (`last` for the last).
fn facts_with(entries: u64, last: u64, relocation_type: u32) -> TableFacts<'static> {
    facts_with_sections(entries, last, relocation_type, &[])
}

/// `facts_with`, with `extra` sections after the object's own.
fn facts_with_sections(
    entries: u64,
    last: u64,
    relocation_type: u32,
    extra: &[SectionFact],
) -> TableFacts<'static> {
    let memory = memory();
    let position = RunPosition::default();
    let mut facts = TableFacts::new(memory);
    for fact in [
        section(TEXT, 1, 0, false),
        section(RODATA, 1, 0, true),
        section(RELA_RODATA, relocation_type, RODATA, false),
    ]
    .into_iter()
    .chain(extra.iter().copied())
    {
        facts.sections.push(fact, position).unwrap();
    }
    for (index, value) in [
        (TABLE, (Some(RODATA), 0)),
        (8, (Some(TEXT), 0x20)),
        (9, (Some(TEXT), 0x30)),
        (11, (Some(TEXT), last)),
    ] {
        facts
            .symbols
            .push(((SYMTAB, index), value), position)
            .unwrap();
    }
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
            .push(((RELA_RODATA, 4 * entry), (SYMTAB, label, 0)), position)
            .unwrap();
    }
    facts.finish(&mut || Ok(())).unwrap();
    facts
}

fn facts(entries: u64, last: u64) -> TableFacts<'static> {
    facts_with(entries, last, SHT_RELA)
}

fn run(records: &[FunctionRecord], facts: &TableFacts<'_>) -> Vec<JumpTable> {
    jump_tables(records, TEXT, EXTENT, facts, memory(), &mut || Ok(()))
        .unwrap()
        .tables
}

const EXTENT: CodeRange = CodeRange {
    start: 0,
    length: 0x40,
};

#[test]
fn a_bounded_relocated_table_yields_its_cases_in_index_order() {
    let tables = run(&records(true), &facts(3, 0x38));
    assert_eq!(
        tables,
        [JumpTable {
            site: 0x14,
            first_case: Some(0),
            entries: vec![0x20, 0x30, 0x38],
        }],
        "`bltu 2, a1` admits indices 0..=2"
    );
}

#[test]
fn an_unbounded_index_stays_an_indirect_gap() {
    assert!(run(&records(false), &facts(3, 0x38)).is_empty());
}

#[test]
fn a_selected_entry_without_its_relocation_stays_an_indirect_gap() {
    assert!(run(&records(true), &facts(2, 0x30)).is_empty());
}

#[test]
fn an_entry_outside_the_function_stays_an_indirect_gap() {
    assert!(run(&records(true), &facts(3, 0x80)).is_empty());
}

#[test]
fn a_check_that_does_not_guard_the_dispatch_states_no_bound() {
    // The same comparison, but its in-range edge is not the only way in.
    let mut second_way_in = records(true);
    second_way_in.push(FunctionRecord::Edge {
        from: 0x2c,
        target: Some(8),
        relation: EdgeKind::Jump,
        external: false,
    });
    assert!(run(&second_way_in, &facts(3, 0x38)).is_empty());
    // The comparison's taken edge, not its fall-through, enters the block.
    let mut wrong_side = records(true);
    for record in &mut wrong_side {
        if let FunctionRecord::Edge {
            relation, target, ..
        } = record
        {
            *relation = match *target {
                Some(8) => EdgeKind::Taken,
                _ => EdgeKind::Fallthrough,
            };
        }
    }
    assert!(run(&wrong_side, &facts(3, 0x38)).is_empty());
}

#[test]
fn a_rebased_switch_value_reports_its_first_case() {
    let mut rebased = records(true);
    // index = entry a1 + (-8): `switch (x) { case 8: … }`.
    rebased.push(FunctionRecord::Expression {
        id: 4,
        offset: 0,
        expression: Expression::Integer {
            op: IntegerOp::Add,
            left: AbstractValue::Expression { id: 0 },
            right: AbstractValue::Constant {
                value: (-8_i32) as u32,
            },
        },
    });
    for record in &mut rebased {
        match record {
            FunctionRecord::Expression {
                id: 1,
                expression: Expression::Integer { left, .. },
                ..
            } => *left = AbstractValue::Expression { id: 4 },
            FunctionRecord::Condition { right, .. } => *right = AbstractValue::Expression { id: 4 },
            _ => {}
        }
    }
    let tables = run(&rebased, &facts(3, 0x38));
    assert_eq!(tables.len(), 1);
    assert_eq!(tables[0].first_case, Some(8));
}

#[test]
fn a_rel_table_whose_addends_live_in_its_words_is_never_proven() {
    // SHT_REL (9): each entry's addend is in the table word, not read here.
    assert!(run(&records(true), &facts_with(3, 0x38, 9)).is_empty());
}

#[test]
fn only_rela_words_of_read_only_data_are_kept() {
    let memory = memory();
    let mut facts = TableFacts::new(memory);
    let mut c = || Ok(());
    for fact in [
        section(RODATA, 1, 0, true),
        section(RELA_RODATA, SHT_RELA, RODATA, false),
        section(20, 1, 0, false),
        section(21, SHT_RELA, 20, false),
        section(22, 9, RODATA, false),
    ] {
        facts.sections.push(fact, RunPosition::default()).unwrap();
    }
    let relocation = |section, addend| RelocationRecord {
        section,
        index: 0,
        offset: 0,
        relocation_type: 1,
        symbol_table_section: SYMTAB,
        symbol_index: 8,
        addend,
    };
    facts
        .relocation(&relocation(RELA_RODATA, Some(0)), &mut c)
        .unwrap();
    facts.relocation(&relocation(21, Some(0)), &mut c).unwrap();
    facts.relocation(&relocation(22, None), &mut c).unwrap();
    assert_eq!(
        facts.words.len(),
        1,
        "a debug-like section's and a REL section's words are dropped as they stream"
    );
}

#[test]
fn the_recognizer_stops_when_the_work_budget_does() {
    let mut exhausted = || Err(Error::new(ErrorCode::ResourceLimited, "work budget"));
    let error = jump_tables(
        &records(true),
        TEXT,
        EXTENT,
        &facts(3, 0x38),
        memory(),
        &mut exhausted,
    )
    .err()
    .unwrap();
    assert_eq!(error.code, ErrorCode::ResourceLimited);
}

#[test]
fn a_return_is_never_a_dispatch_candidate() {
    let mut returning = records(true);
    for record in &mut returning {
        if let FunctionRecord::Instruction { decoded, .. } = record {
            decoded.flow = InstructionFlow::Indirect {
                base: 1,
                offset: 0,
                link: false,
            };
        }
        if let FunctionRecord::Value { register, .. } = record {
            *register = 1;
        }
    }
    assert!(run(&returning, &facts(3, 0x38)).is_empty());
}

#[test]
fn the_first_case_is_read_through_narrow_extensions_or_left_unknown() {
    use super::first_case;
    let value = |id| AbstractValue::Expression { id };
    let number = |value: u32| AbstractValue::Constant { value };
    let integer = |op, left, right| Expression::Integer { op, left, right };
    let expressions = [
        Expression::EntryRegister { register: 10 },
        // `addi a0, a0, -8; andi a0, a0, 0xff`: `switch ((uint8_t) x)`.
        integer(IntegerOp::Add, value(0), number((-8_i32) as u32)),
        integer(IntegerOp::And, value(1), number(0xff)),
        // `slli 16; srli 16` around a constant-first addition.
        integer(IntegerOp::Add, number((-3_i32) as u32), value(0)),
        integer(IntegerOp::Shl, value(3), number(16)),
        integer(IntegerOp::Shr, value(4), number(16)),
        // A product says nothing about the case values.
        integer(IntegerOp::Mul, value(0), number(3)),
        integer(IntegerOp::And, value(0), number(0xff)),
    ];
    let expressions: Vec<&Expression> = expressions.iter().collect();
    assert_eq!(first_case(&value(2), &expressions), Some(8));
    assert_eq!(first_case(&value(5), &expressions), Some(3));
    assert_eq!(first_case(&value(6), &expressions), None);
    assert_eq!(first_case(&value(7), &expressions), Some(0));
    assert_eq!(first_case(&value(0), &expressions), Some(0));
}

#[test]
fn a_section_relocated_twice_proves_no_table() {
    let twice = facts_with_sections(3, 0x38, SHT_RELA, &[section(6, SHT_RELA, RODATA, false)]);
    assert!(run(&records(true), &twice).is_empty());
}
