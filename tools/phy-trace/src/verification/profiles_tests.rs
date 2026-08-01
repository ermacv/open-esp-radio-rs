use super::*;

#[test]
fn checked_in_profile_parses() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("profiles/esp32s31.profile");
    let profiles = load(&path).unwrap();
    assert_eq!(profiles.len(), 41);
    assert!(profiles.iter().all(|profile| !profile.scenarios.is_empty()));
    assert_eq!(
        profiles
            .iter()
            .filter(|profile| profile.contract == ProfileContract::State)
            .count(),
        7
    );
    assert_eq!(
        profiles
            .iter()
            .find(|profile| profile.name == "rom-nrx-frequency")
            .unwrap()
            .scenarios
            .len(),
        4
    );
}
