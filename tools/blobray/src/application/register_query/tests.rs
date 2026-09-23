use super::*;
use crate::{BlobrayApplication, RegisterInventoryState, RegisterReviewState, RegisterSelector};
use std::{fs, path::Path};

fn project(root: &Path) -> std::path::PathBuf {
    let target =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/generic-project/target.toml");
    let manifest = root.join("project.toml");
    fs::write(
        &manifest,
        format!(
            "schema = 4\nid = \"capture\"\ntarget-spec = {:?}\nchip-pack = \"chip.toml\"\n",
            target.display().to_string()
        ),
    )
    .unwrap();
    fs::write(root.join("chip.toml"), "schema = 3\nid = \"fixture\"\nsvd = [\"base.svd\"]\nknowledge-packs = []\n[applicability]\nchips = [\"fixture\"]\n").unwrap();
    manifest
}

fn svd(name: &str) -> String {
    format!(
        r#"<device schemaVersion="1.3"><name>Test</name><version>1</version><description>fixture</description><addressUnitBits>8</addressUnitBits><width>32</width><peripherals><peripheral><name>DEV</name><baseAddress>4096</baseAddress><registers><register><name>{name}</name><addressOffset>0</addressOffset><size>32</size></register></registers></peripheral></peripherals></device>"#
    )
}

fn publish(manifest: &Path, paths: &[std::path::PathBuf]) {
    use crate::application::{output_set::OutputSet, query_store::QueryStore};
    let outputs = OutputSet::new(paths, false).unwrap();
    for (index, path) in paths.iter().enumerate() {
        outputs
            .file(index, "register fixture")
            .unwrap()
            .bytes(&fs::read(path).unwrap())
            .unwrap();
    }
    QueryStore::open_analysis_epoch(manifest)
        .unwrap()
        .publish_analysis_outputs(&outputs.receipts().unwrap())
        .unwrap();
}

