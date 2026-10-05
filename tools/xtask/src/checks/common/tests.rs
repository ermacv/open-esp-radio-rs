use super::*;
use std::fs;

/// A fixture repository: `files` as `(path, contents)`, read through the
/// repository model.
fn repository(files: &[(&str, &str)]) -> (tempfile::TempDir, oer_repo::Model) {
    let dir = tempfile::tempdir().unwrap();
    for (path, contents) in files {
        let path = dir.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
    let model = oer_repo::Model::load(&oer_repo::Repo::from_dir(dir.path()).unwrap()).unwrap();
    (dir, model)
}

const WORKSPACE: (&str, &str) = (
    "Cargo.toml",
    "[workspace]\nmembers = ['libraries/policy', 'crates/dependency']\nexclude = ['island']\n",
);

fn package(name: &str, class: &str) -> String {
    format!(
        "[package]\nname = '{name}'\n[features]\nleft = []\nright = []\n[package.metadata.open-radio]\n{class}\n"
    )
}

fn chip(id: &str, family: &str, target: &str) -> String {
    format!(
        "schema = 1\nid = \"{id}\"\nfamily = \"{family}\"\nrust-target = \"{target}\"\n\
         boot = \"staged\"\nespflash-chip = \"{id}\"\nrevisions = [\"rev0\"]\n\
         [properties]\nwifi-bands = [\"2g4\"]\nbluetooth = [\"le\"]\nieee802154 = false\ncores = 1\n"
    )
}

#[test]
fn production_and_source_packages_span_every_workspace() {
    let (dir, model) = repository(&[
        WORKSPACE,
        (
            "libraries/policy/Cargo.toml",
            &package("policy", "layer = 'service'\nplatform = 'portable'"),
        ),
        (
            "crates/dependency/Cargo.toml",
            &package(
                "target-library",
                "layer = 'experiment'\nplatform = 'portable'",
            ),
        ),
        (
            "island/Cargo.toml",
            &format!(
                "{}[workspace]\n",
                package("island", "layer = 'contract'\nplatform = 'portable'")
            ),
        ),
    ]);
    let names = |packages: Vec<Classified>| {
        packages
            .into_iter()
            .map(|package| package.package.name)
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(
        names(production_packages(dir.path(), &model).unwrap()),
        BTreeSet::from(["island".to_owned(), "policy".to_owned()])
    );
    assert_eq!(names(source_packages(dir.path(), &model).unwrap()).len(), 3);
}

#[test]
fn an_unclassified_package_cannot_disappear_from_architecture_checks() {
    let (dir, model) = repository(&[
        WORKSPACE,
        (
            "libraries/policy/Cargo.toml",
            &package("policy", "layer = 'service'\nplatform = 'portable'"),
        ),
        (
            "crates/dependency/Cargo.toml",
            "[package]\nname = 'target-library'\n",
        ),
    ]);
    let error = production_packages(dir.path(), &model)
        .expect_err("missing classification must fail")
        .to_string();
    assert!(
        error.contains("lacks [package.metadata.open-radio]"),
        "{error}"
    );
}

#[test]
fn declared_alternatives_preserve_minimum_and_default_compilation() {
    let classify = |class: &str| {
        let (_dir, model) = repository(&[
            WORKSPACE,
            ("libraries/policy/Cargo.toml", &package("policy", class)),
        ]);
        model
            .classification(model.package("policy").unwrap())
            .unwrap()
            .clone()
    };
    let facade = classify(
        "layer = 'facade'\nplatform = 'portable'\nsupported-feature-profiles = ['left', 'right']",
    );
    let profiles = compilation_profiles(&facade);
    assert!(profiles.contains(&vec![]));
    assert!(profiles.contains(&vec!["--no-default-features".into()]));
    assert!(
        !profiles
            .iter()
            .flatten()
            .any(|flag| flag == "--all-features")
    );
    for feature in ["left", "right"] {
        assert!(
            profiles
                .iter()
                .any(|flags| flags.last().is_some_and(|flag| flag == feature))
        );
    }
    // A lower composition can require a choice even though the facade must
    // remain usable with no features. Its default is still a supported build.
    let composition = classify(
        "layer = 'composition'\nplatform = 'portable'\nsupported-feature-profiles = ['left', 'right']",
    );
    let profiles = compilation_profiles(&composition);
    assert!(profiles.contains(&vec![]));
    assert!(!profiles.contains(&vec!["--no-default-features".into()]));
    assert_eq!(profiles.len(), 3);
    let plain = classify("layer = 'composition'\nplatform = 'portable'");
    assert_eq!(
        compilation_profiles(&plain),
        [
            vec!["--no-default-features".to_owned()],
            vec![],
            vec!["--all-features".to_owned()]
        ]
    );
}

#[test]
fn a_chip_package_compiles_for_its_own_chip_target() {
    let (dir, model) = repository(&[
        WORKSPACE,
        (
            "libraries/policy/Cargo.toml",
            &package(
                "policy",
                "layer = 'service'\nplatform = 'chip'\nchip = 'esp32x9'",
            ),
        ),
        (
            "crates/dependency/Cargo.toml",
            &package(
                "target-library",
                "layer = 'contract'\nplatform = 'portable'",
            ),
        ),
        (
            "platform/esp32x9/chip.toml",
            &chip("esp32x9", "vendor", "riscv32imac-unknown-none-elf"),
        ),
    ]);
    let packages = production_packages(dir.path(), &model).unwrap();
    let configurations = architecture_configurations(dir.path(), &packages).unwrap();
    let target = |package: &str| {
        configurations
            .iter()
            .filter(|configuration| configuration.package == package)
            .map(|configuration| configuration.target.as_str())
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(
        target("policy"),
        BTreeSet::from(["riscv32imac-unknown-none-elf"])
    );
    assert_eq!(
        target("target-library"),
        BTreeSet::from(["riscv32imac-unknown-none-elf"])
    );
}

#[test]
fn a_family_package_compiles_for_every_target_of_its_family() {
    let chips = [
        (
            "platform/esp32x7/chip.toml",
            chip("esp32x7", "vendor", "riscv32imafc-unknown-none-elf"),
        ),
        (
            "platform/esp32x8/chip.toml",
            chip("esp32x8", "vendor", "riscv32imac-unknown-none-elf"),
        ),
        (
            "platform/esp32x9/chip.toml",
            chip("esp32x9", "other", "xtensa-esp32-none-elf"),
        ),
    ];
    let configurations = |family: &str| {
        let policy = package(
            "policy",
            &format!("layer = 'hardware'\nplatform = 'family'\nfamily = '{family}'"),
        );
        let mut files = vec![WORKSPACE, ("libraries/policy/Cargo.toml", policy.as_str())];
        files.extend(chips.iter().map(|(path, text)| (*path, text.as_str())));
        let (dir, model) = repository(&files);
        let packages = production_packages(dir.path(), &model).unwrap();
        architecture_configurations(dir.path(), &packages)
    };
    let targets = configurations("vendor")
        .unwrap()
        .into_iter()
        .map(|configuration| configuration.target)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        targets,
        BTreeSet::from([
            "riscv32imac-unknown-none-elf".to_owned(),
            "riscv32imafc-unknown-none-elf".to_owned()
        ])
    );
    let error = configurations("absent").unwrap_err().to_string();
    assert!(error.contains("which no chip declares"), "{error}");
}

#[test]
fn a_profile_selecting_a_chip_compiles_only_for_that_chip_target() {
    let chips = [
        (
            "platform/esp32x7/chip.toml",
            chip("esp32x7", "vendor", "riscv32imafc-unknown-none-elf"),
        ),
        (
            "platform/esp32x8/chip.toml",
            chip("esp32x8", "vendor", "riscv32imac-unknown-none-elf"),
        ),
    ];
    // A profile selects a chip through the package's feature table too.
    let policy = "[package]\nname = 'policy'\n[features]\nesp32x7 = []\nesp32x8 = []\n\
                  esp32x8-radio = ['esp32x8', 'dep:radio']\nembassy-radio = ['esp32x8-radio']\n\
                  [package.metadata.open-radio]\nlayer = 'adapter'\nplatform = 'portable'\n\
                  supported-feature-profiles = ['esp32x7', 'embassy-radio']\n";
    let mut files = vec![WORKSPACE, ("libraries/policy/Cargo.toml", policy)];
    files.extend(chips.iter().map(|(path, text)| (*path, text.as_str())));
    let (dir, model) = repository(&files);
    let packages = production_packages(dir.path(), &model).unwrap();
    let found = architecture_configurations(dir.path(), &packages)
        .unwrap()
        .into_iter()
        .map(|configuration| (configuration.target, configuration.features.join(" ")))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        found,
        BTreeSet::from([
            ("riscv32imac-unknown-none-elf".to_owned(), String::new()),
            ("riscv32imafc-unknown-none-elf".to_owned(), String::new()),
            (
                "riscv32imac-unknown-none-elf".to_owned(),
                "--no-default-features --features embassy-radio".to_owned()
            ),
            (
                "riscv32imafc-unknown-none-elf".to_owned(),
                "--no-default-features --features esp32x7".to_owned()
            ),
        ])
    );
}
