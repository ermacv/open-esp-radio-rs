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
        "blobray-code-boundaries-{}-{name}.toml",
        std::process::id()
    ))
}

fn write_accepted_pack(pack: &std::path::Path, facts: &CodeBoundaryFacts) {
    let mut review = ReviewedCodeBoundary::unreviewed(&facts.candidates[0]);
    review.status = CodeBoundaryStatus::Accepted;
    review.name = Some("recovered_fn".to_owned());
    let contents = render_code_boundary_pack(&CodeBoundaryPack {
        schema: 1,
        id: "fixture".to_owned(),
        inputs: facts
            .inputs
            .iter()
            .map(|input| ReviewedCodeInput {
                source: input.source.clone(),
                artifact_sha256: input.artifact_sha256.clone(),
            })
            .collect(),
        boundaries: vec![review],
    });
    fs::write(pack, contents).unwrap();
}

#[test]
fn sparse_template_derives_unreviewed_candidates_from_current_facts() {
    let mut facts = fixture();
    let pack = path("sparse-template");
    let _ = fs::remove_file(&pack);
    write_code_boundary_pack_template(&pack, &facts, "fixture").unwrap();
    let contents = fs::read_to_string(&pack).unwrap();
    assert!(!contents.contains("[[boundaries]]"));
    assert_eq!(
        CodeWorkspace::load(&facts, &pack, "fixture")
            .unwrap()
            .summary()
            .unreviewed,
        1
    );

    let mut added = facts.candidates[0].clone();
    added.entry_offset = 0x24;
    added.end_limit_offset = 0x30;
    facts.candidates.push(added);
    let workspace = CodeWorkspace::load(&facts, &pack, "fixture").unwrap();
    assert_eq!(workspace.summary().unreviewed, 2);
    assert_eq!(
        workspace
            .entries()
            .last()
            .unwrap()
            .review
            .end_exclusive_offset,
        0x30
    );
    assert_eq!(fs::read_to_string(&pack).unwrap(), contents);
    let rebase = CodeRebaseCandidate::prepare(&facts, &pack, "fixture").unwrap();
    assert!(rebase.summary().current);
    assert!(rebase.summary().safe_to_apply);
    assert_eq!(rebase.summary().added, 0);
    fs::remove_file(pack).unwrap();
}

#[test]
fn accepted_boundary_is_distinct_from_generated_candidate() {
    let facts = fixture();
    let pack = path("accepted");
    write_accepted_pack(&pack, &facts);
    let workspace = CodeWorkspace::load(&facts, &pack, "fixture").unwrap();
    assert_eq!(workspace.summary().accepted, 1);
    fs::remove_file(pack).unwrap();
}

#[test]
fn reviewed_boundary_cannot_expand_past_generated_gap() {
    let facts = fixture();
    let pack = path("oversized");
    write_accepted_pack(&pack, &facts);
    let reviewed = fs::read_to_string(&pack)
        .unwrap()
        .replace("end-exclusive-offset = 0x20", "end-exclusive-offset = 0x24");
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
    write_accepted_pack(&pack, &facts);
    let reviewed = fs::read_to_string(&pack)
        .unwrap()
        .replace("entry-offset = 0x10", "entry-offset = 0x14");
    fs::write(&pack, reviewed).unwrap();
    let error = CodeWorkspace::load(&facts, &pack, "fixture").unwrap_err();
    assert!(error.to_string().contains("stale reviewed code boundary"));
    fs::remove_file(pack).unwrap();
}

#[test]
fn sparse_pack_still_requires_current_input_guards() {
    let facts = fixture();
    let pack = path("sparse-stale-input");
    let _ = fs::remove_file(&pack);
    write_code_boundary_pack_template(&pack, &facts, "fixture").unwrap();
    let revised = with_revision(&facts, &"fedcba9876543210".repeat(4));
    let error = CodeWorkspace::load(&revised, &pack, "fixture").unwrap_err();
    assert!(error.to_string().contains("SHA-256 guards"));
    fs::remove_file(pack).unwrap();
}

#[test]
fn legacy_unreviewed_entries_load_and_rebase_to_sparse_overlay() {
    let facts = fixture();
    let pack = path("legacy");
    write_accepted_pack(&pack, &facts);
    let legacy = fs::read_to_string(&pack).unwrap().replace(
        "status = \"accepted\"\nname = \"recovered_fn\"",
        "status = \"unreviewed\"",
    );
    fs::write(&pack, legacy).unwrap();
    assert_eq!(
        CodeWorkspace::load(&facts, &pack, "fixture")
            .unwrap()
            .summary()
            .unreviewed,
        1
    );
    let mut rediscovered = facts.clone();
    rediscovered.candidates[0].entry_offset = 0x14;
    assert_eq!(
        CodeWorkspace::load(&rediscovered, &pack, "fixture")
            .unwrap()
            .summary()
            .unreviewed,
        1
    );
    let rebased = CodeRebaseCandidate::prepare(&rediscovered, &pack, "fixture").unwrap();
    assert!(rebased.summary().safe_to_apply);
    assert!(!rebased.contents().contains("[[boundaries]]"));
    fs::remove_file(pack).unwrap();
}

