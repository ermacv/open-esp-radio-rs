//! Interface workspace validation tests.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use super::*;

fn fake_digest(character: char) -> String {
    std::iter::repeat_n(character, 64).collect()
}

fn fixture_directory() -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "vendor-workbench-interface-pack-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn write_facts(path: &Path, digest: &str) {
    std::fs::write(
        path,
        format!(
            r#"{{
  "schema_version": 2,
  "command": "interfaces discover",
  "artifacts": [{{"index":0,"sources":["libpp"],"sha256":"{digest}"}}],
  "calls": [{{
    "artifact":0,
    "member":"event.o",
    "function":"post_event",
    "function_address":"0x100",
    "site":"0x120",
    "kind":"call",
    "target":{{
      "root":{{
        "kind":"relocated-symbol",
        "member":"event.o",
        "symbol":"g_services",
        "addend":0,
        "addressing":"absolute"
      }},
      "loads":[
        {{"site":"0x110","offset":0,"width":32}},
        {{"site":"0x114","offset":16,"width":32}}
      ],
      "container_depth":1,
      "slot_offset":16,
      "jalr_offset":0
    }},
    "arguments":[
      {{"index":0,"kind":"unknown"}},
      {{"index":1,"kind":"constant","value":"0x0000002a"}},
      {{"index":2,"kind":"pointer-provenance","canonical":"arg0+4"}}
    ]
  }}],
  "table_candidates": [{{
    "artifact": 0,
    "root": {{
      "kind":"relocated-symbol",
      "member":"event.o",
      "symbol":"g_services",
      "addend":0,
      "addressing":"absolute"
    }},
    "container_path":[{{"offset":0,"width":32}}],
    "slots":[{{"offset":16,"width":32}}],
    "functions":["post_event"]
  }}]
}}"#
        ),
    )
    .unwrap();
}

fn write_catalog(path: &Path) {
    std::fs::write(
        path,
        r#"schema = 1
id = "embedded"

[[operations]]
id = "rtos.queue.send-from-isr"
domain = "rtos"
summary = "Send from interrupt context"
argument-roles = ["queue", "item", "task-woken-out"]
return-role = "success"
effects = ["scheduler.queue-send"]
replacement = "async.channel.try-send"
"#,
    )
    .unwrap();
}

fn reviewed_pack(digest: &str, semantic: &str) -> String {
    format!(
        r#"schema = 1
id = "fixture"
calling-convention = "riscv-ilp32"

[[anchors]]
id = "wifi-osi"
status = "reviewed"
origin = "observed"
source = "libpp"
root-kind = "relocated-symbol"
member = "event.o"
symbol = "g_services"
addend = 0
addressing = "absolute"
container-path = [{{ offset = 0, width = 32 }}]
layout-version = "vendor-v9"
pointer-width = 32
layout-size = 32
slot-stride = 4

[[anchors.guards]]
kind = "artifact-sha256"
sha256 = "{digest}"

[[anchors.slots]]
offset = 16
width = 32
status = "reviewed"
origin = "observed"
name = "queue_send_from_isr"
arguments = ["opaque-handle", "const-ptr", "out-ptr"]
return = "bool"
semantic = "{semantic}"
"#
    )
}

#[test]
fn reviewed_slot_links_observed_layout_to_reusable_semantics() {
    let directory = fixture_directory();
    let facts = directory.join("facts.json");
    let pack = directory.join("pack.toml");
    let catalog = directory.join("semantics.toml");
    let digest = fake_digest('a');
    write_facts(&facts, &digest);
    write_catalog(&catalog);
    std::fs::write(&pack, reviewed_pack(&digest, "rtos.queue.send-from-isr")).unwrap();

    let workspace = InterfaceWorkspace::load(&facts, &pack, &[catalog], "riscv-ilp32").unwrap();
    std::fs::remove_dir_all(directory).unwrap();
    assert_eq!(
        workspace.summary(),
        InterfaceWorkspaceSummary {
            fact_tables: 1,
            observed_slots: 1,
            observed_calls: 1,
            reviewed_anchors: 1,
            reviewed_slots: 1,
            semantic_links: 1,
            semantic_operations: 1,
            artifact_guards: 1,
            resolved_calls: 1,
            ..InterfaceWorkspaceSummary::default()
        }
    );
    assert_eq!(workspace.bindings().len(), 1);
    assert_eq!(workspace.bindings()[0].anchor, "wifi-osi");
    assert_eq!(workspace.bindings()[0].calls.len(), 1);
    assert_eq!(workspace.bindings()[0].calls[0].site, 0x120);
    assert_eq!(
        workspace.bindings()[0].calls[0].arguments[1].expression,
        "0x0000002a"
    );
    assert_eq!(
        workspace.bindings()[0]
            .functions
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["post_event"]
    );
}

#[test]
fn inconsistent_call_site_cannot_hide_behind_a_table_aggregate() {
    let directory = fixture_directory();
    let facts = directory.join("facts.json");
    let digest = fake_digest('a');
    write_facts(&facts, &digest);
    let inconsistent = std::fs::read_to_string(&facts)
        .unwrap()
        .replace("\"slot_offset\":16", "\"slot_offset\":20");
    std::fs::write(&facts, inconsistent).unwrap();

    let error = InterfaceFacts::load(&facts).unwrap_err();
    std::fs::remove_dir_all(directory).unwrap();
    assert!(
        error
            .to_string()
            .contains("inconsistent container/slot metadata")
    );
}

