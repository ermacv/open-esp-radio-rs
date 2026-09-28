use super::*;
pub(super) fn cli(f: &Fixture, args: &[&str]) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["--format", "json", args[0], "--project"])
        .arg(&f.project)
        .args(["--limit-mode", "watchdog"])
        .args(&args[1..])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    if out.stdout.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&out.stdout).unwrap()
    }
}
pub(super) fn propose(
    f: &Fixture,
    proposal: KnowledgeProposal,
    base: Option<KnowledgeRevisionId>,
) -> app::RunRecord {
    f.app
        .start_knowledge(
            &f.project,
            &KnowledgeChange {
                expected_base: base,
                actor: "fixture".into(),
                reason: "captured structural interface".into(),
                action: KnowledgeAction::Propose { proposal },
            },
            budget(),
        )
        .unwrap()
        .wait()
}
pub(super) fn review(
    f: &Fixture,
    base: KnowledgeRevisionId,
    id: AssertionId,
    decision: ReviewDecision,
) -> app::RunRecord {
    f.app
        .start_knowledge(
            &f.project,
            &KnowledgeChange {
                expected_base: Some(base),
                actor: "fixture".into(),
                reason: "conditional fixture contract".into(),
                action: KnowledgeAction::Review {
                    assertion: id,
                    decision,
                    supersedes: None,
                },
            },
            budget(),
        )
        .unwrap()
        .wait()
}
