use super::*;

#[test]
fn capability_contract_rejects_stale_installations() {
    assert!(require_capabilities(&format!("{REQUIRED_CAPABILITIES}\n")).is_ok());
    assert!(require_capabilities("schema=1 client=1 managed=1\n").is_err());
}
