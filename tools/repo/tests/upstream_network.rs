mod support;

use oer_xtask::{
    checks::network::{Boundary, audit},
    graph::Graph,
};
use serde_json::{Value, json};
use support::Fixture;

const XARXA: &str = "git+https://github.com/embassy-rs/xarxa?rev=14c369bbcbe8ee7167488ac9c9e18be059d83555#14c369bbcbe8ee7167488ac9c9e18be059d83555";
const EMBASSY: &str = "git+https://github.com/embassy-rs/embassy?rev=c0fdd08e94138105fba8be3133c4ced91afc30fc#c0fdd08e94138105fba8be3133c4ced91afc30fc";

fn set_source(metadata: &mut Value, name: &str, source: Option<&str>) {
    let p = metadata["packages"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|p| p["name"] == name)
        .unwrap();
    p["source"] = json!(source);
}

#[test]
fn upstream_requires_original_sources_and_one_selected_integration() {
    let fixture = Fixture::new();
    fixture.package(
        "adapter",
        "application",
        r#"
[dependencies]
network = { package = "embassy-net", path = "../stack" }
driver = { package = "xarxa-driver", path = "../helper" }
xarxa = { path = "../xarxa" }
endpoint = { package = "open-esp-radio-xarxa-upstream", path = "../endpoint" }
bridge = { package = "open-esp-radio-esp32s31-wifi-xarxa-upstream", path = "../bridge" }
[features]
default = ["upstream-network"]
upstream-network = []
owned-network = []
compat-network = []
"#,
    );
    for (path, name) in [
        ("stack", "embassy-net"),
        ("helper", "xarxa-driver"),
        ("xarxa", "xarxa"),
        ("endpoint", "open-esp-radio-xarxa-upstream"),
        ("bridge", "open-esp-radio-esp32s31-wifi-xarxa-upstream"),
    ] {
        fixture.package(path, name, "");
    }
    let mut original = fixture.metadata();
    set_source(&mut original, "embassy-net", Some(EMBASSY));
    set_source(&mut original, "xarxa", Some(XARXA));
    set_source(&mut original, "xarxa-driver", Some(XARXA));
    let check = |metadata| {
        audit(
            &Graph::from_value(metadata).unwrap(),
            &fixture.manifest,
            Boundary::UpstreamApplication,
            fixture.root(),
        )
    };
    check(original.clone()).unwrap();
    for name in ["embassy-net", "xarxa", "xarxa-driver"] {
        for source in [
            None,
            Some("registry+https://github.com/rust-lang/crates.io-index"),
            Some(
                "git+https://github.com/ermacv/xarxa.git?rev=122e97146fc0a174ef3310f4526defc37663bed4#122e97146fc0a174ef3310f4526defc37663bed4",
            ),
        ] {
            let mut changed = original.clone();
            set_source(&mut changed, name, source);
            assert!(
                check(changed).is_err(),
                "must reject substitution of {name}"
            );
        }
    }
    let mut changed = original;
    let root = changed["resolve"]["root"].clone();
    let node = changed["resolve"]["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|node| node["id"] == root)
        .unwrap();
    node["features"]
        .as_array_mut()
        .unwrap()
        .push(json!("owned-network"));
    assert!(check(changed).is_err());
}
