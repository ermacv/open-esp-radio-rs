//! End-to-end behavior over synthetic archive revisions.

use object::{
    Architecture, BinaryFormat, Endianness, RelocationFlags, SectionKind, SymbolFlags, SymbolKind,
    SymbolScope,
    write::{Object, Relocation, Symbol, SymbolSection},
};
use oer_symbol_lineage::{
    archive::{Function, Revision, read_archive},
    body::{Body, Reference, RelocationSite},
    correspond::{Evidence, Policy, correlate},
    lineage::{NameClass, trace},
};

const R_RISCV_CALL: u32 = 18;
const RET: [u8; 2] = [0x82, 0x80];

/// `auipc ra, 0 ; jalr ra` followed by distinct filler and `ret`.
fn caller_bytes(filler: u8) -> Vec<u8> {
    let mut bytes = vec![0x97, 0x00, 0x00, 0x00, 0xe7, 0x80, 0x00, 0x00];
    bytes.extend([0x01, filler]);
    bytes.extend(RET);
    bytes
}

fn function(name: &str, bytes: &[u8], callees: &[&str]) -> Function {
    let sites = callees
        .iter()
        .enumerate()
        .map(|(index, callee)| RelocationSite {
            offset: index * 8,
            r_type: R_RISCV_CALL,
            reference: Reference::Symbol((*callee).to_owned()),
        })
        .collect();
    Function {
        member: "0.o".to_owned(),
        name: name.to_owned(),
        body: Body::new(bytes, sites),
    }
}

fn revision(label: &str, functions: Vec<Function>) -> Revision {
    Revision {
        label: label.to_owned(),
        sha256: label.to_owned(),
        functions,
    }
}

fn leaf(seed: u8, length: usize) -> Vec<u8> {
    let mut bytes: Vec<u8> = (0..length)
        .map(|index| seed.wrapping_add((index as u8).wrapping_mul(7)))
        .collect();
    bytes.extend(RET);
    bytes
}

fn tokens(name: &str) -> String {
    format!("r_sym_bt_{name}")
}

#[test]
fn source_names_cross_the_obfuscation_revision() {
    let callee_old = leaf(0x10, 40);
    let mut callee_new = callee_old.clone();
    callee_new[5] ^= 0xff;
    let named = revision(
        "named",
        vec![
            function("r_caller", &caller_bytes(1), &["r_changed_callee"]),
            function("r_changed_callee", &callee_old, &[]),
            function("r_stable_name", &leaf(0x40, 12), &[]),
        ],
    );
    let obfuscated = revision(
        "obfuscated",
        vec![
            function(
                &tokens("Caller0Token0Aa1Bb2Cc"),
                &caller_bytes(1),
                &[&tokens("Callee0Token0Dd3Ee4Ff")],
            ),
            function(&tokens("Callee0Token0Dd3Ee4Ff"), &callee_new, &[]),
            function("r_stable_name", &leaf(0x40, 12), &[]),
        ],
    );
    let pairs = correlate(&named, &obfuscated, Policy::default());
    let evidence = |right: usize| {
        pairs
            .iter()
            .find(|pair| pair.right == right)
            .map(|pair| pair.evidence)
    };
    assert_eq!(evidence(0), Some(Evidence::ExactBody));
    assert_eq!(evidence(1), Some(Evidence::CallGraph));
    assert_eq!(evidence(2), Some(Evidence::SameName));

    let lineage = trace(
        &[named, obfuscated],
        Policy::default(),
        &NameClass::Prefixes(vec!["r_sym_".into()]),
    );
    let names: Vec<_> = lineage
        .functions
        .iter()
        .map(|f| f.source_name.as_deref())
        .collect();
    assert_eq!(
        names,
        [
            Some("r_caller"),
            Some("r_changed_callee"),
            Some("r_stable_name")
        ]
    );
    assert_eq!(
        lineage.functions[1].steps[0].previous_name,
        "r_changed_callee"
    );
}

