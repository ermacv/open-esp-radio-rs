use super::*;

#[test]
fn empty_effect_contract_requires_explicit_return_comparison() {
    let input = r#"
schema = 3
default-disposition = "not-yet-ported"
default-protocol = "shared"
[[functions]]
source = "vendor"
symbol = "calculate"
disposition = "direct"
rust-component = "production::calculate"
binding = "v2"
rust-binding = "exact-production-entry"
rust-probe = "probe_calculate"
comparison-plan = "direct-effects-v1"
effect-contract = "exact-effects-v2"
"#;
    for suffix in ["", "compare-return = false"] {
        let document = toml_edit::de::from_str(&format!("{input}\n{suffix}")).unwrap();
        assert!(Manifest::finish(document).is_err());
    }
    let document = toml_edit::de::from_str(&format!("{input}\ncompare-return = true")).unwrap();
    assert!(Manifest::finish(document).is_ok());
}
