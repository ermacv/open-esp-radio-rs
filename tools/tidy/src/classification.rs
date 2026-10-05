//! Every package is classified: its `[package.metadata.open-radio]` table
//! parses into an [`oer_repo::Classification`], a `chip` names a chip with
//! a profile and a `family` the family of some chip, every `inputs` pattern
//! matches a file, and the package name follows the rule. The dependency
//! rules between classified packages are [`oer_repo::policy`]'s.

use oer_repo::{
    Platform,
    classification::{check_name, input_matches},
};

use crate::Context;

/// The workspace directory whose packages name themselves: Blobray's.
const OWN_NAMES: &str = "tools/blobray";

pub fn check(context: &Context<'_>) -> Vec<String> {
    let model = &context.model;
    let mut problems = vec![];
    for package in model.packages() {
        let class = match model.classification(package) {
            Ok(class) => class,
            Err(error) => {
                problems.push(format!("{}: {error}", package.manifest));
                continue;
            }
        };
        for pattern in &class.inputs {
            if !context
                .repo
                .files()
                .any(|file| input_matches(pattern, file))
            {
                problems.push(format!(
                    "{}: open-radio.inputs pattern `{pattern}` matches no file",
                    package.manifest
                ));
            }
        }
        match &class.platform {
            Platform::Chip(chip) if !model.chips.ids().any(|id| id == chip) => {
                problems.push(format!(
                    "{}: open-radio.chip `{chip}` has no platform/{chip}/chip.toml",
                    package.manifest
                ));
            }
            Platform::Family(family) if !model.chips.families().any(|id| id == family) => {
                problems.push(format!(
                    "{}: open-radio.family `{family}` is the family of no chip",
                    package.manifest
                ));
            }
            _ => {}
        }
        if !package.directory.starts_with(OWN_NAMES)
            && let Err(error) = check_name(&package.name, class.layer)
        {
            problems.push(format!("{}: {error}", package.manifest));
        }
    }
    problems.sort();
    problems.dedup();
    problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::problems;

    const CHIP: (&str, &str) = (
        "platform/esp32s31/chip.toml",
        "schema = 1\nid = \"esp32s31\"\nfamily = \"espressif\"\nrust-target = \"riscv32imafc-unknown-none-elf\"\nboot = \"staged\"\nespflash-chip = \"esp32s31\"\nrevisions = [\"rev0\"]\n[properties]\nwifi-bands = [\"2g4\"]\nbluetooth = [\"le\"]\nieee802154 = true\ncores = 2\n",
    );

    fn manifest(name: &str, table: &str) -> String {
        format!("[package]\nname = \"{name}\"\n[package.metadata.open-radio]\n{table}\n")
    }

    #[test]
    fn the_tree_check_reports_every_misclassified_package() {
        let files = [
            CHIP,
            (
                "a/Cargo.toml",
                &manifest(
                    "oer-a",
                    "scope = \"production\"\nlayer = \"contract\"\nplatform = \"portable\"",
                ),
            ),
            (
                "b/Cargo.toml",
                &manifest(
                    "oer-b",
                    "layer = \"hardware\"\nplatform = \"chip\"\nchip = \"esp32c9\"",
                ),
            ),
            (
                "c/Cargo.toml",
                &manifest(
                    "open-radio-c",
                    "layer = \"tool\"\nplatform = \"host\"\nhost-layer = \"build\"",
                ),
            ),
            ("d/Cargo.toml", "[package]\nname = \"oer-d\"\n"),
            (
                "g/Cargo.toml",
                &manifest(
                    "oer-g",
                    "layer = \"hardware\"\nplatform = \"family\"\nfamily = \"espressif\"",
                ),
            ),
            (
                "h/Cargo.toml",
                &manifest(
                    "oer-h",
                    "layer = \"tool\"\nplatform = \"host\"\nhost-layer = \"build\"\ninputs = [\"a\", \"gone/**/x\"]",
                ),
            ),
            (
                "tools/blobray/x/Cargo.toml",
                &manifest(
                    "blobray-x",
                    "layer = \"tool\"\nplatform = \"host\"\nhost-layer = \"build\"",
                ),
            ),
        ];
        assert_eq!(
            problems(&files, check),
            [
                "a/Cargo.toml: package oer-a has unknown key open-radio.scope (the layer implies the scope)",
                "b/Cargo.toml: open-radio.chip `esp32c9` has no platform/esp32c9/chip.toml",
                "c/Cargo.toml: package open-radio-c does not follow the `oer-<tokens>` naming rule (docs/architecture.md)",
                "d/Cargo.toml: package oer-d lacks [package.metadata.open-radio]",
                "h/Cargo.toml: open-radio.inputs pattern `gone/**/x` matches no file",
            ]
        );
    }
}
