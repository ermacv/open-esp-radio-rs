use std::fs;

use super::*;
use crate::artifacts::symbol_inventory::{CodeBoundaryCandidateFact, CodeBoundaryFacts};

fn fixture() -> CodeBoundaryFacts {
    CodeBoundaryFacts {
        inputs: Vec::new(),
        candidates: vec![CodeBoundaryCandidateFact {
            source: "rom".to_owned(),
            artifact_sha256: "0123456789abcdef".repeat(4),
            member: None,
            object_kind: "executable".to_owned(),
            section: ".text".to_owned(),
            section_address: 0x4000_0000,
            entry_offset: 0x10,
            end_limit_offset: 0x20,
            symbol_names: vec!["zero_sized_hint".to_owned()],
            direct_control_flow: Vec::new(),
        }],
    }
}

fn path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "vendor-workbench-code-boundaries-{}-{name}.toml",
        std::process::id()
    ))
}

#[test]
fn accepted_boundary_is_distinct_from_generated_candidate() {
    let facts = fixture();
    let pack = path("accepted");
    let _ = fs::remove_file(&pack);
    write_code_boundary_pack_template(&pack, &facts, "fixture").unwrap();
    let reviewed = fs::read_to_string(&pack).unwrap().replace(
        "status = \"unreviewed\"",
        "status = \"accepted\"\nname = \"recovered_fn\"",
    );
    fs::write(&pack, reviewed).unwrap();
    let workspace = CodeWorkspace::load(&facts, &pack, "fixture").unwrap();
    assert_eq!(workspace.summary().accepted, 1);
    fs::remove_file(pack).unwrap();
}

#[test]
fn reviewed_boundary_cannot_expand_past_generated_gap() {
    let facts = fixture();
    let pack = path("oversized");
    let _ = fs::remove_file(&pack);
    write_code_boundary_pack_template(&pack, &facts, "fixture").unwrap();
    let reviewed = fs::read_to_string(&pack)
        .unwrap()
        .replace("end-exclusive-offset = 0x20", "end-exclusive-offset = 0x24")
        .replace(
            "status = \"unreviewed\"",
            "status = \"accepted\"\nname = \"recovered_fn\"",
        );
    fs::write(&pack, reviewed).unwrap();
    let error = CodeWorkspace::load(&facts, &pack, "fixture").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("outside generated candidate limit")
    );
    fs::remove_file(pack).unwrap();
}

#[test]
fn stale_candidate_identity_is_rejected() {
    let facts = fixture();
    let pack = path("stale");
    let _ = fs::remove_file(&pack);
    write_code_boundary_pack_template(&pack, &facts, "fixture").unwrap();
    let reviewed = fs::read_to_string(&pack)
        .unwrap()
        .replace("entry-offset = 0x10", "entry-offset = 0x14");
    fs::write(&pack, reviewed).unwrap();
    let error = CodeWorkspace::load(&facts, &pack, "fixture").unwrap_err();
    assert!(error.to_string().contains("stale reviewed code boundary"));
    fs::remove_file(pack).unwrap();
}
