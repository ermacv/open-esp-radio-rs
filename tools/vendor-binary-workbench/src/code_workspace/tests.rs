use std::fs;

use super::*;
use crate::artifacts::symbol_inventory::{
    CodeBoundaryCandidateFact, CodeBoundaryFacts, CodeBoundaryInputFact,
};

fn fixture() -> CodeBoundaryFacts {
    CodeBoundaryFacts {
        inputs: vec![CodeBoundaryInputFact {
            source: "rom".to_owned(),
            artifact_sha256: "0123456789abcdef".repeat(4),
        }],
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

fn with_revision(facts: &CodeBoundaryFacts, digest: &str) -> CodeBoundaryFacts {
    let mut revised = facts.clone();
    revised.inputs[0].artifact_sha256 = digest.to_owned();
    revised.candidates[0].artifact_sha256 = digest.to_owned();
    revised
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

#[test]
fn rebase_preserves_review_when_only_artifact_guard_changes() {
    let facts = fixture();
    let pack = path("rebase-guard");
    let rebased = path("rebase-guard-output");
    let _ = fs::remove_file(&pack);
    let _ = fs::remove_file(&rebased);
    write_code_boundary_pack_template(&pack, &facts, "fixture").unwrap();
    let reviewed = fs::read_to_string(&pack).unwrap().replace(
        "status = \"unreviewed\"",
        "status = \"accepted\"\nname = \"recovered_fn\"",
    );
    fs::write(&pack, reviewed).unwrap();

    let revised = with_revision(&facts, &"fedcba9876543210".repeat(4));
    let candidate = CodeRebaseCandidate::prepare(&revised, &pack, "fixture").unwrap();
    assert_eq!(
        candidate.summary(),
        CodeRebaseSummary {
            current: false,
            safe_to_apply: true,
            preserved: 1,
            ..CodeRebaseSummary::default()
        }
    );
    fs::write(&rebased, candidate.contents()).unwrap();
    let workspace = CodeWorkspace::load(&revised, &rebased, "fixture").unwrap();
    assert_eq!(workspace.summary().accepted, 1);
    fs::remove_file(pack).unwrap();
    fs::remove_file(rebased).unwrap();
}

#[test]
fn rebase_requires_review_when_candidates_change() {
    let facts = fixture();
    let pack = path("rebase-added");
    let _ = fs::remove_file(&pack);
    write_code_boundary_pack_template(&pack, &facts, "fixture").unwrap();
    let mut revised = with_revision(&facts, &"fedcba9876543210".repeat(4));
    let mut added = revised.candidates[0].clone();
    added.entry_offset = 0x24;
    added.end_limit_offset = 0x30;
    revised.candidates.push(added);

    let candidate = CodeRebaseCandidate::prepare(&revised, &pack, "fixture").unwrap();
    assert!(!candidate.summary().safe_to_apply);
    assert_eq!(candidate.summary().preserved, 1);
    assert_eq!(candidate.summary().added, 1);
    fs::remove_file(pack).unwrap();
}