#[test]
fn snapshot_owns_observations_publication_and_assertions_until_reload() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let manifest = project(root);
    let mut text = fs::read_to_string(&manifest).unwrap();
    text.push_str("[reviewed-knowledge]\npacks = [\"review.toml\"]\ndefault-pack = \"review.toml\"\n[registers]\nmodel = \"model.toml\"\nfacts = \"facts.json\"\nowned-ranges = [\"dev\"]\n");
    fs::write(&manifest, text).unwrap();
    let inputs = [
        ("base.svd", svd("IMPORTED")),
        ("model.toml", "schema = 3\nchip = \"fixture\"\naddress-space = \"cpu\"\nfragments = [\"fragment.toml\"]\n[device]\nname = \"TEST\"\nversion = \"1\"\ndescription = \"fixture\"\naddress-unit-bits = 8\nwidth = 32\n".into()),
        ("fragment.toml", "schema = 2\n[[peripherals]]\nname = \"DEV\"\nbaseAddress = 4096\n[[peripherals.registers]]\n[peripherals.registers.register]\nname = \"CONTROL\"\naddressOffset = 0\nsize = 32\n".into()),
        ("review.toml", "schema = 2\nid = \"review\"\n[classification]\nprovenance = \"reviewed\"\naccuracy = \"exact\"\ncompleteness = \"partial\"\n[applies-to]\nchips = [\"fixture\"]\n[[assertions]]\nid = \"control.identity\"\nsubject = \"register:fixture/cpu/0x1000/32\"\nkind = \"register-identity\"\nvalue = \"DEV.REVIEWED\"\n[[assertions.evidence]]\nsource = \"fixture\"\nlocator = \"manual\"\n".into()),
        ("facts.json", serde_json::json!({"schema_version":6,"command":"mmio discover","analysis_mode":"best-effort","access_count_mode":"maximum-per-path","completeness_claim":false,"code_selection":{"symbols":"all","symbol_prefix":""},"ranges":[{"name":"dev","start":"0x1000","end_exclusive":"0x1100"}],"artifacts":[],"registers":[],"diagnostics":[],"observations":[]}).to_string()),
    ];
    for (name, value) in &inputs {
        fs::write(root.join(name), value).unwrap();
    }
    publish(&manifest, &[root.join("facts.json")]);
    let mut app = BlobrayApplication::open(&manifest).unwrap();
    let capture = app.register_inventory().unwrap();
    let register = capture.inventory().at_address(0x1000)[0];
    let selector = RegisterSelector::Subject(register.id.clone());
    let detail = app.register_detail(&selector).unwrap().unwrap();
    assert_eq!(detail.review_status, RegisterReviewState::Manual);
    assert_eq!(detail.inventory_snapshot, capture.id());
    assert_eq!(
        app.resolved
            .register_query()
            .unwrap()
            .publication
            .assertions()
            .unwrap()
            .len(),
        1
    );
    let publication = app.snapshot().unwrap().registers.publication;
    assert_eq!(publication.unwrap().manual, 1);
    // A distinct session computes its inventory without acquiring the active writer.
    let writer =
        crate::application::query_store::QueryStore::open_analysis_epoch(&manifest).unwrap();
    let second = BlobrayApplication::open(&manifest).unwrap();
    assert_eq!(second.register_inventory().unwrap().id(), capture.id());
    drop(writer);
    for (name, _) in &inputs {
        fs::remove_file(root.join(name)).unwrap();
    }
    assert!(Arc::ptr_eq(&capture, &app.register_inventory().unwrap()));
    assert_eq!(app.register_detail(&selector).unwrap().unwrap(), detail);
    assert_eq!(
        app.resolved
            .register_query()
            .unwrap()
            .publication
            .assertions()
            .unwrap()
            .len(),
        1
    );
    let workspace = app.snapshot().unwrap();
    assert_eq!(workspace.registers.publication, publication);
    let RegisterInventoryState::Available { snapshot } = workspace.registers.inventory else {
        panic!("capture remains available")
    };
    assert!(Arc::ptr_eq(&snapshot, &capture));
    assert!(
        app.reload().is_err(),
        "missing authenticated review configuration prevents reload"
    );
    assert!(Arc::ptr_eq(&capture, &app.register_inventory().unwrap()));
    for (name, value) in &inputs {
        fs::write(root.join(name), value).unwrap();
    }
    fs::write(root.join("base.svd"), svd("CHANGED")).unwrap();
    app.reload().unwrap();
    let updated = app.register_inventory().unwrap();
    assert_ne!(updated.id(), capture.id());
    assert!(!Arc::ptr_eq(&capture, &updated));
    assert_eq!(capture.inventory().at_address(0x1000)[0], register);
    assert_eq!(detail.inventory_snapshot, capture.id());
}

#[test]
fn missing_sources_are_retained_as_an_empty_graph_until_explicit_reload() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let manifest = project(root);
    let mut app = BlobrayApplication::open(&manifest).unwrap();
    let capture = app.register_inventory().unwrap();
    assert!(capture.inventory().registers.is_empty());
    assert!(!capture.inventory().sources.is_empty());
    assert!(!capture.inventory().gaps.is_empty());
    let workspace = app.snapshot().unwrap();
    assert_eq!(workspace.registers.register_count(), 0);
    let snapshot = workspace.registers.inventory.snapshot().unwrap();
    assert_eq!(snapshot.inventory(), capture.inventory());
    fs::write(root.join("base.svd"), svd("FOUND")).unwrap();
    assert!(Arc::ptr_eq(&capture, &app.register_inventory().unwrap()));
    assert!(
        app.register_detail(&RegisterSelector::Address(0x1000))
            .unwrap()
            .is_none()
    );
    app.reload().unwrap();
    assert_eq!(
        app.register_inventory()
            .unwrap()
            .inventory()
            .registers
            .len(),
        1
    );
    assert!(capture.inventory().registers.is_empty());
}