#[test]
fn rebase_requires_fresh_review_when_artifact_guard_changes() {
    let facts = fixture();
    let pack = path("rebase-guard");
    write_accepted_pack(&pack, &facts);
    let original = fs::read_to_string(&pack).unwrap();
    let revised = with_revision(&facts, &"fedcba9876543210".repeat(4));
    let candidate = CodeRebaseCandidate::prepare(&revised, &pack, "fixture").unwrap();
    assert_eq!(
        candidate.summary(),
        CodeRebaseSummary {
            current: false,
            safe_to_apply: false,
            changed: 1,
            ..CodeRebaseSummary::default()
        }
    );
    let rebased: CodeBoundaryPack = toml_edit::de::from_str(candidate.contents()).unwrap();
    let workspace = CodeWorkspace::from_pack(&revised, rebased, "fixture").unwrap();
    assert_eq!(workspace.summary().accepted, 0);
    assert_eq!(workspace.summary().unreviewed, 1);
    assert!(candidate.contents().contains("# name = \"recovered_fn\""));
    assert_eq!(fs::read_to_string(&pack).unwrap(), original);
    fs::remove_file(pack).unwrap();
}

#[test]
fn rebase_preserves_review_when_generated_candidates_are_added() {
    let mut facts = fixture();
    let pack = path("rebase-added");
    write_accepted_pack(&pack, &facts);
    let mut added = facts.candidates[0].clone();
    added.entry_offset = 0x24;
    added.end_limit_offset = 0x30;
    facts.candidates.push(added);
    let candidate = CodeRebaseCandidate::prepare(&facts, &pack, "fixture").unwrap();
    assert!(candidate.summary().current);
    assert!(candidate.summary().safe_to_apply);
    assert_eq!(candidate.summary().preserved, 1);
    assert_eq!(candidate.summary().added, 0);
    fs::remove_file(pack).unwrap();
}

#[test]
fn rebase_keeps_removed_review_as_inactive_intent() {
    let mut facts = fixture();
    let pack = path("rebase-removed");
    write_accepted_pack(&pack, &facts);
    facts.candidates.clear();
    let candidate = CodeRebaseCandidate::prepare(&facts, &pack, "fixture").unwrap();
    assert!(!candidate.summary().safe_to_apply);
    assert_eq!(candidate.summary().removed, 1);
    assert!(candidate.contents().contains("# name = \"recovered_fn\""));
    candidate.validate(&facts, "fixture").unwrap();
    fs::remove_file(pack).unwrap();
}

#[test]
fn rebase_accepts_input_without_boundary_candidates() {
    let mut facts = fixture();
    let pack = path("rebase-input-only");
    write_accepted_pack(&pack, &facts);
    facts.inputs.push(CodeBoundaryInputFact {
        source: "replay".to_owned(),
        artifact_sha256: "fedcba9876543210".repeat(4),
    });
    let candidate = CodeRebaseCandidate::prepare(&facts, &pack, "fixture").unwrap();
    assert!(candidate.summary().safe_to_apply);
    assert_eq!(candidate.summary().inputs_added, 1);
    assert_eq!(candidate.summary().preserved, 1);
    assert_eq!(candidate.summary().added, 0);
    fs::remove_file(pack).unwrap();
}

#[test]
fn rejected_decisions_remain_guarded_by_candidate_identity() {
    let facts = fixture();
    let pack = path("rejected-stale");
    write_accepted_pack(&pack, &facts);
    let rejected = fs::read_to_string(&pack).unwrap().replace(
        "status = \"accepted\"\nname = \"recovered_fn\"",
        "status = \"rejected\"\nreason = \"embedded data\"",
    );
    fs::write(&pack, rejected).unwrap();
    assert_eq!(
        CodeWorkspace::load(&facts, &pack, "fixture")
            .unwrap()
            .summary()
            .rejected,
        1
    );
    let mut rediscovered = facts.clone();
    rediscovered.candidates.clear();
    assert!(
        CodeWorkspace::load(&rediscovered, &pack, "fixture")
            .unwrap_err()
            .to_string()
            .contains("stale reviewed code boundary")
    );
    let rebase = CodeRebaseCandidate::prepare(&rediscovered, &pack, "fixture").unwrap();
    assert!(!rebase.summary().safe_to_apply);
    assert!(rebase.contents().contains("# reason = \"embedded data\""));
    fs::remove_file(pack).unwrap();
}
