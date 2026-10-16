//! Features that exist only for tests are enabled only by
//! dev-dependencies.
//!
//! `oer-hil-executor`'s `test-support` feature adds a stand session
//! without a lease (`StandSession::for_tests`). A normal or build
//! dependency that enabled it, directly, through a `[features]` entry of
//! its own, or through one of `oer-hil-executor`'s other features (its
//! `default` included), would let production code run a driver as the
//! stand without the arbiter's grant (#761, review of #775).

use crate::Context;

/// `(package, feature)`: features only dev-dependencies may enable.
const DEV_ONLY: &[(&str, &str)] = &[("oer-hil-executor", "test-support")];

pub fn check(context: &Context<'_>) -> Vec<String> {
    let mut problems = Vec::new();
    for package in context.model.packages() {
        // The package's own features: none but the feature itself, `default`
        // included, may list it, or a normal dependency could enable it.
        for (name, feature) in DEV_ONLY.iter().filter(|(name, _)| *name == package.name) {
            for (own, list) in &package.features {
                if own != feature && list.iter().any(|entry| entry == feature) {
                    problems.push(format!(
                        "{}: its feature `{own}` enables `{feature}`, which only dev-dependencies of {name} may enable",
                        package.manifest
                    ));
                }
            }
        }
        for dependency in &package.dependencies {
            let dev = dependency.table.contains("dev-dependencies");
            for (name, feature) in DEV_ONLY {
                if dependency.package == *name
                    && !dev
                    && dependency.features.iter().any(|enabled| enabled == feature)
                {
                    problems.push(format!(
                        "{}: enables `{name}/{feature}` in `{}`; only dev-dependencies may",
                        package.manifest, dependency.table
                    ));
                }
                let forwards = |entry: &String| {
                    entry == &format!("{}/{feature}", dependency.key)
                        || entry == &format!("{}?/{feature}", dependency.key)
                };
                if dependency.package == *name && package.feature_entries.iter().any(forwards) {
                    problems.push(format!(
                        "{}: a `[features]` entry enables `{name}/{feature}`; only dev-dependencies may",
                        package.manifest
                    ));
                }
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::problems;

    fn manifest(name: &str, body: &str) -> String {
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n{body}")
    }

    fn run(user: &str) -> Vec<String> {
        run_with("[features]\ntest-support = []\n", user)
    }

    fn run_with(executor: &str, user: &str) -> Vec<String> {
        problems(
            &[
                (
                    "Cargo.toml",
                    "[workspace]\nresolver = \"3\"\nmembers = [\"executor\", \"user\"]\n",
                ),
                (
                    "executor/Cargo.toml",
                    &manifest("oer-hil-executor", executor),
                ),
                ("executor/src/lib.rs", ""),
                ("user/Cargo.toml", &manifest("oer-user", user)),
                ("user/src/lib.rs", ""),
            ],
            check,
        )
    }

    #[test]
    fn a_dev_dependency_may_enable_test_support() {
        let user = "[dev-dependencies]\noer-hil-executor = { path = \"../executor\", features = [\"test-support\"] }\n";
        assert_eq!(run(user), Vec::<String>::new());
    }

    #[test]
    fn a_normal_dependency_or_a_feature_entry_may_not() {
        let normal = "[dependencies]\noer-hil-executor = { path = \"../executor\", features = [\"test-support\"] }\n";
        assert_eq!(run(normal).len(), 1);
        let forwarded = "[features]\ntests = [\"oer-hil-executor/test-support\"]\n\n[dependencies]\noer-hil-executor = { path = \"../executor\" }\n";
        assert_eq!(run(forwarded).len(), 1);
    }

    #[test]
    fn the_executor_s_own_features_may_not_enable_test_support() {
        let by_default = "[features]\ndefault = [\"test-support\"]\ntest-support = []\n";
        assert_eq!(run_with(by_default, "").len(), 1);
        let through_another = "[features]\nlab = [\"test-support\"]\ntest-support = []\n";
        assert_eq!(run_with(through_another, "").len(), 1);
    }
}
