//! Regression tests for observation ownership independent of reviewed bindings.

use std::{fs, sync::Arc};

use super::*;
use crate::InterfaceObservationState;

struct Fixture(tempfile::TempDir);

impl Fixture {
    fn new(pack: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let target = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/generic-project/target.toml");
        let pack = if pack {
            "pack = \"reviewed.toml\"\n[interfaces.capability-context]\noutput = \"capabilities.json\"\n"
        } else {
            ""
        };
        fs::write(
            directory.path().join("project.toml"),
            format!(
                "schema = 4\nid = \"fixture\"\ntarget-spec = {:?}\n[interfaces]\nfacts = \"facts.json\"\n{pack}",
                target.to_str().unwrap()
            ),
        )
        .unwrap();
        Self(directory)
    }

    fn session(&self) -> ProjectSession {
        ProjectSession::open_with(&self.0.path().join("project.toml"), Default::default()).unwrap()
    }

    fn facts(&self, function: &str) {
        let document = serde_json::json!({
            "schema_version": 11, "command": "interfaces discover",
            "limits": {"max_state_updates":4096,"max_value_alternatives":64},
            "analysis_scope": {
                "architecture":"riscv32", "calling_convention":"riscv-ilp32",
                "evidence":"control-flow-merged register provenance",
                "relocation_evidence":["absolute","pc-relative","got"],
                "semantic_claim":false, "table_layout_claim":false,
                "linker_resolution_claim":false, "completeness_claim":false
            },
            "artifacts":[{
                "index":0, "path":"fixture.a", "roles":[], "sources":["fixture"],
                "sha256":"a".repeat(64), "container":"archive",
                "functions":1, "reviewed_boundaries":0
            }],
            "table_candidates":[{
                "artifact":0, "root":{"kind":"function-argument", "owner":{"kind":"synthetic","namespace":"snapshot-fixture","key":function}, "argument":0, "canonical":"arg0"},
                "container_path":[], "functions":[function], "call_sites":1,
                "slots":[{"offset":0,"width":32,"functions":[function],"call_sites":1}]
            }],
            "calls":[], "assignments":[], "gaps":[], "decode_blockers":[],
            "analysis_failures":[{
                "owner":{"kind":"synthetic","namespace":"snapshot-fixture","key":function},
                "artifact":0,"member":"worker.o","function":function,"error":"unsupported body"
            }]
        });
        fs::write(self.0.path().join("facts.json"), document.to_string()).unwrap();
        self.publish(true);
    }

    fn publish(&self, include_facts: bool) {
        use crate::application::{output_set::OutputSet, query_store::QueryStore};
        let paths = if include_facts {
            vec![self.0.path().join("facts.json")]
        } else {
            Vec::new()
        };
        let outputs = OutputSet::new(&paths, false).unwrap();
        for (index, path) in paths.iter().enumerate() {
            outputs
                .file(index, "interface fixture")
                .unwrap()
                .bytes(&fs::read(path).unwrap())
                .unwrap();
        }
        QueryStore::open_analysis_epoch(&self.0.path().join("project.toml"))
            .unwrap()
            .publish_analysis_outputs(&outputs.receipts().unwrap())
            .unwrap();
    }

    fn pack(&self, text: &str) {
        fs::write(self.0.path().join("reviewed.toml"), text).unwrap();
    }
}

#[test]
fn observations_are_available_without_a_reviewed_pack() {
    let fixture = Fixture::new(false);
    fixture.facts("unreviewed_worker");
    let session = fixture.session();
    let mut diagnostics = Vec::new();
    let report = collect(&session, &mut diagnostics);
    assert!(diagnostics.is_empty());
    assert_eq!(
        report.observation_state,
        InterfaceObservationState::Available
    );
    assert_eq!(report.observed_slots, 1);
    assert_eq!(report.unreviewed_slots, 1);
    assert!(report.slots.is_empty());
    let facts = report.observations.as_ref().unwrap();
    assert!(Arc::ptr_eq(
        facts,
        session.interface_facts().unwrap().unwrap()
    ));
    assert_eq!(facts.analysis_failures[0].function, "unreviewed_worker");
    let serialized = serde_json::to_value(&report).unwrap();
    assert_eq!(
        serialized["observations"],
        serde_json::to_value(facts).unwrap()
    );
}