#[test]
fn one_site_may_preserve_multiple_argument_provenance_variants() {
    let directory = fixture_directory();
    let facts = directory.join("facts.json");
    let digest = fake_digest('a');
    write_facts(&facts, &digest);
    let mut document =
        serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&facts).unwrap())
            .unwrap();
    let calls = document["calls"].as_array_mut().unwrap();
    let mut alternate = calls[0].clone();
    alternate["arguments"][1]["value"] = serde_json::json!("0x0000002b");
    calls.push(alternate);
    std::fs::write(&facts, serde_json::to_string(&document).unwrap()).unwrap();

    let loaded = InterfaceFacts::load(&facts).unwrap();
    std::fs::remove_dir_all(directory).unwrap();
    assert_eq!(loaded.calls.len(), 2);
}

#[test]
fn artifact_guard_mismatch_makes_an_observed_anchor_stale() {
    let directory = fixture_directory();
    let facts = directory.join("facts.json");
    let pack = directory.join("pack.toml");
    let catalog = directory.join("semantics.toml");
    let digest = fake_digest('a');
    write_facts(&facts, &digest);
    write_catalog(&catalog);
    let other = fake_digest('b');
    std::fs::write(&pack, reviewed_pack(&other, "rtos.queue.send-from-isr")).unwrap();

    let error = InterfaceWorkspace::load(&facts, &pack, &[catalog], "riscv-ilp32").unwrap_err();
    std::fs::remove_dir_all(directory).unwrap();
    assert!(error.to_string().contains("stale"));
    assert!(error.to_string().contains(&pack.display().to_string()));
}

#[test]
fn unknown_semantic_operation_is_not_accepted_by_name() {
    let directory = fixture_directory();
    let facts = directory.join("facts.json");
    let pack = directory.join("pack.toml");
    let catalog = directory.join("semantics.toml");
    let digest = fake_digest('a');
    write_facts(&facts, &digest);
    write_catalog(&catalog);
    let pack_text = reviewed_pack(&digest, "rtos.magic-name");
    let expected_span = pack_text.find("\"rtos.magic-name\"").unwrap();
    std::fs::write(&pack, &pack_text).unwrap();

    let error = InterfaceWorkspace::load(&facts, &pack, &[catalog], "riscv-ilp32").unwrap_err();
    assert!(error.to_string().contains("unknown semantic operation"));
    assert!(error.to_string().contains(&pack.display().to_string()));
    let actual_span = match error {
        crate::error::WorkbenchError::ManifestSource { span, .. } => span.offset(),
        error => panic!("expected source diagnostic, got {error:?}"),
    };
    assert_eq!(actual_span, expected_span);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn generated_pack_is_unreviewed_valid_and_never_overwritten() {
    let directory = fixture_directory();
    let facts_path = directory.join("facts.json");
    let pack_path = directory.join("pack.toml");
    let digest = fake_digest('a');
    write_facts(&facts_path, &digest);
    let facts = InterfaceFacts::load(&facts_path).unwrap();
    write_pack_template(&pack_path, &facts, "fixture", "riscv-ilp32").unwrap();

    let workspace =
        InterfaceWorkspace::load(&facts_path, &pack_path, &[] as &[PathBuf], "riscv-ilp32")
            .unwrap();
    let overwrite = write_pack_template(&pack_path, &facts, "fixture", "riscv-ilp32")
        .unwrap_err()
        .to_string();
    std::fs::remove_dir_all(directory).unwrap();
    assert_eq!(workspace.summary().unreviewed_anchors, 1);
    assert_eq!(workspace.summary().unreviewed_slots, 1);
    assert!(overwrite.contains("refusing to overwrite"));
}

#[test]
fn runtime_version_guard_can_replace_artifact_pinning() {
    let directory = fixture_directory();
    let facts = directory.join("facts.json");
    let pack = directory.join("pack.toml");
    let digest = fake_digest('a');
    write_facts(&facts, &digest);
    std::fs::write(
        &pack,
        r#"schema = 1
id = "fixture"
calling-convention = "riscv-ilp32"

[[anchors]]
id = "wifi-osi"
status = "reviewed"
source = "libpp"
root-kind = "relocated-symbol"
member = "event.o"
symbol = "g_services"
addressing = "absolute"
container-path = [{ offset = 0, width = 32 }]
layout-version = "runtime-v9"
pointer-width = 32
layout-size = 32
slot-stride = 4

[[anchors.guards]]
kind = "runtime-value"
purpose = "version"
offset = 0
width = 32
mask = 0xffffffff
value = 9

[[anchors.slots]]
offset = 16
width = 32
status = "reviewed"
name = "unknown_callback"
arguments = []
return = "void"
"#,
    )
    .unwrap();

    let workspace =
        InterfaceWorkspace::load(&facts, &pack, &[] as &[PathBuf], "riscv-ilp32").unwrap();
    std::fs::remove_dir_all(directory).unwrap();
    assert_eq!(workspace.summary().runtime_guards, 1);
    assert_eq!(workspace.summary().reviewed_slots, 1);
}

#[test]
fn shipped_catalog_covers_the_initial_cross_platform_domains() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("catalogs/embedded-semantics.toml");
    let catalogs = SemanticCatalogs::load(&[path]).unwrap();
    for operation in [
        "rtos.queue.send-from-isr",
        "storage.nvs.read",
        "logging.record",
        "time.blocking-delay",
    ] {
        assert!(catalogs.get(operation).is_some(), "missing {operation}");
    }
}