#[test]
fn identical_duplicate_bodies_stay_unpaired() {
    let body = leaf(0x20, 16);
    let left = revision(
        "left",
        vec![function("r_a", &body, &[]), function("r_b", &body, &[])],
    );
    let right = revision(
        "right",
        vec![
            function(&tokens("Dup0Token0Aa1Bb2Cc3Dd"), &body, &[]),
            function(&tokens("Dup1Token0Ee4Ff5Gg6Hh"), &body, &[]),
        ],
    );
    assert!(correlate(&left, &right, Policy::default()).is_empty());
}

#[test]
fn call_graph_votes_need_a_similar_body() {
    let left = revision(
        "left",
        vec![
            function("r_caller", &caller_bytes(2), &["r_callee"]),
            function("r_callee", &leaf(0x30, 40), &[]),
        ],
    );
    let right = revision(
        "right",
        vec![
            function(
                &tokens("Caller0Token0Aa1Bb2Cc"),
                &caller_bytes(2),
                &[&tokens("Stub0Token0Dd3Ee4Ff5")],
            ),
            function(&tokens("Stub0Token0Dd3Ee4Ff5"), &RET, &[]),
        ],
    );
    let pairs = correlate(&left, &right, Policy::default());
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].right, 0);
}

#[test]
fn names_carry_through_every_revision_and_new_functions_stay_unknown() {
    let body = leaf(0x50, 24);
    let token = tokens("Kept0Token0Aa1Bb2Cc3D");
    let revisions = [
        revision("r0", vec![function("r_kept", &body, &[])]),
        revision("r1", vec![function(&token, &body, &[])]),
        revision(
            "r2",
            vec![
                function(&token, &body, &[]),
                function(&tokens("New0Token0Ee4Ff5Gg6Hh"), &leaf(0x70, 30), &[]),
            ],
        ),
    ];
    let lineage = trace(&revisions, Policy::default(), &NameClass::Heuristic);
    assert_eq!(lineage.functions[0].source_name.as_deref(), Some("r_kept"));
    assert_eq!(lineage.functions[0].origin.as_deref(), Some("r0"));
    assert_eq!(lineage.functions[0].steps.len(), 2);
    assert_eq!(lineage.functions[1].source_name, None);
}

#[test]
fn archive_reader_extracts_functions_and_their_calls() {
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let section = object.add_section(Vec::new(), b".text.r_caller".to_vec(), SectionKind::Text);
    let bytes = caller_bytes(3);
    object.append_section_data(section, &bytes, 2);
    object.add_symbol(Symbol {
        name: b"r_caller".to_vec(),
        value: 0,
        size: bytes.len() as u64,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(section),
        flags: SymbolFlags::None,
    });
    let callee = object.add_symbol(Symbol {
        name: b"r_callee".to_vec(),
        value: 0,
        size: 0,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Undefined,
        flags: SymbolFlags::None,
    });
    object
        .add_relocation(
            section,
            Relocation {
                offset: 0,
                symbol: callee,
                addend: 0,
                flags: RelocationFlags::Elf {
                    r_type: R_RISCV_CALL,
                },
            },
        )
        .unwrap();
    let elf = object.write().unwrap();
    let mut archive = b"!<arch>\n".to_vec();
    archive.extend(
        format!(
            "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
            "0.o/",
            0,
            0,
            0,
            644,
            elf.len()
        )
        .as_bytes(),
    );
    archive.extend(&elf);
    if elf.len() % 2 == 1 {
        archive.push(b'\n');
    }

    let revision = read_archive("synthetic", &archive).unwrap();
    assert_eq!(revision.functions.len(), 1);
    let function = &revision.functions[0];
    assert_eq!(
        (function.member.as_str(), function.name.as_str()),
        ("0.o", "r_caller")
    );
    assert_eq!(function.body.calls, ["r_callee"]);
    assert_eq!(function.body.size, bytes.len());
}
