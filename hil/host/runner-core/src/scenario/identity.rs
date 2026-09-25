//! Canonical executable scenario values from the shared HIL schema contract;
//! parsing, validation and evidence decisions remain with their respective owner.
pub use oer_hil_schema::scenario::normalize;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    #[test]
    fn every_catalog_scenario_has_the_same_identity_before_and_after_typed_defaults() {
        let root = crate::repository_root().unwrap();
        let catalog = crate::scenario::Catalog::load(&root.join("hil/scenarios")).unwrap();
        for scenario in catalog.all() {
            let raw: Value =
                toml::from_str(&std::fs::read_to_string(&scenario.source).unwrap()).unwrap();
            assert_eq!(
                normalize(&raw),
                normalize(&serde_json::to_value(scenario).unwrap()),
                "{}",
                scenario.id
            );
        }
    }
}