#[test]
fn broken_review_does_not_hide_or_replace_observations() {
    let fixture = Fixture::new(true);
    fixture.facts("worker");
    fixture.pack("schema = 999\n");
    let session = fixture.session();
    let mut diagnostics = Vec::new();
    let report = collect(&session, &mut diagnostics);
    assert_eq!(
        report.observation_state,
        InterfaceObservationState::Available
    );
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].component, "interface-review");
    assert_eq!(report.observations.unwrap().analysis_failures.len(), 1);
    assert_eq!(report.observed_slots, 1);
}

#[test]
fn review_borrows_captured_facts_and_reload_observes_new_generation() {
    let fixture = Fixture::new(true);
    fixture.facts("before");
    fixture.pack("schema = 3\nid = \"fixture\"\ncalling-convention = \"riscv-ilp32\"\n");
    let session = fixture.session();
    let facts = session.interface_facts().unwrap().unwrap().clone();
    // Review must consume the captured query even when its export disappears.
    fs::remove_file(fixture.0.path().join("facts.json")).unwrap();
    let workspace = session.interface_workspace().unwrap().unwrap();
    assert!(std::ptr::eq(workspace.facts(), facts.as_ref()));
    let mut diagnostics = Vec::new();
    let old_report = collect(&session, &mut diagnostics);
    assert!(diagnostics.is_empty());
    fixture.facts("after");
    let still_old = collect(&session, &mut diagnostics);
    assert!(Arc::ptr_eq(
        still_old.observations.as_ref().unwrap(),
        &facts
    ));
    let new_session = fixture.session();
    let new_report = collect(&new_session, &mut diagnostics);
    assert!(diagnostics.is_empty());
    assert_eq!(
        old_report.observations.unwrap().analysis_failures[0].function,
        "before"
    );
    assert_eq!(
        new_report.observations.unwrap().analysis_failures[0].function,
        "after"
    );
}

#[test]
fn missing_publication_missing_member_and_parse_failure_remain_explicit_until_reload() {
    let fixture = Fixture::new(false);
    let missing_session = fixture.session();
    let mut diagnostics = Vec::new();
    assert!(matches!(
        collect(&missing_session, &mut diagnostics).observation_state,
        InterfaceObservationState::Failed { .. }
    ));
    assert!(
        diagnostics[0]
            .message
            .contains("no published analysis epoch")
    );
    fixture.publish(false);
    let absent_session = fixture.session();
    diagnostics.clear();
    assert!(matches!(
        collect(&absent_session, &mut diagnostics).observation_state,
        InterfaceObservationState::Failed { .. }
    ));
    assert!(diagnostics[0].message.contains("has no output"));
    fs::write(fixture.0.path().join("facts.json"), "{}").unwrap();
    fixture.publish(true);
    let failed_session = fixture.session();
    assert!(matches!(
        collect(&failed_session, &mut Vec::new()).observation_state,
        InterfaceObservationState::Failed { .. }
    ));
    fixture.facts("repaired");
    for session in [&missing_session, &absent_session, &failed_session] {
        assert!(matches!(
            collect(session, &mut Vec::new()).observation_state,
            InterfaceObservationState::Failed { .. }
        ));
    }
    assert_eq!(
        collect(&fixture.session(), &mut Vec::new()).observation_state,
        InterfaceObservationState::Available
    );
}

#[test]
fn first_interface_query_uses_previously_selected_epoch_even_after_exports_are_deleted() {
    let fixture = Fixture::new(false);
    fixture.facts("before");
    let session = fixture.session();
    let path = fixture.0.path().join("facts.json");
    // Select publication without populating the interface observation cache.
    session.artifacts.read_output(&path).unwrap().unwrap();
    let epoch = session.artifacts.selected_epoch().unwrap();
    fixture.facts("after");
    fs::remove_file(path).unwrap();
    let old = collect(&session, &mut Vec::new());
    assert_eq!(
        old.observations.unwrap().analysis_failures[0].function,
        "before"
    );
    let fresh = fixture.session();
    let new = collect(&fresh, &mut Vec::new());
    assert_eq!(
        new.observations.unwrap().analysis_failures[0].function,
        "after"
    );
    assert_ne!(epoch, fresh.artifacts.selected_epoch().unwrap());
}
